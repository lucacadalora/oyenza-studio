//! The app: owns the island window, state, renderer and services; routes
//! window messages, service events and page commands.

use crate::activities::{self, Mode};
use crate::clock;
use crate::commands::{Command, MediaCmd, OyenzaCmd, PairCmd, PhoneCmd};
use crate::events::{AppEvent, AppleMusicStatus, EventSink, OyenzaEvent, SendHwnd, WM_APP_APPBAR, WM_APP_EVENT, WM_APP_FRAME, WM_APP_TRAY};
use crate::gfx::{px_rect, px_union, scale_about, union_rect, Canvas, Gfx, HitMask};
use crate::icons::Icon;
use crate::island::{Geom, Island, HIT_MARGIN, SHOULDER};
use crate::oyen;
use crate::pages::{self, PageKind};
use crate::paths;
use crate::services::{self, Services};
use crate::state::{
    AppState, HudKind, InlineAction, LyricsState, LyricsStatus, Notice, NoticeAction, OyenzaLink, PairedPhone, PhoneAsk, PhoneDrop, PhoneTransfer, PhoneView,
};
use crate::tray::{self, Tray};
use crate::ui::{wid, CursorKind, Hit, Input, KeyInput, MouseButton, Ui};
use crate::window::{self, FrameClock, FrameTick, MonitorInfo};
use crate::{dragdrop, selftest, shell};
use poni_core::frametime::{ambient_divisor, MotionStats};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::health::fmt_steps;
use poni_core::oyenza;
use poni_core::pairdrop::{MeetEvent, PeerEvent};
use poni_core::phonedrop::{Event as PhoneEvent, QrMatrix};
use poni_core::settings::{IdleStyle, Settings, Theme};
use poni_core::settings::MonitorChoice;
use poni_core::store::{load_json, parse_json, save_json_atomic, LoadOutcome, Store};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use windows::core::{w, Result};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Not exported by windows-rs 0.58's WindowsAndMessaging module.
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_HOVER: usize = 1;
const TIMER_COLLAPSE: usize = 2;
const TIMER_WAKE: usize = 3;
const TIMER_SAVE: usize = 4;
const TIMER_STATS: usize = 5;
const TIMER_MINUTE: usize = 6;
pub const TIMER_SELFTEST: usize = 7;
const TIMER_TRAY: usize = 8;
const TIMER_RAISE: usize = 9;
/// Raises that follow a new foreground window, and their spacing.
const RAISE_REPEATS: u8 = 4;
const RAISE_INTERVAL_MS: u32 = 500;

const WM_CAPTURECHANGED_: u32 = 0x0215;
const WM_WTSSESSION_CHANGE_: u32 = 0x02B1;
const WTS_SESSION_LOCK_: usize = 7;
const WTS_SESSION_UNLOCK_: usize = 8;

/// Steps for copying the Apple Music token (README).
const APPLE_MUSIC_HELP_URL: &str = "https://github.com/lucacadalora/poni#apple-music-lyrics";
/// Phone drop closes itself after this long without a request from a phone.
const PHONE_IDLE_S: f64 = 600.0;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Posted messages that arrived while the app was borrowed (a nested
    /// message loop ran inside a handler). Replayed when the borrow ends.
    static MISSED: RefCell<Vec<(u32, WPARAM, LPARAM)>> = const { RefCell::new(Vec::new()) };
    /// A WM_APP_FRAME was swallowed: re-arm the frame clock.
    static FRAME_MISSED: Cell<bool> = const { Cell::new(false) };
}

/// Modal work that must run without the App borrowed (it pumps messages).
pub enum Deferred {
    DragOut(Vec<String>),
    Menu(POINT),
}

pub fn main_entry() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let selftest_dir = args.iter().position(|a| a == "--selftest").map(|i| args.get(i + 1).cloned().unwrap_or_else(|| "selftest-out".into()));
    if let Some(dir) = &selftest_dir {
        let home = std::path::Path::new(dir).join("home");
        let _ = std::fs::remove_dir_all(&home);
        std::env::set_var("PONI_HOME", &home);
    }
    crate::plog::init(paths::log_path());
    pinfo!("Poni {} starting (args: {:?})", env!("CARGO_PKG_VERSION"), &args[1..]);
    std::panic::set_hook(Box::new(|info| {
        perror!("panic: {info}");
    }));

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // Single instance (the self-test may run next to a normal instance).
    let mutex_name = if selftest_dir.is_some() { w!("Local\\Poni.Island.SelfTest") } else { w!("Local\\Poni.Island") };
    let _mutex = unsafe { CreateMutexW(None, true, mutex_name) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        pinfo!("another instance is running; exiting");
        return 0;
    }
    if let Err(e) = unsafe { OleInitialize(None) } {
        perror!("OleInitialize failed: {e}");
        return 2;
    }
    if crate::autostart::packaged() {
        pinfo!("running as the Microsoft Store package");
    }
    let code = match run(selftest_dir) {
        Ok(code) => code,
        Err(e) => {
            perror!("fatal: {e}");
            3
        }
    };
    unsafe { OleUninitialize() };
    pinfo!("Poni exiting with code {code}");
    code
}

fn run(selftest_dir: Option<String>) -> Result<i32> {
    window::register_classes(island_wndproc, helper_wndproc)?;
    let hwnd = window::create_island_window()?;
    let helper = window::create_helper_window()?;
    pinfo!("windows created");

    let (raw_settings, s_out): (Settings, _) = load_json(&paths::settings_path());
    if let Some(m) = s_out.message() {
        pwarn!("settings: {m}");
    }
    let settings = raw_settings.clone().sanitized();
    // Write new files, and write migrated / clamped values back so a
    // migration (e.g. the v2 hover delay) happens once and sticks.
    if s_out == LoadOutcome::Missing || (settings != raw_settings && s_out.writable()) {
        if let Err(e) = save_json_atomic(&paths::settings_path(), &settings) {
            pwarn!("saving settings failed: {e}");
        } else if s_out != LoadOutcome::Missing {
            pinfo!("settings: wrote back migrated / out-of-range values (v{})", settings.settings_version);
        }
    }
    let (store, d_out): (Store, _) = load_json(&paths::store_path());
    if let Some(m) = d_out.message() {
        pwarn!("data: {m}");
    }

    let (tx, rx) = mpsc::channel();
    let sink = EventSink::new(tx, SendHwnd::new(hwnd));
    let app = App::new(hwnd, helper, sink, rx, settings, store, selftest_dir, s_out.writable(), d_out.writable())?;
    APP.with(|c| *c.borrow_mut() = Some(app));
    with_app(|app| app.start());
    pinfo!("started; entering message loop");

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let code = APP.with(|c| c.borrow_mut().take().map(|mut a| a.shutdown()).unwrap_or(0));
    Ok(code)
}

/// Run `f` with the app if it isn't already borrowed (re-entrancy safe).
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let (r, hwnd) = APP.with(|c| match c.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(app) => {
                let r = f(app);
                if FRAME_MISSED.with(|m| m.replace(false)) {
                    app.clock.frame_handled();
                    app.request_frame();
                }
                (Some(r), Some(app.hwnd))
            }
            None => (None, None),
        },
        Err(_) => (None, None),
    });
    // Re-post messages a nested loop swallowed while we were borrowed.
    if let Some(hwnd) = hwnd {
        let missed = MISSED.with(|m| std::mem::take(&mut *m.borrow_mut()));
        for (msg, wp, lp) in missed {
            unsafe {
                let _ = PostMessageW(hwnd, msg, wp, lp);
            }
        }
    }
    r
}

/// Remember a message that couldn't be handled because the app was borrowed.
fn note_missed(msg: u32, wp: WPARAM, lp: LPARAM) -> bool {
    match msg {
        WM_APP_FRAME => {
            FRAME_MISSED.with(|f| f.set(true));
            true
        }
        WM_APP_EVENT | WM_MOUSELEAVE | WM_TIMER | WM_LBUTTONUP | WM_APP_TRAY | WM_APP_APPBAR => {
            MISSED.with(|m| {
                let mut v = m.borrow_mut();
                if v.len() < 64 && !(msg != WM_TIMER && v.iter().any(|x| x.0 == msg && x.1 == wp)) {
                    v.push((msg, wp, lp));
                }
            });
            true
        }
        _ => false,
    }
}

