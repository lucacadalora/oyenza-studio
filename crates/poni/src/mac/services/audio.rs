//! Output volume and mute (Core Audio), with a notification on every change:
//! the volume pop-up. Follows the default output device when it changes
//! (headphones plugged in, AirPods connected).
//!
//! Also the headphone pop-up: Core Audio lists a Bluetooth audio device while
//! it's connected, so a Bluetooth output appearing in the device list, or
//! leaving it, is headphones (or a speaker) connecting or going. Windows'
//! pop-up also shows the battery; macOS gives apps a headset's battery only
//! through IOBluetooth's private calls and a Bluetooth permission, so the
//! Mac's has none (macOS shows AirPods' own battery when they connect).
//!
//! And whether sound plays at all: Core Audio says when some app starts or
//! stops playing through the default output, which wakes the now-playing
//! reader (media.rs) only while there is something to hear.
//!
//! Core Audio calls the listeners on its own thread; they only read what
//! changed and send it on. Nothing polls.

use super::cf;
use crate::events::{AppEvent, EventSink};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

#[repr(C)]
#[derive(Clone, Copy)]
struct Addr {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
}

/// `kAudioObjectSystemObject`.
const SYSTEM: u32 = 1;
const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
const VOLUME: u32 = fourcc(b"volm");
const MUTE: u32 = fourcc(b"mute");
/// The system's device list, and a device's transport, UID, name and streams.
const DEVICES: u32 = fourcc(b"dev#");
const TRANSPORT: u32 = fourcc(b"tran");
const UID: u32 = fourcc(b"uid ");
const NAME: u32 = fourcc(b"lnam");
const STREAMS: u32 = fourcc(b"stm#");
/// `kAudioDeviceTransportTypeBluetooth` and `kAudioDeviceTransportTypeBluetoothLE`.
const BLUETOOTH: [u32; 2] = [fourcc(b"blue"), fourcc(b"blea")];
/// A headset whose name macOS doesn't give.
const FALLBACK_NAME: &str = "Bluetooth headphones";
/// The main element, then the left and right channels.
const ELEMENTS: [u32; 3] = [0, 1, 2];
/// `kAudioDevicePropertyDeviceIsRunningSomewhere`: some app plays through
/// the device (music, a video, a call).
const RUNNING: u32 = fourcc(b"gone");

type Listener = extern "C" fn(u32, u32, *const Addr, *mut c_void) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(id: u32, addr: *const Addr, qual_size: u32, qual: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
    fn AudioObjectGetPropertyDataSize(id: u32, addr: *const Addr, qual_size: u32, qual: *const c_void, size: *mut u32) -> i32;
    fn AudioHardwareCreateAggregateDevice(desc: *const c_void, out: *mut u32) -> i32;
    fn AudioHardwareDestroyAggregateDevice(id: u32) -> i32;
    fn AudioObjectHasProperty(id: u32, addr: *const Addr) -> u8;
    fn AudioObjectAddPropertyListener(id: u32, addr: *const Addr, listener: Listener, client: *mut c_void) -> i32;
    fn AudioObjectRemovePropertyListener(id: u32, addr: *const Addr, listener: Listener, client: *mut c_void) -> i32;
}

struct Watch {
    sink: EventSink,
    /// The device whose volume is being watched (0 = none).
    device: u32,
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

/// The Bluetooth outputs connected at the last listing: (UID, name). None
/// before the first, which is taken silently (what's already connected isn't news).
static BLUETOOTH_NOW: Mutex<Option<Vec<(String, String)>>> = Mutex::new(None);

/// Device list changes heard (the smoke test's proof that the listener runs).
static LIST_CHANGES: AtomicU32 = AtomicU32::new(0);

fn get<T: Copy>(id: u32, addr: Addr, default: T) -> Option<T> {
    let mut v = default;
    let mut size = std::mem::size_of::<T>() as u32;
    let r = unsafe { AudioObjectGetPropertyData(id, &addr, 0, std::ptr::null(), &mut size, &mut v as *mut T as *mut c_void) };
    (r == 0).then_some(v)
}

fn has(id: u32, addr: Addr) -> bool {
    unsafe { AudioObjectHasProperty(id, &addr) != 0 }
}

fn default_device() -> Option<u32> {
    get(SYSTEM, Addr { selector: DEFAULT_OUTPUT, scope: SCOPE_GLOBAL, element: 0 }, 0u32).filter(|d| *d != 0)
}

/// (level 0..=1, muted) of `device`: its main volume, or the mean of its
/// channels when it only has per-channel volume.
fn read(device: u32) -> Option<(f32, bool)> {
    let main = Addr { selector: VOLUME, scope: SCOPE_OUTPUT, element: 0 };
    let level = if has(device, main) {
        get(device, main, 0f32)?
    } else {
        let ch: Vec<f32> = [1, 2].iter().filter_map(|&e| get(device, Addr { selector: VOLUME, scope: SCOPE_OUTPUT, element: e }, 0f32)).collect();
        if ch.is_empty() {
            return None;
        }
        ch.iter().sum::<f32>() / ch.len() as f32
    };
    let muted = get(device, Addr { selector: MUTE, scope: SCOPE_OUTPUT, element: 0 }, 0u32).unwrap_or(0) != 0;
    Some((level.clamp(0.0, 1.0), muted))
}

/// The current level, if there is an output device (for the smoke test).
pub fn current() -> Option<(f32, bool)> {
    default_device().and_then(read)
}

fn device_addrs() -> Vec<Addr> {
    let mut v: Vec<Addr> = ELEMENTS.iter().map(|&e| Addr { selector: VOLUME, scope: SCOPE_OUTPUT, element: e }).collect();
    v.push(Addr { selector: MUTE, scope: SCOPE_OUTPUT, element: 0 });
    v
}

fn running_addr() -> Addr {
    Addr { selector: RUNNING, scope: SCOPE_GLOBAL, element: 0 }
}

fn running(device: u32) -> bool {
    device != 0 && get(device, running_addr(), 0u32).unwrap_or(0) != 0
}

/// Whether some app plays sound through the default output now.
pub fn sound_running() -> bool {
    default_device().is_some_and(running)
}

extern "C" fn on_running(id: u32, _n: u32, _addrs: *const Addr, _client: *mut c_void) -> i32 {
    let watched = WATCH.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|w| w.device == id));
    if watched {
        super::media::sound(running(id));
    }
    0
}

