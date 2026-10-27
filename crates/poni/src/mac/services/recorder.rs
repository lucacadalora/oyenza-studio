//! Oyenza Studio Recorder on the Mac: the separate recorder app built from
//! Cap. The island starts, pauses and stops its recordings through the
//! recorder's own links (`poni_core::recorder`), and the recorder tells the
//! island how they go through the island's message port ([`listen`]): the
//! same messages and 1/0 replies as `WM_COPYDATA` on Windows.
//!
//! Nothing here runs at rest: the port is a Mach port the main run loop
//! waits on, [`installed`] is read at start and as the island opens, and
//! [`running`] only while a recording is under way.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSRunningApplication, NSScreen, NSWorkspace};
use objc2_core_foundation::{kCFRunLoopCommonModes, CFData, CFMessagePort, CFRetained, CFRunLoop, CFRunLoopSource, CFString};
use objc2_foundation::{NSString, NSURL};
use poni_core::recorder;
use std::ffi::c_void;

/// The island's message port, which the recorder looks up by this name.
const PORT: &str = "com.oyenza.poni.recorder";
/// The recorder's bundle identifier (its `branding/apply.py`).
const RECORDER_ID: &str = "com.oyenza.studio.recorder";

/// Whether some app opens `oyenza-recorder:` links, i.e. the recorder is
/// installed. Read when the menu bar's menu is built.
pub fn installed() -> bool {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&format!("{}://", recorder::SCHEME))) else {
        return false;
    };
    NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url).is_some()
}

/// The Record page's Record: `target` (the island's screen, or a window or
/// an area the recorder's overlay asks for) with `inputs`.
pub fn record(mtm: MainThreadMarker, target: recorder::Target, inputs: recorder::Inputs) {
    let screen = if target == recorder::Target::Screen { screen_name(mtm) } else { None };
    super::open(&recorder::record_link(target, screen.as_deref(), inputs));
}

pub fn pause() {
    super::open(&recorder::pause_link());
}

pub fn resume() {
    super::open(&recorder::resume_link());
}

pub fn stop() {
    super::open(&recorder::stop_link());
}

/// The recorder is still running (it quits or crashes mid-recording: the
/// island lets go of the recording).
pub fn running() -> bool {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(RECORDER_ID)).count() > 0
}

/// The island's message port on the main run loop, kept for the app's life.
pub struct Port {
    _port: CFRetained<CFMessagePort>,
    _source: CFRetained<CFRunLoopSource>,
}

/// Open the island's message port: each request from the recorder goes to
/// `on_message` on the main thread, and its answer is the reply. None when
/// the name is taken (another copy of the island) or CF says no.
pub fn listen() -> Option<Port> {
    let name = CFString::from_str(PORT);
    // SAFETY: a null context and no should-free flag are allowed; the callout
    // only reads the request's bytes while it runs.
    let port = unsafe { CFMessagePort::new_local(None, Some(&*name), Some(on_request), std::ptr::null_mut(), std::ptr::null_mut()) }?;
    let source = CFMessagePort::new_run_loop_source(None, Some(&*port), 0)?;
    // Common modes: the recorder's word arrives while a menu is open too.
    CFRunLoop::main()?.add_source(Some(&*source), unsafe { kCFRunLoopCommonModes });
    Some(Port { _port: port, _source: source })
}

/// One request through the port: "1" when the island took the message, "0"
/// when it didn't, or couldn't read it. CF releases the reply.
unsafe extern "C-unwind" fn on_request(_port: *mut CFMessagePort, msgid: i32, data: *const CFData, _info: *mut c_void) -> *const CFData {
    let taken = msgid as usize == recorder::COPYDATA_ID
        && unsafe { data.as_ref() }
            .map(|d| d.to_vec())
            .filter(|b| b.len() <= recorder::MESSAGE_MAX)
            .and_then(|b| recorder::parse_message(&b))
            .is_some_and(crate::app::recorder_message);
    CFRetained::into_raw(CFData::from_bytes(if taken { b"1" } else { b"0" })).as_ptr()
}

/// Record the island's screen with the camera, microphone and system sound
/// the recorder used last time.
pub fn record_screen(mtm: MainThreadMarker) {
    match screen_name(mtm) {
        Some(name) => super::open(&recorder::record_screen_link(&name)),
        None => crate::pwarn!("recorder: the island's screen has no name"),
    }
}

/// The screen the island sits on (`window::pick_screen`: the notch screen,
/// else the menu-bar screen), by the name the recorder gives screens.
fn screen_name(mtm: MainThreadMarker) -> Option<String> {
    let island = crate::window::pick_screen(mtm)?;
    NSScreen::screens(mtm)
        .iter()
        .find(|s| {
            let f = s.frame();
            f.origin.x == island.x && f.origin.y == island.y && f.size.width == island.w && f.size.height == island.h
        })
        .map(|s| s.localizedName().to_string())
}