unsafe extern "system" fn island_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mut deferred = Vec::new();
    let res = with_app(|app| app.handle_island(msg, wp, lp, &mut deferred));
    for d in deferred {
        run_deferred(d);
    }
    match res {
        Some(Some(r)) => r,
        Some(None) => DefWindowProcW(hwnd, msg, wp, lp),
        None if note_missed(msg, wp, lp) => LRESULT(0),
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn helper_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mut deferred = Vec::new();
    let res = with_app(|app| app.handle_helper(msg, wp, lp, &mut deferred));
    for d in deferred {
        run_deferred(d);
    }
    match res {
        Some(Some(r)) => r,
        Some(None) => DefWindowProcW(hwnd, msg, wp, lp),
        // Helper messages are re-posted to the island window, which forwards
        // tray/appbar ids back to the helper handler.
        None if matches!(msg, WM_APP_TRAY | WM_APP_APPBAR) => {
            let _ = note_missed(msg, wp, lp);
            LRESULT(0)
        }
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn run_deferred(d: Deferred) {
    match d {
        Deferred::DragOut(paths) => {
            with_app(|app| app.release_mouse());
            if let Err(e) = dragdrop::drag_out(&paths) {
                pwarn!("drag out failed: {e}");
            }
            with_app(|app| app.request_frame());
        }
        Deferred::Menu(pt) => {
            let Some((helper, items)) = with_app(|app| (app.helper, tray::menu_items(&app.st.settings, app.st.pinned, &app.st.oyenza.link))) else { return };
            if let Some(cmd) = tray::show_menu(helper, pt, &items) {
                with_app(|app| app.on_menu(cmd));
            }
        }
    }
}

pub struct App {
    hwnd: HWND,
    helper: HWND,
    gfx: Gfx,
    pub st: AppState,
    island: Island,
    mode: Mode,
    input: Input,
    hits: Vec<Hit>,
    hovering: bool,
    expanded: bool,
    drag_hover: bool,
    text_focus: bool,
    prev_foreground: HWND,
    monitor: MonitorInfo,
    clock: FrameClock,
    sink: EventSink,
    rx: Receiver<AppEvent>,
    services: Option<Services>,
    /// The Glass theme's screen copy, light/dark choice and glass.
    glass: crate::glass::Glass,
    /// The glass was still changing on the last frame (turning light or dark).
    glass_moving: bool,
    /// Animation time of the last frame (the vblank it was shown on).
    last_frame_t: f64,
    /// The window: a fixed-size surface the island is drawn centred in.
    surface: Surface,
    /// Centre of the surface in DIPs (where the island hangs from).
    cx: f32,
    /// Canvas pixels drawn last frame (the next frame clears them).
    prev_canvas_px: Option<RECT>,
    /// GPU renderer: the hit mask last pushed to the window, and when.
    mask: Option<(HitMask, f64)>,
    /// Smoothness of the morph in progress (logged when it settles).
    motion: Option<MotionStats>,
    motion_name: &'static str,
    /// Motions logged so far, and when a hitchy one was last logged.
    motions_logged: u32,
    hitch_logged_at: f64,
    /// Per-frame timing of the first few morphs (for the log): vblanks since
    /// the last frame, then ms from vblank to drawing, to draw, to present
    /// and to update the hit mask.
    trace: Vec<(u32, [f32; 4])>,
    /// DWM's counters when the morph began (refreshes, frames composed,
    /// frames missed), to tell our dropped frames from the compositor's.
    dwm_at_start: Option<(u64, u64, u64)>,
    /// Log lines written off the frame path (file writes can take tens of ms).
    log_queue: Vec<String>,
    /// Self-test: read the next frame back, and where it was.
    capture_next: bool,
    capture_rect: RECT,
    fullscreen_hidden: bool,
    /// The last full-screen state reported, hidden or not.
    fullscreen_on: bool,
    /// Follow-up raises still due (TIMER_RAISE).
    raises_left: u8,
    /// Window classes already logged for covering the island (once each).
    covered_by: HashSet<String>,
    shown: bool,
    stats_on: bool,
    /// The System page's sampler thread (started the first time the page opens).
    sysmon: Option<services::sysmon::SysMon>,
    sysmon_on: bool,
    /// App icons the System page asked for and hasn't got yet (the Shelf's
    /// thumbnail worker answers both pages with the same event).
    icon_wait: HashSet<String>,
    tray: Option<Tray>,
    taskbar_created_msg: u32,
    selftest: Option<selftest::SelfTest>,
    /// Text we put on the clipboard ourselves (don't record it again).
    own_clip: Option<String>,
    exit_code: i32,
    /// Settings as last written to disk (to diff page-made changes).
    saved_settings: Settings,
    /// False when the file existed but couldn't be read: never overwrite it.
    settings_writable: bool,
    store_writable: bool,
    /// The current press started as a double-click (its click is ignored).
    dbl_press: bool,
    /// Pending high surrogate from WM_CHAR.
    high_surrogate: Option<u16>,
    /// Unconsumed wheel notches accumulated for page switching.
    wheel_acc: f32,
    wheel_t: f64,
    page_switch_t: f64,
    /// Session locked / display off: pause ambient animation.
    locked: bool,
    display_off: bool,
    tray_retries: u32,
    /// Content of the previous mode, fading out while the outline morphs:
    /// (mode, start time, its geometry, the opacity it fades from).
    outgoing: Option<(Mode, f64, Geom, f32)>,
    /// A page switch in progress: (from page, start time, slide direction).
    page_anim: Option<(PageKind, f64, f32)>,
    /// Where the hover (dwell) timer was last armed; None when it isn't.
    hover_arm: Option<Point>,
    /// The pointer left the open island: close it on the next frame.
    collapse_due: bool,
    /// When the pointer last closed the island by leaving it.
    left_at: f64,
    /// Phone drop's web server while the Shelf's phone panel is open.
    phone: Option<services::phonedrop::PhoneDrop>,
    /// The Shelf paths last handed to it (to notice changes).
    phone_offer: Vec<String>,
    /// The Shelf paths last handed to the paired-phones service.
    pair_offer: Option<Vec<String>>,
}

/// The island window: position and size in screen pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Surface {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// The grow springs overshoot by about 3 %; the surface leaves 5 %.
const OVERSHOOT: f32 = 1.05;
/// Hit-mask updates while the island moves (the mask is invisible, so it
/// may trail the outline by a frame or two).
const MASK_INTERVAL: f64 = 1.0 / 30.0;

/// How long outgoing content takes to fade (seconds).
const OUTGOING_FADE: f64 = 0.14;
/// Page switch cross-fade duration (seconds).
const PAGE_SWITCH: f64 = 0.26;
/// Pointer travel (DIPs) that restarts the hover dwell: the island opens
/// only once the pointer rests on it, not when it brushes past.
const DWELL_SLOP: f32 = 4.0;
/// The pointer coming back onto the island this soon after leaving it
/// (while it is still shrinking) opens it again at once, from where the
/// outline is: overshooting an edge costs no close-and-reopen.
const REOPEN_S: f64 = 0.3;
/// A hold that kept the island open (typing, a file drag, Phone drop, the
/// pin) ended while the pointer is elsewhere: close after this grace, so
/// the change can be seen.
const HOLD_GRACE_MS: u32 = 380;
/// Collapsed pill growth (w, h, radius) while the pointer is on it.
const HOVER_LIFT: (f32, f32, f32) = (12.0, 4.0, 2.0);
/// How long the inline notice pill shows its check mark (seconds).
const INLINE_DONE_S: f64 = 0.6;
/// Incoming content starts at this scale and this many DIPs up, and
/// settles down from the bezel as it fades in.
const CONTENT_SCALE_FROM: f32 = 0.94;
const CONTENT_DROP: f32 = 6.0;
/// Smallest scale the outgoing content shrinks to as the outline narrows.
const GHOST_MIN_SCALE: f32 = 0.6;

/// How much of the outgoing content still shows `age` seconds into its fade
/// (a smoothstep: it holds, then leaves).
fn outgoing_left(age: f64) -> f32 {
    let t = ((age / OUTGOING_FADE) as f32).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

impl App {
    #[allow(clippy::too_many_arguments)]
    fn new(
        hwnd: HWND,
        helper: HWND,
        sink: EventSink,
        rx: Receiver<AppEvent>,
        settings: Settings,
        store: Store,
        selftest_dir: Option<String>,
        settings_writable: bool,
        store_writable: bool,
    ) -> Result<Self> {
        let monitor = window::pick_monitor(settings.monitor);
        let gfx = Gfx::new(monitor.dpi as f32, hwnd)?;
        let mut st = AppState::new(settings, store);
        st.now = clock::mono();
        st.local = clock::local_now();
        st.unix = clock::unix_now();
        st.accent = theme_accent(st.settings.theme);
        let mode = Mode::Idle;
        let island = Island::new(activities::geom(&mode, &st));
        let clock = FrameClock::start(SendHwnd::new(hwnd), monitor.handle);
        let taskbar_created_msg = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        let selftest = selftest_dir.map(selftest::SelfTest::new);
        let saved_settings = st.settings.clone();
        Ok(Self {
            hwnd,
            helper,
            gfx,
            st,
            island,
            mode,
            input: Input::default(),
            hits: Vec::new(),
            hovering: false,
            expanded: false,
            drag_hover: false,
            text_focus: false,
            prev_foreground: HWND::default(),
            monitor,
            clock,
            sink,
            rx,
            services: None,
            glass: crate::glass::Glass::new(services::apps_use_light_theme()),
            glass_moving: false,
            last_frame_t: 0.0,
            surface: Surface::default(),
            cx: 0.0,
            prev_canvas_px: None,
            mask: None,
            motion: None,
            motion_name: "morph",
            motions_logged: 0,
            hitch_logged_at: f64::NEG_INFINITY,
            trace: Vec::new(),
            dwm_at_start: None,
            log_queue: Vec::new(),
            capture_next: false,
            capture_rect: RECT::default(),
            fullscreen_hidden: false,
            fullscreen_on: false,
            raises_left: 0,
            covered_by: HashSet::new(),
            shown: false,
            stats_on: false,
            sysmon: None,
            sysmon_on: false,
            icon_wait: HashSet::new(),
            tray: None,
            taskbar_created_msg,
            selftest,
            own_clip: None,
            exit_code: 0,
            saved_settings,
            settings_writable,
            store_writable,
            dbl_press: false,
            high_surrogate: None,
            wheel_acc: 0.0,
            wheel_t: 0.0,
            page_switch_t: 0.0,
            locked: false,
            display_off: false,
            tray_retries: 0,
            outgoing: None,
            page_anim: None,
            hover_arm: None,
            collapse_due: false,
            left_at: f64::NEG_INFINITY,
            phone: None,
            phone_offer: Vec::new(),
            pair_offer: None,
        })
    }

    fn start(&mut self) {
        self.layout_surface();
        let fake = self.selftest.is_some();
        self.st.power = services::power::query();
        services::power::register(self.helper);
        if !fake {
            self.services = Some(Services::start(&self.sink, self.hwnd, self.helper, &self.st.settings, self.oyenza_activity()));
            self.st.paired = self.services.as_ref().map(|s| paired_phones(s.pairs())).unwrap_or_default();
            self.sync_pair_offer();
            self.try_add_tray();
            services::power::register_session(self.helper);
        }
        if let Err(e) = dragdrop::register(self.hwnd, self.sink.clone()) {
            pwarn!("drop target registration failed: {e}");
        }
        // Refresh shelf thumbnails.
        for p in self.st.store.shelf.paths() {
            self.st.cmds.push(Command::LoadThumb(p));
        }
        self.schedule_minute_timer();
        if let Some(t) = self.selftest.as_mut() {
            t.begin(&mut self.st);
            unsafe { SetTimer(self.hwnd, TIMER_SELFTEST, 50, None) };
        }
        self.request_frame();
    }

    fn shutdown(&mut self) -> i32 {
        self.save_now();
        self.glass.stop(self.hwnd, &mut self.gfx);
        if let Some(p) = self.phone.take() {
            p.stop();
        }
        if let Some(t) = self.tray.take() {
            t.remove();
        }
        if let Some(s) = self.services.take() {
            s.stop(self.helper);
        }
        dragdrop::revoke(self.hwnd);
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DestroyWindow(self.helper);
        }
        self.exit_code
    }

    /// Leave the message loop. Never call DestroyWindow while the app is
    /// borrowed: WM_DESTROY would re-enter and be swallowed.
    fn quit(&mut self, code: i32) {
        self.exit_code = code;
        self.clock.set_active(false);
        unsafe { PostQuitMessage(code) };
    }

    pub fn request_frame(&mut self) {
        self.clock.set_active(true);
    }

    /// Switch the renderer to `dpi` and size the window for that scale.
    fn set_dpi(&mut self, dpi: u32) {
        self.gfx.set_dpi(dpi as f32);
        self.layout_surface();
    }

    /// Size and place the window once for the largest island (the widest,
    /// tallest expanded page plus spring overshoot). The island is drawn
    /// centred in it, so a morph never moves or resizes the window: the
    /// centre stays put and both edges glide at sub-pixel precision. Call at
    /// start and whenever the scale, the monitor or the expanded width changes.
    fn layout_surface(&mut self) {
        let tallest = PageKind::ALL.iter().map(|p| pages::content_height(*p, &self.st)).fold(0.0f32, f32::max);
        self.layout_surface_for(self.st.settings.expanded_width, pages::HEADER_H + tallest + pages::PAD);
    }

    /// Lay the window out for an island body up to `body_w`×`body_h` DIPs.
    fn layout_surface_for(&mut self, body_w: f32, body_h: f32) {
        let s = self.gfx.scale();
        let mon_w = self.monitor.rect.right - self.monitor.rect.left;
        let mut w = ((body_w * OVERSHOOT + 2.0 * (SHOULDER + HIT_MARGIN)) * s).ceil() as i32;
        // An even margin on both sides puts the centre on the monitor's centre.
        if (mon_w - w) % 2 != 0 {
            w += 1;
        }
        let h = ((body_h * OVERSHOOT + HIT_MARGIN) * s).ceil() as i32;
        let surface = Surface { x: self.monitor.rect.left + (mon_w - w) / 2, y: self.monitor.top(), w, h };
        if surface != self.surface {
            self.surface = surface;
            self.cx = w as f32 / 2.0 / s;
            self.prev_canvas_px = None;
            self.mask = None;
            if let Err(e) = self.gfx.set_surface(w, h) {
                pwarn!("window surface {w}x{h} failed: {e}");
            }
        }
    }

    /// The island no longer fits the window (a target beyond the laid-out
    /// size, with room for its overshoot, or an outline past it): grow it.
    fn ensure_room(&mut self) {
        let g = self.island.current();
        let t = self.island.target();
        let (bw, bh) = ((t.w * OVERSHOOT).max(g.w), (t.h * OVERSHOOT).max(g.h));
        let s = self.gfx.scale();
        let need_w = ((bw + 2.0 * (SHOULDER + HIT_MARGIN)) * s).ceil() as i32;
        let need_h = ((bh + HIT_MARGIN) * s).ceil() as i32;
        if need_w > self.surface.w || need_h > self.surface.h {
            let tallest = PageKind::ALL.iter().map(|p| pages::content_height(*p, &self.st)).fold(0.0f32, f32::max);
            let body_h = (pages::HEADER_H + tallest + pages::PAD).max(bh / OVERSHOOT * 1.1);
            pinfo!("island outgrew the window; growing it to fit {bw:.0}x{bh:.0} DIP");
            self.layout_surface_for(self.st.settings.expanded_width.max(bw / OVERSHOOT * 1.1), body_h);
        }
    }

    /// Add the tray icon; Explorer can be busy at logon, so retry a few times.
    fn try_add_tray(&mut self) {
        if self.tray.is_some() {
            return;
        }
        match Tray::add(self.helper) {
            Ok(t) => {
                self.tray = Some(t);
                self.tray_retries = 0;
            }
            Err(e) => {
                self.tray_retries += 1;
                if self.tray_retries <= 15 {
                    pwarn!("tray icon failed ({e}); retry {} in 2 s", self.tray_retries);
                    unsafe {
                        SetTimer(self.hwnd, TIMER_TRAY, 2000, None);
                    }
                } else {
                    pwarn!("tray icon failed ({e}); giving up");
                }
            }
        }
    }

    /// Write settings only when they changed and the file is safe to overwrite.
    fn save_settings(&mut self) {
        if self.st.settings == self.saved_settings {
            return;
        }
        if !self.settings_writable {
            pwarn!("settings.json couldn't be read at start; not overwriting it");
            return;
        }
        match save_json_atomic(&paths::settings_path(), &self.st.settings) {
            Ok(()) => self.saved_settings = self.st.settings.clone(),
            Err(e) => pwarn!("saving settings failed: {e}"),
        }
    }

    fn release_mouse(&mut self) {
        self.input.down = false;
        self.input.pressed_id = None;
        unsafe {
            let _ = ReleaseCapture();
        }
    }

    // ------------------------------------------------------------ messages

    fn handle_island(&mut self, msg: u32, wp: WPARAM, lp: LPARAM, deferred: &mut Vec<Deferred>) -> Option<LRESULT> {
        match msg {
            WM_APP_FRAME => {
                // Clear `pending` after the frame so a slow frame can't keep the
                // queue permanently non-empty (which starves WM_TIMER).
                let tick = self.clock.tick();
                self.frame(deferred, Some(tick));
                self.clock.frame_handled();
                Some(LRESULT(0))
            }
            WM_APP_EVENT => {
                while let Ok(ev) = self.rx.try_recv() {
                    self.on_event(ev);
                }
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_TIMER => {
                self.on_timer(wp.0);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let p = self.mouse_dip(lp);
                if self.over_stale_mask(p) {
                    // The pointer is over a part of the hit mask the outline
                    // already left: it isn't on the island.
                    if self.hovering {
                        self.hovering = false;
                        self.input.mouse = None;
                        self.cancel_hover();
                        self.pointer_left();
                        self.request_frame();
                    }
                    return Some(LRESULT(0));
                }
                self.input.mouse = Some(p);
                if !self.hovering {
                    self.hovering = true;
                    self.collapse_due = false;
                    unsafe {
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                        let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
                    }
                    if !self.expanded && clock::mono() - self.left_at < REOPEN_S {
                        // Back onto the island while it is still closing.
                        self.expand();
                    } else if !self.expanded {
                        self.arm_hover(p);
                    }
                } else if let Some(p0) = self.hover_arm.filter(|_| !self.expanded) {
                    // Dwell: moving on restarts the wait, so brushing past on
                    // the way to a tab or title bar doesn't open the island.
                    if (p.x - p0.x).hypot(p.y - p0.y) > DWELL_SLOP {
                        self.arm_hover(p);
                    }
                }
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.hovering = false;
                self.input.mouse = None;
                self.cancel_hover();
                self.pointer_left();
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                self.dbl_press = msg == WM_LBUTTONDBLCLK;
                let p = self.mouse_dip(lp);
                self.input.mouse = Some(p);
                self.input.down = true;
                self.input.press_pos = Some(p);
                self.input.pressed_id = self.hit_at(p).map(|h| h.id);
                unsafe { SetCapture(self.hwnd) };
                if !self.expanded {
                    let inline = self.st.notice.as_ref().and_then(|n| n.inline).filter(|_| self.input.pressed_id == Some(wid("notice.act")));
                    if let Some(a) = inline {
                        // The notice's inline pill acts in place without
                        // expanding. The press is consumed here, so the
                        // release can't click the pill a second time.
                        self.input.pressed_id = None;
                        self.run_inline_action(a);
                    } else {
                        // Clicking a notice jumps to its page.
                        if let Some(n) = self.st.notice.take() {
                            match n.action {
                                Some(NoticeAction::OpenPage(p)) => self.st.page = p,
                                Some(NoticeAction::StartFocus) => {
                                    self.st.page = PageKind::Focus;
                                    if !self.st.pomodoro.is_active() {
                                        self.st.pomodoro.start(self.st.now);
                                    }
                                }
                                None => {}
                            }
                        }
                        self.st.hud = None;
                        self.expand();
                    }
                } else if self.text_focus && self.input.pressed_id != self.input.focused {
                    self.blur_text();
                }
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let p = self.mouse_dip(lp);
                self.input.mouse = Some(p);
                let hit = self.hit_at(p).map(|h| h.id);
                if let (Some(pid), Some(h)) = (self.input.pressed_id, hit) {
                    // The second press of a double-click doesn't click again
                    // (no double toggles / double opens).
                    if pid == h && !self.dbl_press {
                        self.input.clicked = Some((h, MouseButton::Left));
                    }
                }
                self.dbl_press = false;
                self.release_mouse();
                if !self.hovering_inside(p) {
                    self.input.mouse = None;
                }
                if !self.hovering {
                    // Released outside after a press inside (a slider drag).
                    self.pointer_left();
                }
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED_ => {
                // Capture taken by someone else mid-press: drop the stale press.
                if lp.0 != self.hwnd.0 as isize && (self.input.down || self.input.pressed_id.is_some()) {
                    self.input.down = false;
                    self.input.pressed_id = None;
                    if !self.hovering {
                        self.pointer_left();
                    }
                    self.request_frame();
                }
                Some(LRESULT(0))
            }
            WM_SYSCOMMAND if (wp.0 & 0xFFF0) == SC_CLOSE as usize => {
                // Alt+F4 while typing in a field just closes the island, not the app.
                self.blur_text();
                self.collapse();
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                let p = self.mouse_dip(lp);
                match self.hit_at(p) {
                    Some(h) => self.input.clicked = Some((h.id, MouseButton::Right)),
                    None => {
                        let mut pt = POINT::default();
                        unsafe {
                            let _ = GetCursorPos(&mut pt);
                        }
                        deferred.push(Deferred::Menu(pt));
                    }
                }
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16 as f32 / WHEEL_DELTA as f32;
                self.input.wheel += delta;
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_SETCURSOR => {
                let kind = self.input.mouse.and_then(|p| self.hit_at(p)).map(|h| h.cursor).unwrap_or(CursorKind::Arrow);
                unsafe {
                    let id = match kind {
                        CursorKind::Arrow => IDC_ARROW,
                        CursorKind::Hand => IDC_HAND,
                        CursorKind::Text => IDC_IBEAM,
                    };
                    if let Ok(c) = LoadCursorW(None, id) {
                        SetCursor(c);
                    }
                }
                Some(LRESULT(1))
            }
            WM_MOUSEACTIVATE => Some(LRESULT(if self.text_focus { MA_ACTIVATE as isize } else { MA_NOACTIVATE as isize })),
            WM_CHAR => {
                if self.text_focus {
                    let u = wp.0 as u32;
                    let c = if (0xD800..0xDC00).contains(&u) {
                        self.high_surrogate = Some(u as u16);
                        None
                    } else if (0xDC00..0xE000).contains(&u) {
                        self.high_surrogate.take().and_then(|h| char::decode_utf16([h, u as u16]).next().and_then(|r| r.ok()))
                    } else {
                        self.high_surrogate = None;
                        char::from_u32(u)
                    };
                    if let Some(c) = c.filter(|c| !c.is_control()) {
                        self.input.keys.push(KeyInput::Char(c));
                        self.request_frame();
                    }
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                if self.text_focus {
                    let k = match VIRTUAL_KEY(wp.0 as u16) {
                        VK_BACK => Some(KeyInput::Backspace),
                        VK_DELETE => Some(KeyInput::Delete),
                        VK_RETURN => Some(KeyInput::Enter),
                        VK_ESCAPE => Some(KeyInput::Escape),
                        VK_LEFT => Some(KeyInput::Left),
                        VK_RIGHT => Some(KeyInput::Right),
                        VK_HOME => Some(KeyInput::Home),
                        VK_END => Some(KeyInput::End),
                        VK_V if unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0 => {
                            if let Some(text) = services::clipboard::read_text(self.hwnd) {
                                for c in text.chars().filter(|c| !c.is_control()) {
                                    self.input.keys.push(KeyInput::Char(c));
                                }
                            }
                            None
                        }
                        _ => None,
                    };
                    if let Some(k) = k {
                        self.input.keys.push(k);
                    }
                    self.request_frame();
                    return Some(LRESULT(0));
                }
                None
            }
            WM_ACTIVATE => {
                if (wp.0 & 0xffff) as u32 == WA_INACTIVE && self.text_focus {
                    self.text_focus = false;
                    self.input.focused = None;
                    self.set_noactivate(true);
                    self.maybe_schedule_collapse();
                    self.request_frame();
                }
                None
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
                self.refresh_monitor();
                if msg == WM_SETTINGCHANGE {
                    self.glass.app_light = services::apps_use_light_theme();
                }
                if msg == WM_SETTINGCHANGE && self.st.settings.theme == Theme::Glass {
                    self.st.accent = theme_accent(Theme::Glass);
                } else if msg == WM_SETTINGCHANGE && !oyen::on(self.st.settings.theme) {
                    if let Some(a) = services::system_accent() {
                        self.st.accent = a;
                    }
                }
                Some(LRESULT(0))
            }
            WM_CLOSE => {
                self.quit(0);
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn handle_helper(&mut self, msg: u32, wp: WPARAM, lp: LPARAM, deferred: &mut Vec<Deferred>) -> Option<LRESULT> {
        if msg == self.taskbar_created_msg && msg != 0 {
            if let Some(t) = self.tray.take() {
                t.remove();
            }
            self.tray_retries = 0;
            self.try_add_tray();
            if let Some(s) = self.services.as_ref() {
                s.on_taskbar_created();
            }
            // A restarted Explorer puts its taskbar at the top of the topmost band.
            self.start_raises();
            return Some(LRESULT(0));
        }
        match msg {
            WM_APP_TRAY => {
                let ev = (lp.0 & 0xffff) as u32;
                match ev {
                    WM_LBUTTONUP => {
                        if self.expanded {
                            self.collapse();
                        } else {
                            self.expand();
                        }
                        self.request_frame();
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let mut pt = POINT::default();
                        unsafe {
                            let _ = GetCursorPos(&mut pt);
                        }
                        deferred.push(Deferred::Menu(pt));
                    }
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_APP_APPBAR => {
                if let Some(s) = self.services.as_ref() {
                    if let Some(fs) = s.on_appbar_message(wp, lp, self.monitor.handle) {
                        self.on_event(AppEvent::Fullscreen(fs));
                    }
                }
                Some(LRESULT(0))
            }
            WM_POWERBROADCAST => {
                if let Some(on) = services::power::display_state_change(wp, lp) {
                    self.display_off = !on;
                }
                let p = services::power::query();
                self.on_event(AppEvent::Power(p));
                self.tell_oyenza_activity();
                self.request_frame();
                Some(LRESULT(1))
            }
            WM_WTSSESSION_CHANGE_ => {
                match wp.0 {
                    WTS_SESSION_LOCK_ => self.locked = true,
                    WTS_SESSION_UNLOCK_ => self.locked = false,
                    _ => {}
                }
                self.tell_oyenza_activity();
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_QUERYENDSESSION => Some(LRESULT(1)),
            WM_ENDSESSION => {
                self.save_now();
                Some(LRESULT(0))
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                self.refresh_monitor();
                None
            }
            _ => None,
        }
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_HOVER => {
                unsafe {
                    let _ = KillTimer(self.hwnd, TIMER_HOVER);
                }
                if !self.hovering || self.expanded {
                    self.hover_arm = None;
                } else if self.st.notice.as_ref().is_some_and(|n| n.inline.is_some()) {
                    // A notice with an inline pill stays put under a resting
                    // pointer so the pill can be clicked (a click on the rest
                    // of the notice still opens its page). The dwell stays
                    // armed: the next movement restarts it.
                } else {
                    self.expand();
                }
            }
            TIMER_RAISE => {
                self.raises_left = self.raises_left.saturating_sub(1);
                if self.shown && !self.fullscreen_hidden {
                    self.raise_if_covered();
                } else {
                    self.raises_left = 0;
                }
                if self.raises_left == 0 {
                    unsafe {
                        let _ = KillTimer(self.hwnd, TIMER_RAISE);
                    }
                }
            }
            TIMER_COLLAPSE => {
                unsafe {
                    let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
                }
                if self.free_to_collapse() {
                    self.collapse();
                }
            }
            TIMER_WAKE => unsafe {
                let _ = KillTimer(self.hwnd, TIMER_WAKE);
            },
            TIMER_SAVE => {
                unsafe {
                    let _ = KillTimer(self.hwnd, TIMER_SAVE);
                }
                self.save_now();
            }
            TIMER_STATS => {
                self.st.stats = services::sysstats::sample();
            }
            TIMER_MINUTE => {
                self.schedule_minute_timer();
                self.close_idle_phone();
            }
            TIMER_TRAY => {
                unsafe {
                    let _ = KillTimer(self.hwnd, TIMER_TRAY);
                }
                self.try_add_tray();
            }
            TIMER_SELFTEST => {
                if let Some(mut t) = self.selftest.take() {
                    let done = t.step(self);
                    if let Some(code) = done {
                        unsafe {
                            let _ = KillTimer(self.hwnd, TIMER_SELFTEST);
                        }
                        self.quit(code);
                    }
                    self.selftest = Some(t);
                }
            }
            _ => {}
        }
        self.request_frame();
    }

    /// Save the Apple Music token copied from the browser (sealed with DPAPI)
    /// and keep it off the clipboard and out of Poni's clipboard history.
    fn use_apple_music_token(&mut self) {
        let text = services::clipboard::read_text(self.hwnd).unwrap_or_default();
        let Some(token) = poni_core::applemusic::clean_user_token(&text) else {
            self.st.notify(Icon::Lyrics, palette::ORANGE, "No Apple Music token copied", "Copy media-user-token first", 6.0, None);
            return;
        };
        if let Err(e) = services::secret::save(&paths::apple_music_token_path(), &token) {
            pwarn!("apple music: cannot save the token ({e})");
            self.st.notify(Icon::Lyrics, palette::ORANGE, "Couldn't save the token", "Details in poni.log", 5.0, None);
            return;
        }
        pinfo!("apple music: token saved");
        self.forget_clipboard_text(&token);
        if let Some(s) = self.services.as_ref() {
            s.apple_music_token_changed();
        }
        self.refetch_lyrics();
        self.st.notify(Icon::Lyrics, palette::BLUE, "Checking the Apple Music token", "One moment", 4.0, None);
    }

    fn forget_apple_music_token(&mut self) {
        if let Err(e) = services::secret::delete(&paths::apple_music_token_path()) {
            pwarn!("apple music: cannot remove the token ({e})");
            self.st.notify(Icon::Lyrics, palette::ORANGE, "Couldn't remove the token", "Details in poni.log", 5.0, None);
            return;
        }
        pinfo!("apple music: token removed");
        if let Some(s) = self.services.as_ref() {
            s.apple_music_token_changed();
        }
        self.refetch_lyrics();
        self.st.notify(Icon::Lyrics, palette::BLUE, "Apple Music lyrics off", "Lyrics from LRCLIB only", 4.0, None);
    }

    fn on_apple_music(&mut self, status: AppleMusicStatus) {
        let (color, title, subtitle) = match status {
            AppleMusicStatus::Accepted => (palette::GREEN, "Apple Music lyrics on", "LRCLIB fills in the rest"),
            AppleMusicStatus::Rejected => (palette::ORANGE, "Apple Music token not accepted", "Copy a fresh one from music.apple.com"),
            AppleMusicStatus::Unreachable => (palette::ORANGE, "Can't reach Apple Music", "Token saved; trying again later"),
            AppleMusicStatus::NoAccess => (palette::ORANGE, "No lyrics from Apple Music", "They need an Apple Music subscription"),
        };
        self.st.notify(Icon::Lyrics, color, title, subtitle, 6.0, None);
    }

    // ------------------------------------------------------------ Oyenza

    /// How the Oyenza worker should pace itself: in use (unlocked, display
    /// on) and the power source.
    fn oyenza_activity(&self) -> services::oyenza::Activity {
        let p = self.st.power;
        services::oyenza::Activity { in_use: !self.locked && !self.display_off, on_ac: p.on_ac || !p.has_battery, saver: p.saver }
    }

    fn tell_oyenza_activity(&self) {
        if let Some(s) = self.services.as_ref() {
            s.oyenza_activity(self.oyenza_activity());
        }
    }

    fn oyenza(&self, c: OyenzaCmd) {
        if let Some(s) = self.services.as_ref() {
            s.oyenza(c);
        }
    }

    fn on_oyenza(&mut self, ev: OyenzaEvent) {
        let now = self.st.now;
        let link_before = self.st.oyenza.link.clone();
        match ev {
            OyenzaEvent::Restored { account } => self.st.oyenza.link = OyenzaLink::Connected { account },
            OyenzaEvent::Connecting { code, url, expires_in_s } => {
                self.st.oyenza.link = OyenzaLink::Connecting { code, url, until: now + expires_in_s as f64 };
                // Keep the code on screen while the person goes to oyenza.app.
                if self.st.oyenza.pinned_before.is_none() {
                    self.st.oyenza.pinned_before = Some(self.st.pinned);
                }
                self.st.pinned = true;
                self.st.page = PageKind::Oyenza;
                self.expand();
            }
            OyenzaEvent::Connected { account } => {
                self.end_sign_in();
                self.st.notify(Icon::Check, palette::GREEN, "Connected to Oyenza", format!("as {account}"), 6.0, Some(NoticeAction::OpenPage(PageKind::Oyenza)));
                self.st.oyenza.link = OyenzaLink::Connected { account };
            }
            OyenzaEvent::ConnectFailed(code) => {
                self.end_sign_in();
                self.st.oyenza.link = OyenzaLink::Off;
                let (title, subtitle) = oyenza::connect_failure_text(&code);
                self.st.notify(Icon::Unlink, palette::ORANGE, title, subtitle, 6.0, None);
            }
            OyenzaEvent::ConnectCancelled => {
                self.end_sign_in();
                self.st.oyenza.link = OyenzaLink::Off;
            }
            OyenzaEvent::Money { arrived, today } => {
                let o = &mut self.st.oyenza;
                if let Some(t) = today {
                    o.today = t;
                    o.today.truncate(oyenza::MAX_TODAY_ROWS);
                }
                oyenza::merge_today(&mut o.today, &arrived);
                o.updated_unix = Some(self.st.unix);
                let fresh = oyenza::unseen(&mut o.shown, &arrived);
                if let Some(n) = oyenza::money_notice(&fresh) {
                    self.oyenza_notice(Icon::MoneyIn, palette::GREEN, n.title, n.shown, Some(n.hidden));
                }
            }
            OyenzaEvent::Bills { today, bills } => {
                let day = if today.is_empty() { self.st.day_key() } else { today };
                self.st.oyenza.bills = bills;
                self.st.oyenza.bills_today = day.clone();
                self.st.oyenza.updated_unix = Some(self.st.unix);
                self.remind_bills(&day);
            }
            OyenzaEvent::NoteSaved(saved) => {
                let o = &mut self.st.oyenza;
                o.saving = false;
                o.input.clear();
                o.error = None;
                o.saved = Some((saved, now + oyenza::UNDO_S));
            }
            OyenzaEvent::NoteFailed { text, code } => {
                let o = &mut self.st.oyenza;
                o.saving = false;
                if o.input.trim().is_empty() {
                    o.input = text;
                }
                o.error = Some(oyenza::note_error_text(&code).into());
            }
            OyenzaEvent::Undone => self.st.oyenza.flash = Some(("Taken back".into(), now + 3.0)),
            OyenzaEvent::UndoFailed(code) => {
                let line = if code == "too_late" { "Too late to undo here. Change it in oyenza.app" } else { "Couldn't undo. Change it in oyenza.app" };
                self.st.oyenza.flash = Some((line.into(), now + 5.0));
            }
            OyenzaEvent::Disconnected { by_server } => {
                self.end_sign_in();
                self.st.oyenza = crate::state::OyenzaState::default();
                let (title, subtitle) =
                    if by_server { ("Oyenza disconnected", "Sign in again in Settings") } else { ("Disconnected from Oyenza", "Sign in again any time in Settings") };
                self.st.notify(Icon::Unlink, palette::TEXT_DIM, title, subtitle, 6.0, None);
            }
        }
        // The Oyenza tab came or went: the window makes room for its page (or no longer has to).
        if self.st.oyenza.link != link_before {
            self.layout_surface();
        }
    }

    /// The code is gone (signed in, refused, cancelled): the island stops
    /// staying open for it.
    fn end_sign_in(&mut self) {
        if let Some(p) = self.st.oyenza.pinned_before.take() {
            self.st.pinned = p;
            if !p {
                self.maybe_schedule_collapse();
            }
        }
    }

    /// A money or bill notice. Its sum waits for Show unless the tray's "Show
    /// amounts on the island" is on (`hidden` None: nothing to hide). While a
    /// full-screen app hides the island it waits, and shows when the island
    /// is back if it isn't old by then.
    fn oyenza_notice(&mut self, icon: Icon, color: Color, title: String, shown: String, hidden: Option<String>) {
        let now = self.st.now;
        let hide = hidden.is_some() && !self.st.settings.oyenza_show_amounts;
        let notice = Notice {
            icon,
            color,
            title,
            subtitle: if hide { hidden.unwrap_or_default() } else { shown.clone() },
            until: now + oyenza::NOTICE_S,
            action: Some(NoticeAction::OpenPage(PageKind::Oyenza)),
            inline: hide.then_some(InlineAction::ShowAmount),
            reveal: hide.then_some(shown),
        };
        if self.fullscreen_hidden {
            self.st.oyenza.held = Some((notice, now));
        } else {
            self.st.notice = Some(notice);
        }
    }

    fn show_held_notice(&mut self, now: f64) {
        if let Some((mut n, at)) = self.st.oyenza.held.take() {
            if now - at <= oyenza::HELD_MAX_S {
                n.until = now + oyenza::NOTICE_S;
                self.st.notice = Some(n);
            }
        }
    }

    /// One reminder per bill, the day before it (or on the day, if the PC was
    /// off), from the morning on; the rest come with the next look.
    fn remind_bills(&mut self, today: &str) {
        let st = &mut self.st;
        oyenza::prune_noticed(&mut st.store.oyenza_noticed, today);
        if st.local.hour < oyenza::BILL_HOUR {
            return;
        }
        let Some(b) = oyenza::bills_to_remind(&st.oyenza.bills, today, &st.store.oyenza_noticed).first().map(|b| (*b).clone()) else { return };
        st.store.oyenza_noticed.push(oyenza::notice_key(&b));
        st.save();
        let (shown, hidden) = oyenza::bill_subtitles(&b);
        let icon = if b.is_card() { Icon::Card } else { Icon::Calendar };
        self.oyenza_notice(icon, palette::ORANGE, oyenza::bill_title(&b, today), shown, hidden);
    }

    /// Remove history entries holding `secret` and clear the clipboard.
    fn forget_clipboard_text(&mut self, secret: &str) {
        let hits: Vec<usize> = self.st.clip.iter().enumerate().filter(|(_, c)| c.text.contains(secret)).map(|(i, _)| i).collect();
        for i in hits.into_iter().rev() {
            self.st.clip.remove(i);
        }
        self.st.ui.clip_flash = None;
        self.own_clip = Some(String::new());
        services::clipboard::set_text(self.helper, "");
    }

    /// Look up the current track's lyrics again (the sources changed).
    fn refetch_lyrics(&mut self) {
        if !self.st.settings.lyrics {
            return;
        }
        let Some(m) = self.st.media.clone().filter(|m| !m.title.is_empty()) else { return };
        if let Some(svc) = self.services.as_ref() {
            self.st.lyrics = LyricsState { track_key: m.track_key.clone(), status: LyricsStatus::Loading, ..Default::default() };
            svc.request_lyrics(&m);
        }
    }

    /// Apply side effects of a settings change (from the menu, a reload or a page).
    fn settings_changed(&mut self, before: &Settings) {
        let now = &self.st.settings;
        if before.theme != now.theme {
            self.st.accent = theme_accent(now.theme);
        }
        if before.hide_in_fullscreen != now.hide_in_fullscreen {
            // Apply at once: show the island over the full-screen app, or hide it.
            self.on_event(AppEvent::Fullscreen(self.fullscreen_on));
        }
        let now = &self.st.settings;
        if before.clipboard_history && !now.clipboard_history {
            // Privacy: turning history off forgets everything, pinned items included.
            self.st.clip = poni_core::cliphist::ClipHistory::default();
            self.st.ui.clip_flash = None;
            self.st.ui.clip_scroll = 0.0;
        }
        if !before.lyrics && now.lyrics {
            if let Some(m) = self.st.media.clone() {
                let stale = self.st.lyrics.track_key != m.track_key || self.st.lyrics.status == LyricsStatus::Idle;
                if stale && !m.title.is_empty() {
                    if let Some(svc) = self.services.as_ref() {
                        self.st.lyrics = LyricsState { track_key: m.track_key.clone(), status: LyricsStatus::Loading, ..Default::default() };
                        svc.request_lyrics(&m);
                    }
                }
            }
        }
        self.st.pomodoro.cfg = self.st.settings.pomodoro.clone();
        if let Some(s) = self.services.as_ref() {
            s.set_clipboard_history(self.st.settings.clipboard_history);
        }
    }

    fn on_menu(&mut self, cmd: tray::MenuCmd) {
        use tray::MenuCmd::*;
        let before = self.st.settings.clone();
        let s = &mut self.st.settings;
        match cmd {
            ToggleIdleStyle(style) => s.idle_style = style,
            SetTheme(theme) => s.theme = theme,
            ToggleClipboard => s.clipboard_history = !s.clipboard_history,
            ToggleLyrics => s.lyrics = !s.lyrics,
            TogglePronunciation => s.lyrics_pronunciation = !s.lyrics_pronunciation,
            SetHideFullscreen(hide) => s.hide_in_fullscreen = hide,
            ToggleSounds => s.sounds = !s.sounds,
            TogglePinned => self.st.pinned = !self.st.pinned,
            ToggleAutostart => {
                let on = crate::autostart::is_enabled();
                if let Err(e) = crate::autostart::set_enabled(!on) {
                    pwarn!("autostart toggle failed: {e}");
                }
                // The Settings page's switch follows.
                crate::autostart::report(self.sink.clone(), None);
            }
            OpenSettings => {
                if !paths::settings_path().exists() {
                    let _ = save_json_atomic(&paths::settings_path(), &self.st.settings);
                }
                shell::open(&paths::settings_path().to_string_lossy());
            }
            ReloadSettings => {
                // Never move the user's file aside or reset on a typo: keep the
                // current settings and say what's wrong.
                let path = paths::settings_path();
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse_json::<Settings>(&t)) {
                    Ok(s2) => {
                        let s2 = s2.sanitized();
                        let monitor_changed = s2.monitor != self.st.settings.monitor;
                        self.st.settings = s2.clone();
                        self.saved_settings = s2;
                        self.settings_writable = true;
                        if monitor_changed {
                            self.monitor = window::pick_monitor(self.st.settings.monitor);
                            self.set_dpi(self.monitor.dpi);
                        } else {
                            // The expanded width may have changed.
                            self.layout_surface();
                        }
                        self.refresh_monitor();
                    }
                    Err(e) => {
                        pwarn!("settings.json not reloaded: {e}");
                        self.st.notify(crate::icons::Icon::Sliders, palette::ORANGE, "Settings not reloaded", "settings.json has an error; see poni.log", 5.0, None);
                    }
                }
            }
            OpenDataFolder => shell::open(&paths::data_dir().to_string_lossy()),
            About => shell::open("https://lrclib.net"),
            AppleMusicToken => self.use_apple_music_token(),
            AppleMusicHelp => shell::open(APPLE_MUSIC_HELP_URL),
            AppleMusicForget => self.forget_apple_music_token(),
            OyenzaSignIn => self.oyenza(OyenzaCmd::SignIn),
            OyenzaCancel => self.oyenza(OyenzaCmd::Cancel),
            OyenzaDisconnect => self.oyenza(OyenzaCmd::Disconnect),
            OyenzaOpen => shell::open(oyenza::WEB),
            ToggleOyenzaAmounts => s.oyenza_show_amounts = !s.oyenza_show_amounts,
            Quit => {
                self.quit(0);
                return;
            }
        }
        self.settings_changed(&before);
        self.save_settings();
        self.request_frame();
    }

    // ------------------------------------------------------------ events

    pub fn on_event(&mut self, ev: AppEvent) {
        let now = clock::mono();
        self.st.now = now;
        let media_before = matches!(ev, AppEvent::Media(_) | AppEvent::MediaTimeline { .. }).then(|| self.media_snapshot());
        match ev {
            AppEvent::Backdrop => {
                if self.glass.on_backdrop(self.hwnd, &mut self.gfx) {
                    self.request_frame();
                }
            }
            AppEvent::Media(session) => {
                let prev_key = self.st.media.as_ref().map(|m| m.track_key.clone());
                match session {
                    Some(mut s) => {
                        // Keep already-decoded art if the track didn't change.
                        if s.art.is_none() {
                            if let Some(prev) = &self.st.media {
                                if prev.track_key == s.track_key {
                                    s.art = prev.art.clone();
                                    s.art_color = prev.art_color;
                                }
                            }
                        }
                        let changed = prev_key.as_deref() != Some(s.track_key.as_str());
                        if changed {
                            self.st.lyrics = LyricsState { track_key: s.track_key.clone(), status: LyricsStatus::Idle, ..Default::default() };
                            if self.st.settings.lyrics && !s.title.is_empty() {
                                if let Some(svc) = self.services.as_ref() {
                                    self.st.lyrics.status = LyricsStatus::Loading;
                                    svc.request_lyrics(&s);
                                }
                            }
                        }
                        self.st.media = Some(s);
                    }
                    None => {
                        self.st.media = None;
                        self.st.lyrics = LyricsState::default();
                    }
                }
            }
            AppEvent::MediaTimeline { track_key, timeline } => {
                if let Some(m) = self.st.media.as_mut() {
                    if m.track_key == track_key {
                        m.timeline = timeline;
                    }
                }
            }
            AppEvent::MediaArt { track_key, art } => {
                if let Some(m) = self.st.media.as_mut() {
                    if m.track_key == track_key {
                        m.art_color = art.as_ref().map(|a| a.accent_color());
                        m.art = art;
                    }
                }
            }
            AppEvent::Lyrics { track_key, result } => {
                if self.st.lyrics.track_key == track_key {
                    match result {
                        crate::events::LyricsResult::Found { lyrics, source, ruby } => {
                            self.st.lyrics.status = LyricsStatus::Found;
                            self.st.lyrics.lyrics = Some(lyrics);
                            self.st.lyrics.source = source;
                            self.st.lyrics.ruby = ruby;
                        }
                        crate::events::LyricsResult::Instrumental => self.st.lyrics.status = LyricsStatus::Instrumental,
                        crate::events::LyricsResult::NotFound => self.st.lyrics.status = LyricsStatus::NotFound,
                        crate::events::LyricsResult::Error(e) => self.st.lyrics.status = LyricsStatus::Error(e),
                    }
                }
            }
            AppEvent::Volume { level, muted } => {
                let first = !self.st.volume.known;
                self.st.volume = crate::state::VolumeState { level, muted, known: true };
                if !first && self.st.settings.hud_volume {
                    self.st.show_hud(HudKind::Volume { level, muted }, 1.6);
                }
            }
            AppEvent::Power(p) => {
                let old = self.st.power;
                self.st.power = p;
                if self.st.settings.hud_power && p.has_battery {
                    if p.on_ac != old.on_ac {
                        self.st.show_hud(HudKind::Charging { percent: p.percent, plugged: p.on_ac }, 2.2);
                    } else if !p.on_ac && ((old.percent > 20 && p.percent <= 20) || (old.percent > 10 && p.percent <= 10)) {
                        self.st.show_hud(HudKind::LowBattery { percent: p.percent }, 4.0);
                    }
                }
            }
            AppEvent::Bluetooth { name, connected, battery, audio } => {
                if self.st.settings.hud_bluetooth {
                    self.st.show_hud(HudKind::Bluetooth { name, connected, battery, audio }, 2.6);
                }
            }
            AppEvent::Fullscreen(on) => {
                let hide = on && self.st.settings.hide_in_fullscreen;
                if hide != self.fullscreen_hidden {
                    self.fullscreen_hidden = hide;
                    unsafe {
                        let _ = ShowWindow(self.hwnd, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE });
                    }
                    if hide {
                        self.collapse();
                        self.cancel_hover();
                        unsafe {
                            let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
                        }
                        self.hovering = false;
                        self.input.mouse = None;
                    }
                    pinfo!("fullscreen app {}: island {}", if on { "detected" } else { "gone" }, if hide { "hidden" } else { "shown" });
                    if !hide {
                        self.show_held_notice(now);
                    }
                }
                self.fullscreen_on = on;
                if !hide {
                    // A borderless game or a browser in F11 may have taken the
                    // top of the topmost band, and an island shown again after
                    // a full-screen app is not raised by showing it.
                    self.start_raises();
                }
            }
            AppEvent::Foreground => self.start_raises(),
            AppEvent::ShelfAdd(paths) => {
                let n = self.st.store.shelf.add_paths(&paths, self.st.unix);
                for p in &paths {
                    self.st.cmds.push(Command::LoadThumb(p.clone()));
                }
                self.st.save();
                self.st.page = PageKind::Shelf;
                self.drag_hover = false;
                self.st.drag_hover = false;
                self.expand();
                pinfo!("shelf: {n} new item(s)");
            }
            AppEvent::DragHover(on) => {
                self.drag_hover = on;
                self.st.drag_hover = on;
                if on {
                    self.st.page = PageKind::Shelf;
                    self.expand();
                } else {
                    self.maybe_schedule_collapse();
                }
            }
            AppEvent::ShelfThumb { path, image } => {
                // The answer to an app icon the System page asked for (only that
                // answer touches its icons), and to the Shelf when it asked too.
                let ours = self.icon_wait.remove(&path);
                let for_shelf = !ours || self.st.shelf_thumbs.contains_key(&path);
                if ours {
                    let px = pages::system::icon_px(self.gfx.scale());
                    self.st.app_icons.insert(path.clone(), image.as_ref().map(|i| pages::system::shrink_icon(i, px)));
                }
                if for_shelf {
                    self.st.shelf_thumbs.insert(path, image);
                }
            }
            AppEvent::Stats(s) => self.st.stats = s,
            AppEvent::System(r) => {
                // A reading still on its way when the page closed is dropped.
                if self.sysmon_on {
                    self.st.system.push(*r);
                }
            }
            AppEvent::ClipboardText(t) => {
                if self.own_clip.as_deref() == Some(t.as_str()) {
                    self.own_clip = None;
                } else {
                    self.st.clip.push(&t, self.st.unix);
                }
            }
            AppEvent::AppleMusic(status) => self.on_apple_music(status),
            AppEvent::Phone(e) => self.on_phone(e),
            AppEvent::Paired(e) => self.on_paired(e),
            AppEvent::Oyenza(ev) => self.on_oyenza(ev),
            AppEvent::Autostart(on) => self.st.ui.autostart = Some(on),
            AppEvent::Log(s) => pinfo!("{s}"),
        }
        if let Some(before) = media_before {
            self.after_media_change(before, now);
        }
        self.request_frame();
    }

    /// (player, track, playing) of the current media session.
    fn media_snapshot(&self) -> Option<(String, String, bool)> {
        self.st.media.as_ref().map(|m| (m.app_id.clone(), m.track_key.clone(), m.is_playing()))
    }

    /// Track the pause grace period and the title peek after a media update.
    fn after_media_change(&mut self, before: Option<(String, String, bool)>, now: f64) {
        let Some(m) = self.st.media.as_ref() else {
            self.st.media_paused_at = None;
            return;
        };
        let playing = m.is_playing();
        let same_player = before.as_ref().is_some_and(|b| b.0 == m.app_id);
        let new_track = before.as_ref().is_none_or(|b| b.1 != m.track_key);
        if playing {
            self.st.media_paused_at = None;
        } else if !same_player {
            // A different (or first) session that isn't playing: no grace.
            self.st.media_paused_at = None;
        } else if before.as_ref().is_some_and(|b| b.2) {
            // Playing -> paused: the pill lingers for a while.
            self.st.media_paused_at = Some(now);
        }
        // A new track starting while collapsed: show its title for a moment.
        if playing && new_track && !self.expanded {
            self.st.ui.peek_until = now + 3.0;
        }
    }

    // ------------------------------------------------------------ island state

    fn expand(&mut self) {
        if !self.expanded {
            self.expanded = true;
            self.cancel_hover();
            if self.st.page == PageKind::Oyenza {
                self.oyenza(OyenzaCmd::Refresh);
            }
        }
        self.request_frame();
    }

    /// (Re)start the hover dwell from `p`.
    fn arm_hover(&mut self, p: Point) {
        self.hover_arm = Some(p);
        unsafe {
            SetTimer(self.hwnd, TIMER_HOVER, self.st.settings.hover_delay_ms.max(1), None);
        }
    }

    fn cancel_hover(&mut self) {
        self.hover_arm = None;
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_HOVER);
        }
    }

    fn collapse(&mut self) {
        if self.expanded {
            self.expanded = false;
            if self.text_focus {
                self.blur_text();
            }
            self.set_stats(false);
            self.set_sysmon(false);
            self.st.ui.clip_flash = None;
            // Sums shown in the Oyenza tab hide again.
            self.st.oyenza.reveal = false;
            // Settings closes with the island: it opens again on the page it was opened from.
            if self.st.page == PageKind::Settings {
                self.st.page = self.st.ui.before_settings.take().filter(|p| pages::tabs(&self.st).contains(p)).unwrap_or(PageKind::ALL[0]);
            }
            // A notice that arrived while expanded gets its full time now.
            let min_until = self.st.now + 4.0;
            if let Some(n) = self.st.notice.as_mut() {
                n.until = n.until.max(min_until);
            }
        }
        self.request_frame();
    }

    /// Nothing holds the open island: no pointer on it, no pin, typing,
    /// file drag, press or Phone drop.
    fn free_to_collapse(&self) -> bool {
        !self.hovering && !self.st.pinned && !self.text_focus && !self.drag_hover && self.input.pressed_id.is_none() && !self.phone_holds()
    }

    /// The pointer left the open island: it starts closing on the next frame,
    /// or after `collapse_delay_ms` when the user set a grace.
    fn pointer_left(&mut self) {
        if !self.expanded || !self.free_to_collapse() {
            return;
        }
        match self.st.settings.collapse_delay_ms {
            0 => {
                self.collapse_due = true;
                self.request_frame();
            }
            ms => unsafe {
                SetTimer(self.hwnd, TIMER_COLLAPSE, ms, None);
            },
        }
    }

    /// A hold ended (see [`HOLD_GRACE_MS`]): close soon unless the pointer is on the island.
    fn maybe_schedule_collapse(&mut self) {
        if self.expanded && !self.hovering && !self.st.pinned && !self.text_focus && !self.drag_hover && !self.phone_holds() {
            unsafe {
                SetTimer(self.hwnd, TIMER_COLLAPSE, HOLD_GRACE_MS.max(self.st.settings.collapse_delay_ms), None);
            }
        }
    }

    fn set_stats(&mut self, on: bool) {
        if on == self.stats_on {
            return;
        }
        self.stats_on = on;
        unsafe {
            if on {
                self.st.stats = services::sysstats::sample();
                SetTimer(self.hwnd, TIMER_STATS, 1000, None);
            } else {
                let _ = KillTimer(self.hwnd, TIMER_STATS);
            }
        }
    }

    /// Readings for the System page run only while it is open; each opening
    /// starts a fresh minute of history.
    fn set_sysmon(&mut self, on: bool) {
        if on == self.sysmon_on {
            return;
        }
        self.sysmon_on = on;
        // The self-test shows sample readings; the real sources get their own check.
        if self.selftest.is_some() {
            return;
        }
        if on {
            self.st.system.clear();
            if self.sysmon.is_none() {
                match services::sysmon::SysMon::start(self.sink.clone()) {
                    Ok(s) => self.sysmon = Some(s),
                    Err(e) => pwarn!("sysmon: couldn't start ({e})"),
                }
            }
        }
        if let Some(s) = &self.sysmon {
            s.run(on);
        }
    }

    /// Puts the island back on top after another window came forward. Apps
    /// often make themselves topmost just after they take the focus, with no
    /// event of their own, so a few more checks follow over the next two
    /// seconds. (Exclusive full-screen still covers every window.)
    fn start_raises(&mut self) {
        if !self.shown || self.fullscreen_hidden {
            return;
        }
        self.raise_if_covered();
        self.raises_left = RAISE_REPEATS;
        unsafe {
            SetTimer(self.hwnd, TIMER_RAISE, RAISE_INTERVAL_MS, None);
        }
    }

    /// Raises the island when a window sits above it or it lost its topmost
    /// style, and logs each culprit once.
    fn raise_if_covered(&mut self) {
        let topmost = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } & WS_EX_TOPMOST.0 as isize != 0;
        let above = services::fullscreen::window_above(self.hwnd);
        if topmost && above.is_none() {
            return;
        }
        let key = if topmost { above.clone().unwrap_or_default() } else { String::new() };
        if self.covered_by.insert(key) {
            match (&above, topmost) {
                (_, false) => pinfo!("island lost its topmost style; raised again"),
                (Some(class), true) => pinfo!("island covered by a {class} window; raised again"),
                (None, true) => {}
            }
        }
        self.raise_topmost();
    }

    /// Top of the topmost band, without taking the focus.
    fn raise_topmost(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn set_noactivate(&self, on: bool) {
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let flag = WS_EX_NOACTIVATE.0 as isize;
            let new = if on { ex | flag } else { ex & !flag };
            if new != ex {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, new);
            }
        }
    }

    fn focus_text(&mut self) {
        if self.text_focus {
            return;
        }
        self.text_focus = true;
        unsafe {
            self.prev_foreground = GetForegroundWindow();
            self.set_noactivate(false);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(self.hwnd);
        }
    }

    fn blur_text(&mut self) {
        self.input.focused = None;
        if !self.text_focus {
            return;
        }
        self.text_focus = false;
        self.set_noactivate(true);
        unsafe {
            if !self.prev_foreground.is_invalid() && IsWindow(self.prev_foreground).as_bool() && GetForegroundWindow() == self.hwnd {
                let _ = SetForegroundWindow(self.prev_foreground);
            }
        }
        self.maybe_schedule_collapse();
    }

    fn refresh_monitor(&mut self) {
        let m = match self.st.settings.monitor {
            MonitorChoice::Cursor => {
                window::monitor_info(windows::Win32::Graphics::Gdi::HMONITOR(self.monitor.handle as *mut core::ffi::c_void))
                    .unwrap_or_else(|| window::pick_monitor(MonitorChoice::Primary))
            }
            c => window::pick_monitor(c),
        };
        if m != self.monitor {
            pinfo!("monitor: {:?} dpi {}", m.rect, m.dpi);
            self.monitor = m;
            self.set_dpi(m.dpi);
            self.layout_surface();
        }
        // The refresh rate may have changed too (60 <-> 144 Hz).
        self.clock.set_monitor(m.handle);
        self.request_frame();
    }

    fn mouse_dip(&self, lp: LPARAM) -> Point {
        let x = (lp.0 & 0xffff) as u16 as i16 as f32;
        let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as f32;
        let s = self.gfx.scale();
        Point::new(x / s, y / s)
    }

    fn hit_at(&self, p: Point) -> Option<Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(p)).copied()
    }

    /// Whether `p` is over a part of the window that takes the mouse: the
    /// whole canvas while expanded (hit margin), only the island collapsed.
    fn hovering_inside(&self, p: Point) -> bool {
        if !self.expanded {
            return self.island.body_rect_at(self.cx).contains(p);
        }
        self.island.canvas_rect_at(self.cx).contains(p)
    }

    /// GPU renderer: the invisible hit mask trails a moving outline by up to
    /// [`MASK_INTERVAL`]. A pointer over the part the island already left
    /// (well outside its shape and shoulders) gets no hover.
    fn over_stale_mask(&self, p: Point) -> bool {
        if !self.gfx.needs_hit_mask() || self.island.settled() || self.input.down {
            return false;
        }
        let r = if self.expanded { self.island.canvas_rect_at(self.cx) } else { self.island.body_rect_at(self.cx).inset(-SHOULDER - 2.0, 0.0) };
        !Rect::new(r.x, r.y, r.w, r.h + 2.0).contains(p)
    }

    fn target_mode(&self) -> Mode {
        if self.expanded {
            return Mode::Expanded(self.st.page);
        }
        activities::collapsed_mode(&self.st)
    }

    /// Island geometry for `m`. The resting pill lifts a little under the
    /// pointer: instant feedback while the hover dwell runs.
    fn geom_for(&self, m: &Mode) -> Geom {
        let g = activities::geom(m, &self.st);
        let liftable = match m {
            Mode::Idle => self.st.settings.idle_style != IdleStyle::Hidden,
            Mode::Activity(_) => true,
            _ => false,
        };
        if liftable && self.hovering && !self.expanded {
            Geom::new(g.w + HOVER_LIFT.0, g.h + HOVER_LIFT.1, g.r + HOVER_LIFT.2)
        } else {
            g
        }
    }

    // ------------------------------------------------------------ frame

    fn refresh_time(&mut self) {
        self.st.now = clock::mono();
        self.st.local = clock::local_now();
        self.st.unix = clock::unix_now();
    }

    fn tick(&mut self) {
        let now = self.st.now;
        if let Some(h) = &self.st.hud {
            if now >= h.until {
                self.st.hud = None;
            }
        }
        if let Some(n) = &self.st.notice {
            if !self.expanded && now >= n.until {
                self.st.notice = None;
            }
        }
        if let Some(ended) = self.st.pomodoro.tick(now) {
            pages::focus::on_phase_ended(&mut self.st, ended);
        }
        // Signed out: the Oyenza tab is gone.
        if self.st.page == PageKind::Oyenza && !self.st.oyenza.tab_visible() {
            self.st.page = PageKind::ALL[0];
        }
    }

    /// Draw and show one frame. `tick` is the vblank that triggered it (None
    /// for self-test captures, which run off the clock).
    fn frame(&mut self, deferred: &mut Vec<Deferred>, tick: Option<FrameTick>) {
        if self.fullscreen_hidden {
            // Nothing is drawn, but timers, chimes and commands keep working.
            // Glass stops copying the screen: nobody sees the island.
            let off = RECT::default();
            self.glass.sync(false, self.hwnd, &self.sink, self.monitor.handle, off, &mut self.gfx);
            self.refresh_time();
            self.tick();
            self.run_commands(deferred);
            self.clock.set_active(false);
            self.motion = None;
            self.schedule_wake();
            return;
        }
        let rendered_at = clock::mono();
        self.refresh_time();
        // Stamp the frame with the vblank it will be shown after, so every
        // animation step is a whole number of refresh periods: even motion at
        // 60, 144 or 240 Hz, whatever the thread's wake-up jitter.
        let period = tick.map_or_else(|| self.clock.period(), |t| t.period);
        let anim_t = tick.map_or(rendered_at, |t| t.vblank + t.period).max(self.last_frame_t);
        let resumed = tick.is_some_and(|t| t.resumed) || self.last_frame_t == 0.0;
        // Coming out of rest, a motion advances one refresh, not the whole
        // time the clock was parked.
        let mut dt = if resumed { period } else { anim_t - self.last_frame_t };
        self.last_frame_t = anim_t;
        self.st.now = self.st.now.max(anim_t);
        self.tick();
        let now = self.st.now;

        // The pointer left the open island since the last frame: the close
        // starts on this frame, no timer in between.
        if std::mem::take(&mut self.collapse_due) && self.expanded && self.free_to_collapse() {
            self.collapse();
            self.left_at = clock::mono();
        }

        let target = self.target_mode();
        let mut retarget = false;
        if target != self.mode {
            let new_content = activities::content_key(&target) != activities::content_key(&self.mode);
            // A morph turned around mid-fade (the pointer came back while the
            // island closed): the content coming back is the one still fading
            // out, and it fades back in from where it is, not from nothing.
            let back = self
                .outgoing
                .filter(|o| new_content && activities::content_key(&o.0) == activities::content_key(&target))
                .map(|(_, t0, _, from)| from * outgoing_left(now - t0));
            if new_content {
                // The leaving content fades from how visible it is now.
                self.outgoing = Some((self.mode, now, self.island.target(), self.island.content_alpha()));
            }
            if let (Mode::Expanded(a), Mode::Expanded(b)) = (self.mode, target) {
                if a != b {
                    let dir = if b.tab_index() > a.tab_index() { 1.0 } else { -1.0 };
                    self.page_anim = Some((a, now, dir));
                }
            }
            self.motion_name = motion_name(&self.mode, &target);
            self.island.set_target(self.geom_for(&target), now, new_content);
            if let Some(a) = back {
                self.island.resume_content(a);
            }
            self.mode = target;
            retarget = true;
        } else {
            // Size can change within a mode (a page's height, the hover lift); follow it.
            let g = self.geom_for(&self.mode);
            if g != self.island.target() {
                if self.island.settled() {
                    self.motion_name = "resize";
                }
                self.island.set_target(g, now, false);
                retarget = true;
            }
        }
        // A motion that starts on this frame moves one refresh's worth, even
        // when the previous frame was a slower ambient one.
        if retarget {
            dt = dt.min(period);
        }
        let dt = dt.clamp(0.0, 0.05) as f32;
        self.set_stats(self.mode == Mode::Expanded(PageKind::Today));
        self.set_sysmon(self.mode == Mode::Expanded(PageKind::System));
        self.island.step(dt, now);
        self.ensure_room();
        self.note_motion(retarget, rendered_at, period, dt as f64);

        let scale = self.gfx.scale();
        let (sw, sh) = (self.surface.w, self.surface.h);
        let body = self.island.body_rect_at(self.cx);
        let canvas_r = self.island.canvas_rect_at(self.cx);
        let geom = self.island.current();
        let content_alpha = self.island.content_alpha();
        // Clear what the island covered last frame as well as this one.
        let canvas_px = px_rect(canvas_r, scale, 1, sw, sh);
        let full = RECT { left: 0, top: 0, right: sw, bottom: sh };
        let dirty = px_union(self.prev_canvas_px.unwrap_or(full), canvas_px);
        self.prev_canvas_px = Some(canvas_px);
        if std::mem::take(&mut self.capture_next) {
            self.capture_rect = px_rect(canvas_r, scale, 0, sw, sh);
            self.gfx.request_capture(self.capture_rect);
        }
        let mut hits = std::mem::take(&mut self.hits);
        hits.clear();

        let hidden_idle = self.mode == Mode::Idle && self.st.settings.idle_style == IdleStyle::Hidden;
        // Content is masked to the island silhouette only while the outline
        // or the content moves. At rest every layout fits inside the body,
        // and skipping the mask layer keeps ambient frames cheap.
        let morphing = !self.island.settled() || self.outgoing.is_some();
        let mode = self.mode;
        let transition = self.page_anim.and_then(|(from, t0, dir)| {
            let t = ((now - t0) / PAGE_SWITCH) as f32;
            (t < 1.0).then_some(pages::PageTransition { from, t, dir })
        });
        if transition.is_none() {
            self.page_anim = None;
        }
        // Pages lay out at their final size, centred on the morphing outline,
        // so the header and the page body don't slide with the edges; the
        // outline reveals them.
        let lay = match mode {
            Mode::Expanded(_) => {
                let tgt = self.island.target();
                Rect::new(body.center().x - tgt.w / 2.0, body.y, tgt.w, tgt.h)
            }
            _ => body,
        };
        let mut wheel_left = 0.0;
        let draw_from = clock::mono();
        let mut drawn_at = draw_from;
        // Glass: copy the screen under the window while the GPU draws (the
        // self-test brings its own backdrop), then draw this frame's glass
        // before the frame starts. The look follows the glass, light or dark.
        let glass_theme = self.st.settings.theme == Theme::Glass;
        let window_px = RECT { left: self.surface.x, top: self.surface.y, right: self.surface.x + sw, bottom: self.surface.y + sh };
        // Only while someone can see it: not while hidden, locked or the display is off.
        let copy_on = glass_theme
            && self.selftest.is_none()
            && !hidden_idle
            && !self.locked
            && !self.display_off
            && matches!(self.gfx.renderer(), crate::gfx::Renderer::Gpu { .. });
        self.glass.sync(copy_on, self.hwnd, &self.sink, self.monitor.handle, window_px, &mut self.gfx);
        let mut glass_moving = false;
        let glass = if glass_theme && !hidden_idle {
            let small = !matches!(mode, Mode::Expanded(_));
            let (light, p, moving) = self.glass.frame(body, geom.r, SHOULDER, small, now, dt, scale, &mut self.gfx);
            glass_moving = moving;
            self.gfx.set_look(poni_core::look::glass(light));
            self.gfx.prepare_glass(&p, canvas_px);
            Some(p)
        } else {
            self.gfx.set_look(poni_core::look::of(self.st.settings.theme));
            None
        };
        let (repaint, anim, focus, ui_live) = match self.gfx.begin(dirty) {
            Ok(mut canvas) => {
                if self.expanded && !canvas.on_gpu() {
                    // Near-invisible hit margin: a forgiving edge for the open
                    // panel. Collapsed, only the visible shape takes the mouse.
                    // (The GPU renderer keeps this in its hit mask instead.)
                    canvas.fill_rect(canvas_r, Color::black(1.0 / 255.0));
                }
                if let Some(p) = glass.as_ref() {
                    canvas.glass(p, canvas_r);
                } else {
                    let island_color = if hidden_idle { Color::black(1.0 / 255.0) } else { palette::ISLAND };
                    let border = if hidden_idle { None } else { Some(canvas.look().border) };
                    canvas.notch(body, geom.r, SHOULDER, island_color, border);
                }

                // Content scales about the top centre, where the island hangs.
                let pivot = Point::new(body.center().x, body.y);
                if morphing {
                    canvas.clip(body);
                    canvas.push_shape_mask(body, geom.r, SHOULDER);
                }

                // Outgoing content: laid out at its own (old) size, centred,
                // shrinking with the outline instead of being sliced, and
                // fading out on a smoothstep (it holds, then leaves). Drawn as
                // one layer, so it shrinks as a picture. Ghost input: no clicks.
                if let Some((m, t0, g, from)) = self.outgoing {
                    if now - t0 >= OUTGOING_FADE {
                        self.outgoing = None;
                    } else {
                        let old_body = Rect::new(body.center().x - g.w / 2.0, body.y, g.w, g.h);
                        let alpha = from * outgoing_left(now - t0);
                        let xf = scale_about(pivot, (geom.w / g.w.max(1.0)).clamp(GHOST_MIN_SCALE, 1.0));
                        let st = &mut self.st;
                        canvas.layered(old_body, alpha, xf, |c| {
                            let mut ghost_input = Input::default();
                            let mut ghost_hits = Vec::new();
                            let mut ghost = Ui::new(c, &mut ghost_input, &mut ghost_hits, now);
                            match m {
                                Mode::Expanded(page) => pages::draw_expanded(&mut ghost, st, old_body, page, None),
                                _ => activities::draw(&mut ghost, st, &m, old_body),
                            }
                        });
                    }
                }

                // Incoming content settles down from the bezel as it fades in,
                // as one layer, so its text doesn't re-hint every frame.
                let out = if content_alpha < 1.0 {
                    let grow = scale_about(pivot, CONTENT_SCALE_FROM + (1.0 - CONTENT_SCALE_FROM) * content_alpha);
                    let xf = grow * Matrix3x2::translation(0.0, -CONTENT_DROP * (1.0 - content_alpha));
                    let (input, st) = (&mut self.input, &mut self.st);
                    canvas.layered(union_rect(lay, body), content_alpha, xf, |c| draw_content(c, input, &mut hits, st, mode, lay, body, transition, now))
                } else {
                    draw_content(&mut canvas, &mut self.input, &mut hits, &mut self.st, mode, lay, body, transition, now)
                };
                wheel_left = self.input.wheel;
                if morphing {
                    canvas.pop_shape_mask();
                    canvas.unclip();
                }
                drop(canvas);
                if let Err(e) = self.gfx.end() {
                    pwarn!("EndDraw failed: {e}");
                }
                drawn_at = clock::mono();
                out
            }
            Err(e) => {
                pwarn!("begin frame failed: {e}");
                (false, false, None, false)
            }
        };
        self.hits = hits;

        // Unconsumed wheel over the header switches pages: one page per whole
        // notch, with a short cooldown so touchpads don't race through. Over
        // page content a wheel the page didn't use does nothing (a short list
        // must not jump pages where a long one would scroll).
        if let Mode::Expanded(p) = self.mode {
            let over_header = self.input.mouse.is_some_and(|m| m.y < body.y + pages::HEADER_H);
            if wheel_left != 0.0 && over_header {
                if now - self.wheel_t > 0.25 || wheel_left.signum() != self.wheel_acc.signum() {
                    self.wheel_acc = 0.0;
                }
                self.wheel_t = now;
                self.wheel_acc += wheel_left;
                if self.wheel_acc.abs() >= 1.0 && now - self.page_switch_t > 0.3 {
                    let next = pages::step_tab(p, &self.st, self.wheel_acc < 0.0);
                    self.st.cmds.push(Command::SetPage(next));
                    self.wheel_acc = 0.0;
                    self.page_switch_t = now;
                }
            }
        }
        self.input.end_frame();

        // Present. A renderer switch (GPU lost, or back) leaves the other
        // renderer's content in the window's bitmap: push the hit mask in
        // full, and redraw everything on the next frame too.
        let mut renderer_changed = self.gfx.take_renderer_changed();
        let (x, y) = (self.surface.x, self.surface.y);
        if let Err(e) = self.gfx.present(x, y) {
            pwarn!("present failed: {e}");
        }
        renderer_changed |= self.gfx.take_renderer_changed();
        if renderer_changed {
            self.mask = None;
            self.prev_canvas_px = None;
        }
        let presented_at = clock::mono();
        if self.gfx.needs_hit_mask() {
            self.update_hit_mask(body, geom.r, canvas_r, now);
        }
        if let (Some(t), true) = (tick, self.motion.is_some() && self.motions_logged < 6 && self.trace.len() < 200) {
            let ms = |a: f64, b: f64| ((b - a) * 1000.0) as f32;
            self.trace.push((t.periods, [ms(t.vblank, draw_from), ms(draw_from, drawn_at), ms(drawn_at, presented_at), ms(presented_at, clock::mono())]));
        }
        if !self.shown {
            self.shown = true;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }

        match focus {
            Some(true) => self.focus_text(),
            Some(false) => self.blur_text(),
            None => {}
        }
        self.run_commands(deferred);

        // Decide whether to keep the frame clock running. Real motion runs
        // at the display's full refresh rate; ambient motion runs on every
        // n-th refresh (the equalizer near 60 fps, a drifting progress bar
        // near 30) and pauses while the session is locked or the display off.
        let ambient_ok = !self.locked && !self.display_off;
        let eq_live = ambient_ok && activities::is_live(&self.mode, &self.st);
        let drift_live = ambient_ok && ui_live;
        self.glass_moving = glass_moving;
        let moving = !self.island.settled()
            || anim
            || repaint
            || renderer_changed
            || glass_moving
            || !self.st.cmds.is_empty()
            || self.outgoing.is_some()
            || self.page_anim.is_some();
        // The display mode's rate, not the measured period: a divisor that
        // flips between 1 and 2 would be the uneven cadence it exists to avoid.
        let hz = self.clock.nominal_hz();
        self.clock.set_divisor(if moving {
            1
        } else if eq_live {
            ambient_divisor(hz, 60.0)
        } else {
            ambient_divisor(hz, 30.0)
        });
        self.clock.set_active(moving || eq_live || drift_live);
        self.schedule_wake();
        if !self.log_queue.is_empty() {
            let lines = std::mem::take(&mut self.log_queue);
            std::thread::spawn(move || lines.iter().for_each(|l| crate::plog::write("INFO", l)));
        }
    }

    /// GPU renderer: keep the window's invisible hit mask on the island.
    /// While the outline moves it follows at [`MASK_INTERVAL`]; at rest it
    /// matches exactly.
    fn update_hit_mask(&mut self, body: Rect, radius: f32, canvas_r: Rect, now: f64) {
        let mask = if self.expanded { HitMask { rect: Some(canvas_r), notch: None } } else { HitMask { rect: None, notch: Some((body, radius, SHOULDER)) } };
        let due = match &self.mask {
            None => true,
            Some((m, at)) => *m != mask && (self.island.settled() || now - at >= MASK_INTERVAL),
        };
        if !due {
            return;
        }
        let prev = self.mask.map(|(m, _)| m);
        match self.gfx.push_hit_mask(self.surface.x, self.surface.y, &mask, prev.as_ref()) {
            Ok(()) => self.mask = Some((mask, now)),
            Err(e) => {
                pwarn!("hit mask update failed: {e}");
                self.mask = None;
            }
        }
    }

    /// Track how smoothly the outline moves. The first morphs after start
    /// are logged (the CI smoke test reads them); after that only morphs
    /// that dropped frames, at most one a minute.
    fn note_motion(&mut self, retarget: bool, t: f64, period: f64, dt: f64) {
        let moving = !self.island.settled();
        if retarget || !moving {
            if let Some(mut m) = self.motion.take() {
                if !moving {
                    m.frame(t);
                }
                let early = self.motions_logged < 20;
                if m.frames >= 4 && (early || (m.hitches > 0 && t - self.hitch_logged_at > 60.0)) {
                    let dwm = match (self.dwm_at_start, dwm_counters()) {
                        (Some(a), Some(b)) => format!(", dwm {} refreshes / {} composed / {} missed", b.0.wrapping_sub(a.0), b.1.wrapping_sub(a.1), b.2.wrapping_sub(a.2)),
                        _ => String::new(),
                    };
                    self.log_queue.push(format!("{} ({}{dwm})", m.summary(), self.renderer_name()));
                    if self.motions_logged < 6 && !self.trace.is_empty() {
                        let col = |i: usize| self.trace.iter().map(|f| format!("{:.1}", f.1[i])).collect::<Vec<_>>().join(" ");
                        let periods: Vec<String> = self.trace.iter().map(|f| f.0.to_string()).collect();
                        self.log_queue.push(format!("  vblanks {}; late ms {}; draw ms {}; present ms {}; mask ms {}", periods.join(" "), col(0), col(1), col(2), col(3)));
                    }
                    self.motions_logged += 1;
                    if !early {
                        self.hitch_logged_at = t;
                    }
                }
            }
        }
        if moving {
            match self.motion.as_mut() {
                Some(m) => m.frame(t),
                None => {
                    self.motion = Some(MotionStats::begin(self.motion_name, t, period, dt));
                    self.trace.clear();
                    self.dwm_at_start = dwm_counters();
                }
            }
        }
    }

    fn renderer_name(&self) -> &'static str {
        match self.gfx.renderer() {
            crate::gfx::Renderer::Gpu { warp: false } => "gpu",
            crate::gfx::Renderer::Gpu { warp: true } => "gpu-warp",
            crate::gfx::Renderer::Software => "software",
        }
    }

    /// Wake up for the next visible change (HUD expiry, countdown second, …).
    fn schedule_wake(&mut self) {
        let now = self.st.now;
        let mut next: Option<f64> = None;
        let mut consider = |t: f64| {
            if t > now {
                next = Some(next.map_or(t, |n: f64| n.min(t)));
            }
        };
        if let Some(h) = &self.st.hud {
            consider(h.until);
        }
        if let Some(n) = &self.st.notice {
            consider(n.until);
        }
        // While hidden for a full-screen app only the phase end matters.
        let visible = !self.fullscreen_hidden;
        if let Some(b) = self.st.pomodoro.next_second_boundary(now).filter(|_| visible) {
            consider(now + b + 0.005);
        }
        if let poni_core::pomodoro::RunState::Running { ends_at } = self.st.pomodoro.run {
            consider(ends_at + 0.005);
        }
        if let Some(t) = activities::next_wake(&self.mode, &self.st).filter(|_| visible) {
            consider(t);
        }
        if let Mode::Expanded(p) = self.mode {
            if let Some(t) = pages::next_wake(p, &self.st).filter(|_| visible) {
                consider(t + 0.005);
            }
        }
        if let Some(t) = next {
            let ms = (((t - now) * 1000.0).ceil() as u32).clamp(10, 3_600_000);
            unsafe {
                SetTimer(self.hwnd, TIMER_WAKE, ms, None);
            }
        }
    }

    fn schedule_minute_timer(&mut self) {
        let t = clock::local_now();
        let ms = (60 - t.second as u32).max(1) * 1000 + 20;
        unsafe {
            SetTimer(self.hwnd, TIMER_MINUTE, ms, None);
        }
    }

    /// Run a notice's inline action in place (the island stays collapsed),
    /// then show a check mark in the pill briefly before the notice leaves.
    fn run_inline_action(&mut self, a: InlineAction) {
        self.refresh_time();
        let st = &mut self.st;
        // Already done: the check mark is showing.
        if st.notice.as_ref().is_none_or(|n| n.icon == Icon::Check) {
            return;
        }
        if a == InlineAction::ShowAmount {
            // The sum and sender take the subtitle's place for a few seconds; the
            // pill turns into a check (activities.rs inline_done).
            let until = st.now + oyenza::SHOWN_S;
            if let Some(n) = st.notice.as_mut() {
                if let Some(text) = n.reveal.take() {
                    n.subtitle = text;
                    n.until = n.until.max(until);
                }
            }
            return;
        }
        match a {
            InlineAction::StartFocus => st.pomodoro.start(st.now),
            InlineAction::ShowAmount => {}
        }
        let until = st.now + INLINE_DONE_S;
        if let Some(n) = st.notice.as_mut() {
            n.icon = Icon::Check;
            n.until = until;
        }
        pinfo!("notice: inline action {a:?} done");
    }

    fn set_page(&mut self, p: PageKind) {
        if p != self.st.page {
            self.st.page = p;
            // Settings is never where the island opens next time.
            if p != PageKind::Settings {
                self.st.store.last_page = p.key().to_string();
                unsafe {
                    SetTimer(self.hwnd, TIMER_SAVE, 800, None);
                }
            } else {
                crate::autostart::report(self.sink.clone(), None);
            }
            self.set_stats(p == PageKind::Today && self.expanded);
            self.set_sysmon(p == PageKind::System && self.expanded);
            if self.text_focus {
                self.blur_text();
            }
            if p == PageKind::Oyenza {
                self.oyenza(OyenzaCmd::Refresh);
            }
        }
    }

    fn run_commands(&mut self, deferred: &mut Vec<Deferred>) {
        let cmds = std::mem::take(&mut self.st.cmds);
        for c in cmds {
            match c {
                Command::Media(m) => {
                    if let Some(s) = self.services.as_ref() {
                        s.media_cmd(m.clone());
                    }
                    // Optimistic UI for play/pause.
                    if m == MediaCmd::PlayPause {
                        if let Some(ms) = self.st.media.as_mut() {
                            let now = self.st.now;
                            let pos = ms.timeline.position_at(now);
                            ms.timeline.position_s = pos;
                            ms.timeline.sampled_at = now;
                            ms.timeline.playing = !ms.timeline.playing;
                            // The player's confirmation will match this, so
                            // start (or end) the pause grace period here.
                            self.st.media_paused_at = (!ms.timeline.playing).then_some(now);
                        }
                    }
                }
                Command::OpenPath(p) => shell::open(&p),
                Command::RevealPath(p) => shell::reveal(&p),
                Command::DragOut(paths) => deferred.push(Deferred::DragOut(paths)),
                Command::LoadThumb(p) => {
                    if !self.st.shelf_thumbs.contains_key(&p) {
                        self.st.shelf_thumbs.insert(p.clone(), None);
                        services::imaging::load_thumb_async(p, self.sink.clone());
                    }
                }
                Command::CopyText(t) => {
                    self.own_clip = Some(t.clone());
                    services::clipboard::set_text(self.helper, &t);
                }
                Command::FocusText => self.focus_text(),
                Command::BlurText => self.blur_text(),
                Command::SaveSettings => {
                    let before = self.saved_settings.clone();
                    self.settings_changed(&before);
                    self.save_settings();
                }
                Command::SaveStore => unsafe {
                    SetTimer(self.hwnd, TIMER_SAVE, 800, None);
                },
                Command::SetPage(p) => self.set_page(p),
                Command::OpenSystem(m) => {
                    self.st.ui.system_module = m;
                    self.set_page(PageKind::System);
                }
                Command::LoadAppIcon(path) => {
                    if !self.st.app_icons.contains_key(&path) {
                        self.st.app_icons.insert(path.clone(), None);
                        self.icon_wait.insert(path.clone());
                        services::imaging::load_thumb_async(path, self.sink.clone());
                    }
                }
                Command::OpenTaskManager => shell::open_task_manager(),
                Command::SetPinned(b) => {
                    self.st.pinned = b;
                    if !b {
                        self.maybe_schedule_collapse();
                    }
                }
                Command::Collapse => self.collapse(),
                Command::Play(s) => services::sound::play(s),
                Command::OpenUrl(u) => shell::open(&u),
                Command::WantStats(b) => self.set_stats(b),
                Command::SetAutostart(on) => {
                    self.st.ui.autostart = Some(on);
                    crate::autostart::report(self.sink.clone(), Some(on));
                }
                Command::Phone(c) => self.phone_cmd(c),
                Command::Pair(c) => self.pair_cmd(c),
                Command::Oyenza(c) => self.oyenza(c),
            }
        }
        self.sync_phone_offer();
        self.sync_pair_offer();
    }

    // ------------------------------------------------------------ phone drop

    /// The QR code waits for a phone, or a phone waits for an answer (Shelf page only).
    fn phone_holds(&self) -> bool {
        self.st.phone_holds_island()
    }

    fn phone_cmd(&mut self, c: PhoneCmd) {
        match c {
            PhoneCmd::Open => self.open_phone(),
            PhoneCmd::Close => self.close_phone(),
            PhoneCmd::Answer { id, accept } => {
                if let Some(svc) = &self.phone {
                    svc.answer(id, accept);
                }
                let until = self.st.now + pages::shelf::PHONE_OUTCOME_S;
                if let Some(pd) = self.st.phone.as_mut() {
                    if let Some(a) = pd.ask.take_if(|a| a.id == id) {
                        if accept {
                            pd.transfer = Some(PhoneTransfer { id, from: a.from, file: 0, files: a.files.len(), done: 0, total: a.total });
                        } else {
                            pd.outcome = Some((format!("Declined {} from {}", files_label(a.files.len()), a.from), false, until));
                        }
                    }
                }
                self.maybe_schedule_collapse();
            }
        }
    }

    fn open_phone(&mut self) {
        if let Some(old) = self.phone.take() {
            old.stop();
        }
        let mut pd = PhoneDrop::default();
        if self.selftest.is_some() {
            pd.error = Some("Phone drop is off in the self-test.".into());
        } else {
            let offer = self.st.store.shelf.paths();
            match services::phonedrop::PhoneDrop::start(self.sink.clone(), offer.iter().map(PathBuf::from).collect()) {
                Ok((svc, url)) => {
                    pd.qr = QrMatrix::encode(&url);
                    pd.url = url;
                    self.phone_offer = offer;
                    self.phone = Some(svc);
                }
                Err(e) => {
                    pwarn!("phone drop: {e}");
                    pd.error = Some(e);
                }
            }
        }
        self.st.phone = Some(pd);
        self.st.page = PageKind::Shelf;
        self.expand();
    }

    fn close_phone(&mut self) {
        if let Some(svc) = self.phone.take() {
            svc.stop();
        }
        if matches!(self.st.phone.as_ref().map(|p| &p.view), Some(PhoneView::Pairing { .. })) {
            if let Some(s) = &self.services {
                s.pair_cancel();
            }
        }
        self.st.phone = None;
        self.phone_offer.clear();
        self.maybe_schedule_collapse();
        self.request_frame();
    }

    /// Minute tick: nobody has used the link for a while, so close it.
    fn close_idle_phone(&mut self) {
        let idle = self.phone.as_ref().is_some_and(|svc| !svc.busy() && svc.idle_secs() >= PHONE_IDLE_S);
        let quiet = self.st.phone.as_ref().is_some_and(|p| p.ask.is_none() && p.transfer.is_none());
        if idle && quiet {
            pinfo!("phone drop: closed after {:.0} minutes without a phone", PHONE_IDLE_S / 60.0);
            self.close_phone();
        }
    }

    /// Keep the files a phone can save in step with the Shelf.
    fn sync_phone_offer(&mut self) {
        let Some(svc) = &self.phone else { return };
        let paths = self.st.store.shelf.paths();
        if paths != self.phone_offer {
            svc.set_offer(paths.iter().map(PathBuf::from).collect());
            self.phone_offer = paths;
        }
    }

    fn on_phone(&mut self, e: PhoneEvent) {
        let now = self.st.now;
        let until = now + pages::shelf::PHONE_OUTCOME_S;
        let Some(pd) = self.st.phone.as_mut() else { return };
        match e {
            PhoneEvent::Visit { from } => {
                pd.visitor = Some(from);
                self.maybe_schedule_collapse();
            }
            PhoneEvent::Ask(a) => {
                pd.outcome = None;
                pd.visitor.get_or_insert_with(|| a.from.clone());
                let files: Vec<(String, u64)> = a.files.iter().map(|f| (f.name.clone(), f.size)).collect();
                pinfo!("phone drop: {} asks to send {}", a.from, files_label(files.len()));
                pd.ask = Some(PhoneAsk { id: a.id, from: a.from.clone(), total: a.total(), files });
                self.st.page = PageKind::Shelf;
                self.expand();
                self.st.chime();
            }
            PhoneEvent::AskEnded { id } => {
                if let Some(a) = pd.ask.take_if(|a| a.id == id) {
                    pd.outcome = Some((format!("{} stopped waiting", a.from), false, until));
                }
                self.maybe_schedule_collapse();
            }
            PhoneEvent::Progress { id, file, files, done, total } => match pd.transfer.as_mut() {
                Some(t) if t.id == id => {
                    t.file = file;
                    t.files = files;
                    t.done = done;
                    t.total = total;
                }
                _ => {
                    let from = pd.visitor.clone().unwrap_or_else(|| "Phone".into());
                    pd.transfer = Some(PhoneTransfer { id, from, file, files, done, total });
                }
            },
            PhoneEvent::Received { from, paths, .. } => {
                pd.transfer = None;
                let n = paths.len();
                let title = format!("{} from {from} {} on the Shelf", files_label(n), if n == 1 { "is" } else { "are" });
                pd.outcome = Some((title.clone(), true, until));
                let added: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                self.st.store.shelf.add_paths(&added, self.st.unix);
                for p in added {
                    self.st.cmds.push(Command::LoadThumb(p));
                }
                self.st.save();
                pinfo!("phone drop: {} from {from}", files_label(n));
                if !self.expanded {
                    self.st.notify(Icon::Phone, palette::GREEN, title, "Open the Shelf to see them", 6.0, Some(NoticeAction::OpenPage(PageKind::Shelf)));
                }
                self.st.chime();
                self.maybe_schedule_collapse();
            }
            PhoneEvent::Failed { from, reason, detail, .. } => {
                pd.transfer = None;
                pd.outcome = Some((format!("Nothing kept from {from}. {reason}."), false, until));
                pwarn!("phone drop: transfer from {from} failed: {reason} {detail}");
                self.maybe_schedule_collapse();
            }
        }
    }

    // ------------------------------------------------------------ paired phones

    fn pair_cmd(&mut self, c: PairCmd) {
        let now = self.st.now;
        match c {
            PairCmd::Begin => {
                let r = match &self.services {
                    Some(s) => s.pair_begin(),
                    None => Err("Pairing is off in the self-test.".into()),
                };
                let Some(pd) = self.st.phone.as_mut() else { return };
                pd.outcome = None;
                match r {
                    Ok(url) => {
                        pinfo!("pair drop: showing a pairing code");
                        pd.view = PhoneView::Pairing { qr: QrMatrix::encode(&url), url, until: now + poni_core::pairdrop::meet::PAIRING_FOR.as_secs_f64() };
                    }
                    Err(e) => {
                        pwarn!("pair drop: {e}");
                        pd.outcome = Some((e, false, now + pages::shelf::PHONE_OUTCOME_S));
                    }
                }
            }
            PairCmd::Back => {
                if matches!(self.st.phone.as_ref().map(|p| &p.view), Some(PhoneView::Pairing { .. })) {
                    if let Some(s) = &self.services {
                        s.pair_cancel();
                    }
                }
                if let Some(pd) = self.st.phone.as_mut() {
                    pd.view = PhoneView::Lan;
                }
            }
            PairCmd::List => {
                if matches!(self.st.phone.as_ref().map(|p| &p.view), Some(PhoneView::Pairing { .. })) {
                    if let Some(s) = &self.services {
                        s.pair_cancel();
                    }
                }
                if let Some(pd) = self.st.phone.as_mut() {
                    pd.view = PhoneView::Paired;
                }
            }
            PairCmd::Remove(room) => {
                if let Some(s) = &self.services {
                    s.pair_remove(&room);
                }
                if let Some(i) = self.st.paired.iter().position(|p| p.room == room) {
                    let p = self.st.paired.remove(i);
                    pinfo!("pair drop: removed a paired phone");
                    if let Some(pd) = self.st.phone.as_mut() {
                        pd.outcome = Some((format!("{} can't send to this PC any more", p.name), false, now + pages::shelf::PHONE_OUTCOME_S));
                        pd.outcome_line = Some("Pair it again any time from Pair a phone.".into());
                    }
                }
            }
        }
        self.request_frame();
    }

    /// Keep the files paired phones may save in step with the Shelf.
    fn sync_pair_offer(&mut self) {
        let Some(svc) = &self.services else { return };
        let paths = self.st.store.shelf.paths();
        if self.pair_offer.as_ref() != Some(&paths) {
            svc.pair_shelf(paths.iter().map(PathBuf::from).collect());
            self.pair_offer = Some(paths);
        }
    }

    fn on_paired(&mut self, e: MeetEvent) {
        let now = self.st.now;
        let until = now + pages::shelf::PHONE_OUTCOME_S;
        match e {
            MeetEvent::Pairs(list) => self.st.paired = paired_phones(list),
            MeetEvent::Server { reachable } => {
                if self.st.pair_server.is_some() || !reachable {
                    pinfo!("pair drop: meeting server {}", if reachable { "reachable again" } else { "unreachable" });
                }
                self.st.pair_server = Some(reachable);
            }
            MeetEvent::Paired { name, .. } => {
                pinfo!("pair drop: a phone finished pairing");
                if let Some(pd) = self.st.phone.as_mut() {
                    pd.view = PhoneView::Lan;
                    pd.outcome = Some((format!("{name} is paired"), true, until));
                    pd.outcome_line = Some("It can now send from any network, even with this panel closed.".into());
                }
                if !self.hovering {
                    // The island closes now that nothing waits: say it on the pill.
                    self.st.notify(Icon::Phone, palette::GREEN, format!("{name} is paired"), "It can send from any network now", 6.0, Some(NoticeAction::OpenPage(PageKind::Shelf)));
                }
                self.st.chime();
                self.maybe_schedule_collapse();
            }
            MeetEvent::Phone { phone, ev, .. } => match ev {
                PeerEvent::Hello { .. } => pinfo!("pair drop: a paired phone connected"),
                PeerEvent::Progress { files, done: 0, .. } => {
                    if !self.expanded {
                        self.st.notify(Icon::Phone, palette::GREEN, format!("{phone} is sending {}", files_label(files)), "They go straight onto the Shelf", 4.0, None);
                    }
                }
                PeerEvent::Progress { .. } => {}
                PeerEvent::Received { paths } => {
                    let n = paths.len();
                    let title = format!("{} from {phone} {} on the Shelf", files_label(n), if n == 1 { "is" } else { "are" });
                    let added: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    self.st.store.shelf.add_paths(&added, self.st.unix);
                    for p in added {
                        self.st.cmds.push(Command::LoadThumb(p));
                    }
                    self.st.save();
                    pinfo!("pair drop: {} from a paired phone", files_label(n));
                    if let Some(pd) = self.st.phone.as_mut() {
                        pd.outcome = Some((title.clone(), true, until));
                        pd.outcome_line = None;
                    }
                    if !self.expanded {
                        self.st.notify(Icon::Phone, palette::GREEN, title, "Open the Shelf to see them", 6.0, Some(NoticeAction::OpenPage(PageKind::Shelf)));
                    }
                    self.st.chime();
                    self.maybe_schedule_collapse();
                }
                PeerEvent::Failed { reason, detail } => {
                    pwarn!("pair drop: a send from a paired phone failed: {reason} {detail}");
                    if !self.expanded {
                        self.st.notify(Icon::Phone, palette::TEXT, format!("Nothing kept from {phone}"), format!("{reason}."), 6.0, None);
                    } else if let Some(pd) = self.st.phone.as_mut() {
                        pd.outcome = Some((format!("Nothing kept from {phone}. {reason}."), false, until));
                        pd.outcome_line = None;
                    }
                }
                PeerEvent::Ended { .. } => {}
            },
        }
        self.request_frame();
    }

    fn save_now(&mut self) {
        if !self.store_writable {
            pwarn!("data.json couldn't be read at start; not overwriting it");
            return;
        }
        if let Err(e) = save_json_atomic(&paths::store_path(), &self.st.store) {
            pwarn!("saving data failed: {e}");
        }
    }

    // ------------------------------------------------------------ self-test hooks

    /// Self-test only: render as if the monitor had `dpi` (120 = 125 %, 144 = 150 %, 192 = 200 %).
    pub fn selftest_set_dpi(&mut self, dpi: u32) {
        let dpi = dpi.clamp(96, 480);
        self.monitor.dpi = dpi;
        self.set_dpi(dpi);
        self.request_frame();
    }

    pub fn selftest_set_expanded(&mut self, on: bool) {
        if on {
            self.expand();
        } else {
            self.collapse();
        }
    }

    /// Self-test only: start the real Phone drop service, fetch its page from
    /// the network address it picked, and stop it. Returns (ok, detail).
    pub fn selftest_phone_drop(&mut self) -> (bool, String) {
        match services::phonedrop::PhoneDrop::start(self.sink.clone(), Vec::new()) {
            Err(e) => (false, e),
            Ok((svc, url)) => {
                let r = selftest::fetch_phone_page(&url);
                svc.stop();
                r
            }
        }
    }

    /// At rest: outline settled, mode current, no page switch or outgoing fade.
    pub fn selftest_settled(&self) -> bool {
        self.island.settled() && self.target_mode() == self.mode && self.page_anim.is_none() && self.outgoing.is_none() && !self.glass_moving
    }

    /// Self-test, Glass theme: `px` (BGRA, the window's size) is what's under
    /// the window, and Windows is in light (`app_light`) or dark mode.
    pub fn selftest_glass_backdrop(&mut self, px: Vec<u8>, w: i32, h: i32, app_light: bool) {
        self.glass.app_light = app_light;
        self.glass.inject(px, w, h, &mut self.gfx);
    }

    /// Self-test: a new scene starts (the glass takes its side at once).
    pub fn selftest_glass_reseed(&mut self) {
        self.glass.reseed();
    }

    /// Self-test: the lens strength, and whether the glass shows the backdrop.
    pub fn selftest_glass_switches(&mut self, lens: f32, use_copy: bool) {
        self.glass.lens = lens;
        self.glass.use_copy = use_copy;
    }

    /// Self-test: the last frame's glass came from the shader, the backdrop
    /// is on the GPU, and the look content was painted in.
    pub fn selftest_glass_state(&self) -> (bool, bool, &'static poni_core::look::Look) {
        (self.gfx.glass_drew(), self.gfx.glass_has_copy(), self.gfx.look())
    }

    /// Self-test: the captured rectangle (surface pixels).
    pub fn selftest_capture_rect(&self) -> RECT {
        self.capture_rect
    }

    /// Render a frame right now (bypassing the clock) and return the pixels
    /// of the island's canvas (island, shoulders and hit margin) and where
    /// that canvas sits on screen.
    pub fn selftest_capture(&mut self) -> (i32, i32, Vec<u8>, (i32, i32)) {
        let mut d = Vec::new();
        self.capture_next = true;
        self.frame(&mut d, None);
        let r = self.capture_rect;
        (r.right - r.left, r.bottom - r.top, self.gfx.take_capture(), (self.surface.x + r.left, self.surface.y + r.top))
    }

    /// The window's screen rectangle as laid out (left, top, width, height).
    pub fn selftest_surface(&self) -> (i32, i32, i32, i32) {
        (self.surface.x, self.surface.y, self.surface.w, self.surface.h)
    }

    pub fn renderer(&self) -> &'static str {
        self.renderer_name()
    }

    pub fn selftest_hover(&mut self, p: Option<Point>) {
        self.input.mouse = p;
        self.hovering = p.is_some();
    }

    /// The middle of a widget drawn in the last frame, found by its id.
    pub fn selftest_hit_center(&self, id: crate::ui::WidgetId) -> Option<Point> {
        self.hits.iter().find(|h| h.id == id).map(|h| h.rect.center())
    }

    pub fn selftest_click(&mut self, p: Point) {
        self.input.mouse = Some(p);
        if let Some(h) = self.hit_at(p) {
            self.input.clicked = Some((h.id, MouseButton::Left));
        }
    }

    /// The island body in window DIPs (where clicks land).
    pub fn island_body(&self) -> Rect {
        self.island.body_rect_at(self.cx)
    }

    /// The island body relative to the last captured frame (DIPs).
    pub fn island_body_in_capture(&self) -> Rect {
        let s = self.gfx.scale();
        let r = self.capture_rect;
        self.island.body_rect_at(self.cx).offset(-(r.left as f32) / s, -(r.top as f32) / s)
    }

    pub fn scale(&self) -> f32 {
        self.gfx.scale()
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

/// Oyen's orange in his theme, Windows' accent colour otherwise.
fn theme_accent(theme: Theme) -> Color {
    if oyen::on(theme) {
        oyen::ORANGE
    } else if theme == Theme::Glass {
        services::system_accent_base().unwrap_or(palette::BLUE)
    } else {
        services::system_accent().unwrap_or(palette::BLUE)
    }
}

/// The island's current content: a page (expanded) or the collapsed activity.
/// Returns (repaint, anim, focus, live) as requested by the drawing code.
#[allow(clippy::too_many_arguments)]
fn draw_content(
    c: &mut Canvas,
    input: &mut Input,
    hits: &mut Vec<Hit>,
    st: &mut AppState,
    mode: Mode,
    lay: Rect,
    body: Rect,
    transition: Option<pages::PageTransition>,
    now: f64,
) -> (bool, bool, Option<bool>, bool) {
    let mut ui = Ui::new(c, input, hits, now);
    match mode {
        Mode::Expanded(page) => pages::draw_expanded(&mut ui, st, lay, page, transition),
        _ => activities::draw(&mut ui, st, &mode, body),
    }
    (ui.repaint_requested(), ui.anim_requested(), ui.focus_request(), ui.live_requested())
}

/// DWM's (refreshes, frames composed, frames missed) counters, if available.
fn dwm_counters() -> Option<(u64, u64, u64)> {
    use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
    let mut info = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut info) }.ok()?;
    Some((info.cRefresh, info.cFrame, info.cFramesMissed))
}

/// Name of a morph for the motion log.
fn motion_name(from: &Mode, to: &Mode) -> &'static str {
    match (from, to) {
        (Mode::Expanded(_), Mode::Expanded(_)) => "page",
        (_, Mode::Expanded(_)) => "expand",
        (Mode::Expanded(_), _) => "collapse",
        _ => "pill",
    }
}

/// "1 file", "3 files".
/// The panel's list: most recently used first.
fn paired_phones(pairs: Vec<poni_core::pairdrop::Pair>) -> Vec<PairedPhone> {
    let mut v: Vec<PairedPhone> = pairs.into_iter().map(|p| PairedPhone { room: p.room(), name: p.name, seen: p.seen.max(p.added) }).collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.seen));
    v
}

fn files_label(n: usize) -> String {
    format!("{n} {}", if n == 1 { "file" } else { "files" })
}

/// The island's notice for the moment new numbers from the phone put today
/// over the step goal (see `poni_core::health::goal_reached`).
pub(crate) fn steps_goal_notice(steps: u32, until: f64) -> Notice {
    Notice {
        icon: Icon::Steps,
        color: palette::GREEN,
        title: format!("{} steps today", fmt_steps(steps)),
        subtitle: "Daily step goal reached".into(),
        until,
        action: Some(NoticeAction::OpenPage(PageKind::Health)),
        inline: None,
        reveal: None,
    }
}
