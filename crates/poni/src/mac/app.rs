//! The Mac app: owns the island panel, state and renderer; routes AppKit
//! input, timers and page commands.
//!
//! The island behaves as on Windows (`../app.rs`): it lifts under the pointer,
//! opens once the pointer rests on it, closes a moment after it leaves,
//! morphs between states on the same springs, and runs the same pages. This
//! file is the AppKit plumbing around that, plus the camera notch
//! (`notch.rs`). AppKit calls in from the main thread only (view input,
//! the display link, timers, the menu, mouse monitors); every entry point
//! takes the app from a thread-local and never holds it while AppKit could
//! call back in.

use crate::activities::{self, Mode};
use crate::clock;
use crate::commands::{Command, PhoneCmd, Sound};
use crate::events::{AppEvent, EventSink};
use crate::gfx::{scale_about, union_rect, Canvas, Gfx, Matrix3x2};
use crate::icons::Icon;
use crate::island::{Geom, Island, HIT_MARGIN, SHOULDER};
use crate::menubar::{Login, MenuBar, MenuCmd};
use crate::notch::{self, Fit, Notch};
use crate::oyen;
use crate::pages::{self, PageKind};
use crate::paths;
use crate::services;
use crate::state::{AppState, HudKind, InlineAction, NoticeAction, PhoneAsk, PhoneDrop, PhoneTransfer, VolumeState};
use crate::ui::{wid, CursorKind, Hit, Input, KeyInput, MouseButton, Ui};
use crate::window::{self, ScreenInfo, ViewInput};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDidChangeScreenParametersNotification, NSCursor, NSEvent, NSEventMask, NSMenu};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSRunLoop, NSRunLoopCommonModes, NSTimer};
use poni_core::frametime::MotionStats;
use poni_core::phonedrop::{Event as PhoneEvent, QrMatrix};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::settings::{IdleStyle, Settings, Theme};
use poni_core::store::{load_json, parse_json, save_json_atomic, LoadOutcome, Store};
use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver};

/// Where the README explains the project.
const ABOUT_URL: &str = "https://github.com/lucacadalora/poni";

/// The grow springs overshoot by about 3 %; the window leaves 5 %.
const OVERSHOOT: f32 = 1.05;
/// How long outgoing content takes to fade (seconds).
const OUTGOING_FADE: f64 = 0.14;
/// Page switch cross-fade duration (seconds).
const PAGE_SWITCH: f64 = 0.26;
/// Pointer travel (DIPs) that restarts the hover dwell: the island opens
/// only once the pointer rests on it, not when it brushes past.
const DWELL_SLOP: f32 = 4.0;
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
/// Phone drop closes itself after this long without a request from a phone.
const PHONE_IDLE_S: f64 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timer {
    Hover,
    Collapse,
    Wake,
    Save,
    Minute,
    Smoke,
    Stats,
}
const TIMERS: [Timer; 7] = [Timer::Hover, Timer::Collapse, Timer::Wake, Timer::Save, Timer::Minute, Timer::Smoke, Timer::Stats];

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Mouse monitors and observers stay registered for the app's life.
    static KEEP: RefCell<Vec<Retained<AnyObject>>> = const { RefCell::new(Vec::new()) };
}

/// Work that must run without the app borrowed (AppKit may call back in).
enum Deferred {
    TakeKeyboard,
    ReleaseKeyboard,
    Show,
    /// Open the menu bar item's menu (the smoke test's picture of it).
    OpenMenu,
}

/// Run `f` with the app if it isn't already borrowed (re-entrancy safe), then
/// run the AppKit calls it queued.
fn enter<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    let r = APP.with(|c| match c.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    });
    let pending = APP.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(|a| std::mem::take(&mut a.pending)))).unwrap_or_default();
    for d in pending {
        match d {
            Deferred::TakeKeyboard => window::take_keyboard(),
            Deferred::ReleaseKeyboard => window::release_keyboard(),
            Deferred::Show => window::show(),
            Deferred::OpenMenu => crate::menubar::open_menu(),
        }
    }
    r
}

// ---------------------------------------------------------------- entry points (called by AppKit glue)

pub fn main_entry() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let arg_after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args.get(i + 1).cloned().unwrap_or_else(|| "poni-out".into()));
    if let Some(dir) = arg_after("--render") {
        return crate::render::run(&dir, &args);
    }
    let smoke_dir = arg_after("--smoke").map(PathBuf::from);
    if let Some(dir) = &smoke_dir {
        // Absolute: Finder icons and file URLs need full paths.
        let home = std::path::absolute(dir.join("home")).unwrap_or_else(|_| dir.join("home"));
        let _ = std::fs::remove_dir_all(&home);
        std::env::set_var("PONI_HOME", &home);
    }
    crate::plog::init(paths::log_path());
    pinfo!("Oyen Poni {} starting on macOS (args: {:?})", env!("CARGO_PKG_VERSION"), &args[1..]);
    std::panic::set_hook(Box::new(|info| {
        perror!("panic: {info}");
    }));
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("Oyen Poni must start on the main thread");
        return 2;
    };
    let _lock = match single_instance() {
        Ok(lock) => lock,
        Err(()) => {
            pinfo!("another instance is running; exiting");
            return 0;
        }
    };
    let nsapp = NSApplication::sharedApplication(mtm);
    // A menu bar app: no Dock icon, no menu bar of its own.
    nsapp.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    window::create(mtm);
    let app = App::new(mtm, smoke_dir);
    APP.with(|c| *c.borrow_mut() = Some(app));
    enter(|a| a.start());
    install_monitors();
    pinfo!("started; running");
    nsapp.run();
    0
}

/// Hold a lock file for as long as Poni runs. `Err` = another copy holds it.
fn single_instance() -> Result<Option<std::fs::File>, ()> {
    use std::os::fd::AsRawFd;
    extern "C" {
        fn flock(fd: i32, op: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    let path = paths::lock_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path) else {
        return Ok(None);
    };
    if unsafe { flock(f.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        return Err(());
    }
    Ok(Some(f))
}

/// Pointer moved anywhere on screen (global and local mouse monitors).
pub fn pointer_moved() {
    if let Some((x, y)) = window::pointer() {
        enter(|a| a.on_pointer(Point::new(x, y)));
    }
}

fn install_monitors() {
    let mask = NSEventMask::MouseMoved | NSEventMask::LeftMouseDragged;
    let global = RcBlock::new(|_e: NonNull<NSEvent>| pointer_moved());
    let local = RcBlock::new(|e: NonNull<NSEvent>| -> *mut NSEvent {
        pointer_moved();
        e.as_ptr()
    });
    let mut keep = Vec::new();
    // Mouse movement needs no permission (only keyboard monitoring does).
    if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global) {
        keep.push(m);
    }
    if let Some(m) = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) } {
        keep.push(m);
    }
    let screens = RcBlock::new(|_n: NonNull<NSNotification>| {
        enter(|a| a.refresh_screen());
    });
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(Some(NSApplicationDidChangeScreenParametersNotification), None, None, &screens)
    };
    // Registered for the app's whole life.
    std::mem::forget(observer);
    KEEP.with(|k| k.borrow_mut().extend(keep));
}

pub fn view_input(input: ViewInput, event: Option<&NSEvent>) {
    let menu = enter(|a| a.on_view_input(input)).flatten();
    if let (Some(menu), Some(event), Some(view)) = (menu, event, window::view()) {
        NSMenu::popUpContextMenu_withEvent_forView(&menu, event, &view);
    }
}

