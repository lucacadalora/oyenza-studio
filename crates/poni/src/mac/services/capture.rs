//! Meeting notes' two sources on the Mac, which `services::notes` (the
//! Windows app's file) records with: the microphone, and the Mac's own sound
//! (whatever it plays: the voices of a call in Meet, Zoom or Teams, with no
//! bot joining it). Each is a Core Audio IOProc whose frames become 16 kHz
//! mono (`poni_core::ielts::Downmix`) in a queue the recording thread empties
//! every 40 ms, as it empties WASAPI's buffers on Windows.
//!
//! The Mac's own sound comes through a process tap (macOS 14.2 and later): a
//! stereo tap of every app but Oyenza Studio, in a private aggregate device
//! of our own clocked by the output device. macOS asks the person once
//! (NSAudioCaptureUsageDescription); until they allow it the tap hears
//! silence. The tap's functions are looked up when first needed, so the app
//! still starts on macOS 14.0 and 14.1, where only the microphone records.
//!
//! The microphone is the default input device, after macOS's question (the
//! IELTS coach's too). While the question is up the recording goes on with
//! the Mac's sound alone and looks again every 2 s (`Source::moved`). The
//! tap is made on a thread of its own (`poni-notes-tap`), since making it
//! can wait on macOS's question for as long as the person takes to answer:
//! the recording waits for it a moment, then goes on with the microphone,
//! and the Mac's sound joins once the tap is there.
//! Nothing here runs between recordings: the IOProcs, the tap and the
//! aggregate device exist only from `open` to `close`.

use crate::services::mic::MicError;
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, sel, Message};
use objc2_avf_audio::{AVAudioApplication, AVAudioApplicationRecordPermission};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use poni_core::ielts::{peak, Downmix, SampleFormat};
use poni_core::notes::RATE;
use std::ffi::{c_char, c_void};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Mutex;
use std::time::Duration;

/// The level meters: the loudest sample of the last ~100 ms, eased down.
const METER_FALL: f32 = 0.82;
/// What a queue keeps for a reader that stopped reading: 10 s.
const MAX_QUEUE: usize = RATE as usize * 10;
/// How long a recording's start waits for the tap (a tap macOS already
/// allows takes a fraction of this).
const TAP_WAIT: Duration = Duration::from_millis(1500);

