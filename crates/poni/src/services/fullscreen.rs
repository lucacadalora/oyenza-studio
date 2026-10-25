//! Full-screen detection: the island hides while a game, a video or a
//! slideshow covers its monitor.
//!
//! Everything is event-driven (nothing polls):
//! * the shell's `ABN_FULLSCREENAPP` notification, delivered to the helper
//!   window registered as an AppBar. It never calls `ABM_SETPOS`, so it
//!   reserves no screen space;
//! * a global out-of-context `EVENT_SYSTEM_FOREGROUND` WinEvent hook;
//! * `EVENT_OBJECT_LOCATIONCHANGE` hooks scoped to the thread of the foreground
//!   window (and of a full-screen window kept while focus is on another
//!   monitor), so F11 in a browser is noticed without seeing the window and
//!   caret churn of every other app. The callback ignores everything but
//!   `OBJID_WINDOW` of the watched windows, and a move only triggers a full
//!   re-evaluation when it changes how the window relates to the island's
//!   monitor (spans it, covers it, sits on it).
//!
//! Every signal runs the same rule ([`evaluate`]) and only changes are
//! reported. WinEvent callbacks are free functions running on the thread that
//! installed them (the UI thread), so the shared state lives in a thread-local.
//!
//! The same signals keep, for Step aside, the app windows at the top edge of
//! the island's monitor (a maximized browser, a snapped window): looked at
//! from scratch when another window comes to the front,
//! and one window at a time as the watched ones move. The app hears of a
//! change ([`AppEvent::TopWindows`]) and decides whether one sits under the
//! island. With it off (always, since 0.2.6), nothing is looked at.