pub fn on_frame_tick(timestamp: f64, target: f64) {
    let dirty = enter(|a| a.begin_frame(timestamp, target)).flatten();
    if let Some(d) = dirty {
        window::redraw(d);
    }
    enter(|a| a.end_frame());
}

pub fn draw_into(ctx: &CGContext, dirty: Rect) {
    enter(|a| a.draw(ctx, dirty));
}

/// An event raised on the main thread (drag and drop): apply it now.
pub fn post(ev: AppEvent) {
    enter(|a| a.on_event(ev));
}

/// A Shelf item was dragged far enough to leave the island: start the
/// system drag from the press in progress.
pub fn start_pending_drag() {
    let paths = enter(|a| {
        let p = a.pending_drag.take();
        if p.is_some() {
            a.input.down = false;
            a.input.pressed_id = None;
        }
        p
    })
    .flatten();
    if let Some(paths) = paths {
        if !window::begin_drag(&paths) {
            pwarn!("drag out: no press to start from");
        }
    }
}

pub fn on_resign_key() {
    enter(|a| a.on_resign_key());
}

pub fn on_menu(cmd: MenuCmd) {
    enter(|a| a.on_menu(cmd));
}

pub fn drain_events() {
    enter(|a| {
        while let Ok(ev) = a.rx.try_recv() {
            a.on_event(ev);
        }
        a.request_frame();
    });
}

// ---------------------------------------------------------------- the app

/// What the last drawn frame asked for.
#[derive(Clone, Copy, Debug, Default)]
struct Drawn {
    repaint: bool,
    anim: bool,
    focus: Option<bool>,
    live: bool,
}

pub struct App {
    mtm: MainThreadMarker,
    gfx: Gfx,
    pub st: AppState,
    island: Island,
    mode: Mode,
    input: Input,
    hits: Vec<Hit>,
    hovering: bool,
    expanded: bool,
    text_focus: bool,
    cursor: CursorKind,
    screen: ScreenInfo,
    /// The window: a fixed-size surface (DIPs) the island is drawn centred in.
    surface: (f32, f32),
    /// Centre of the surface (where the island hangs from).
    cx: f32,
    #[allow(dead_code)]
    sink: EventSink,
    rx: Receiver<AppEvent>,
    menubar: Option<MenuBar>,
    pending: Vec<Deferred>,
    /// Animation time of the last frame (the refresh it was shown on).
    last_frame_t: f64,
    /// The canvas drawn last frame (the next frame redraws it too).
    prev_canvas: Option<Rect>,
    drawn: Drawn,
    /// Content of the previous mode, fading out while the outline morphs.
    outgoing: Option<(Mode, f64, Geom)>,
    /// A page switch in progress: (from page, start time, slide direction).
    page_anim: Option<(PageKind, f64, f32)>,
    /// Where the hover (dwell) timer was last armed; None when it isn't.
    hover_arm: Option<Point>,
    /// The current press started as a double-click (its click is ignored).
    dbl_press: bool,
    wheel_acc: f32,
    wheel_t: f64,
    page_switch_t: f64,
    /// Deadlines (mono seconds) of the app's timers, and the one NSTimer
    /// set for the earliest.
    timers: [Option<f64>; 7],
    ns_timer: Option<(f64, Retained<NSTimer>)>,
    saved_settings: Settings,
    settings_writable: bool,
    store_writable: bool,
    first_run: bool,
    /// The Today page's stats are being sampled (once a second).
    stats_on: bool,
    /// Text we put on the clipboard ourselves (not recorded again).
    own_clip: Option<String>,
    clipboard: Option<services::clipboard::Watcher>,
    /// Files are being dragged over the island.
    drag_hover: bool,
    /// Shelf items waiting to be dragged out (started by the next drag event).
    pending_drag: Option<Vec<String>>,
    /// Phone drop's web server while the Shelf's phone panel is open, and the
    /// shelf files it offers.
    phone: Option<services::phonedrop::PhoneDrop>,
    phone_offer: Vec<String>,
    motion: Option<MotionStats>,
    motion_name: &'static str,
    motions_logged: u32,
    frames_drawn: u64,
    smoke: Option<Smoke>,
}

impl App {
    fn new(mtm: MainThreadMarker, smoke_dir: Option<PathBuf>) -> App {
        let (raw_settings, s_out): (Settings, _) = load_json(&paths::settings_path());
        if let Some(m) = s_out.message() {
            pwarn!("settings: {m}");
        }
        let settings = raw_settings.clone().sanitized();
        if s_out == LoadOutcome::Missing || (settings != raw_settings && s_out.writable()) {
            if let Err(e) = save_json_atomic(&paths::settings_path(), &settings) {
                pwarn!("saving settings failed: {e}");
            }
        }
        let (store, d_out): (Store, _) = load_json(&paths::store_path());
        if let Some(m) = d_out.message() {
            pwarn!("data: {m}");
        }
        let (tx, rx) = mpsc::channel();
        let sink = EventSink::new(tx);
        let screen = window::pick_screen(mtm).unwrap_or(ScreenInfo { x: 0.0, y: 0.0, w: 1440.0, h: 900.0, scale: 2.0, notch: None, cx: 720.0 });
        pinfo!("screen {}x{} pt at {}x, notch {:?}", screen.w, screen.h, screen.scale, screen.notch);
        let gfx = Gfx::new(screen.scale);
        let mut st = AppState::new(settings, store);
        st.now = clock::mono();
        st.local = clock::local_now();
        st.unix = clock::unix_now();
        st.accent = theme_accent(st.settings.theme);
        let saved_settings = st.settings.clone();
        let smoke = smoke_dir.map(Smoke::new);
        App {
            mtm,
            gfx,
            st,
            island: Island::new(Geom::new(118.0, 30.0, 14.0)),
            mode: Mode::Idle,
            input: Input::default(),
            hits: Vec::new(),
            hovering: false,
            expanded: false,
            text_focus: false,
            cursor: CursorKind::Arrow,
            screen,
            surface: (0.0, 0.0),
            cx: 0.0,
            sink,
            rx,
            menubar: None,
            pending: Vec::new(),
            last_frame_t: 0.0,
            prev_canvas: None,
            drawn: Drawn::default(),
            outgoing: None,
            page_anim: None,
            hover_arm: None,
            dbl_press: false,
            wheel_acc: 0.0,
            wheel_t: 0.0,
            page_switch_t: 0.0,
            timers: [None; 7],
            ns_timer: None,
            saved_settings,
            settings_writable: s_out.writable(),
            store_writable: d_out.writable(),
            first_run: s_out == LoadOutcome::Missing,
            stats_on: false,
            own_clip: None,
            clipboard: None,
            drag_hover: false,
            pending_drag: None,
            phone: None,
            phone_offer: Vec::new(),
            motion: None,
            motion_name: "morph",
            motions_logged: 0,
            frames_drawn: 0,
            smoke,
        }
    }