#[repr(C)]
struct Addr {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

/// `kAudioObjectSystemObject`.
const SYSTEM: u32 = 1;
const DEFAULT_INPUT: u32 = fourcc(b"dIn ");
const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const DEVICE_UID: u32 = fourcc(b"uid ");
const STREAM_FORMAT: u32 = fourcc(b"sfmt");
/// `kAudioTapPropertyFormat`.
const TAP_FORMAT: u32 = fourcc(b"tfmt");
/// `kAudioHardwarePropertyTranslatePIDToProcessObject`.
const PID_TO_PROCESS: u32 = fourcc(b"id2p");
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_INPUT: u32 = fourcc(b"inpt");
const LPCM: u32 = fourcc(b"lpcm");
const FLAG_FLOAT: u32 = 1;
const FLAG_NON_INTERLEAVED: u32 = 1 << 5;

/// `AudioStreamBasicDescription`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Asbd {
    rate: f64,
    format: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

type IoProc = extern "C" fn(u32, *const c_void, *const AudioBufferList, *const c_void, *mut AudioBufferList, *const c_void, *mut c_void) -> i32;
/// `AudioHardwareCreateProcessTap` and `AudioHardwareDestroyProcessTap` (macOS 14.2).
type CreateTap = unsafe extern "C" fn(*mut AnyObject, *mut u32) -> i32;
type DestroyTap = unsafe extern "C" fn(u32) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(id: u32, addr: *const Addr, qual_size: u32, qual: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
    fn AudioHardwareCreateAggregateDevice(desc: *const c_void, out: *mut u32) -> i32;
    fn AudioHardwareDestroyAggregateDevice(id: u32) -> i32;
    fn AudioDeviceCreateIOProcID(device: u32, proc_: IoProc, client: *mut c_void, out: *mut *mut c_void) -> i32;
    fn AudioDeviceDestroyIOProcID(device: u32, proc_id: *mut c_void) -> i32;
    fn AudioDeviceStart(device: u32, proc_id: *mut c_void) -> i32;
    fn AudioDeviceStop(device: u32, proc_id: *mut c_void) -> i32;
}

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
/// `RTLD_DEFAULT` on macOS.
const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

/// A property of a Core Audio object as plain data (`qual`: its qualifier).
fn get<T: Copy + Default>(id: u32, selector: u32, scope: u32, qual: Option<&i32>) -> Option<T> {
    let addr = Addr { selector, scope, element: 0 };
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    let (qs, qp) = qual.map_or((0, std::ptr::null()), |q| (std::mem::size_of::<i32>() as u32, (q as *const i32).cast()));
    // SAFETY: `v` is plain data of the size passed.
    let r = unsafe { AudioObjectGetPropertyData(id, &addr, qs, qp, &mut size, (&mut v as *mut T).cast()) };
    (r == 0).then_some(v)
}

/// A device's UID (a CFString Core Audio hands over, released here).
fn uid(id: u32) -> Option<String> {
    let addr = Addr { selector: DEVICE_UID, scope: SCOPE_GLOBAL, element: 0 };
    let mut p: *mut NSString = std::ptr::null_mut();
    let mut size = std::mem::size_of::<*mut NSString>() as u32;
    // SAFETY: the property is a CFStringRef we own (+1), toll-free an NSString.
    let r = unsafe { AudioObjectGetPropertyData(id, &addr, 0, std::ptr::null(), &mut size, (&mut p as *mut *mut NSString).cast()) };
    if r != 0 || p.is_null() {
        return None;
    }
    unsafe { Retained::from_raw(p) }.map(|s| s.to_string())
}

/// The default input (`DEFAULT_INPUT`) or output device now; 0 = none.
fn default_device(which: u32) -> u32 {
    get::<u32>(SYSTEM, which, SCOPE_GLOBAL, None).unwrap_or(0)
}

/// What a source's IOProc hands over, read by the recording thread.
struct Feed {
    q: Mutex<Queue>,
}

struct Queue {
    downmix: Downmix,
    rate: u32,
    /// The channels the downmix was made for.
    channels: usize,
    interleaved: bool,
    /// 16 kHz mono samples not read yet.
    out: Vec<i16>,
    /// Non-interleaved buffers, interleaved for the downmix.
    scratch: Vec<u8>,
}

impl Queue {
    fn new(f: &Asbd) -> Queue {
        let channels = f.channels.max(1) as usize;
        Queue {
            downmix: Downmix::new(f.rate.round() as u32, channels as u16, SampleFormat::F32),
            rate: f.rate.round() as u32,
            channels,
            interleaved: f.flags & FLAG_NON_INTERLEAVED == 0,
            out: Vec::with_capacity(RATE as usize),
            scratch: Vec::new(),
        }
    }

    /// One IOProc's input: the first stream when interleaved, every buffer
    /// (one a channel) when not.
    fn feed(&mut self, list: &AudioBufferList) {
        let n = list.count as usize;
        if n == 0 {
            return;
        }
        let first = list.buffers.as_ptr();
        // SAFETY: Core Audio's list holds `count` buffers, each `bytes` long.
        unsafe {
            if self.interleaved {
                let b = &*first;
                if b.data.is_null() || b.channels == 0 {
                    return;
                }
                self.fit(b.channels as usize);
                self.downmix.push(std::slice::from_raw_parts(b.data as *const u8, b.bytes as usize), &mut self.out);
            } else {
                self.fit(n);
                let frames = (0..n).map(|c| (*first.add(c)).bytes as usize / 4).min().unwrap_or(0);
                self.scratch.clear();
                for f in 0..frames {
                    for c in 0..n {
                        let b = &*first.add(c);
                        let v = if b.data.is_null() { 0.0 } else { *(b.data as *const f32).add(f) };
                        self.scratch.extend_from_slice(&v.to_le_bytes());
                    }
                }
                let scratch = std::mem::take(&mut self.scratch);
                self.downmix.push(&scratch, &mut self.out);
                self.scratch = scratch;
            }
        }
        if self.out.len() > MAX_QUEUE {
            let extra = self.out.len() - MAX_QUEUE;
            self.out.drain(..extra);
        }
    }