use crate::events::{AppEvent, EventSink, WM_APP_APPBAR};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{E_UNEXPECTED, HMODULE, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONULL,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{
    SHAppBarMessage, SHQueryUserNotificationState, ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP, APPBARDATA,
    QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
    GetDesktopWindow, GetShellWindow, IsIconic, IsWindow, IsWindowVisible, IsZoomed, CHILDID_SELF, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, GWL_EXSTYLE, GWL_STYLE, GW_CHILD, GW_HWNDNEXT, GW_HWNDPREV, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_THICKFRAME,
};

/// Desktop and shell surfaces that cover a whole monitor without being a
/// full-screen app: the desktop, taskbars, and the Alt+Tab / Task View hosts.
const SHELL_CLASSES: [&str; 8] = [
    // The desktop window itself (GetDesktopWindow); it can be "foreground"
    // when nothing else is, e.g. right after the last window closes.
    "#32769",
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "ForegroundStaging",
    "MultitaskingViewFrame",
    "XamlExplorerHostIslandWindow",
];

/// Pop-ups that may sit above the island: menus, tooltips and the shell's
/// flyouts (Start and Search, the tray overflow, taskbar thumbnails).
const POPUP_CLASSES: [&str; 7] = [
    "#32768",
    "tooltips_class32",
    "Xaml_WindowedPopupClass",
    "Windows.UI.Core.CoreWindow",
    "TaskListThumbnailWnd",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

/// Re-runs of [`evaluate`] when signals arrive while it is running.
const MAX_PASSES: usize = 3;

thread_local! {
    static WATCH: RefCell<Option<Rc<Watch>>> = const { RefCell::new(None) };
}

static WARNED_THREAD_HOOK: AtomicBool = AtomicBool::new(false);

/// Shared by the AppBar path and the WinEvent callbacks (UI thread only).
struct Watch {
    sink: EventSink,
    pid: u32,
    /// The state last reported to the app.
    fullscreen: Cell<bool>,
    /// Cached island window (looked up by class, our process only).
    island: Cell<HWND>,
    /// Foreground window at the last evaluation.
    fg: Cell<Tracked>,
    /// The window that made the island's monitor full-screen. Kept while the
    /// focus is on another monitor (watching a video on one screen while
    /// working on the other); null otherwise.
    culprit: Cell<Tracked>,
    /// The window behind the last "full screen" verdict (the game, for
    /// Show over games); null while the monitor isn't full screen.
    decided: Cell<HWND>,
    thread_hooks: RefCell<Vec<ThreadHooks>>,
    busy: Cell<bool>,
    dirty: Cell<bool>,
    /// Step aside: the thin band along the top edge of the island's monitor
    /// that the app asked about (screen pixels); None while Stay is picked.
    top_band: Cell<Option<RECT>>,
    /// The app windows reaching into that band (at the top edge) and where,
    /// clipped to it, as last reported to the app.
    top: RefCell<Vec<(HWND, [i32; 4])>>,
}

/// A window and its geometry at the last evaluation, so that the per-move
/// location events can cheaply tell whether anything relevant changed.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Tracked {
    hwnd: HWND,
    /// A visible foreign app window whose rect contains the island's monitor.
    spans: bool,
    /// `spans`, and frameless and not maximized: full screen on its own.
    covers: bool,
    on_island_monitor: bool,
}

impl Tracked {
    const NONE: Tracked =
        Tracked { hwnd: HWND(std::ptr::null_mut()), spans: false, covers: false, on_island_monitor: false };
}

pub struct FullscreenWatcher {
    helper: HWND,
    fg_hook: HWINEVENTHOOK,
    appbar: Cell<bool>,
}

impl FullscreenWatcher {
    /// Must be called on the UI thread (it owns the message loop that
    /// delivers the WinEvent callbacks and the AppBar messages).
    pub fn start(helper: HWND, sink: EventSink) -> windows::core::Result<Self> {
        if current().is_some() {
            return Err(windows::core::Error::new(E_UNEXPECTED, "full-screen watcher already running"));
        }
        let appbar = register_appbar(helper);
        let fg_hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                HMODULE::default(),
                Some(on_win_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if fg_hook.is_invalid() {
            if !appbar {
                return Err(windows::core::Error::new(E_UNEXPECTED, "no AppBar and no foreground hook"));
            }
            crate::pwarn!("fullscreen: foreground hook failed; relying on AppBar notifications");
        }
        let watch = Rc::new(Watch {
            sink,
            pid: unsafe { GetCurrentProcessId() },
            fullscreen: Cell::new(false),
            island: Cell::new(HWND::default()),
            fg: Cell::new(Tracked::NONE),
            culprit: Cell::new(Tracked::NONE),
            decided: Cell::new(HWND::default()),
            thread_hooks: RefCell::new(Vec::new()),
            busy: Cell::new(false),
            dirty: Cell::new(false),
            top_band: Cell::new(None),
            top: RefCell::new(Vec::new()),
        });
        let me = Self { helper, fg_hook, appbar: Cell::new(appbar) };
        let stored = WATCH.try_with(|w| match w.try_borrow_mut() {
            Ok(mut g) => {
                *g = Some(watch.clone());
                true
            }
            Err(_) => false,
        });
        if !matches!(stored, Ok(true)) {
            // `me` is dropped here, which removes the AppBar and the hook.
            return Err(windows::core::Error::new(E_UNEXPECTED, "full-screen watcher state unavailable"));
        }
        // Poni may start while something already runs full screen.
        if let Some(on) = reevaluate(&watch, None) {
            watch.sink.send(AppEvent::Fullscreen(on));
        }
        Ok(me)
    }

    /// The helper window's `WM_APP_APPBAR` callback. Returns `Some(state)`
    /// only when the full-screen state of the island's monitor changed.
    pub fn on_appbar_message(&self, wp: WPARAM, _lp: LPARAM, monitor: isize) -> Option<bool> {
        if wp.0 != ABN_FULLSCREENAPP as usize {
            return None;
        }
        // lParam says "opening"/"closing" for *some* monitor; re-evaluate ours.
        let w = current()?;
        let hint = HMONITOR(monitor as *mut core::ffi::c_void);
        let on = reevaluate(&w, Some(hint))?;
        // Also queue the new state behind any WinEvent report still waiting in
        // the channel, so an older report cannot overwrite it. The app ignores
        // the repeat (it only acts when the state differs).
        w.sink.send(AppEvent::Fullscreen(on));
        Some(on)
    }

    /// Re-register the AppBar after Explorer restarted (`TaskbarCreated`):
    /// a new shell has no record of the old registration.
    pub fn on_taskbar_created(&self) {
        if self.appbar.get() {
            unregister_appbar(self.helper);
        }
        self.appbar.set(register_appbar(self.helper));
        if let Some(w) = current() {
            if let Some(on) = reevaluate(&w, None) {
                w.sink.send(AppEvent::Fullscreen(on));
            }
            rescan_top(&w);
        }
    }

    /// Step aside on, with the band along the top edge of the island's
    /// monitor in screen pixels, or off (`None`: Stay). Either way every window is
    /// looked at again: the island's monitor or its scale may have changed.
    pub fn watch_top(&self, band: Option<RECT>) {
        if let Some(w) = current() {
            w.top_band.set(band);
            rescan_top(&w);
        }
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for FullscreenWatcher {
    fn drop(&mut self) {
        let watch = WATCH.try_with(|w| w.try_borrow_mut().ok().and_then(|mut g| g.take())).ok().flatten();
        if let Some(w) = watch {
            // Unhooks every per-thread hook.
            drop(w.thread_hooks.take());
        }
        if !self.fg_hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.fg_hook);
            }
        }
        if self.appbar.get() {
            unregister_appbar(self.helper);
        }
    }
}

// ------------------------------------------------------------------ AppBar

fn appbar_data(helper: HWND) -> APPBARDATA {
    APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        hWnd: helper,
        uCallbackMessage: WM_APP_APPBAR,
        ..Default::default()
    }
}

