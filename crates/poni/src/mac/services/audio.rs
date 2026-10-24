//! Output volume and mute (Core Audio), with a notification on every change:
//! the volume pop-up. Follows the default output device when it changes
//! (headphones plugged in, AirPods connected).
//!
//! Core Audio calls the listeners on its own thread; they only read the new
//! level and send it to the main thread.

use crate::events::{AppEvent, EventSink};
use std::ffi::c_void;
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
/// The main element, then the left and right channels.
const ELEMENTS: [u32; 3] = [0, 1, 2];

type Listener = extern "C" fn(u32, u32, *const Addr, *mut c_void) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(id: u32, addr: *const Addr, qual_size: u32, qual: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
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

extern "C" fn on_volume(id: u32, _n: u32, _addrs: *const Addr, _client: *mut c_void) -> i32 {
    let sink = WATCH.lock().ok().and_then(|g| g.as_ref().filter(|w| w.device == id).map(|w| w.sink.clone()));
    if let (Some(sink), Some((level, muted))) = (sink, read(id)) {
        sink.send(AppEvent::Volume { level, muted });
    }
    0
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
    if let Some((level, muted)) = (new != 0).then(|| read(new)).flatten() {
        sink.send(AppEvent::Volume { level, muted });
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
}