extern "C" fn on_volume(id: u32, _n: u32, _addrs: *const Addr, _client: *mut c_void) -> i32 {
    let sink = WATCH.lock().ok().and_then(|g| g.as_ref().filter(|w| w.device == id).map(|w| w.sink.clone()));
    if let (Some(sink), Some((level, muted))) = (sink, read(id)) {
        sink.send(AppEvent::Volume { level, muted, switched: false });
    }
    0
}

/// Every audio device there is now.
fn devices() -> Vec<u32> {
    let addr = Addr { selector: DEVICES, scope: SCOPE_GLOBAL, element: 0 };
    let mut size = 0u32;
    if unsafe { AudioObjectGetPropertyDataSize(SYSTEM, &addr, 0, std::ptr::null(), &mut size) } != 0 {
        return Vec::new();
    }
    let mut ids = vec![0u32; size as usize / 4];
    let r = unsafe { AudioObjectGetPropertyData(SYSTEM, &addr, 0, std::ptr::null(), &mut size, ids.as_mut_ptr() as *mut c_void) };
    if r != 0 {
        return Vec::new();
    }
    ids.truncate(size as usize / 4);
    ids
}

/// A device's string property (its UID or name).
fn text_of(id: u32, selector: u32) -> Option<String> {
    let s = get(id, Addr { selector, scope: SCOPE_GLOBAL, element: 0 }, std::ptr::null::<c_void>())?;
    if s.is_null() {
        return None;
    }
    let t = cf::text(s);
    // The property hands over a reference of its own.
    unsafe { cf::CFRelease(s) };
    t
}

/// Bluetooth headphones and speakers connected now: devices on a Bluetooth
/// transport that play sound (a headset's microphone alone doesn't count).
fn bluetooth_outputs() -> Vec<(String, String)> {
    devices()
        .into_iter()
        .filter(|&d| get(d, Addr { selector: TRANSPORT, scope: SCOPE_GLOBAL, element: 0 }, 0u32).is_some_and(|t| BLUETOOTH.contains(&t)))
        .filter(|&d| {
            let mut size = 0u32;
            let addr = Addr { selector: STREAMS, scope: SCOPE_OUTPUT, element: 0 };
            unsafe { AudioObjectGetPropertyDataSize(d, &addr, 0, std::ptr::null(), &mut size) == 0 && size > 0 }
        })
        .filter_map(|d| {
            let uid = text_of(d, UID)?;
            let name = text_of(d, NAME).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| FALLBACK_NAME.to_string());
            Some((uid, name))
        })
        .collect()
}