fn register_appbar(helper: HWND) -> bool {
    let mut abd = appbar_data(helper);
    let ok = unsafe { SHAppBarMessage(ABM_NEW, &mut abd) } != 0;
    if !ok {
        crate::pwarn!("fullscreen: AppBar registration failed; relying on window events");
    }
    ok
}

fn unregister_appbar(helper: HWND) {
    let mut abd = appbar_data(helper);
    unsafe {
        SHAppBarMessage(ABM_REMOVE, &mut abd);
    }
}

// ------------------------------------------------------------------ WinEvents

fn current() -> Option<Rc<Watch>> {
    WATCH.try_with(|w| w.try_borrow().ok().and_then(|g| g.clone())).ok().flatten()
}

unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    idobject: i32,
    idchild: i32,
    _thread: u32,
    _time: u32,
) {
    // Cheapest filter first: caret and cursor events end here.
    if event != EVENT_SYSTEM_FOREGROUND && (idobject != OBJID_WINDOW.0 || idchild != CHILDID_SELF as i32) {
        return;
    }
    let Some(w) = current() else { return };
    if event == EVENT_OBJECT_LOCATIONCHANGE {
        let fg = w.fg.get();
        let culprit = w.culprit.get();
        let last = if hwnd == fg.hwnd {
            fg
        } else if hwnd == culprit.hwnd {
            culprit
        } else {
            return;
        };
        // Step aside: maximized, restored, snapped or dragged, the window
        // may have come to the top of the island's monitor or left it.
        moved_top(&w, hwnd);
        // Window drags fire this once per mouse move: only a change in what
        // the rule looks at is worth a full evaluation.
        let mon = island_monitor(&w, None);
        if track(&w, hwnd, mon, monitor_rect(mon).as_ref()) == last {
            return;
        }
    } else if event != EVENT_SYSTEM_FOREGROUND {
        // EVENT_OBJECT_DESTROY ..= EVENT_OBJECT_HIDE: only the kept window matters.
        if hwnd.is_invalid() || hwnd != w.culprit.get().hwnd {
            return;
        }
        moved_top(&w, hwnd);
    } else {
        // Another window in front: what reaches the top may have changed
        // with it (one minimized or closed, another virtual desktop).
        rescan_top(&w);
    }
    match reevaluate(&w, None) {
        Some(on) => w.sink.send(AppEvent::Fullscreen(on)),
        // Still full-screen, but the new foreground window (a game often makes
        // itself topmost) may now sit above the island: report it again so the
        // app can raise the island.
        None if event == EVENT_SYSTEM_FOREGROUND && w.fullscreen.get() => w.sink.send(AppEvent::Fullscreen(true)),
        // Any other new foreground window can do the same (a browser left
        // topmost after F11, a tool's "always on top").
        None if event == EVENT_SYSTEM_FOREGROUND => w.sink.send(AppEvent::Foreground),
        None => {}
    }
}

