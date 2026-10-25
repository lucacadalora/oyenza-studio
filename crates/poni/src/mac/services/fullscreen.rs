//! Full-screen apps on the Mac: the island steps off them, as on Windows
//! (Settings › In full-screen apps, or the same submenu in the menu bar).
//!
//! The question is the one Windows asks: is the app in front full screen on
//! the island's screen? The window list answers it (a public call): one of
//! that app's windows covers nearly all of the screen and the menu bar
//! doesn't show there, as in a full-screen Space or under a borderless game
//! that hides it. (macOS doesn't give other apps the presentation options of
//! the app in front: they read 0.) Bounds and owners need no permission;
//! window titles would, and aren't read.
//!
//! It's asked when NSWorkspace says the app in front or the Space changed,
//! and once more a moment after a Space change (a full-screen window is
//! still sliding into its Space when the change is announced). Nothing polls.
//! The app then hides or shows the island itself (`window::set_shown`):
//! macOS carries a panel that is on every Space onto a full-screen app's
//! Space as it opens.

use super::cf;
use crate::events::{AppEvent, EventSink};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidActivateApplicationNotification,
};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSTimer};
use std::ptr::NonNull;
use std::sync::Mutex;

/// `kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements`.
const ON_SCREEN: u32 = (1 << 0) | (1 << 4);
/// `kCGMainMenuWindowLevel`: the menu bar.
const MENU_BAR_LEVEL: i64 = 24;
/// Of the screen an app's window must cover. Under a MacBook's notch a
/// full-screen window starts below the menu bar's strip: 94% of the screen
/// at the largest text size.
const COVER: f64 = 0.9;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> cf::CFTypeRef;
    fn CGRectMakeWithDictionaryRepresentation(dict: cf::CFTypeRef, rect: *mut CGRect) -> bool;
}

static SINK: Mutex<Option<EventSink>> = Mutex::new(None);
/// The last answer, so the log says only what changed.
static LAST: Mutex<Option<bool>> = Mutex::new(None);

/// What the window list says about one on-screen window.
struct Win {
    pid: i64,
    layer: i64,
    bounds: CGRect,
}

fn on_screen_windows() -> Vec<Win> {
    let list = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN, 0) };
    if list.is_null() {
        return Vec::new();
    }
    let k_bounds = cf::key("kCGWindowBounds");
    let mut out = Vec::new();
    for i in 0..unsafe { cf::CFArrayGetCount(list) } {
        let d = unsafe { cf::CFArrayGetValueAtIndex(list, i) };
        let (Some(pid), Some(layer)) = (cf::dict_i64(d, "kCGWindowOwnerPID"), cf::dict_i64(d, "kCGWindowLayer")) else { continue };
        let b = unsafe { cf::CFDictionaryGetValue(d, &*k_bounds as *const _ as cf::CFTypeRef) };
        let mut bounds = CGRect::default();
        if b.is_null() || !unsafe { CGRectMakeWithDictionaryRepresentation(b, &mut bounds) } {
            continue;
        }
        out.push(Win { pid, layer, bounds });
    }
    unsafe { cf::CFRelease(list) };
    out
}

/// Of `screen`, the share `r` covers.
fn share(r: &CGRect, screen: &NSRect) -> f64 {
    let w = (r.origin.x + r.size.width).min(screen.origin.x + screen.size.width) - r.origin.x.max(screen.origin.x);
    let h = (r.origin.y + r.size.height).min(screen.origin.y + screen.size.height) - r.origin.y.max(screen.origin.y);
    let area = screen.size.width * screen.size.height;
    if w <= 0.0 || h <= 0.0 || area <= 0.0 {
        0.0
    } else {
        w * h / area
    }
}

/// Whether the app in front is full screen on the island's screen, and why
/// (for the log).
pub fn front_app_full_screen() -> (bool, String) {
    let Some(front) = NSWorkspace::sharedWorkspace().frontmostApplication() else { return (false, "no app in front".into()) };
    let pid = front.processIdentifier() as i64;
    let Some(screen) = crate::window::screen_rect_top_left() else { return (false, "the island's screen is unknown".into()) };
    let wins = on_screen_windows();
    let cover = wins.iter().filter(|w| w.pid == pid && w.layer == 0).map(|w| share(&w.bounds, &screen)).fold(0.0, f64::max);
    let menu_bar = wins.iter().any(|w| w.layer == MENU_BAR_LEVEL && share(&w.bounds, &screen) > 0.0);
    (!menu_bar && cover >= COVER, format!("the app in front covers {:.0}% of the screen, menu bar shown: {menu_bar}", cover * 100.0))
}

fn report() {
    let (full, why) = front_app_full_screen();
    let changed = LAST.lock().map(|mut l| l.replace(full) != Some(full)).unwrap_or(true);
    if let Ok(g) = SINK.lock() {
        if let Some(s) = g.as_ref() {
            s.send(AppEvent::Fullscreen(full));
            if changed {
                s.send(AppEvent::Log(format!("full screen: {full} ({why})")));
            }
        }
    }
}

define_class!(
    /// Hears the app in front and the Space change (main thread).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PoniFullScreenObserver"]
    struct Observer;

    unsafe impl NSObjectProtocol for Observer {}

    impl Observer {
        #[unsafe(method(appChanged:))]
        fn app_changed(&self, _n: &NSNotification) {
            report();
        }

        #[unsafe(method(spaceChanged:))]
        fn space_changed(&self, _n: &NSNotification) {
            report();
            // A full-screen window is still sliding into its Space when the change is announced.
            let again = RcBlock::new(|_t: NonNull<NSTimer>| report());
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.6, false, &again) };
        }
    }
);

/// Start listening, for the app's life.
pub fn watch(mtm: MainThreadMarker, sink: EventSink) {
    if let Ok(mut g) = SINK.lock() {
        *g = Some(sink);
    }
    let observer: Retained<Observer> = unsafe { msg_send![Observer::alloc(mtm), init] };
    let obs: &AnyObject = observer.as_ref();
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    unsafe {
        center.addObserver_selector_name_object(obs, sel!(appChanged:), Some(NSWorkspaceDidActivateApplicationNotification), None);
        center.addObserver_selector_name_object(obs, sel!(spaceChanged:), Some(NSWorkspaceActiveSpaceDidChangeNotification), None);
    }
    std::mem::forget(observer);
    report();
}

/// Ask again now (the island moved to another screen).
pub fn check() {
    report();
}

/// The smoke test's full-screen app (`poni --full-screen-window <seconds>`):
/// a plain window that goes full screen into a Space of its own, as a video
/// or a game does, and quits after the given seconds.
pub fn test_window(secs: f64) -> i32 {
    let Some(mtm) = MainThreadMarker::new() else { return 2 };
    let app = NSApplication::sharedApplication(mtm);
    // Full screen is for apps with a Dock icon (the island has none).
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let rect = NSRect::new(NSPoint::new(160.0, 160.0), NSSize::new(640.0, 400.0));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable;
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false) };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
    window.setBackgroundColor(Some(&NSColor::systemTealColor()));
    window.makeKeyAndOrderFront(None);
    app.activate();
    let full = RcBlock::new(move |_t: NonNull<NSTimer>| window.toggleFullScreen(None));
    let quit = RcBlock::new(|_t: NonNull<NSTimer>| std::process::exit(0));
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.5, false, &full);
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(secs.max(1.0), false, &quit);
    }
    app.run();
    0
}
