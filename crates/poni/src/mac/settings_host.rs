//! The Settings window on the Mac: a real window, with the system's title
//! bar buttons over the sidebar as in System Settings, that shows the
//! Settings content (`settings_window::View`). The island's gear and the
//! menu bar item's Settings… open it, ⌘W and its close button close it, and
//! it is light or dark as the Mac is. The window is made when it opens and
//! let go when it closes, so nothing of it stays at rest.
//!
//! Everything here runs on the main thread. The window and its view live in
//! a thread-local; the view draws and forwards input through the app
//! (`crate::app::draw_settings`, `settings_input`).

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AllocAnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSBackingStoreType, NSEvent, NSEventModifierFlags,
    NSGraphicsContext, NSResponder, NSScreen, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{ns_string, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize};
use std::cell::RefCell;

/// The window's content size (points).
pub const WIDTH: f64 = crate::settings_window::CARD_W as f64;
pub const HEIGHT: f64 = 600.0;
/// The title bar: the window's buttons sit in it, over the sidebar.
pub const TITLEBAR: f32 = 28.0;

/// Input the view forwards to the app (points in the view, y down).
#[derive(Clone, Copy, Debug)]
pub enum Input {
    Down { x: f32, y: f32 },
    Up { x: f32, y: f32 },
    Move { x: f32, y: f32 },
    Leave,
    Wheel { x: f32, y: f32, notches: f32 },
}

define_class!(
    /// The Settings window: tells the app when it closes.
    #[unsafe(super(NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "StudioSettingsWindow"]
    pub struct SettingsWindow;

    unsafe impl NSObjectProtocol for SettingsWindow {}

    impl SettingsWindow {
        #[unsafe(method(close))]
        fn close(&self) {
            unsafe {
                let _: () = msg_send![super(self), close];
            }
            crate::app::settings_closed();
        }
    }
);

define_class!(
    /// Draws the Settings content (y down, like the Windows canvas) and
    /// forwards input.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "StudioSettingsView"]
    pub struct SettingsView;

    unsafe impl NSObjectProtocol for SettingsView {}

    impl SettingsView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        // The view moves the window itself, from its empty parts.
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if let Some(ctx) = NSGraphicsContext::currentContext() {
                crate::app::draw_settings(&ctx.CGContext());
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let (x, y) = self.local(event);
            if crate::app::settings_input(Input::Down { x, y }) {
                // Nothing there takes the press: the window moves with the drag.
                if let Some(win) = self.window() {
                    win.performWindowDragWithEvent(event);
                }
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let (x, y) = self.local(event);
            crate::app::settings_input(Input::Up { x, y });
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let (x, y) = self.local(event);
            crate::app::settings_input(Input::Move { x, y });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let (x, y) = self.local(event);
            crate::app::settings_input(Input::Move { x, y });
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            crate::app::settings_input(Input::Leave);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // Wheel notches, as on Windows; a trackpad's points become fractions of one.
            let dy = event.scrollingDeltaY() as f32;
            let notches = if event.hasPreciseScrollingDeltas() { dy / 24.0 } else { dy };
            if notches != 0.0 {
                let (x, y) = self.local(event);
                crate::app::settings_input(Input::Wheel { x, y, notches });
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            // ⌘W closes it, as any Mac window.
            let command = event.modifierFlags().contains(NSEventModifierFlags::Command);
            let text = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
            if command && text.eq_ignore_ascii_case("w") {
                if let Some(win) = self.window() {
                    win.performClose(None);
                }
            }
        }

        // Light or dark: the Mac switched, so the window draws again.
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            self.setNeedsDisplay(true);
        }
    }
);

impl SettingsView {
    /// An event's position in the view (points, y down).
    fn local(&self, event: &NSEvent) -> (f32, f32) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        (p.x as f32, p.y as f32)
    }
}