/// Location hooks (and, for the kept full-screen window, destroy/hide hooks)
/// on one foreign UI thread.
struct ThreadHooks {
    tid: u32,
    life: bool,
    hooks: [HWINEVENTHOOK; 2],
}

impl ThreadHooks {
    fn install(tid: u32, pid: u32, life: bool) -> Option<Self> {
        let hook = |min: u32, max: u32| unsafe {
            SetWinEventHook(min, max, HMODULE::default(), Some(on_win_event), pid, tid, WINEVENT_OUTOFCONTEXT)
        };
        let loc = hook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE);
        if loc.is_invalid() {
            if !WARNED_THREAD_HOOK.swap(true, Ordering::Relaxed) {
                crate::pwarn!("fullscreen: location hook failed ({})", windows::core::Error::from_win32());
            }
            return None;
        }
        // DESTROY..=HIDE also spans SHOW; the callback filters by window.
        let gone = if life { hook(EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE) } else { HWINEVENTHOOK::default() };
        Some(Self { tid, life, hooks: [loc, gone] })
    }
}

impl Drop for ThreadHooks {
    fn drop(&mut self) {
        for h in self.hooks {
            if !h.is_invalid() {
                unsafe {
                    let _ = UnhookWinEvent(h);
                }
            }
        }
    }
}

/// Keep location hooks on the threads of the foreground window and the kept
/// full-screen window (at most two threads), re-using hooks that still fit.
fn update_thread_hooks(w: &Watch) {
    let fg = w.fg.get().hwnd;
    let culprit = w.culprit.get().hwnd;
    let mut want: Vec<(u32, u32, bool)> = Vec::with_capacity(2);
    if let Some((tid, pid)) = foreign_thread(w, fg) {
        want.push((tid, pid, false));
    }
    if culprit != fg {
        if let Some((tid, pid)) = foreign_thread(w, culprit) {
            match want.iter_mut().find(|t| t.0 == tid) {
                Some(t) => t.2 = true,
                None => want.push((tid, pid, true)),
            }
        }
    }
    let mut old = w.thread_hooks.take();
    let mut new = Vec::with_capacity(want.len());
    for (tid, pid, life) in want {
        match old.iter().position(|h| h.tid == tid && h.life == life) {
            Some(i) => new.push(old.swap_remove(i)),
            None => new.extend(ThreadHooks::install(tid, pid, life)),
        }
    }
    drop(old); // unhooks threads no longer watched
    w.thread_hooks.replace(new);
}

/// (thread id, process id) of a window owned by another process.
fn foreign_thread(w: &Watch, hwnd: HWND) -> Option<(u32, u32)> {
    if hwnd.is_invalid() {
        return None;
    }
    let mut pid = 0u32;
    let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (tid != 0 && pid != w.pid).then_some((tid, pid))
}

// ------------------------------------------------------------------ the rule