    /// A stream whose channel count changed gets a downmix of its own.
    fn fit(&mut self, channels: usize) {
        if channels != self.channels {
            self.channels = channels;
            self.downmix = Downmix::new(self.rate, channels as u16, SampleFormat::F32);
        }
    }
}

/// The IOProc: the input goes to its queue; output, if the device has any,
/// stays silent.
extern "C" fn io_proc(
    _device: u32,
    _now: *const c_void,
    input: *const AudioBufferList,
    _input_time: *const c_void,
    output: *mut AudioBufferList,
    _output_time: *const c_void,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the Feed `open` boxed, alive until `close` has
    // stopped and destroyed this IOProc; the lists are Core Audio's.
    unsafe {
        if !output.is_null() {
            let first = (*output).buffers.as_mut_ptr();
            for i in 0..(*output).count as usize {
                let b = &mut *first.add(i);
                if !b.data.is_null() {
                    std::ptr::write_bytes(b.data as *mut u8, 0, b.bytes as usize);
                }
            }
        }
        if input.is_null() || client.is_null() {
            return 0;
        }
        let feed = &*(client as *const Feed);
        if let Ok(mut q) = feed.q.lock() {
            q.feed(&*input);
        }
    }
    0
}

/// A running IOProc, and for the Mac's sound the tap and aggregate device
/// it hears through: all of it stopped and gone when this is dropped.
struct Running {
    device: u32,
    proc_id: *mut c_void,
    feed: Box<Feed>,
    /// (aggregate device, tap), for the Mac's sound.
    tap: Option<(u32, u32)>,
}

// SAFETY: plain Core Audio ids (the IOProc's id is an opaque handle any
// thread may pass back) and a boxed feed behind a mutex.
unsafe impl Send for Running {}

impl Drop for Running {
    fn drop(&mut self) {
        // SAFETY: our IOProc and the devices we made; stopped before the
        // feed it writes to goes (after this, with the struct).
        unsafe {
            let _ = AudioDeviceStop(self.device, self.proc_id);
            let _ = AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            if let Some((aggregate, tap)) = self.tap {
                let _ = AudioHardwareDestroyAggregateDevice(aggregate);
                if let Some(destroy) = tap_fn::<DestroyTap>(c"AudioHardwareDestroyProcessTap") {
                    let _ = destroy(tap);
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Mic,
    Pc,
}

/// One source of a meeting's recording, as the Windows app's `Source`.
pub struct Source {
    kind: Kind,
    pub what: &'static str,
    run: Option<Running>,
    /// The tap's thread, until it answers (the Mac's sound only).
    pending: Option<Receiver<Result<(u32, Running), MicError>>>,
    /// The output device a tap failed on (no tap API, or macOS said no):
    /// no new try on it, since each try makes and drops an aggregate device,
    /// which every app on the Mac hears about (the call included). A new
    /// output device gets a new try.
    failed_on: Option<u32>,
    /// The default device it opened, to notice a new default.
    id: u32,
    /// What `read` brought: 16 kHz mono.
    pub pcm: Vec<i16>,
    pub level: f32,
}

impl Source {
    pub fn mic() -> Source {
        Source::new(Kind::Mic, "notes: microphone")
    }

    pub fn pc() -> Source {
        Source::new(Kind::Pc, "notes: the Mac's sound")
    }

    fn new(kind: Kind, what: &'static str) -> Source {
        Source { kind, what, run: None, pending: None, failed_on: None, id: 0, pcm: Vec::with_capacity(RATE as usize / 4), level: 0.0 }
    }

    pub fn open(&mut self) -> Result<(), MicError> {
        self.close();
        let (id, run) = match self.kind {
            Kind::Mic => open_mic()?,
            Kind::Pc => self.open_pc_aside()?,
        };
        self.id = id;
        self.run = Some(run);
        Ok(())
    }

    /// The Mac's sound, from the tap's own thread: its answer if it comes
    /// within TAP_WAIT, else a "not yet" and the answer at a later `open`
    /// (the recording looks every 2 s). A recording that ends first drops
    /// the receiver, and the tap goes with the thread's answer.
    fn open_pc_aside(&mut self) -> Result<(u32, Running), MicError> {
        let fresh = self.pending.is_none();
        if fresh {
            let output = default_device(DEFAULT_OUTPUT);
            if self.failed_on == Some(output) {
                return Err(MicError::Failed("the tap didn't work on this output device".into()));
            }
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("poni-notes-tap".into())
                .spawn(move || {
                    let _ = tx.send(open_pc());
                })
                .map_err(|e| MicError::Failed(format!("no thread for the tap ({e})")))?;
            self.pending = Some(rx);
        }
        let Some(rx) = self.pending.as_ref() else { return Err(MicError::Failed("no tap".into())) };
        let got = if fresh { rx.recv_timeout(TAP_WAIT).map_err(|e| e == RecvTimeoutError::Timeout) } else { rx.try_recv().map_err(|e| e == TryRecvError::Empty) };
        match got {
            Ok(r) => {
                self.pending = None;
                if let Err(e) = &r {
                    crate::pinfo!("notes: the Mac's sound can't be recorded ({e}); the microphone records alone");
                    self.failed_on = Some(default_device(DEFAULT_OUTPUT));
                }
                r
            }
            Err(true) => {
                if fresh {
                    crate::pinfo!("notes: the Mac's sound isn't there yet (macOS may be asking); the recording goes on without it for now");
                }
                Err(MicError::Failed("the tap isn't there yet".into()))
            }
            Err(false) => {
                self.pending = None;
                Err(MicError::Failed("the tap's thread ended".into()))
            }
        }
    }

    /// Stop the IOProc (and drop the tap and aggregate device); a tap still
    /// being made stays on its way.
    pub fn close(&mut self) {
        self.run = None;
    }

    /// Everything the IOProc handed over since the last read.
    pub fn read(&mut self) {
        self.pcm.clear();
        if let Some(r) = &self.run {
            if let Ok(mut q) = r.feed.q.lock() {
                self.pcm.append(&mut q.out);
            }
        }
        self.level = (self.level * METER_FALL).max(peak(&self.pcm));
    }

    /// Not open (macOS still asking, say), or the Mac's default device
    /// changed (a headset plugged in): open it again.
    pub fn moved(&self) -> bool {
        let which = if self.kind == Kind::Mic { DEFAULT_INPUT } else { DEFAULT_OUTPUT };
        self.run.is_none() || default_device(which) != self.id
    }
}


/// Start our IOProc on `device`, its frames in format `f` going to a queue.
fn start(device: u32, f: &Asbd, tap: Option<(u32, u32)>) -> Result<Running, MicError> {
    if f.format != LPCM || f.flags & FLAG_FLOAT == 0 || f.bits != 32 {
        return Err(MicError::Failed(format!("the sound comes as {:08x}/{:x}/{} bits, not 32-bit float", f.format, f.flags, f.bits)));
    }
    let feed = Box::new(Feed { q: Mutex::new(Queue::new(f)) });
    let mut proc_id: *mut c_void = std::ptr::null_mut();
    let client = (&*feed as *const Feed).cast_mut().cast::<c_void>();
    // SAFETY: the feed outlives the IOProc (`Source::close`).
    let r = unsafe { AudioDeviceCreateIOProcID(device, io_proc, client, &mut proc_id) };
    if r != 0 || proc_id.is_null() {
        return Err(MicError::Failed(format!("no IOProc ({r})")));
    }
    let r = unsafe { AudioDeviceStart(device, proc_id) };
    if r != 0 {
        unsafe {
            let _ = AudioDeviceDestroyIOProcID(device, proc_id);
        }
        return Err(MicError::Failed(format!("the device didn't start ({r})")));
    }
    Ok(Running { device, proc_id, feed, tap })
}

/// The default microphone, once macOS allows it. Not asked yet: macOS asks
/// now, and this source stays closed until the answer (the recording looks
/// again every 2 s).
fn open_mic() -> Result<(u32, Running), MicError> {
    let id = default_device(DEFAULT_INPUT);
    if id == 0 {
        return Err(MicError::NoDevice);
    }
    // SAFETY: AVFAudio's shared instance, from any thread.
    let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
    if permission == AVAudioApplicationRecordPermission::Denied {
        return Err(MicError::Denied);
    }
    if permission != AVAudioApplicationRecordPermission::Granted {
        crate::pinfo!("notes: macOS asks the person for the microphone");
        let answered = RcBlock::new(|granted: Bool| crate::pinfo!("notes: the microphone {}", if granted.as_bool() { "allowed" } else { "turned down" }));
        unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&answered) };
        return Err(MicError::Denied);
    }
    let f = get::<Asbd>(id, STREAM_FORMAT, SCOPE_INPUT, None).ok_or_else(|| MicError::Failed("the microphone's format".into()))?;
    let run = start(id, &f, None)?;
    crate::pinfo!("notes: microphone: device {id}, {:.0} Hz, {} channel(s)", f.rate, f.channels);
    Ok((id, run))
}

/// One of the tap functions, where this macOS has them (14.2 and later).
fn tap_fn<T: Copy>(name: &std::ffi::CStr) -> Option<T> {
    // SAFETY: a plain lookup; a found symbol has the type the caller names.
    let p = unsafe { dlsym(RTLD_DEFAULT, name.as_ptr()) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

/// The Mac's own sound: a tap of every app but this one, in a private
/// aggregate device clocked by the default output device.
fn open_pc() -> Result<(u32, Running), MicError> {
    let (Some(create), Some(class)) = (tap_fn::<CreateTap>(c"AudioHardwareCreateProcessTap"), AnyClass::get(c"CATapDescription")) else {
        // macOS 14.0 and 14.1: no taps, so the microphone records alone.
        return Err(MicError::NoDevice);
    };
    let output = default_device(DEFAULT_OUTPUT);
    let output_uid = uid(output).ok_or(MicError::NoDevice)?;
    // This app's own sounds (the island's chimes) stay out of the notes.
    let me = get::<u32>(SYSTEM, PID_TO_PROCESS, SCOPE_GLOBAL, Some(&(std::process::id() as i32))).unwrap_or(0);
    let exclude: Retained<NSArray<NSNumber>> = NSArray::from_retained_slice(&if me != 0 { vec![NSNumber::new_u32(me)] } else { Vec::new() });
    // SAFETY: CATapDescription's own initializer and properties (macOS 14.2),
    // each setter only where the class answers to it.
    let desc: Retained<AnyObject> = unsafe {
        let a: Allocated<AnyObject> = msg_send![class, alloc];
        let d: Option<Retained<AnyObject>> = msg_send![a, initStereoGlobalTapButExcludeProcesses: &*exclude];
        let d = d.ok_or_else(|| MicError::Failed("no tap description".into()))?;
        let answers = |s| -> bool { msg_send![&*d, respondsToSelector: s] };
        if answers(sel!(setPrivateTap:)) {
            let _: () = msg_send![&*d, setPrivateTap: true];
        }
        if answers(sel!(setName:)) {
            let _: () = msg_send![&*d, setName: &*NSString::from_str("Oyenza Studio meeting notes")];
        }
        d
    };
    let tap_uid = unsafe {
        let u: Option<Retained<AnyObject>> = msg_send![&*desc, UUID];
        let u = u.ok_or_else(|| MicError::Failed("the tap has no UUID".into()))?;
        let s: Retained<NSString> = msg_send![&*u, UUIDString];
        s.to_string()
    };
    let mut tap = 0u32;
    crate::pinfo!("notes: the Mac's sound: making the tap");
    // SAFETY: the description is a live CATapDescription.
    let r = unsafe { create(Retained::as_ptr(&desc).cast_mut(), &mut tap) };
    if r != 0 || tap == 0 {
        return Err(MicError::Failed(format!("no tap ({r})")));
    }
    let destroy_tap = || {
        if let Some(destroy) = tap_fn::<DestroyTap>(c"AudioHardwareDestroyProcessTap") {
            let _ = unsafe { destroy(tap) };
        }
    };
    let Some(f) = get::<Asbd>(tap, TAP_FORMAT, SCOPE_GLOBAL, None) else {
        destroy_tap();
        return Err(MicError::Failed("the tap's format".into()));
    };
    let aggregate = match aggregate(&output_uid, &tap_uid) {
        Ok(a) => a,
        Err(e) => {
            destroy_tap();
            return Err(e);
        }
    };
    crate::pinfo!("notes: the Mac's sound: tap {tap}, aggregate device {aggregate}, {:.0} Hz, {} channel(s)", f.rate, f.channels);
    match start(aggregate, &f, Some((aggregate, tap))) {
        Ok(run) => {
            crate::pinfo!("notes: the Mac's sound: started");
            Ok((output, run))
        }
        Err(e) => {
            unsafe {
                let _ = AudioHardwareDestroyAggregateDevice(aggregate);
            }
            destroy_tap();
            Err(e)
        }
    }
}

/// Any object, for a dictionary's values.
fn any<T: Message>(o: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(o) }
}

/// A private aggregate device: the output device for its clock, and the tap.
fn aggregate(output_uid: &str, tap_uid: &str) -> Result<u32, MicError> {
    let s = NSString::from_str;
    let dict = |pairs: Vec<(&str, Retained<AnyObject>)>| -> Retained<NSDictionary<NSString, AnyObject>> {
        let keys: Vec<Retained<NSString>> = pairs.iter().map(|(k, _)| s(k)).collect();
        let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let values: Vec<&AnyObject> = pairs.iter().map(|(_, v)| &**v).collect();
        NSDictionary::from_slices(&keys, &values)
    };
    let sub = dict(vec![("uid", any(s(output_uid)))]);
    let tap = dict(vec![("uid", any(s(tap_uid))), ("drift", any(NSNumber::new_bool(true)))]);
    let unique = format!("com.oyenza.studio.notes.{}", std::process::id());
    let desc = dict(vec![
        ("name", any(s("Oyenza Studio meeting notes"))),
        ("uid", any(s(&unique))),
        ("master", any(s(output_uid))),
        ("private", any(NSNumber::new_bool(true))),
        ("stacked", any(NSNumber::new_bool(false))),
        ("tapautostart", any(NSNumber::new_bool(true))),
        ("subdevices", any(NSArray::from_retained_slice(&[sub]))),
        ("taps", any(NSArray::from_retained_slice(&[tap]))),
    ]);
    let mut id = 0u32;
    // SAFETY: an NSDictionary is a CFDictionaryRef (toll-free).
    let r = unsafe { AudioHardwareCreateAggregateDevice(Retained::as_ptr(&desc).cast(), &mut id) };
    if r != 0 || id == 0 {
        return Err(MicError::Failed(format!("no aggregate device ({r})")));
    }
    Ok(id)
}