struct Host {
    window: Retained<SettingsWindow>,
    view: Retained<SettingsView>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// The window is open.
pub fn is_open() -> bool {
    HOST.with(|h| h.borrow().is_some())
}

/// Make the window (not shown yet: `show`), centred on the screen with the
/// menu bar. Returns its backing scale.
pub fn create(mtm: MainThreadMarker) -> f32 {
    if is_open() {
        return scale();
    }
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable | NSWindowStyleMask::FullSizeContentView;
    let window: Retained<SettingsWindow> =
        unsafe { msg_send![SettingsWindow::alloc(mtm), initWithContentRect: rect, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false] };
    window.setTitle(ns_string!("Oyenza Studio Settings"));
    window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    window.setTitlebarAppearsTransparent(true);
    window.setAcceptsMouseMovedEvents(true);
    unsafe { window.setReleasedWhenClosed(false) };

    let view: Retained<SettingsView> = unsafe { msg_send![SettingsView::alloc(mtm), initWithFrame: rect] };
    // Hover: the pointer's moves over the view and its leaving it.
    let options = NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
    let owner: &AnyObject = view.as_ref();
    let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), rect, options, Some(owner), None) };
    view.addTrackingArea(&area);
    window.setContentView(Some(&view));
    window.makeFirstResponder(Some(&view));
    window.center();
    let s = window.backingScaleFactor() as f32;
    HOST.with(|h| *h.borrow_mut() = Some(Host { window, view }));
    s
}

/// Bring the window to the front, and the app with it.
pub fn show() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    HOST.with(|h| {
        if let Some(host) = h.borrow().as_ref() {
            host.window.makeKeyAndOrderFront(None);
        }
    });
    NSApplication::sharedApplication(mtm).activate();
}

/// Close it, as its close button does (`SettingsWindow::close` tells the app).
pub fn close() {
    let window = HOST.with(|h| h.borrow().as_ref().map(|host| host.window.clone()));
    if let Some(w) = window {
        unsafe {
            let _: () = msg_send![&*w, close];
        }
    }
}

/// Let go of the closed window. AppKit may still hold it for the rest of
/// this turn of the run loop, so the last reference goes with its
/// autorelease pool, not here.
pub fn release() {
    if let Some(host) = HOST.with(|h| h.borrow_mut().take()) {
        let _ = Retained::autorelease_ptr(host.window);
        let _ = Retained::autorelease_ptr(host.view);
    }
}

/// Draw the window again (on the next display pass).
pub fn redraw() {
    HOST.with(|h| {
        if let Some(host) = h.borrow().as_ref() {
            host.view.setNeedsDisplay(true);
        }
    });
}

/// The window's backing scale (2 on a Retina screen).
pub fn scale() -> f32 {
    HOST.with(|h| h.borrow().as_ref().map(|host| host.window.backingScaleFactor() as f32)).unwrap_or(2.0)
}

/// Light or dark, as the window shows now (the Mac's mode, or the smoke
/// test's pick).
pub fn is_light() -> bool {
    HOST.with(|h| {
        let host = h.borrow();
        let Some(host) = host.as_ref() else { return false };
        let names = NSArray::from_slice(&[unsafe { NSAppearanceNameAqua }, unsafe { NSAppearanceNameDarkAqua }]);
        let best = host.view.effectiveAppearance().bestMatchFromAppearancesWithNames(&names);
        best.is_none_or(|n| !n.isEqualToString(unsafe { NSAppearanceNameDarkAqua }))
    })
}

/// The smoke test's light (or dark) window, whatever mode the Mac is in.
pub fn force_appearance(light: bool) {
    HOST.with(|h| {
        if let Some(host) = h.borrow().as_ref() {
            let name = if light { unsafe { NSAppearanceNameAqua } } else { unsafe { NSAppearanceNameDarkAqua } };
            host.window.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
            host.view.setNeedsDisplay(true);
        }
    });
}

/// Where the window is, in the window list's coordinates (points, origin at
/// the top left of the main display, y down): the smoke test's picture of it.
pub fn frame_top_left() -> Option<(f64, f64, f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    let primary_h = NSScreen::screens(mtm).iter().next().map(|s| s.frame().size.height)?;
    HOST.with(|h| {
        h.borrow().as_ref().map(|host| {
            let f = host.window.frame();
            (f.origin.x, primary_h - (f.origin.y + f.size.height), f.size.width, f.size.height)
        })
    })
}