/// Re-evaluate and remember the result; `Some(state)` only on a change.
/// Signals that arrive while an evaluation runs (an OS call may dispatch
/// sent messages and WinEvents) mark it dirty and are folded into a re-run.
fn reevaluate(w: &Watch, hint: Option<HMONITOR>) -> Option<bool> {
    if w.busy.replace(true) {
        w.dirty.set(true);
        return None;
    }
    let before = w.fullscreen.get();
    let mut verdict = Verdict::off();
    for _ in 0..MAX_PASSES {
        w.dirty.set(false);
        verdict = evaluate(w, hint);
        if !w.dirty.get() {
            break;
        }
    }
    w.fullscreen.set(verdict.on);
    w.decided.set(if verdict.on { verdict.hwnd } else { HWND::default() });
    update_thread_hooks(w);
    w.busy.set(false);
    if verdict.on == before {
        return None;
    }
    if verdict.on {
        let class = if verdict.hwnd.is_invalid() { "-".to_string() } else { class_name(verdict.hwnd) };
        crate::pinfo!("fullscreen: {} (window class {class})", verdict.why);
    } else {
        crate::pinfo!("fullscreen: monitor uncovered");
    }
    Some(verdict.on)
}

struct Verdict {
    on: bool,
    why: &'static str,
    /// The window that decided it (its class goes to the log).
    hwnd: HWND,
}

impl Verdict {
    fn off() -> Self {
        Self { on: false, why: "", hwnd: HWND::default() }
    }
    fn on(why: &'static str, hwnd: HWND) -> Self {
        Self { on: true, why, hwnd }
    }
}

/// The island's monitor counts as full-screen when:
/// 1. the foreground window covers it on its own: it spans the monitor, is a
///    foreign app window (not ours, not the desktop or shell) and is
///    frameless and not maximized (see [`is_ordinary_window`]); or
/// 2. the shell reports presentation mode; or
/// 3. the shell reports a full-screen app (`QUNS_BUSY`) or Direct3D full
///    screen, and the foreground window spans the island's monitor. Those
///    states are system-wide, so they alone would also hide the island while
///    a video plays full screen on *another* monitor; requiring the
///    foreground window to span this one keeps the island where the user
///    works, while still catching full-screen windows that keep a frame or a
///    maximized state; or
/// 4. focus is on another monitor and the window that last made this monitor
///    full-screen still covers it.
fn evaluate(w: &Watch, hint: Option<HMONITOR>) -> Verdict {
    let mon = island_monitor(w, hint);
    let rect = monitor_rect(mon);
    let fg = track(w, unsafe { GetForegroundWindow() }, mon, rect.as_ref());
    w.fg.set(fg);
    if fg.covers {
        w.culprit.set(fg);
        return Verdict::on("the foreground window covers the monitor", fg.hwnd);
    }
    let quns = unsafe { SHQueryUserNotificationState() }.unwrap_or(QUERY_USER_NOTIFICATION_STATE(0));
    if quns == QUNS_PRESENTATION_MODE {
        return Verdict::on("presentation mode", fg.hwnd);
    }
    if fg.spans && fg.on_island_monitor {
        if quns == QUNS_RUNNING_D3D_FULL_SCREEN {
            return Verdict::on("Direct3D full screen", fg.hwnd);
        }
        if quns == QUNS_BUSY {
            return Verdict::on("the shell reports a full-screen app", fg.hwnd);
        }
    }
    let kept = w.culprit.get();
    if !fg.on_island_monitor && !kept.hwnd.is_invalid() && kept.hwnd != fg.hwnd {
        let now = track(w, kept.hwnd, mon, rect.as_ref());
        if now.covers {
            w.culprit.set(now);
            return Verdict::on("a full-screen window stays on this monitor", now.hwnd);
        }
    }
    w.culprit.set(Tracked::NONE);
    Verdict::off()
}