    fn start(&mut self) {
        self.layout_surface();
        let g = self.geom_for(&self.mode);
        self.island.snap(g);
        self.menubar = Some(MenuBar::add(self.mtm, &self.st.settings, self.st.pinned, services::login::status()));
        self.st.power = services::power::query();
        services::power::watch(self.sink.clone());
        services::audio::start(self.sink.clone());
        self.sync_clipboard();
        // Shelf tiles for what was left on the Shelf.
        for p in self.st.store.shelf.paths() {
            self.st.cmds.push(Command::LoadThumb(p));
        }
        self.schedule_minute_timer();
        if self.first_run && self.smoke.is_none() {
            let place = if self.screen.notch.is_some() { "Point at the notch to open it" } else { "Point at the island to open it" };
            self.st.notify(Icon::CheckCircle, self.st.accent, "Oyen Poni is on", place, 6.0, None);
        }
        if self.smoke.is_some() {
            self.set_timer(Timer::Smoke, 600);
        }
        self.pending.push(Deferred::Show);
        self.request_frame();
    }

    /// Save and leave (Quit, or the smoke test finishing).
    fn quit(&mut self, code: i32) {
        self.save_now();
        self.save_settings();
        pinfo!("Oyen Poni exiting with code {code}");
        std::process::exit(code);
    }

    pub fn request_frame(&mut self) {
        window::set_frames(true);
    }

    // ------------------------------------------------------------ timers

    fn set_timer(&mut self, t: Timer, ms: u32) {
        self.timers[t as usize] = Some(clock::mono() + ms as f64 / 1000.0);
        self.sync_timer();
    }

    fn kill_timer(&mut self, t: Timer) {
        self.timers[t as usize] = None;
    }