/// The device list changed: Bluetooth outputs that came are connected, those
/// that left are gone (by UID: Core Audio may give a device a new number).
extern "C" fn on_devices(_id: u32, _n: u32, _addrs: *const Addr, _client: *mut c_void) -> i32 {
    LIST_CHANGES.fetch_add(1, Ordering::Relaxed);
    let now = bluetooth_outputs();
    let changes: Vec<(String, bool)> = {
        let Ok(mut g) = BLUETOOTH_NOW.lock() else { return 0 };
        let before = g.replace(now.clone()).unwrap_or_else(|| now.clone());
        let came = now.iter().filter(|d| !before.iter().any(|b| b.0 == d.0)).map(|d| (d.1.clone(), true));
        let went = before.iter().filter(|b| !now.iter().any(|d| d.0 == b.0)).map(|b| (b.1.clone(), false));
        came.chain(went).collect()
    };
    if changes.is_empty() {
        return 0;
    }
    let Some(sink) = WATCH.lock().ok().and_then(|g| g.as_ref().map(|w| w.sink.clone())) else { return 0 };
    for (name, connected) in changes {
        pinfo!("audio: Bluetooth output {}: {name}", if connected { "connected" } else { "gone" });
        sink.send(AppEvent::Bluetooth { name, connected, battery: None, audio: true });
    }
    0
}

/// Device list changes heard so far.
pub fn list_changes() -> u32 {
    LIST_CHANGES.load(Ordering::Relaxed)
}

/// The smoke test: add a device only this app can see and take it away again
/// (an empty, private aggregate device), so the device list changes on a Mac
/// with no Bluetooth headphones to connect.
pub fn smoke_device_change() -> Result<(), i32> {
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSNumber, NSString};
    let keys = [NSString::from_str("uid"), NSString::from_str("name"), NSString::from_str("private")];
    let (uid, name, private) = (NSString::from_str("com.oyenza.poni.smoke"), NSString::from_str("Oyenza Studio smoke test"), NSNumber::numberWithInt(1));
    let values: [&AnyObject; 3] = [uid.as_ref(), name.as_ref(), private.as_ref()];
    let desc = NSDictionary::from_slices(&[&*keys[0], &*keys[1], &*keys[2]], &values);
    let mut id = 0u32;
    // NSDictionary is a CFDictionary (toll-free bridged).
    let r = unsafe { AudioHardwareCreateAggregateDevice(objc2::rc::Retained::as_ptr(&desc) as *const c_void, &mut id) };
    if r != 0 {
        return Err(r);
    }
    let r = unsafe { AudioHardwareDestroyAggregateDevice(id) };
    if r != 0 {
        return Err(r);
    }
    Ok(())
}

extern "C" fn on_default(_id: u32, _n: u32, _addrs: *const Addr, _client: *mut c_void) -> i32 {
    follow_default();
    0
}

/// Move the volume listeners to the current default output device. The lock
/// is never held while listeners are added or removed: Core Audio may wait
/// for a running listener, which takes the same lock.
fn follow_default() {
    let new = default_device().unwrap_or(0);
    let (old, sink) = {
        let mut g = match WATCH.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(w) = g.as_mut() else { return };
        if w.device == new {
            return;
        }
        let old = w.device;
        w.device = new;
        (old, w.sink.clone())
    };
    for a in device_addrs() {
        unsafe {
            if old != 0 {
                AudioObjectRemovePropertyListener(old, &a, on_volume, std::ptr::null_mut());
            }
            if new != 0 && has(new, a) {
                AudioObjectAddPropertyListener(new, &a, on_volume, std::ptr::null_mut());
            }
        }
    }
    let a = running_addr();
    unsafe {
        if old != 0 {
            AudioObjectRemovePropertyListener(old, &a, on_running, std::ptr::null_mut());
        }
        if new != 0 && has(new, a) {
            AudioObjectAddPropertyListener(new, &a, on_running, std::ptr::null_mut());
        }
    }
    super::media::sound(running(new));
    if let Some((level, muted)) = (new != 0).then(|| read(new)).flatten() {
        sink.send(AppEvent::Volume { level, muted, switched: true });
    }
}

/// Send `AppEvent::Volume` now and on every change.
pub fn start(sink: EventSink) {
    if let Ok(mut g) = WATCH.lock() {
        *g = Some(Watch { sink, device: 0 });
    }
    let addr = Addr { selector: DEFAULT_OUTPUT, scope: SCOPE_GLOBAL, element: 0 };
    let r = unsafe { AudioObjectAddPropertyListener(SYSTEM, &addr, on_default, std::ptr::null_mut()) };
    if r != 0 {
        pwarn!("audio: no default-device notifications ({r})");
    }
    follow_default();
    if default_device().is_none() {
        pinfo!("audio: no output device");
    }
    // Headphones: what's connected now is taken silently, then every change of
    // the device list is compared with it.
    let now = bluetooth_outputs();
    pinfo!("audio: {} Bluetooth output(s) connected", now.len());
    if let Ok(mut g) = BLUETOOTH_NOW.lock() {
        *g = Some(now);
    }
    let addr = Addr { selector: DEVICES, scope: SCOPE_GLOBAL, element: 0 };
    let r = unsafe { AudioObjectAddPropertyListener(SYSTEM, &addr, on_devices, std::ptr::null_mut()) };
    if r != 0 {
        pwarn!("audio: no device-list notifications ({r}); no headphone pop-ups");
    }
}