fn track(w: &Watch, hwnd: HWND, mon: HMONITOR, rect: Option<&RECT>) -> Tracked {
    if hwnd.is_invalid() {
        // No foreground window (mid-switch): it is on no other monitor.
        return Tracked { on_island_monitor: true, ..Tracked::NONE };
    }
    let on_island_monitor = unsafe {
        let m = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        m.is_invalid() || mon.is_invalid() || m == mon
    };
    let spans = rect.is_some_and(|r| spans(w, hwnd, r));
    let covers = spans && !is_ordinary_window(hwnd);
    Tracked { hwnd, spans, covers, on_island_monitor }
}

/// True when `hwnd` is a visible, foreign, non-shell window whose rect
/// contains the monitor rect `mon` (physical pixels: the process is
/// per-monitor-v2 DPI aware, so both come back unscaled).
fn spans(w: &Watch, hwnd: HWND, mon: &RECT) -> bool {
    unsafe {
        if !IsWindow(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() || !contains(&r, mon) {
            return false;
        }
        if hwnd == GetDesktopWindow() || hwnd == GetShellWindow() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == w.pid || is_cloaked(hwnd) {
            return false;
        }
    }
    let class = class_name(hwnd);
    !SHELL_CLASSES.contains(&class.as_str())
}

/// Maximized or framed (caption / sizing border): an ordinary app window.
/// On a monitor without a taskbar, or with an auto-hiding one, a maximized
/// window spills over the whole monitor without being full screen; browsers,
/// video players and borderless games drop their frame for full screen.
fn is_ordinary_window(hwnd: HWND) -> bool {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        IsZoomed(hwnd).as_bool() || style & WS_CAPTION.0 == WS_CAPTION.0 || style & WS_THICKFRAME.0 != 0
    }
}

fn contains(outer: &RECT, inner: &RECT) -> bool {
    outer.left <= inner.left && outer.top <= inner.top && outer.right >= inner.right && outer.bottom >= inner.bottom
}

/// Hidden by DWM (another virtual desktop, a suspended UWP app).
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    ok.is_ok() && cloaked != 0
}

/// The process of the window that makes the island's monitor full screen
/// (the game, under Show over games). UI thread only; None when nothing does.
pub fn fullscreen_pid() -> Option<u32> {
    let w = current()?;
    if !w.fullscreen.get() {
        return None;
    }
    let hwnd = w.decided.get();
    if hwnd.is_invalid() {
        return None;
    }
    // A Microsoft Store game's frame belongs to Application Frame Host; the
    // game's own process owns the core window inside it.
    let hwnd = if class_name(hwnd) == "ApplicationFrameWindow" {
        match unsafe { FindWindowExW(hwnd, HWND::default(), w!("Windows.UI.Core.CoreWindow"), PCWSTR::null()) } {
            Ok(core) if !core.is_invalid() => core,
            _ => hwnd,
        }
    } else {
        hwnd
    };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0 && pid != w.pid).then_some(pid)
}

/// The class of the first window above the island in the z-order that
/// overlaps it: visible, not cloaked, not ours, and not a pop-up or a shell
/// surface (those may cover the island for a moment). `None` when the island
/// is on top.
pub fn window_above(island: HWND) -> Option<String> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(island, &mut r) }.ok()?;
    let pid = unsafe { GetCurrentProcessId() };
    let mut hwnd = island;
    // Each step is one cheap call; the cap only guards against a z-order
    // that changes under the walk.
    for _ in 0..2048 {
        hwnd = unsafe { GetWindow(hwnd, GW_HWNDPREV) }.ok()?;
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() || is_cloaked(hwnd) {
            continue;
        }
        let mut wr = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut wr) }.is_err()
            || wr.left >= r.right
            || wr.right <= r.left
            || wr.top >= r.bottom
            || wr.bottom <= r.top
        {
            continue;
        }
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == pid {
            continue;
        }
        let class = class_name(hwnd);
        if SHELL_CLASSES.contains(&class.as_str()) || POPUP_CLASSES.contains(&class.as_str()) {
            continue;
        }
        return Some(class);
    }
    None
}