    /// Keep one NSTimer armed for the earliest deadline.
    fn sync_timer(&mut self) {
        let Some(next) = self.timers.iter().flatten().copied().reduce(f64::min) else { return };
        if let Some((at, _)) = &self.ns_timer {
            if (*at - next).abs() < 0.0005 {
                return;
            }
        }
        if let Some((_, old)) = self.ns_timer.take() {
            old.invalidate();
        }
        let delay = (next - clock::mono()).max(0.001);
        let block = RcBlock::new(|_t: NonNull<NSTimer>| {
            enter(|a| a.on_timers());
        });
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(delay, false, &block) };
        // Common modes: timers keep firing while a menu is open.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        self.ns_timer = Some((next, timer));
    }

    fn on_timers(&mut self) {
        self.ns_timer = None;
        let now = clock::mono();
        for t in TIMERS {
            if self.timers[t as usize].is_some_and(|at| at <= now + 0.001) {
                self.timers[t as usize] = None;
                self.on_timer(t);
            }
        }
        self.sync_timer();
        self.request_frame();
    }

    fn on_timer(&mut self, t: Timer) {
        match t {
            Timer::Hover => {
                if !self.hovering || self.expanded {
                    self.hover_arm = None;
                } else if self.st.notice.as_ref().is_some_and(|n| n.inline.is_some()) {
                    // A notice with an inline pill stays put under a resting
                    // pointer so the pill can be clicked.
                } else {
                    self.expand();
                }
            }
            Timer::Collapse => {
                if !self.hovering && !self.st.pinned && !self.text_focus && !self.drag_hover && self.input.pressed_id.is_none() && !self.phone_holds() {
                    self.collapse();
                }
            }
            Timer::Wake => {}
            Timer::Save => self.save_now(),
            Timer::Minute => {
                self.schedule_minute_timer();
                self.close_idle_phone();
            }
            Timer::Smoke => self.smoke_step(),
            Timer::Stats => {
                if self.stats_on {
                    self.st.stats = services::sysstats::sample();
                    self.set_timer(Timer::Stats, 1000);
                }
            }
        }
    }

    fn schedule_minute_timer(&mut self) {
        let t = clock::local_now();
        let ms = (60 - t.second as u32).max(1) * 1000 + 20;
        self.set_timer(Timer::Minute, ms);
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
        if let Some(b) = self.st.pomodoro.next_second_boundary(now) {
            consider(now + b + 0.005);
        }
        if let poni_core::pomodoro::RunState::Running { ends_at } = self.st.pomodoro.run {
            consider(ends_at + 0.005);
        }
        if let Some(t) = activities::next_wake(&self.mode, &self.st) {
            consider(t);
        }
        if let Mode::Expanded(p) = self.mode {
            if let Some(t) = pages::next_wake(p, &self.st) {
                consider(t + 0.005);
            }
        }
        if let Some(t) = next {
            let ms = (((t - now) * 1000.0).ceil() as u32).clamp(10, 3_600_000);
            self.set_timer(Timer::Wake, ms);
        }
    }

    // ------------------------------------------------------------ settings and data

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

    fn save_now(&mut self) {
        if !self.store_writable {
            pwarn!("data.json couldn't be read at start; not overwriting it");
            return;
        }
        if let Err(e) = save_json_atomic(&paths::store_path(), &self.st.store) {
            pwarn!("saving data failed: {e}");
        }
    }

    /// Apply side effects of a settings change (from the menu, a reload or a page).
    fn settings_changed(&mut self, before: &Settings) {
        if before.theme != self.st.settings.theme {
            self.st.accent = theme_accent(self.st.settings.theme);
        }
        if before.clipboard_history && !self.st.settings.clipboard_history {
            // Privacy: turning history off forgets everything, pinned items included.
            self.st.clip = poni_core::cliphist::ClipHistory::default();
            self.st.ui.clip_flash = None;
            self.st.ui.clip_scroll = 0.0;
        }
        self.sync_clipboard();
        self.st.pomodoro.cfg = self.st.settings.pomodoro.clone();
        if before.expanded_width != self.st.settings.expanded_width {
            self.layout_surface();
        }
    }

    fn on_menu(&mut self, cmd: MenuCmd) {
        let before = self.st.settings.clone();
        let s = &mut self.st.settings;
        match cmd {
            MenuCmd::TogglePinned => {
                self.st.pinned = !self.st.pinned;
                if !self.st.pinned {
                    self.maybe_schedule_collapse();
                }
            }
            MenuCmd::SetTheme(t) => s.theme = t,
            MenuCmd::SetIdleStyle(style) => s.idle_style = style,
            MenuCmd::ToggleSounds => s.sounds = !s.sounds,
            MenuCmd::ToggleClipboard => s.clipboard_history = !s.clipboard_history,
            MenuCmd::ToggleLogin => self.toggle_login(),
            MenuCmd::OpenSettings => {
                if !paths::settings_path().exists() {
                    let _ = save_json_atomic(&paths::settings_path(), &self.st.settings);
                }
                services::open(&paths::settings_path().to_string_lossy());
            }
            MenuCmd::ReloadSettings => {
                // Never reset on a typo: keep the current settings and say what's wrong.
                let path = paths::settings_path();
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse_json::<Settings>(&t)) {
                    Ok(s2) => {
                        let s2 = s2.sanitized();
                        self.st.settings = s2.clone();
                        self.saved_settings = s2;
                        self.settings_writable = true;
                        self.layout_surface();
                    }
                    Err(e) => {
                        pwarn!("settings.json not reloaded: {e}");
                        self.st.notify(Icon::Sliders, palette::ORANGE, "Settings not reloaded", "settings.json has an error; see poni.log", 5.0, None);
                    }
                }
            }
            MenuCmd::OpenDataFolder => services::open(&paths::data_dir().to_string_lossy()),
            MenuCmd::About => services::open(ABOUT_URL),
            MenuCmd::Quit => {
                self.quit(0);
                return;
            }
        }
        self.settings_changed(&before);
        self.save_settings();
        self.refresh_menu();
        self.request_frame();
    }

    fn refresh_menu(&mut self) {
        let login = services::login::status();
        if let Some(m) = self.menubar.as_mut() {
            m.refresh(self.mtm, &self.st.settings, self.st.pinned, login);
        }
    }

    /// Start at login on or off. macOS may ask the person to allow it in
    /// System Settings > General > Login Items first.
    fn toggle_login(&mut self) {
        let result = match services::login::status() {
            Login::On => services::login::set_enabled(false),
            Login::NeedsApproval => {
                services::login::open_settings();
                Ok(())
            }
            _ => services::login::set_enabled(true),
        };
        match result {
            Err(e) => {
                pwarn!("start at login: {e}");
                self.st.notify(Icon::Sliders, palette::ORANGE, "Couldn't start at login", e, 6.0, None);
            }
            Ok(()) => {
                let now = services::login::status();
                pinfo!("start at login: {now:?}");
                if now == Login::NeedsApproval {
                    services::login::open_settings();
                    self.st.notify(Icon::Sliders, palette::ORANGE, "Allow Oyen Poni to start at login", "Turn it on in Login Items", 8.0, None);
                }
            }
        }
    }

    /// Clipboard history runs only while it is switched on.
    fn sync_clipboard(&mut self) {
        match (self.st.settings.clipboard_history, self.clipboard.is_some()) {
            (true, false) => self.clipboard = Some(services::clipboard::Watcher::start(self.sink.clone())),
            (false, true) => {
                if let Some(w) = self.clipboard.take() {
                    w.stop();
                }
            }
            _ => {}
        }
    }

    fn set_stats(&mut self, on: bool) {
        if on == self.stats_on {
            return;
        }
        self.stats_on = on;
        if on {
            self.st.stats = services::sysstats::sample();
            self.set_timer(Timer::Stats, 1000);
        } else {
            self.kill_timer(Timer::Stats);
        }
    }

    pub fn on_event(&mut self, ev: AppEvent) {
        self.st.now = clock::mono();
        match ev {
            AppEvent::Volume { level, muted } => {
                // The first report only sets the level; changes show the pop-up.
                let first = !self.st.volume.known;
                self.st.volume = VolumeState { level, muted, known: true };
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
                self.st.shelf_thumbs.insert(path, image);
            }
            AppEvent::Stats(s) => self.st.stats = s,
            AppEvent::ClipboardText(t) => {
                if self.own_clip.as_deref() == Some(t.as_str()) {
                    self.own_clip = None;
                } else {
                    self.st.clip.push(&t, self.st.unix);
                }
            }
            AppEvent::Phone(e) => self.on_phone(e),
            AppEvent::Log(s) => pinfo!("{s}"),
            // Music, Bluetooth and full-screen events come with later slices.
            _ => {}
        }
        self.request_frame();
    }

    fn refresh_screen(&mut self) {
        let Some(s) = window::pick_screen(self.mtm) else { return };
        if s != self.screen {
            pinfo!("screen {}x{} pt at {}x, notch {:?}", s.w, s.h, s.scale, s.notch);
            self.screen = s;
            self.gfx.set_scale(s.scale);
            self.layout_surface();
        }
        self.request_frame();
    }

    // ------------------------------------------------------------ geometry

    /// Size and place the window once for the largest island (the widest,
    /// tallest expanded page plus spring overshoot). The island is drawn
    /// centred in it, so a morph never moves or resizes the window.
    fn layout_surface(&mut self) {
        let tallest = PageKind::ALL.iter().map(|p| pages::content_height(*p, &self.st)).fold(0.0f32, f32::max);
        self.layout_surface_for(self.st.settings.expanded_width, pages::HEADER_H + tallest + pages::PAD);
    }

    fn layout_surface_for(&mut self, body_w: f32, body_h: f32) {
        let notch_h = self.screen.notch.map_or(0.0, |n| n.h);
        let mut w = (body_w * OVERSHOOT + 2.0 * (SHOULDER + HIT_MARGIN)).ceil();
        if w as i64 % 2 != 0 {
            w += 1.0;
        }
        let h = ((body_h + notch_h) * OVERSHOOT + HIT_MARGIN).ceil();
        if (w, h) != self.surface {
            self.surface = (w, h);
            self.cx = w / 2.0;
            self.prev_canvas = None;
            window::place(&self.screen, w as f64, h as f64);
        }
    }

    /// The island no longer fits the window: grow it.
    fn ensure_room(&mut self) {
        let g = self.island.current();
        let t = self.island.target();
        let (bw, bh) = ((t.w * OVERSHOOT).max(g.w), (t.h * OVERSHOOT).max(g.h));
        let need_w = bw + 2.0 * (SHOULDER + HIT_MARGIN);
        let need_h = bh + HIT_MARGIN;
        if need_w > self.surface.0 || need_h > self.surface.1 {
            let notch_h = self.screen.notch.map_or(0.0, |n| n.h);
            let tallest = PageKind::ALL.iter().map(|p| pages::content_height(*p, &self.st)).fold(0.0f32, f32::max);
            let body_h = (pages::HEADER_H + tallest + pages::PAD).max((bh - notch_h) / OVERSHOOT * 1.1);
            pinfo!("island outgrew the window; growing it to fit {bw:.0}x{bh:.0} DIP");
            self.layout_surface_for(self.st.settings.expanded_width.max(bw / OVERSHOOT * 1.1), body_h);
        }
    }

    fn target_mode(&self) -> Mode {
        if self.expanded {
            return Mode::Expanded(self.st.page);
        }
        activities::collapsed_mode(&self.st)
    }

    /// Island geometry for `m` (around the notch when the screen has one).
    /// The resting pill lifts a little under the pointer: instant feedback
    /// while the hover dwell runs.
    fn geom_for(&self, m: &Mode) -> Geom {
        let base = activities::geom(m, &self.st);
        let g = match self.screen.notch {
            Some(n) => notch::geom(m, &self.st, n, base),
            None => base,
        };
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

    fn hit_at(&self, p: Point) -> Option<Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(p)).copied()
    }

    /// Whether `p` is over the part of the window that takes the mouse: the
    /// whole canvas while expanded (a forgiving edge), only the island collapsed.
    fn hovering_inside(&self, p: Point) -> bool {
        if !self.expanded {
            return self.island.body_rect_at(self.cx).contains(p);
        }
        self.island.canvas_rect_at(self.cx).contains(p)
    }

    // ------------------------------------------------------------ input

    fn on_pointer(&mut self, p: Point) {
        let inside = self.hovering_inside(p) || self.input.down;
        if !inside {
            if self.hovering {
                self.leave();
            }
            window::set_ignoring(true);
            return;
        }
        window::set_ignoring(false);
        self.input.mouse = Some(p);
        self.update_cursor(p);
        if !self.hovering {
            self.hovering = true;
            self.kill_timer(Timer::Collapse);
            if !self.expanded {
                self.arm_hover(p);
            }
        } else if let Some(p0) = self.hover_arm.filter(|_| !self.expanded) {
            // Dwell: moving on restarts the wait, so brushing past on the way
            // to the menu bar doesn't open the island.
            if (p.x - p0.x).hypot(p.y - p0.y) > DWELL_SLOP {
                self.arm_hover(p);
            }
        }
        self.request_frame();
    }

    fn leave(&mut self) {
        self.hovering = false;
        self.input.mouse = None;
        self.cancel_hover();
        self.maybe_schedule_collapse();
        if self.cursor != CursorKind::Arrow {
            self.cursor = CursorKind::Arrow;
            NSCursor::arrowCursor().set();
        }
        self.request_frame();
    }

    fn update_cursor(&mut self, p: Point) {
        let kind = self.hit_at(p).map(|h| h.cursor).unwrap_or(CursorKind::Arrow);
        if kind != self.cursor {
            self.cursor = kind;
            match kind {
                CursorKind::Arrow => NSCursor::arrowCursor().set(),
                CursorKind::Hand => NSCursor::pointingHandCursor().set(),
                CursorKind::Text => NSCursor::IBeamCursor().set(),
            }
        }
    }

    /// Returns the menu to pop up when a right-click missed every widget.
    fn on_view_input(&mut self, input: ViewInput) -> Option<Retained<NSMenu>> {
        match input {
            ViewInput::Down { x, y, double } => {
                let p = Point::new(x, y);
                if !self.hovering_inside(p) {
                    // A click on the window's clear part: not ours.
                    window::set_ignoring(true);
                    return None;
                }
                self.dbl_press = double;
                self.input.mouse = Some(p);
                self.input.down = true;
                self.input.press_pos = Some(p);
                self.input.pressed_id = self.hit_at(p).map(|h| h.id);
                if !self.expanded {
                    let inline = self.st.notice.as_ref().and_then(|n| n.inline).filter(|_| self.input.pressed_id == Some(wid("notice.act")));
                    if let Some(a) = inline {
                        // The notice's inline pill acts in place without expanding.
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
            }
            ViewInput::Up { x, y } => {
                let p = Point::new(x, y);
                self.input.mouse = Some(p);
                let hit = self.hit_at(p).map(|h| h.id);
                if let (Some(pid), Some(h)) = (self.input.pressed_id, hit) {
                    // The second press of a double-click doesn't click again.
                    if pid == h && !self.dbl_press {
                        self.input.clicked = Some((h, MouseButton::Left));
                    }
                }
                self.dbl_press = false;
                self.input.down = false;
                self.input.pressed_id = None;
                self.pending_drag = None;
                if !self.hovering_inside(p) {
                    self.input.mouse = None;
                    self.leave();
                    window::set_ignoring(true);
                } else if !self.hovering {
                    self.maybe_schedule_collapse();
                }
                self.request_frame();
            }
            ViewInput::RightUp { x, y } => {
                let p = Point::new(x, y);
                match self.hit_at(p) {
                    Some(h) => self.input.clicked = Some((h.id, MouseButton::Right)),
                    None => return self.menubar.as_ref().map(|m| m.menu.clone()),
                }
                self.request_frame();
            }
            ViewInput::Wheel { notches } => {
                self.input.wheel += notches;
                self.request_frame();
            }
            ViewInput::Key { code, text, command } => {
                if !self.text_focus {
                    return None;
                }
                let k = match code {
                    51 => Some(KeyInput::Backspace),
                    117 => Some(KeyInput::Delete),
                    36 | 76 => Some(KeyInput::Enter),
                    53 => Some(KeyInput::Escape),
                    123 => Some(KeyInput::Left),
                    124 => Some(KeyInput::Right),
                    115 => Some(KeyInput::Home),
                    119 => Some(KeyInput::End),
                    _ => None,
                };
                if let Some(k) = k {
                    self.input.keys.push(k);
                } else if command {
                    match text.as_str() {
                        "v" => {
                            if let Some(t) = services::clipboard::read_text() {
                                self.input.keys.extend(t.chars().filter(|c| !c.is_control()).map(KeyInput::Char));
                            }
                        }
                        "a" => self.input.keys.push(KeyInput::SelectAll),
                        _ => {}
                    }
                } else {
                    // Function keys arrive as private-use characters.
                    self.input.keys.extend(text.chars().filter(|c| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(c)).map(KeyInput::Char));
                }
                self.request_frame();
            }
        }
        None
    }

    fn on_resign_key(&mut self) {
        // The person clicked elsewhere: the text field let go of the keyboard.
        self.pending.push(Deferred::ReleaseKeyboard);
        if self.text_focus {
            self.text_focus = false;
            self.input.focused = None;
            self.maybe_schedule_collapse();
            self.request_frame();
        }
    }

    // ------------------------------------------------------------ island state

    fn expand(&mut self) {
        if !self.expanded {
            self.expanded = true;
            self.cancel_hover();
        }
        self.request_frame();
    }

    /// (Re)start the hover dwell from `p`.
    fn arm_hover(&mut self, p: Point) {
        self.hover_arm = Some(p);
        self.set_timer(Timer::Hover, self.st.settings.hover_delay_ms.max(1));
    }

    fn cancel_hover(&mut self) {
        self.hover_arm = None;
        self.kill_timer(Timer::Hover);
    }

    fn collapse(&mut self) {
        if self.expanded {
            self.expanded = false;
            if self.text_focus {
                self.blur_text();
            }
            self.set_stats(false);
            self.st.ui.clip_flash = None;
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

    fn set_page(&mut self, p: PageKind) {
        if p != self.st.page {
            self.st.page = p;
            // Settings is never where the island opens next time.
            if p != PageKind::Settings {
                self.st.store.last_page = p.key().to_string();
                self.set_timer(Timer::Save, 800);
            } else {
                self.st.ui.autostart = Some(services::login::status() == Login::On);
            }
            if self.text_focus {
                self.blur_text();
            }
        }
    }

    fn maybe_schedule_collapse(&mut self) {
        if self.expanded && !self.hovering && !self.st.pinned && !self.text_focus && !self.drag_hover && !self.phone_holds() {
            self.set_timer(Timer::Collapse, self.st.settings.collapse_delay_ms.max(50));
        }
    }

    fn focus_text(&mut self) {
        if self.text_focus {
            return;
        }
        self.text_focus = true;
        self.pending.push(Deferred::TakeKeyboard);
    }

    fn blur_text(&mut self) {
        self.input.focused = None;
        if !self.text_focus {
            return;
        }
        self.text_focus = false;
        self.pending.push(Deferred::ReleaseKeyboard);
        self.maybe_schedule_collapse();
    }

    /// Run a notice's inline action in place (the island stays collapsed),
    /// then show a check mark in the pill briefly before the notice leaves.
    fn run_inline_action(&mut self, a: InlineAction) {
        self.refresh_time();
        let st = &mut self.st;
        if st.notice.as_ref().is_none_or(|n| n.icon == Icon::Check) {
            return;
        }
        if a == InlineAction::ShowAmount {
            // The sum and sender take the subtitle's place for a few seconds.
            let until = st.now + poni_core::oyenza::SHOWN_S;
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

    fn run_commands(&mut self) {
        let cmds = std::mem::take(&mut self.st.cmds);
        for c in cmds {
            match c {
                // Music controls arrive with the media slice.
                Command::Media(_) => {}
                Command::OpenPath(p) => services::open(&p),
                Command::RevealPath(p) => services::reveal(&p),
                // Starts with the next drag event (AppKit needs one to start from).
                Command::DragOut(paths) => self.pending_drag = Some(paths),
                Command::LoadThumb(p) => {
                    if !self.st.shelf_thumbs.contains_key(&p) {
                        self.st.shelf_thumbs.insert(p.clone(), None);
                        let px = (crate::services::imaging::THUMB_DIP * self.gfx.scale()).round() as u32;
                        services::thumbs::load_async(p, px, self.sink.clone());
                    }
                }
                Command::CopyText(t) => {
                    self.own_clip = Some(t.clone());
                    services::clipboard::set_text(&t);
                }
                Command::FocusText => self.focus_text(),
                Command::BlurText => self.blur_text(),
                Command::SaveSettings => {
                    let before = self.saved_settings.clone();
                    self.settings_changed(&before);
                    self.save_settings();
                }
                Command::SaveStore => self.set_timer(Timer::Save, 800),
                Command::SetPage(p) => self.set_page(p),
                Command::OpenSystem(m) => {
                    self.st.ui.system_module = m;
                    self.set_page(PageKind::System);
                }
                // App icons for the System page come with the stats slice.
                Command::LoadAppIcon(p) => {
                    self.st.app_icons.entry(p).or_insert(None);
                }
                Command::SetPinned(b) => {
                    self.st.pinned = b;
                    if !b {
                        self.maybe_schedule_collapse();
                    }
                }
                Command::Collapse => self.collapse(),
                Command::Play(s) => services::play(s == Sound::Chime),
                Command::OpenUrl(u) => services::open(&u),
                Command::WantStats(b) => self.set_stats(b),
                Command::SetAutostart(on) => {
                    if (services::login::status() == Login::On) != on {
                        self.toggle_login();
                    }
                    self.st.ui.autostart = Some(services::login::status() == Login::On);
                    self.refresh_menu();
                }
                Command::Phone(c) => self.phone_cmd(c),
                // Oyenza sign-in comes to the Mac in a later slice.
                Command::Oyenza(_) => {}
            }
        }
        self.sync_phone_offer();
    }

    // ------------------------------------------------------------ phone drop

    /// The QR code waits for a phone, or a phone waits for an answer.
    fn phone_holds(&self) -> bool {
        self.st.phone.as_ref().is_some_and(|p| p.holds_island())
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
        self.st.phone = Some(pd);
        self.st.page = PageKind::Shelf;
        self.expand();
    }

    fn close_phone(&mut self) {
        if let Some(svc) = self.phone.take() {
            svc.stop();
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

    // ------------------------------------------------------------ frames

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
    }

    /// Step the island to the frame the display shows next. Returns the part
    /// of the view to redraw.
    fn begin_frame(&mut self, timestamp: f64, target: f64) -> Option<Rect> {
        // Stamp the frame with the refresh it will be shown on, so every
        // animation step is a whole number of refresh periods.
        let period = (target - timestamp).clamp(1.0 / 240.0, 1.0 / 24.0);
        let mono = clock::mono();
        let anim_t = (mono + period).max(self.last_frame_t);
        let resumed = self.last_frame_t == 0.0 || anim_t - self.last_frame_t > 0.25;
        let mut dt = if resumed { period } else { anim_t - self.last_frame_t };
        self.last_frame_t = anim_t;
        self.refresh_time();
        self.st.now = self.st.now.max(anim_t);
        self.tick();
        let now = self.st.now;

        let target_mode = self.target_mode();
        let mut retarget = false;
        if target_mode != self.mode {
            let new_content = activities::content_key(&target_mode) != activities::content_key(&self.mode);
            if new_content {
                self.outgoing = Some((self.mode, now, self.island.target()));
            }
            if let (Mode::Expanded(a), Mode::Expanded(b)) = (self.mode, target_mode) {
                if a != b {
                    let dir = if b.tab_index() > a.tab_index() { 1.0 } else { -1.0 };
                    self.page_anim = Some((a, now, dir));
                }
            }
            self.motion_name = motion_name(&self.mode, &target_mode);
            let g = self.geom_for(&target_mode);
            self.island.set_target(g, now, new_content);
            self.mode = target_mode;
            retarget = true;
        } else {
            let g = self.geom_for(&self.mode);
            if g != self.island.target() {
                if self.island.settled() {
                    self.motion_name = "resize";
                }
                self.island.set_target(g, now, false);
                retarget = true;
            }
        }
        if retarget {
            dt = dt.min(period);
        }
        let dt = dt.clamp(0.0, 0.05) as f32;
        self.set_stats(self.mode == Mode::Expanded(PageKind::Today));
        self.island.step(dt, now);
        self.ensure_room();
        self.note_motion(retarget, mono, period, dt as f64);

        let canvas_r = self.island.canvas_rect_at(self.cx).inset(-2.0, -2.0);
        let full = Rect::new(0.0, 0.0, self.surface.0, self.surface.1);
        let dirty = union_rect(self.prev_canvas.unwrap_or(full), canvas_r);
        self.prev_canvas = Some(canvas_r);
        Some(dirty)
    }

    /// Draw the island (called from the view's drawRect).
    fn draw(&mut self, ctx: &CGContext, dirty: Rect) {
        CGContext::clear_rect(
            Some(ctx),
            CGRect { origin: CGPoint { x: dirty.x as f64, y: dirty.y as f64 }, size: CGSize { width: dirty.w as f64, height: dirty.h as f64 } },
        );
        let now = self.st.now;
        let body = self.island.body_rect_at(self.cx);
        let geom = self.island.current();
        let content_alpha = self.island.content_alpha();
        let morphing = !self.island.settled() || self.outgoing.is_some();
        let mode = self.mode;
        let notch = self.screen.notch;
        let transition = self.page_anim.and_then(|(from, t0, dir)| {
            let t = ((now - t0) / PAGE_SWITCH) as f32;
            (t < 1.0).then_some(pages::PageTransition { from, t, dir })
        });
        if transition.is_none() {
            self.page_anim = None;
        }
        // Pages lay out at their final size, centred on the morphing outline,
        // so the header and the page body don't slide with the edges.
        let lay = match mode {
            Mode::Expanded(_) => {
                let tgt = self.island.target();
                Rect::new(body.center().x - tgt.w / 2.0, body.y, tgt.w, tgt.h)
            }
            _ => body,
        };
        let shoulder = notch.map_or(SHOULDER, |n| notch::shoulder(n, geom.w));
        let hidden = mode == Mode::Idle && (self.st.settings.idle_style == IdleStyle::Hidden || notch.is_some_and(|_| notch::fit(&mode, &self.st) == Fit::Inside));
        self.gfx.set_look(poni_core::look::of(self.st.settings.theme));
        let mut hits = std::mem::take(&mut self.hits);
        hits.clear();
        let (repaint, anim, focus, live) = {
            let mut canvas = self.gfx.canvas(ctx);
            let border = if hidden { None } else { Some(canvas.look().border) };
            canvas.notch(body, geom.r, shoulder, palette::ISLAND, border);
            let pivot = Point::new(body.center().x, body.y);
            if morphing {
                canvas.clip(body);
                canvas.push_shape_mask(body, geom.r, shoulder);
            }
            // Outgoing content: at its own (old) size, shrinking with the
            // outline, fading out on a smoothstep. Ghost input: no clicks.
            if let Some((m, t0, g)) = self.outgoing {
                let t = ((now - t0) / OUTGOING_FADE) as f32;
                if t >= 1.0 {
                    self.outgoing = None;
                } else {
                    let old_body = Rect::new(body.center().x - g.w / 2.0, body.y, g.w, g.h);
                    let alpha = 1.0 - t * t * (3.0 - 2.0 * t);
                    let xf = scale_about(pivot, (geom.w / g.w.max(1.0)).clamp(GHOST_MIN_SCALE, 1.0));
                    let st = &mut self.st;
                    canvas.layered(old_body, alpha, xf, |c| {
                        let mut ghost_input = Input::default();
                        let mut ghost_hits = Vec::new();
                        draw_content(c, &mut ghost_input, &mut ghost_hits, st, m, old_body, old_body, None, now, notch);
                    });
                }
            }
            // Incoming content settles down from the bezel as it fades in.
            let out = if content_alpha < 1.0 {
                let grow = scale_about(pivot, CONTENT_SCALE_FROM + (1.0 - CONTENT_SCALE_FROM) * content_alpha);
                let xf = grow * Matrix3x2::translation(0.0, -CONTENT_DROP * (1.0 - content_alpha));
                let (input, st) = (&mut self.input, &mut self.st);
                canvas.layered(union_rect(lay, body), content_alpha, xf, |c| draw_content(c, input, &mut hits, st, mode, lay, body, transition, now, notch))
            } else {
                draw_content(&mut canvas, &mut self.input, &mut hits, &mut self.st, mode, lay, body, transition, now, notch)
            };
            if morphing {
                canvas.pop_shape_mask();
                canvas.unclip();
            }
            out
        };
        self.hits = hits;
        self.frames_drawn += 1;

        // Unconsumed wheel over the header switches pages, one per whole notch.
        let wheel_left = self.input.wheel;
        if let Mode::Expanded(p) = self.mode {
            let top = body.y + notch.map_or(0.0, |n| n.h);
            let over_header = self.input.mouse.is_some_and(|m| m.y < top + pages::HEADER_H);
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
        self.drawn = Drawn { repaint, anim, focus, live };
    }

    fn end_frame(&mut self) {
        let d = std::mem::take(&mut self.drawn);
        match d.focus {
            Some(true) => self.focus_text(),
            Some(false) => self.blur_text(),
            None => {}
        }
        self.run_commands();
        let moving = !self.island.settled() || d.anim || d.repaint || !self.st.cmds.is_empty() || self.outgoing.is_some() || self.page_anim.is_some();
        let live = activities::is_live(&self.mode, &self.st) || d.live;
        window::set_frames(moving || live);
        self.schedule_wake();
    }

    /// Log how smoothly the first morphs ran (frames, hitches), as on Windows.
    fn note_motion(&mut self, retarget: bool, t: f64, period: f64, dt: f64) {
        let moving = !self.island.settled();
        if retarget || !moving {
            if let Some(mut m) = self.motion.take() {
                if !moving {
                    m.frame(t);
                }
                if m.frames >= 4 && self.motions_logged < 20 {
                    pinfo!("{} (core graphics)", m.summary());
                    self.motions_logged += 1;
                }
            }
        }
        if moving {
            match self.motion.as_mut() {
                Some(m) => m.frame(t),
                None => self.motion = Some(MotionStats::begin(self.motion_name, t, period, dt)),
            }
        }
    }

    // ------------------------------------------------------------ smoke test (CI)

    /// `--smoke <dir>`: walk the real window through a few states. At each
    /// one, once the island rests, write `ready-<n>-<name>.txt` (the window's
    /// screen rectangle) and wait for CI's screenshot (`shot-<n>-<name>.done`).
    fn smoke_step(&mut self) {
        let Some(mut sm) = self.smoke.take() else { return };
        let settled = self.island.settled() && self.target_mode() == self.mode && self.outgoing.is_none() && self.page_anim.is_none();
        let now = clock::mono();
        match sm.phase {
            SmokePhase::Apply => {
                if sm.step >= SMOKE_STEPS.len() {
                    // Register as a login item and back out again, and report what
                    // macOS said (an ad-hoc signed build may not be allowed).
                    let before = services::login::status();
                    let on = services::login::set_enabled(true);
                    let during = services::login::status();
                    let off = services::login::set_enabled(false);
                    sm.note(
                        "start-at-login",
                        before != Login::Unavailable,
                        format!("before {before:?} ({}), register {on:?}, then {during:?}, unregister {off:?}", services::login::raw_status()),
                    );
                    sm.finish(self.frames_drawn);
                    let code = if sm.failed { 1 } else { 0 };
                    self.smoke = Some(sm);
                    self.quit(code);
                    return;
                }
                self.smoke_apply(SMOKE_STEPS[sm.step]);
                sm.phase = SmokePhase::Settle(now);
            }
            SmokePhase::Settle(t0) => {
                let name = SMOKE_STEPS[sm.step];
                let ready = self.smoke_ready(name, now - t0);
                if (settled && ready) || now - t0 > 6.0 {
                    if !settled {
                        sm.note(name, false, "the island did not settle within 6 s".into());
                    } else {
                        let g = self.island.current();
                        sm.note(name, true, format!("{:?}, {:.0}x{:.0} DIP, {} frames drawn", self.mode, g.w, g.h, self.frames_drawn));
                    }
                    if let Some((check, ok, detail)) = self.smoke_check(name) {
                        sm.note(check, ok, detail);
                    }
                    let rect = window::frame_top_left();
                    sm.write_ready(name, rect);
                    sm.phase = SmokePhase::Wait(now);
                }
            }
            SmokePhase::Wait(t0) => {
                let name = SMOKE_STEPS[sm.step];
                if sm.shot_done(name) || now - t0 > 8.0 {
                    sm.step += 1;
                    sm.phase = SmokePhase::Apply;
                }
            }
        }
        self.smoke = Some(sm);
        self.set_timer(Timer::Smoke, 100);
    }

    fn smoke_apply(&mut self, name: &str) {
        pinfo!("smoke: {name}");
        let centre = Point::new(self.cx, 12.0);
        match name {
            "idle" => {}
            "hover" => {
                // Just the lift: no dwell, so it doesn't open before the screenshot.
                self.on_pointer(centre);
                self.cancel_hover();
            }
            "open-focus" => {
                self.st.page = PageKind::Focus;
                self.expand();
            }
            "open-today" => self.st.cmds.push(Command::SetPage(PageKind::Today)),
            "timer" => {
                self.leave();
                self.collapse();
                self.st.pomodoro.start(self.st.now);
            }
            "notice" => {
                self.st.notify(Icon::CheckCircle, self.st.accent, "Oyen Poni is on", "Point at the island to open it", 30.0, None);
            }
            "volume-hud" => {
                self.st.notice = None;
                if !self.st.volume.known {
                    self.on_event(AppEvent::Volume { level: 0.5, muted: false });
                }
                self.on_event(AppEvent::Volume { level: 0.64, muted: false });
            }
            "charging-hud" => {
                self.st.hud = None;
                self.st.power = crate::state::PowerState { has_battery: true, percent: 76, charging: false, on_ac: false, saver: false };
                self.on_event(AppEvent::Power(crate::state::PowerState { has_battery: true, percent: 76, charging: true, on_ac: true, saver: false }));
            }
            // CI copies SMOKE_CLIP with pbcopy when this step is ready.
            "clipboard-copy" => self.st.hud = None,
            "clipboard-page" => {
                self.st.page = PageKind::Clipboard;
                self.expand();
            }
            "shelf-drop" => {
                let paths = smoke_shelf_files();
                self.on_event(AppEvent::ShelfAdd(paths));
            }
            "menu" => {
                self.collapse();
                self.pending.push(Deferred::OpenMenu);
            }
            _ => {}
        }
        self.request_frame();
    }

    /// A step's own condition to wait for before its picture (data from a
    /// service arriving), on top of the island resting.
    fn smoke_ready(&self, name: &str, waited: f64) -> bool {
        match name {
            // The second stats sample has a real CPU load.
            "open-today" => self.st.stats.valid && waited > 1.3,
            "clipboard-page" => self.st.clip.iter().any(|c| c.text.contains(SMOKE_CLIP)) || waited > 3.0,
            "shelf-drop" => self.st.shelf_thumbs.values().filter(|t| t.is_some()).count() >= 3 || waited > 5.0,
            "menu" => waited > 1.0,
            _ => true,
        }
    }

    /// What each new part of the app did on a real Mac.
    fn smoke_check(&self, name: &str) -> Option<(&'static str, bool, String)> {
        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
        match name {
            "open-today" => {
                let s = &self.st.stats;
                let ok = s.valid && s.mem_total > 0 && s.disk_total > 0 && s.uptime_s > 0;
                let d = format!(
                    "CPU {:.0} %, memory {:.1} of {:.1} GiB, disk {:.0} GiB free of {:.0}, up {} min",
                    s.cpu * 100.0,
                    gib(s.mem_used),
                    gib(s.mem_total),
                    gib(s.disk_free),
                    gib(s.disk_total),
                    s.uptime_s / 60
                );
                Some(("system-stats", ok, d))
            }
            "volume-hud" => {
                let ok = matches!(self.st.hud.as_ref().map(|h| &h.kind), Some(HudKind::Volume { .. }));
                Some(("volume-hud", ok, format!("output device: {:?}", services::audio::current())))
            }
            "charging-hud" => {
                let ok = matches!(self.st.hud.as_ref().map(|h| &h.kind), Some(HudKind::Charging { .. }));
                Some(("charging-hud", ok, format!("this Mac's power: {:?}", services::power::query())))
            }
            "clipboard-page" => {
                let ok = self.st.clip.iter().any(|c| c.text.contains(SMOKE_CLIP));
                Some(("clipboard-history", ok, format!("{} item(s) recorded", self.st.clip.iter().count())))
            }
            "shelf-drop" => {
                let made = self.st.shelf_thumbs.values().filter(|t| t.is_some()).count();
                Some(("shelf-thumbnails", made >= 3, format!("{made} of {} tiles drawn", self.st.shelf_thumbs.len())))
            }
            _ => None,
        }
    }
}

/// Oyen's orange in his theme, the Mac's accent colour otherwise.
fn theme_accent(theme: Theme) -> Color {
    if oyen::on(theme) {
        oyen::ORANGE
    } else {
        services::system_accent().unwrap_or(palette::BLUE)
    }
}

/// Three things to drop on the Shelf in the smoke test: a picture, a text
/// file and a folder (made-up content).
fn smoke_shelf_files() -> Vec<String> {
    let base = paths::data_dir().join("shelf-demo");
    let folder = base.join("Project assets");
    let _ = std::fs::create_dir_all(&folder);
    let _ = std::fs::write(folder.join("readme.txt"), "Drafts and exports for the harbor poster.\n");
    let notes = base.join("Trip notes.txt");
    let _ = std::fs::write(&notes, "Weekend trip\n\n- ferry leaves at 9:40\n- pack the blue jacket\n");
    let sketch = base.join("Harbor sketch.png");
    let _ = crate::png::save_pbgra(&sketch, 240, 240, &crate::render::make_art(240));
    [sketch, notes, folder].iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

fn files_label(n: usize) -> String {
    format!("{n} {}", if n == 1 { "file" } else { "files" })
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

/// The island's current content: a page (expanded) or the collapsed
/// activity, fitted around the notch when there is one. Returns (repaint,
/// anim, focus, live) as requested by the drawing code.
#[allow(clippy::too_many_arguments)]
pub fn draw_content(
    c: &mut Canvas,
    input: &mut Input,
    hits: &mut Vec<Hit>,
    st: &mut AppState,
    mode: Mode,
    lay: Rect,
    body: Rect,
    transition: Option<pages::PageTransition>,
    now: f64,
    notch: Option<Notch>,
) -> (bool, bool, Option<bool>, bool) {
    let mut ui = Ui::new(c, input, hits, now);
    match notch {
        None => match mode {
            Mode::Expanded(page) => pages::draw_expanded(&mut ui, st, lay, page, transition),
            _ => activities::draw(&mut ui, st, &mode, body),
        },
        Some(n) => match mode {
            Mode::Expanded(page) => {
                let lay = notch::content_body(&mode, st, n, lay);
                pages::draw_expanded(&mut ui, st, lay, page, transition)
            }
            _ => match notch::fit(&mode, st) {
                Fit::Inside => {}
                Fit::Ears => {
                    if !notch::draw_ears(&mut ui, st, &mode, body, n) {
                        activities::draw(&mut ui, st, &mode, body);
                    }
                }
                Fit::Below => {
                    let b = notch::content_body(&mode, st, n, body);
                    activities::draw(&mut ui, st, &mode, b);
                }
            },
        },
    }
    (ui.repaint_requested(), ui.anim_requested(), ui.focus_request(), ui.live_requested())
}

// ---------------------------------------------------------------- smoke test bookkeeping

const SMOKE_STEPS: [&str; 12] =
    ["idle", "hover", "open-focus", "open-today", "timer", "notice", "volume-hud", "charging-hud", "clipboard-copy", "clipboard-page", "shelf-drop", "menu"];
/// What CI copies with `pbcopy` during the clipboard step.
const SMOKE_CLIP: &str = "Oyen Poni smoke test";

#[derive(Clone, Copy, Debug)]
enum SmokePhase {
    Apply,
    Settle(f64),
    Wait(f64),
}

struct Smoke {
    dir: PathBuf,
    step: usize,
    phase: SmokePhase,
    checks: Vec<(String, bool, String)>,
    failed: bool,
}

impl Smoke {
    fn new(dir: PathBuf) -> Smoke {
        let _ = std::fs::create_dir_all(&dir);
        Smoke { dir, step: 0, phase: SmokePhase::Apply, checks: Vec::new(), failed: false }
    }

    fn file(&self, kind: &str, name: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{kind}-{:02}-{name}.{ext}", self.step + 1))
    }

    fn note(&mut self, name: &str, ok: bool, detail: String) {
        pinfo!("smoke: {name}: {} ({detail})", if ok { "ok" } else { "FAILED" });
        self.failed |= !ok;
        self.checks.push((name.to_string(), ok, detail));
    }

    fn write_ready(&self, name: &str, r: Option<(f64, f64, f64, f64)>) {
        let line = match r {
            Some((x, y, w, h)) => format!("{x:.0},{y:.0},{w:.0},{h:.0}\n"),
            None => "0,0,0,0\n".into(),
        };
        let _ = std::fs::write(self.file("ready", name, "txt"), line);
    }

    fn shot_done(&self, name: &str) -> bool {
        self.file("shot", name, "done").exists()
    }

    fn finish(&mut self, frames: u64) {
        let ok = frames > 10;
        self.note("frames", ok, format!("{frames} frames drawn"));
        let mut out = String::from("{\n  \"checks\": [\n");
        for (i, (name, ok, detail)) in self.checks.iter().enumerate() {
            let comma = if i + 1 < self.checks.len() { "," } else { "" };
            out.push_str(&format!("    {{\"name\": {name:?}, \"ok\": {ok}, \"detail\": {detail:?}}}{comma}\n"));
        }
        out.push_str(&format!("  ],\n  \"ok\": {}\n}}\n", !self.failed));
        let _ = std::fs::write(self.dir.join("report.json"), out);
    }
}