// ------------------------------------------------------------------ step aside

/// Step aside: every app window at the top edge of the island's monitor
/// (reaching into the band along it), looked at from scratch, reported
/// when it changed.
/// With Stay picked, the list empties and nothing is looked at.
fn rescan_top(w: &Watch) {
    let found = w.top_band.get().map(|band| scan_top(w, &band)).unwrap_or_default();
    set_top(w, found);
}

/// A watched window moved, resized or went away: update its own entry
/// without looking at every other window (a drag fires once per mouse move).
fn moved_top(w: &Watch, hwnd: HWND) {
    let Some(band) = w.top_band.get() else { return };
    let now = at_top(w, hwnd, &band);
    let mut list = w.top.borrow().clone();
    match (list.iter().position(|t| t.0 == hwnd), now) {
        (Some(i), Some(r)) => list[i].1 = r,
        (Some(i), None) => {
            list.remove(i);
        }
        (None, Some(r)) => list.push((hwnd, r)),
        (None, None) => return,
    }
    set_top(w, list);
}

/// Keep `list`, and tell the app when the places in it changed.
fn set_top(w: &Watch, list: Vec<(HWND, [i32; 4])>) {
    let places = |l: &[(HWND, [i32; 4])]| {
        let mut v: Vec<[i32; 4]> = l.iter().map(|t| t.1).collect();
        v.sort_unstable();
        v
    };
    let (before, now) = (places(&w.top.borrow()), places(&list));
    w.top.replace(list);
    if now != before {
        w.sink.send(AppEvent::TopWindows(now));
    }
}

/// The app windows reaching into `band`, topmost first.
fn scan_top(w: &Watch, band: &RECT) -> Vec<(HWND, [i32; 4])> {
    let mut found = Vec::new();
    let mut next = unsafe { GetWindow(GetDesktopWindow(), GW_CHILD) };
    // Each step is one cheap call; the cap only guards against a z-order
    // that changes under the walk.
    for _ in 0..4096 {
        let Ok(hwnd) = next else { break };
        if hwnd.is_invalid() {
            break;
        }
        if let Some(r) = at_top(w, hwnd, band) {
            found.push((hwnd, r));
        }
        next = unsafe { GetWindow(hwnd, GW_HWNDNEXT) };
    }
    found
}

/// Where `hwnd` reaches into `band` (clipped to it), when it is an app
/// window people see and switch to there: visible, not minimized or cloaked,
/// not ours, not the desktop, a taskbar or a pop-up, and not an overlay (see
/// [`is_overlay`]).
fn at_top(w: &Watch, hwnd: HWND, band: &RECT) -> Option<[i32; 4]> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let place = clip(&r, band)?;
        if is_overlay(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32) {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == w.pid || is_cloaked(hwnd) {
            return None;
        }
        let class = class_name(hwnd);
        if SHELL_CLASSES.contains(&class.as_str()) || POPUP_CLASSES.contains(&class.as_str()) {
            return None;
        }
        Some(place)
    }
}

/// Extended styles of windows drawn over others without being one people
/// work in: click-through (a graphics driver's overlay, a screen dimmer,
/// a share's border), never activated (on-screen keyboards, islands like
/// ours), or a tool window without a taskbar button (floating toolbars,
/// hover cards). Some stay visible across the whole screen all day.
fn is_overlay(ex: u32) -> bool {
    ex & (WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0) != 0 || (ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0)
}

/// `r` cut to `band` as (left, top, right, bottom); None when they don't meet.
fn clip(r: &RECT, band: &RECT) -> Option<[i32; 4]> {
    let c = [r.left.max(band.left), r.top.max(band.top), r.right.min(band.right), r.bottom.min(band.bottom)];
    (c[0] < c[2] && c[1] < c[3]).then_some(c)
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// The island's monitor: the app's hint (AppBar path), else the monitor the
/// island window is on, else the primary monitor.
fn island_monitor(w: &Watch, hint: Option<HMONITOR>) -> HMONITOR {
    if let Some(h) = hint.filter(|h| monitor_rect(*h).is_some()) {
        return h;
    }
    let island = island_window(w);
    unsafe {
        if !island.is_invalid() {
            let m = MonitorFromWindow(island, MONITOR_DEFAULTTOPRIMARY);
            if !m.is_invalid() {
                return m;
            }
        }
        MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
    }
}

fn monitor_rect(mon: HMONITOR) -> Option<RECT> {
    if mon.is_invalid() {
        return None;
    }
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(mon, &mut mi).as_bool() }.then_some(mi.rcMonitor)
}

/// Our island window ("Poni.Island" in this process), cached.
fn island_window(w: &Watch) -> HWND {
    let cached = w.island.get();
    if !cached.is_invalid() && unsafe { IsWindow(cached) }.as_bool() {
        return cached;
    }
    let mut after = HWND::default();
    let found = loop {
        let next = unsafe { FindWindowExW(HWND::default(), after, crate::window::ISLAND_CLASS, PCWSTR::null()) };
        let Ok(h) = next else { break HWND::default() };
        if h.is_invalid() {
            break HWND::default();
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        if pid == w.pid {
            break h;
        }
        after = h;
    };
    w.island.set(found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT { left, top, right, bottom }
    }

    #[test]
    fn exact_and_overhanging_rects_cover_the_monitor() {
        let mon = r(1920, 0, 4480, 1440);
        assert!(contains(&mon, &mon));
        assert!(contains(&r(1912, -8, 4488, 1448), &mon));
    }

    #[test]
    fn a_window_is_at_the_top_only_where_it_reaches_into_the_band() {
        // The app's band: 8 DIPs along the top edge (here at 100 %).
        let band = r(0, 0, 1920, 8);
        // Maximized: it overhangs the monitor, cut to the band.
        assert_eq!(clip(&r(-8, -8, 1928, 1048), &band), Some([0, 0, 1920, 8]));
        // Snapped to the left half (its resize border reaches past the middle).
        assert_eq!(clip(&r(-7, 0, 967, 1047), &band), Some([0, 0, 967, 8]));
        // Dragged up to the edge by hand, its top a few pixels down.
        assert_eq!(clip(&r(400, 5, 1500, 900), &band), Some([400, 5, 1500, 8]));
        // Placed lower (a terminal 25 px down), or on the monitor to the
        // right: not at the top, whatever the island covers of its frame.
        assert_eq!(clip(&r(200, 25, 900, 700), &band), None);
        assert_eq!(clip(&r(200, 8, 900, 700), &band), None);
        assert_eq!(clip(&r(1920, 0, 3840, 1040), &band), None);
        // A monitor left of the primary one, its band its own.
        let left = r(-2560, -200, 0, -128);
        assert_eq!(clip(&r(-2568, -208, 8, 1208), &left), Some([-2560, -200, 0, -128]));
    }

    #[test]
    fn overlays_are_not_windows_at_the_top() {
        // A browser, File Explorer, a Store app's frame.
        assert!(!is_overlay(0x0000_0100)); // WS_EX_WINDOWEDGE
        assert!(!is_overlay((WS_EX_TOOLWINDOW | WS_EX_APPWINDOW).0), "a tool window with a taskbar button counts");
        for ex in [WS_EX_TRANSPARENT, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT] {
            assert!(is_overlay(ex.0), "{:#x}", ex.0);
        }
    }

    #[test]
    fn a_rect_short_of_any_edge_does_not_cover() {
        let mon = r(0, 0, 1920, 1080);
        assert!(!contains(&r(0, 0, 1920, 1040), &mon)); // above the taskbar
        assert!(!contains(&r(1, 0, 1920, 1080), &mon));
        assert!(!contains(&r(0, 0, 1919, 1080), &mon));
        assert!(!contains(&r(0, 1, 1920, 1080), &mon));
    }
}
