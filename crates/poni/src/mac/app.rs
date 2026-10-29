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
use crate::commands::{Command, MediaCmd, OyenzaCmd, PhoneCmd, Sound, UsageCmd};
use crate::events::{AppEvent, EventSink, OyenzaEvent};
use crate::gfx::{scale_about, union_rect, Canvas, Gfx, Matrix3x2};
use crate::glass;
use crate::icons::Icon;
use crate::island::{Geom, Island, HIT_MARGIN, SHOULDER};
use crate::menubar::{Login, MenuBar, MenuCmd};
use crate::notch::{self, Fit, Notch};
use crate::oyen;
use crate::pages::{self, PageKind};
use crate::paths;
use crate::pill;
use crate::services;
use crate::settings_host;
use crate::settings_window::{self, Section};
use crate::events::{AppleMusicStatus, LyricsResult};
use crate::state::{
    AppState, HudKind, InlineAction, Locate, LyricsState, LyricsStatus, Notice, NoticeAction, OyenzaLink, PhoneAsk, PhoneDrop, PhoneTransfer, PrayerState,
    VolumeState,
};
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
use poni_core::addons::{Status as AddonStatus, DOWNLOADER, HEALTH, JAPANESE_READINGS, PRAYER_TIMES, SYSTEM};
use poni_core::frametime::MotionStats;
use poni_core::phonedrop::{Event as PhoneEvent, QrMatrix};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::look::{Look, Material};
use poni_core::oyenza;
use poni_core::prayer::{Due, PlaceList, Prayer, PrayerMoment};
use poni_core::settings::{IdleStyle, PrayerSound, Settings, Theme};
use poni_core::sysmon::{CoreKind, Module};
use poni_core::store::{load_json, parse_json, save_json_atomic, LoadOutcome, Store};
use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver};

/// Steps for copying the Apple Music token (README).
const APPLE_MUSIC_HELP_URL: &str = "https://github.com/lucacadalora/oyenza-studio#apple-music-lyrics";

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
/// The hidden resting island's height, and how much taller the floating
/// one is when it has faded all the way in.
const HIDDEN_SLIVER: f32 = 5.0;
const HIDDEN_FADE: f32 = 24.0;
/// Record pressed: how long the Record page says "Opening the recorder"
/// without word from it (a cold start of the recorder takes a few seconds).
/// The recorder's `show` in that time is the launch Record caused: the
/// island takes it without opening, so it doesn't sit over the recorder's
/// window or area picker.
const REC_OPENING_S: f64 = 15.0;
/// How long the Record page shows why a recording didn't start.
const REC_ERROR_S: f64 = 8.0;
/// While a recording is under way, how often the island asks macOS whether
/// the recorder is still running (at most; only on frames that come anyway).
const REC_ALIVE_S: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timer {
    Hover,
    Collapse,
    Wake,
    Save,
    Minute,
    Smoke,
    Stats,
    /// The next read of the update file (`crate::updates`).
    Update,
}
const TIMERS: [Timer; 8] = [Timer::Hover, Timer::Collapse, Timer::Wake, Timer::Save, Timer::Minute, Timer::Smoke, Timer::Stats, Timer::Update];

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Mouse monitors and observers stay registered for the app's life.
    static KEEP: RefCell<Vec<Retained<AnyObject>>> = const { RefCell::new(Vec::new()) };
    /// Recorder messages that came while the app was busy (re-entered): the
    /// next `drain_events` hands them over.
    static REC_MISSED: RefCell<Vec<poni_core::recorder::Message>> = const { RefCell::new(Vec::new()) };
}

/// Work that must run without the app borrowed (AppKit may call back in).
enum Deferred {
    TakeKeyboard,
    ReleaseKeyboard,
    Show,
    /// Open the menu bar item's menu (the smoke test's picture of it).
    OpenMenu,
    /// Bring the Settings window (and the app) to the front.
    ShowSettings,
    /// Close the Settings window (its own close tells the app).
    CloseSettings,
}

/// The Settings window's content and its renderer, while it is open.
struct SettingsUi {
    view: settings_window::View,
    gfx: Gfx,
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
            Deferred::ShowSettings => settings_host::show(),
            Deferred::CloseSettings => settings_host::close(),
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
    if let Some(secs) = arg_after("--full-screen-window") {
        return services::fullscreen::test_window(secs.parse().unwrap_or(30.0));
    }
    if args.iter().any(|a| a == "--smc") {
        return services::smc::report();
    }
    let smoke_dir = arg_after("--smoke").map(PathBuf::from);
    if let Some(dir) = &smoke_dir {
        // Absolute: Finder icons and file URLs need full paths.
        let home = std::path::absolute(dir.join("home")).unwrap_or_else(|_| dir.join("home"));
        let _ = std::fs::remove_dir_all(&home);
        std::env::set_var("PONI_HOME", &home);
    }
    crate::plog::init(paths::log_path());
    pinfo!("Oyenza Studio {} starting on macOS (args: {:?})", env!("CARGO_PKG_VERSION"), &args[1..]);
    std::panic::set_hook(Box::new(|info| {
        perror!("panic: {info}");
    }));
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("Oyenza Studio must start on the main thread");
        return 2;
    };
    let _lock = match single_instance() {
        Ok(lock) => lock,
        Err(()) => {
            pinfo!("another instance is running; exiting");
            return 0;
        }
    };
    // Install on next start: the new version, downloaded and checked
    // earlier, takes this copy's place now, before any window, and opens
    // once this copy has quit.
    if smoke_dir.is_none() && crate::updates::pending_install() {
        return 0;
    }
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
        // Where the pointer is now says whether the island stays open.
        enter(|a| a.menu_closed());
        pointer_moved();
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

/// The Settings window's view draws.
pub fn draw_settings(ctx: &CGContext) {
    enter(|a| a.draw_settings(ctx));
}

/// Input in the Settings window. True for a press where nothing takes it:
/// the window then moves with the drag.
pub fn settings_input(input: settings_host::Input) -> bool {
    enter(|a| a.settings_input(input)).unwrap_or(false)
}

/// The Settings window closed (its button, ⌘W).
pub fn settings_closed() {
    enter(|a| a.settings_closed());
    settings_host::release();
}

pub fn drain_events() {
    enter(|a| {
        while let Ok(ev) = a.rx.try_recv() {
            a.on_event(ev);
        }
        for m in REC_MISSED.with(|q| std::mem::take(&mut *q.borrow_mut())) {
            a.on_recorder_message(m);
        }
        a.request_frame();
    });
}

/// A message from Oyenza Studio Recorder through the island's message port
/// (`services::recorder::listen`), read while the recorder waits for the
/// reply: true tells it the island shows the recording (or its Record page),
/// so it keeps its own bar and controls hidden; false, that it should show
/// them.
pub fn recorder_message(m: poni_core::recorder::Message) -> bool {
    match enter(|a| a.on_recorder_message(m.clone())) {
        Some(taken) => taken,
        None => {
            REC_MISSED.with(|q| q.borrow_mut().push(m));
            dispatch2::DispatchQueue::main().exec_async(drain_events);
            false
        }
    }
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

/// What the smoke run measures of the drawing at rest: frames, the time in
/// `draw`, and the part spent on the island's body (for a floating one, its
/// shadow and hairline too).
#[derive(Clone, Copy, Debug, Default)]
struct DrawCost {
    frames: u32,
    draw: f64,
    body: f64,
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
    /// The Settings window's content and renderer, while it is open.
    settings_ui: Option<SettingsUi>,
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
    timers: [Option<f64>; 8],
    ns_timer: Option<(f64, Retained<NSTimer>)>,
    saved_settings: Settings,
    settings_writable: bool,
    store_writable: bool,
    first_run: bool,
    /// The Today page's stats are being sampled (once a second).
    stats_on: bool,
    /// The System page's sampler thread: from the start while its history is
    /// kept (`Settings::system_history_on`: System on, its history kept),
    /// else from the first time the page opens.
    sysmon: Option<services::sysmon::SysMon>,
    /// The System page is open (its readings come each second).
    sysmon_on: bool,
    /// Nobody can see the screen: (the displays sleep, the Mac is locked).
    away: (bool, bool),
    /// The app in front is full screen on the island's screen (`services::fullscreen`).
    front_full_screen: bool,
    /// A full-screen app is in front and Hide is picked: the island is closed
    /// and draws nothing until it's back (`apply_fullscreen`).
    fullscreen_hidden: bool,
    /// Oyenza sign-in's worker: from the start when this Mac is signed in (or
    /// a sign-out still has to reach the server), else from the first sign-in.
    oyenza: Option<services::oyenza::OyenzaService>,
    /// The Oyenza page is on screen: its day list stays live (`services::oyenza`).
    oyenza_watch: bool,
    /// The Today page is on screen: the worker asks for the day (`services::oyenza`).
    today_watch: bool,
    /// The Health page is on screen (it asks for the phone's steps as it opens).
    health_shown: bool,
    /// The IELTS or Add-ons page is on screen (it asks whether the account
    /// has Studio Pro as it opens).
    pro_shown: bool,
    /// Files are being dragged over the island.
    drag_hover: bool,
    /// The right-click menu is open over the open island: the pointer on
    /// the menu is off the island, so the menu holds it open until it closes.
    menu_open: bool,
    /// A Script page reading held the island open at the last tick.
    script_held: bool,
    /// The Script page was on screen at the last tick.
    script_shown: bool,
    /// Shelf items waiting to be dragged out (started by the next drag event).
    pending_drag: Option<Vec<String>>,
    /// Phone drop's web server while the Shelf's phone panel is open, and the
    /// shelf files it offers.
    phone: Option<services::phonedrop::PhoneDrop>,
    phone_offer: Vec<String>,
    /// What Music and Spotify play, and the lyrics worker.
    media: Option<services::media::MediaService>,
    lyrics: Option<services::lyrics::LyricsService>,
    /// The add-ons' downloader: only while an add-on is switched on.
    addons: Option<services::addons::AddonService>,
    /// The Downloader add-on's runner: started at the first download.
    downloader: Option<services::downloader::DownloaderService>,
    /// The IELTS coach's calls, microphone and voice, each only while needed.
    ielts: services::ielts::Kit,
    /// The IELTS coach paused the music playing as its session began.
    ielts_music: bool,
    /// The IELTS page was on screen at the last tick.
    ielts_shown: bool,
    /// The IELTS page held the island open at the last tick.
    ielts_held: bool,
    /// The Notes page's meeting while it is recorded (only then).
    notes_rec: Option<services::notes::Recording>,
    /// The meeting's audio once stopped, kept until its notes are saved (or
    /// it is thrown away), for Try again.
    notes_take: Option<services::notes::Take>,
    /// When the meeting being taken down began (local time, for its notes).
    notes_started: poni_core::datetime::LocalDateTime,
    /// The Stocks page's prices: started the first time the page opens.
    stocks: Option<services::stocks::StocksService>,
    /// The Stocks page was on screen at the last look (`set_stocks_watch`).
    stocks_open: bool,
    /// The AI tab's worker: started the first time the tab opens or its
    /// notices are on (`set_usage_watch`).
    usage: Option<services::usage::UsageService>,
    /// What it was last told.
    usage_watch: services::usage::Watch,
    /// New versions (`crate::updates`); None in the smoke test.
    updater: Option<crate::updates::Updater>,
    /// Prayer times' adzan, while it plays.
    adzan: services::adzan::AdzanPlayer,
    motion: Option<MotionStats>,
    motion_name: &'static str,
    motions_logged: u32,
    frames_drawn: u64,
    smoke: Option<Smoke>,
    /// The smoke run (`--smoke`): stays true while a step runs (`smoke` is
    /// taken out then), so the AI tab's worker never starts in it.
    smoke_run: bool,
    /// The smoke run: drawing cost at rest, a timer set aside while the
    /// music pill is measured, and where the ear step points.
    cost: DrawCost,
    smoke_timer: Option<poni_core::pomodoro::Pomodoro>,
    smoke_point: Option<Point>,
    /// The headphones step: device list changes heard before it, and how
    /// adding and removing its test device went.
    smoke_devices: Option<(u32, Result<(), i32>)>,
    /// The smoke run's full-screen window (a second copy of the app).
    smoke_full_screen: Option<std::process::Child>,
    /// The Oyenza step's POST through the Mac's HTTP client: (arrived and answered, what happened).
    smoke_post: Option<(bool, String)>,
    /// The IELTS coach's voice steps: the line playing and how it went.
    smoke_voice: Option<SmokeVoice>,
    /// Its microphone steps: what `Mic::start` gave, then what `stop` gave
    /// (whether that's right, and what happened).
    smoke_mic: Option<Result<services::mic::Mic, services::mic::MicError>>,
    smoke_mic_stop: Option<(bool, String)>,
    /// Meeting notes' recording step: the recording (or why none), then
    /// what stopping it gave.
    smoke_notes: Option<Result<(services::notes::Recording, bool, bool), poni_core::notes::CaptureIssue>>,
    smoke_notes_stop: Option<(bool, String)>,
    /// The island floated on the last frame (Pill letting go of the notch).
    was_floating: bool,
    /// The port Oyenza Studio Recorder writes to (`services::recorder`).
    rec_port: Option<services::recorder::Port>,
    /// When the island next asks whether the recorder still runs (mono seconds).
    rec_alive_at: f64,
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
        // The Downloader (Studio Pro): the Mac has it too.
        st.downloader_offered = true;
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
            settings_ui: None,
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
            timers: [None; 8],
            ns_timer: None,
            saved_settings,
            settings_writable: s_out.writable(),
            store_writable: d_out.writable(),
            first_run: s_out == LoadOutcome::Missing,
            stats_on: false,
            sysmon: None,
            sysmon_on: false,
            away: (false, false),
            front_full_screen: false,
            fullscreen_hidden: false,
            oyenza: None,
            oyenza_watch: false,
            today_watch: false,
            health_shown: false,
            pro_shown: false,
            drag_hover: false,
            menu_open: false,
            script_held: false,
            script_shown: false,
            pending_drag: None,
            phone: None,
            phone_offer: Vec::new(),
            media: None,
            lyrics: None,
            addons: None,
            downloader: None,
            ielts: Default::default(),
            ielts_music: false,
            ielts_shown: false,
            ielts_held: false,
            notes_rec: None,
            notes_take: None,
            notes_started: Default::default(),
            stocks: None,
            stocks_open: false,
            usage: None,
            usage_watch: services::usage::Watch::default(),
            updater: None,
            adzan: Default::default(),
            motion: None,
            motion_name: "morph",
            motions_logged: 0,
            frames_drawn: 0,
            smoke_run: smoke.is_some(),
            smoke,
            cost: DrawCost::default(),
            smoke_timer: None,
            smoke_point: None,
            smoke_devices: None,
            smoke_full_screen: None,
            smoke_post: None,
            smoke_voice: None,
            smoke_mic: None,
            smoke_mic_stop: None,
            smoke_notes: None,
            smoke_notes_stop: None,
            was_floating: false,
            rec_port: None,
            rec_alive_at: 0.0,
        }
    }

    fn start(&mut self) {
        self.layout_surface();
        // Floating from the start: before the first frame the gap is taken at once.
        self.island.set_floating(self.gap_target(&self.mode));
        let g = self.geom_for(&self.mode);
        self.island.snap(g);
        if self.smoke.is_none() {
            self.updater = Some(crate::updates::Updater::start(paths::data_dir().join("update.json"), &mut self.st, crate::updates::unix_now()));
        }
        let line = crate::updates::menu_line(&self.st, self.updater.is_some());
        self.menubar = Some(MenuBar::add(self.mtm, &self.st.settings, self.st.pinned, services::login::status(), &self.st.oyenza.link, line));
        self.st.power = services::power::query();
        services::power::watch(self.sink.clone());
        services::audio::start(self.sink.clone());
        self.media = Some(services::media::MediaService::start(self.mtm, self.sink.clone()));
        self.lyrics = match services::lyrics::LyricsService::start(self.sink.clone()) {
            Ok(l) => Some(l),
            Err(e) => {
                pwarn!("lyrics: unavailable ({e})");
                None
            }
        };
        self.sync_addons();
        // A meeting the app closed on, waiting to be written up.
        if self.st.notes_on() && self.smoke.is_none() {
            self.notes_restore();
        }
        self.schedule_update();
        self.fetch_setup();
        // Oyenza Studio Recorder installed: the Record tab can show (one
        // Launch Services question); its port is open either way, for the
        // recorder installed later.
        if self.smoke.is_none() {
            self.st.recorder = services::recorder::installed();
        }
        self.rec_port = services::recorder::listen();
        if self.rec_port.is_none() {
            pwarn!("recorder: the island's message port couldn't open");
        }
        // Shelf tiles for what was left on the Shelf.
        for p in self.st.store.shelf.paths() {
            self.st.cmds.push(Command::LoadThumb(p));
        }
        self.schedule_minute_timer();
        services::sysmon::watch_away(self.mtm, self.sink.clone());
        services::fullscreen::watch(self.mtm, self.sink.clone());
        // Signed in to Oyenza (or a sign-out still to tell it): the worker says
        // who, then asks at its pace. Never signed in: nothing starts.
        if paths::oyenza_link_path().exists() || paths::oyenza_revoke_path().exists() {
            self.start_oyenza();
        }
        if self.st.settings.system_history_on() {
            self.start_sysmon();
        }
        if self.first_run && self.smoke.is_none() {
            let place = if self.screen.notch.is_some() { "Point at the notch to open it" } else { "Point at the island to open it" };
            self.st.notify(Icon::CheckCircle, self.st.accent, "Oyenza Studio is on", place, 6.0, None);
        }
        if self.smoke.is_none() {
            if let Some(dir) = services::trash_app_from_before_rename() {
                self.st.notify(Icon::Trash, self.st.accent, "Oyen Poni is now Oyenza Studio", format!("The old copy in {dir} is in the Trash"), 8.0, None);
            }
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
        // Downloads under way end with the app (std::process::exit runs no
        // destructors): yt-dlp's groups get their SIGTERM now.
        self.downloader = None;
        pinfo!("Oyenza Studio exiting with code {code}");
        std::process::exit(code);
    }

    pub fn request_frame(&mut self) {
        // Hidden for a full-screen app: nothing to draw, but what a frame
        // ticks (a timer's end, a notice's time, commands) still runs, from
        // the wake timer (`hidden_tick`).
        if self.fullscreen_hidden {
            self.set_timer(Timer::Wake, 1);
            return;
        }
        window::set_frames(true);
    }

    /// What a frame does besides drawing, while a full-screen app hides the island.
    fn hidden_tick(&mut self) {
        self.refresh_time();
        self.tick();
        self.run_commands();
        if self.settings_ui.as_ref().is_some_and(|s| s.view.dirty) {
            settings_host::redraw();
        }
        if !self.st.cmds.is_empty() {
            self.set_timer(Timer::Wake, 1);
        }
        self.schedule_wake();
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
        if self.fullscreen_hidden {
            self.hidden_tick();
        } else {
            self.request_frame();
        }
    }

    fn on_timer(&mut self, t: Timer) {
        match t {
            Timer::Hover => {
                if !self.hovering || self.expanded {
                    self.hover_arm = None;
                } else if self.mode == Mode::Notice && self.st.notice.as_ref().is_some_and(|n| n.inline.is_some()) {
                    // A notice with an inline pill stays put under a resting
                    // pointer so the pill can be clicked.
                } else if self.input.mouse.and_then(|p| self.hit_at(p)).is_some_and(|h| h.id == wid(activities::PRAYER_OK_ID)) {
                    // So does the prayer countdown under a pointer resting on
                    // its OK (resting on the rest of it still opens).
                } else {
                    self.expand();
                }
            }
            Timer::Collapse => {
                if !self.hovering && !self.st.pinned && !self.text_focus && !self.drag_hover && !self.menu_open && self.input.pressed_id.is_none() && !self.phone_holds() {
                    self.collapse();
                }
            }
            Timer::Wake => {}
            Timer::Save => self.save_now(),
            Timer::Minute => {
                self.schedule_minute_timer();
                self.close_idle_phone();
                // A prayer's time comes on a whole minute, frames or not
                // (the displays may sleep).
                self.refresh_time();
                self.prayer_tick();
            }
            Timer::Smoke => self.smoke_step(),
            Timer::Update => self.update_due(),
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
        let hidden = self.fullscreen_hidden;
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
        // While a full-screen app hides the island only the phase end matters.
        if let Some(b) = self.st.pomodoro.next_second_boundary(now).filter(|_| !hidden) {
            consider(now + b + 0.005);
        }
        // A recording's time, on the closed island and its page: a frame for
        // each new second while it records (none while paused), resting while
        // the island is hidden or nobody can see the screen.
        let watched_rec = !hidden && !self.away.0 && !self.away.1 && self.st.record_on();
        if let Some(b) = self.st.rec_next_second().filter(|_| watched_rec) {
            consider(now + b + 0.005);
        }
        if let poni_core::pomodoro::RunState::Running { ends_at } = self.st.pomodoro.run {
            consider(ends_at + 0.005);
        }
        if let Some(t) = activities::next_wake(&self.mode, &self.st) {
            consider(t);
        }
        if let Mode::Expanded(p) = self.mode {
            // A page's own changes rest while nobody can see the screen (the
            // Prayer page's countdown ticks every second).
            let away = self.away.0 || self.away.1;
            if let Some(t) = pages::next_wake(p, &self.st).filter(|_| !away) {
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
        // The Settings window shows the change.
        if let Some(s) = self.settings_ui.as_mut() {
            s.view.dirty = true;
        }
        if before.theme != self.st.settings.theme {
            self.st.accent = theme_accent(self.st.settings.theme);
        }
        // The Pill theme's window keeps room for the gap above the island.
        if pill::on(before.theme) != pill::on(self.st.settings.theme) {
            self.layout_surface();
        }
        // Hide or Stay on top of full-screen apps (Settings or the menu bar).
        if before.hide_in_fullscreen != self.st.settings.hide_in_fullscreen {
            self.apply_fullscreen();
            self.refresh_menu();
        }
        if !before.lyrics && self.st.settings.lyrics {
            if let Some(m) = self.st.media.clone() {
                let stale = self.st.lyrics.track_key != m.track_key || self.st.lyrics.status == LyricsStatus::Idle;
                if stale && !m.title.is_empty() {
                    if let Some(l) = self.lyrics.as_ref() {
                        self.st.lyrics = LyricsState { track_key: m.track_key.clone(), status: LyricsStatus::Loading, ..Default::default() };
                        l.request(&m);
                    }
                }
            }
        }
        self.st.pomodoro.cfg = self.st.settings.pomodoro.clone();
        if before.expanded_width != self.st.settings.expanded_width {
            self.layout_surface();
        }
        // System's readings at rest: its history switch, while System is on.
        let history = self.st.settings.system_history_on();
        if before.system_history_on() != history {
            match &self.sysmon {
                Some(s) => s.background(history),
                None if history => self.start_sysmon(),
                None => {}
            }
            // Off while the page is closed: the kept minute goes, so the next opening starts fresh.
            if !history && !self.sysmon_on {
                self.st.system.clear();
            }
        }
        // Prayer times: off stops it at once; a new place, or following the location again.
        if before.addon_on(PRAYER_TIMES.id) && !self.st.settings.addon_on(PRAYER_TIMES.id) {
            self.prayer_off();
        }
        if before.prayer.place != self.st.settings.prayer.place {
            self.resolve_prayer_place();
        }
        if !before.prayer.follow_location && self.st.settings.prayer.follow_location && self.st.prayer_on() {
            self.locate();
        }
        // The Downloader: off stops its downloads before its tools are deleted.
        if before.addon_on(DOWNLOADER.id) && !self.st.settings.addon_on(DOWNLOADER.id) {
            self.downloads_off();
        }
        // IELTS off: a session ends, the microphone and voice stop, the files go.
        let ielts_id = poni_core::addons::IELTS.id;
        if before.addon_on(ielts_id) && !self.st.settings.addon_on(ielts_id) {
            self.ielts_off();
        }
        // Meeting notes off: a recording stops and its audio goes.
        let notes_id = poni_core::addons::MEETING_NOTES.id;
        if before.addon_on(notes_id) && !self.st.settings.addon_on(notes_id) {
            self.notes_off();
        }
        // An add-on switched on downloads its files; off deletes them.
        let mut addons_changed = false;
        for a in pages::addons::OFFERED {
            let on = self.st.settings.addon_on(a.id);
            if before.addon_on(a.id) != on {
                pinfo!("add-ons: {} switched {}", a.id, if on { "on" } else { "off" });
                self.st.addons.insert(a.id, if !on { AddonStatus::Off } else if a.downloads() { AddonStatus::Checking } else { AddonStatus::Ready });
                addons_changed |= a.downloads();
            }
        }
        if addons_changed {
            self.sync_addons();
        }
        // Health on or off: the Oyenza worker asks for the steps only while
        // it's on, and off forgets the ones it had.
        if before.addon_on(HEALTH.id) != self.st.settings.addon_on(HEALTH.id) {
            if !self.st.settings.addon_on(HEALTH.id) {
                self.st.health = None;
            }
            self.tell_oyenza_activity();
        }
        // The AI tab's heads-up notices: its worker reads every few minutes only while they're on.
        if before.usage.notices != self.st.settings.usage.notices {
            self.set_usage_watch();
        }
    }

    /// Hand the add-on switches to the downloader, starting it when one is
    /// on. It checks the files of the ones on (downloading what's missing),
    /// deletes those of one switched off, and sleeps in between; with none
    /// on, it isn't started (see `AppEvent::Addon` for its end). A Studio Pro
    /// add-on's files (the Downloader's tools) wait for Pro (`pro_changed`).
    fn sync_addons(&mut self) {
        let on: Vec<&'static str> = pages::addons::OFFERED.iter().filter(|a| a.downloads() && self.st.settings.addon_on(a.id)).map(|a| a.id).collect();
        let pro = self.st.studio_pro() == Some(true);
        match &self.addons {
            Some(svc) => {
                for a in pages::addons::OFFERED {
                    svc.set(a.id, on.contains(&a.id));
                }
                svc.set_pro(pro);
            }
            None if !on.is_empty() => match services::addons::AddonService::start(self.sink.clone(), on) {
                Ok(svc) => {
                    svc.set_pro(pro);
                    self.addons = Some(svc);
                }
                Err(e) => pwarn!("add-ons: unavailable ({e})"),
            },
            None => {}
        }
    }

    // ------------------------------------------------------------ the Settings window

    /// Open the Settings window on `section` (General when it opens), or
    /// bring it forward, on that section when one is given.
    fn open_settings(&mut self, section: Option<Section>) {
        match self.settings_ui.as_mut() {
            Some(s) => {
                if let Some(sec) = section {
                    s.view.show_section(sec);
                    settings_host::redraw();
                }
            }
            None => {
                let scale = settings_host::create(self.mtm);
                self.settings_ui = Some(SettingsUi { view: settings_window::View::new(section.unwrap_or(Section::General)), gfx: Gfx::new(scale) });
                // Start at login's switch reads it.
                self.st.ui.autostart = Some(services::login::status() == Login::On);
                pinfo!("settings window: open");
            }
        }
        // Add-ons lists IELTS and Meeting notes by the account's Pro, as its tab does.
        if self.settings_ui.as_ref().is_some_and(|s| s.view.section == Section::Addons) {
            self.oyenza(OyenzaCmd::CheckPro);
        }
        self.pending.push(Deferred::ShowSettings);
    }

    /// Draw the Settings window (its view's drawRect), light or dark as the
    /// window shows. The commands its clicks pushed run with the next frame.
    fn draw_settings(&mut self, ctx: &CGContext) {
        let Some(ui) = self.settings_ui.as_mut() else { return };
        let light = settings_host::is_light();
        ui.gfx.set_scale(settings_host::scale());
        ui.gfx.set_look(poni_core::look::glass(light));
        self.st.now = self.st.now.max(clock::mono());
        let now = self.st.now;
        let frame = settings_window::Frame {
            card: Rect::new(0.0, 0.0, settings_host::WIDTH as f32, settings_host::HEIGHT as f32),
            own_chrome: false,
            top: settings_host::TITLEBAR,
        };
        let before = ui.view.section;
        let out = {
            let mut canvas = ui.gfx.canvas(ctx);
            ui.view.draw(&mut canvas, &mut self.st, now, frame, light)
        };
        let after = ui.view.section;
        if out.close {
            self.pending.push(Deferred::CloseSettings);
        }
        if out.again {
            settings_host::redraw();
        }
        if after != before && after == Section::Addons {
            self.oyenza(OyenzaCmd::CheckPro);
        }
        if !self.st.cmds.is_empty() {
            self.request_frame();
        }
    }

    /// Input in the Settings window. True for a press on an empty part:
    /// the window moves with the drag.
    fn settings_input(&mut self, input: settings_host::Input) -> bool {
        let Some(ui) = self.settings_ui.as_mut() else { return false };
        let v = &mut ui.view;
        let redraw = match input {
            settings_host::Input::Move { x, y } => v.hover(Point::new(x, y)),
            settings_host::Input::Leave => {
                v.leave();
                true
            }
            settings_host::Input::Down { x, y } => {
                let p = Point::new(x, y);
                if v.hit_at(p).is_none() {
                    return true;
                }
                v.press(p);
                true
            }
            settings_host::Input::Up { x, y } => {
                v.release(Point::new(x, y));
                true
            }
            settings_host::Input::Wheel { x, y, notches } => {
                v.wheel(Point::new(x, y), notches);
                true
            }
        };
        if redraw {
            settings_host::redraw();
        }
        false
    }

    /// The Settings window closed: its content and renderer go.
    fn settings_closed(&mut self) {
        if self.settings_ui.take().is_some() {
            pinfo!("settings window: closed");
        }
    }

    fn on_menu(&mut self, cmd: MenuCmd) {
        let before = self.st.settings.clone();
        let s = &mut self.st.settings;
        match cmd {
            MenuCmd::ShowSettings => self.open_settings(None),
            MenuCmd::TogglePinned => {
                self.st.pinned = !self.st.pinned;
                if !self.st.pinned {
                    self.maybe_schedule_collapse();
                }
            }
            MenuCmd::SetTheme(t) => s.theme = t,
            MenuCmd::SetIdleStyle(style) => s.idle_style = style,
            MenuCmd::ToggleSounds => s.sounds = !s.sounds,
            MenuCmd::SetHideFullscreen(hide) => s.hide_in_fullscreen = hide,
            MenuCmd::ToggleLyrics => s.lyrics = !s.lyrics,
            MenuCmd::TogglePronunciation => s.lyrics_pronunciation = !s.lyrics_pronunciation,
            MenuCmd::AppleMusicToken => self.use_apple_music_token(),
            MenuCmd::AppleMusicHelp => services::open(APPLE_MUSIC_HELP_URL),
            MenuCmd::AppleMusicForget => self.forget_apple_music_token(),
            MenuCmd::OyenzaSignIn => self.oyenza(OyenzaCmd::SignIn),
            MenuCmd::OyenzaCancel => self.oyenza(OyenzaCmd::Cancel),
            MenuCmd::OyenzaDisconnect => self.oyenza(OyenzaCmd::Disconnect),
            MenuCmd::OyenzaOpen => services::open(oyenza::WEB),
            MenuCmd::ToggleOyenzaAmounts => s.oyenza_show_amounts = !s.oyenza_show_amounts,
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
            MenuCmd::About => services::open(&crate::version::notes_url()),
            // "Update to x…": the island opens on the Update sheet.
            MenuCmd::Update => {
                self.st.open_update();
                self.expand();
            }
            MenuCmd::CheckForUpdates => self.check_for_updates(),
            MenuCmd::RecordScreen => services::recorder::record_screen(self.mtm),
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
        let line = crate::updates::menu_line(&self.st, self.updater.is_some());
        if let Some(m) = self.menubar.as_mut() {
            m.refresh(self.mtm, &self.st.settings, self.st.pinned, login, &self.st.oyenza.link, line);
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
                    self.st.notify(Icon::Sliders, palette::ORANGE, "Allow Oyenza Studio to start at login", "Turn it on in Login Items", 8.0, None);
                }
            }
        }
    }

    /// Save the Apple Music token copied from the browser, and take it off
    /// the clipboard.
    fn use_apple_music_token(&mut self) {
        let text = services::clipboard::read_text().unwrap_or_default();
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
        services::clipboard::set_text("");
        if let Some(l) = self.lyrics.as_ref() {
            l.apple_token_changed();
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
        if let Some(l) = self.lyrics.as_ref() {
            l.apple_token_changed();
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

    /// Look up the current track's lyrics again (the sources changed).
    fn refetch_lyrics(&mut self) {
        if !self.st.settings.lyrics {
            return;
        }
        let Some(m) = self.st.media.clone().filter(|m| !m.title.is_empty()) else { return };
        if let Some(l) = self.lyrics.as_ref() {
            self.st.lyrics = LyricsState { track_key: m.track_key.clone(), status: LyricsStatus::Loading, ..Default::default() };
            l.request(&m);
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

    /// The System page opened or closed. Open, its readings arrive each
    /// second; with the history kept the minute behind them arrives at once,
    /// else each opening starts a fresh minute.
    fn set_sysmon(&mut self, on: bool) {
        if on == self.sysmon_on {
            return;
        }
        self.sysmon_on = on;
        if on && self.sysmon.is_none() {
            self.start_sysmon();
        }
        if let Some(s) = &self.sysmon {
            s.open(on);
        }
        if !on && !self.st.settings.system_history_on() {
            self.st.system.clear();
        }
    }

    fn start_sysmon(&mut self) {
        match services::sysmon::SysMon::start(self.sink.clone(), self.st.settings.system_history_on()) {
            Ok(s) => {
                if self.away.0 || self.away.1 {
                    s.away(true);
                }
                self.sysmon = Some(s);
            }
            Err(e) => pwarn!("sysmon: couldn't start ({e})"),
        }
    }

    // ------------------------------------------------------------ new versions

    /// Arm the update file's timer for the next read, or stop it while a
    /// read runs.
    fn schedule_update(&mut self) {
        match self.updater.as_ref().and_then(|u| u.due_in(crate::updates::unix_now())) {
            Some(s) => self.set_timer(Timer::Update, s.saturating_mul(1000).clamp(1000, u32::MAX as u64) as u32),
            None => self.kill_timer(Timer::Update),
        }
    }

    /// The timer fired, or someone came back: read the update file if a read
    /// is due and someone is at the Mac, not in Low Power Mode. Otherwise it
    /// waits; waking, unlocking and power changes call this again.
    fn update_due(&mut self) {
        let now = crate::updates::unix_now();
        let away = self.away.0 || self.away.1 || self.st.power.saver;
        match self.updater.as_ref().and_then(|u| u.due_in(now)) {
            Some(0) if away => {}
            Some(0) => self.read_update_file(now),
            _ => self.schedule_update(),
        }
    }

    /// Check for updates… (the menu): read the update file now, even in Low
    /// Power Mode (someone asked); the answer goes on the island.
    fn check_for_updates(&mut self) {
        let now = crate::updates::unix_now();
        if self.updater.as_mut().is_some_and(|u| u.ask(now)) {
            self.read_update_file(now);
        }
        self.request_frame();
    }

    /// The add-on downloader's thread, started here when no add-on is on:
    /// the update file is read and the DMG downloads on it.
    fn start_addon_thread(&mut self) {
        if self.addons.is_none() {
            match services::addons::AddonService::start(self.sink.clone(), Vec::new()) {
                Ok(svc) => self.addons = Some(svc),
                Err(e) => pwarn!("add-ons: unavailable ({e})"),
            }
        }
    }

    /// Read the update file on the add-on downloader's thread; the answer
    /// comes back as `UpdateEvent::Checked`.
    fn read_update_file(&mut self, now: u64) {
        self.start_addon_thread();
        let started = self.addons.as_ref().map(|a| a.run(services::update::check_job(self.sink.clone()))).is_some();
        if let Some(u) = self.updater.as_mut() {
            u.begin();
            if !started {
                u.checked(&mut self.st, Err("the add-on thread isn't running".into()), now);
                self.schedule_update();
            }
        }
    }

    /// The offered version's DMG, once per release, on the add-on thread,
    /// which then takes its app out (after a look at the file, and at start
    /// for one already offered). Only where this copy can replace itself.
    fn fetch_setup(&mut self) {
        let Some((d, path)) = self.updater.as_mut().and_then(|u| u.want_setup(&mut self.st)) else { return };
        self.start_addon_thread();
        let job = services::update::download_job(self.sink.clone(), d, path);
        if self.addons.as_ref().map(|a| a.run(job)).is_none() && self.updater.as_mut().is_some_and(|u| u.fetched(&mut self.st, Err("the add-on thread isn't running".into()))) {
            self.expand();
        }
    }

    /// Install now (the Update sheet): put the new app in this copy's place,
    /// quit (saving, as on any quit) and let it open. Where that can't be
    /// done, the DMG: the one downloaded here if it is in (opened, to drag by
    /// hand), else the one on the release page.
    fn update_now(&mut self) {
        let ready = self.updater.as_ref().and_then(|u| u.ready_setup(&self.st));
        if let Some((app, d)) = ready {
            match services::update::install(&app, d.version().unwrap_or_default()) {
                Ok(()) => {
                    pinfo!("updates: installing {}", d.file_name());
                    self.quit(0);
                    return;
                }
                Err(e) => pwarn!("updates: couldn't install in place ({e}); opening the DMG"),
            }
            if let Some(dmg) = self.updater.as_ref().and_then(|u| u.downloaded(&self.st)) {
                services::open(&dmg.to_string_lossy());
                self.collapse();
                self.st.notify(Icon::Download, self.st.accent, "Install it by hand this time", "Drag Oyenza Studio to Applications in the window that opened", 10.0, None);
                return;
            }
        }
        if let Some(url) = self.updater.as_ref().and_then(|u| u.download_page(&self.st)) {
            pinfo!("updates: opening {url}");
            services::open(&url);
        }
    }

    /// The displays slept or woke, or the Mac locked or unlocked. While nobody
    /// can see the screen the System readings rest, and the minute they kept
    /// goes, so a minute from before never shows as the last one.
    fn set_away(&mut self, asleep: Option<bool>, locked: Option<bool>) {
        let was = self.away.0 || self.away.1;
        self.away = (asleep.unwrap_or(self.away.0), locked.unwrap_or(self.away.1));
        let now = self.away.0 || self.away.1;
        if now == was {
            return;
        }
        if let Some(s) = &self.sysmon {
            s.away(now);
        }
        if now {
            self.st.system.clear();
        } else {
            // Back: a read of the update file that came due meanwhile.
            self.update_due();
        }
        self.tell_oyenza_activity();
        self.set_stocks_watch();
        self.set_usage_watch();
        self.apply_fullscreen();
    }

    /// A full-screen app is in front on the island's screen and Hide is
    /// picked: as on Windows, the island leaves the screen and closes, the
    /// prayer sound stays quiet and nothing is drawn until it's back.
    fn apply_fullscreen(&mut self) {
        let away = self.away.0 || self.away.1;
        let hide = self.front_full_screen && !away && self.st.settings.hide_in_fullscreen;
        // Money and bill notices held back wait for the full-screen app to leave.
        if !hide && !self.front_full_screen && self.st.oyenza.held.is_some() {
            self.show_held_notice(clock::mono());
            self.request_frame();
        }
        if hide == self.fullscreen_hidden {
            return;
        }
        pinfo!("fullscreen app {}: island {}", if hide { "in front" } else { "gone" }, if hide { "hidden" } else { "shown" });
        if hide {
            self.collapse();
            self.leave();
            // At rest at once, so the island isn't still closing when it's back.
            self.mode = self.target_mode();
            self.outgoing = None;
            self.page_anim = None;
            self.island.set_floating(self.gap_target(&self.mode));
            let g = self.geom_for(&self.mode);
            self.island.snap(g);
            window::set_frames(false);
        }
        window::set_shown(!hide);
        self.fullscreen_hidden = hide;
        self.request_frame();
    }

    pub fn on_event(&mut self, ev: AppEvent) {
        let now = clock::mono();
        self.st.now = now;
        let media_before = matches!(ev, AppEvent::Media(_) | AppEvent::MediaTimeline { .. }).then(|| self.media_snapshot());
        // What the Settings window shows that a service reports.
        let shown = matches!(ev, AppEvent::Update(_) | AppEvent::Oyenza(_) | AppEvent::Addon { .. } | AppEvent::Usage(_) | AppEvent::Located(_));
        if let Some(s) = self.settings_ui.as_mut().filter(|_| shown) {
            s.view.dirty = true;
        }
        // A download's bytes coming in while nobody can see the island (locked,
        // displays asleep) wait for the next frame something else asks for:
        // the pill catches up then, as nothing else of it moves on its own.
        let unseen = matches!(ev, AppEvent::Download { line: poni_core::ytdlp::Line::Progress { .. }, .. }) && (self.away.0 || self.away.1);
        match ev {
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
                                if let Some(l) = self.lyrics.as_ref() {
                                    self.st.lyrics.status = LyricsStatus::Loading;
                                    l.request(&s);
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
                        LyricsResult::Found { lyrics, source, ruby, wants_ja_readings } => {
                            self.st.lyrics.status = LyricsStatus::Found;
                            self.st.lyrics.lyrics = Some(lyrics);
                            self.st.lyrics.source = source;
                            self.st.lyrics.ruby = ruby;
                            self.st.lyrics.wants_ja_readings = wants_ja_readings;
                            // The first Japanese song whose kanji went unread
                            // switches Japanese readings on, as on Windows
                            // (app.rs): switched off by hand, they stay off.
                            if poni_core::addons::ja_switches_on(&self.st.settings, wants_ja_readings) {
                                pinfo!("lyrics: a Japanese song's kanji went unread; switching Japanese readings on");
                                self.st.settings.set_addon(JAPANESE_READINGS.id, true);
                                self.st.cmds.push(Command::SaveSettings);
                            }
                        }
                        LyricsResult::Instrumental => self.st.lyrics.status = LyricsStatus::Instrumental,
                        LyricsResult::NotFound => self.st.lyrics.status = LyricsStatus::NotFound,
                        LyricsResult::Error(e) => self.st.lyrics.status = LyricsStatus::Error(e),
                    }
                }
            }
            AppEvent::AppleMusic(status) => self.on_apple_music(status),
            AppEvent::Volume { level, muted, switched } => {
                // The first report only sets the level; changes show the pop-up.
                let first = !self.st.volume.known;
                self.st.volume = VolumeState { level, muted, known: true };
                // A new output device's level isn't news while the headphone
                // pop-up says why the device changed.
                let headphones = switched && matches!(self.st.hud.as_ref().map(|h| &h.kind), Some(HudKind::Bluetooth { .. }));
                if !first && !headphones && self.st.settings.hud_volume {
                    self.st.show_hud(HudKind::Volume { level, muted }, 1.6);
                }
            }
            AppEvent::Power(p) => {
                let old = self.st.power;
                self.st.power = p;
                self.tell_oyenza_activity();
                self.set_usage_watch();
                if old.saver && !p.saver {
                    self.update_due();
                }
                if self.st.settings.hud_power && p.has_battery {
                    if p.on_ac != old.on_ac {
                        self.st.show_hud(HudKind::Charging { percent: p.percent, plugged: p.on_ac }, 2.2);
                    } else if !p.on_ac && ((old.percent > 20 && p.percent <= 20) || (old.percent > 10 && p.percent <= 10)) {
                        self.st.show_hud(HudKind::LowBattery { percent: p.percent }, 4.0);
                    }
                }
            }
            AppEvent::ShelfAdd(paths) if self.expanded && self.st.page == PageKind::Script && pages::script::take_drop(&mut self.st, &paths) => {
                // A text file dropped on the Script page becomes the script.
                self.drag_hover = false;
                self.st.drag_hover = false;
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
                    // The Script page takes a text file; everything else goes to the Shelf.
                    if !(self.expanded && self.st.page == PageKind::Script) {
                        self.st.page = PageKind::Shelf;
                    }
                    self.expand();
                } else {
                    self.maybe_schedule_collapse();
                }
            }
            AppEvent::ShelfThumb { path, image } => {
                self.st.shelf_thumbs.insert(path, image);
            }
            AppEvent::Stats(s) => self.st.stats = s,
            AppEvent::System(m) => {
                // A reading still on its way when the page closed is dropped.
                if self.sysmon_on {
                    self.st.system = *m;
                }
            }
            AppEvent::AppIcon { path, image } => {
                let px = pages::system::icon_px(self.gfx.scale());
                self.st.app_icons.insert(path, image.map(|i| pages::system::shrink_icon(&i, px)));
            }
            AppEvent::ScreensAsleep(on) => self.set_away(Some(on), None),
            AppEvent::Locked(on) => self.set_away(None, Some(on)),
            AppEvent::Phone(e) => self.on_phone(e),
            AppEvent::Addon { id, status } => self.on_addon(id, status),
            AppEvent::Download { id, line } => self.download_line(id, line),
            AppEvent::DownloadEnded { id, ok } => self.download_ended(id, ok),
            AppEvent::Ielts(reply) => self.ielts_reply(*reply),
            AppEvent::VoiceEnded => {
                if let Some(v) = self.smoke_voice.as_mut() {
                    v.ended.get_or_insert(now);
                }
                self.ielts_voice_ended();
            }
            AppEvent::MicFailed(why) => self.ielts_mic_failed(why),
            AppEvent::Notes(reply) => self.notes_reply(*reply),
            AppEvent::NotesStopped(issue) => self.notes_stopped(issue),
            AppEvent::Update(services::update::UpdateEvent::Checked(r)) => {
                let open = self.updater.as_mut().is_some_and(|u| u.checked(&mut self.st, r, crate::updates::unix_now()));
                self.schedule_update();
                self.fetch_setup();
                self.refresh_menu();
                // Check for updates… found a new version: the Update sheet.
                if open {
                    self.expand();
                }
                // A frame, so a new version's notice goes up now, not with the next redraw.
                self.request_frame();
            }
            AppEvent::Update(services::update::UpdateEvent::Downloaded(r)) => {
                // Check for updates… was waiting for it: the Update sheet.
                if self.updater.as_mut().is_some_and(|u| u.fetched(&mut self.st, r)) {
                    self.expand();
                }
                self.request_frame();
            }
            AppEvent::Located(found) => self.on_located(found),
            AppEvent::Cities { file, got } => pages::prayer::cities_came(&mut self.st, file, got),
            AppEvent::Stocks(ev) => self.st.stocks.apply(ev, self.st.unix),
            AppEvent::Usage(ev) => self.on_usage(ev),
            AppEvent::AdzanEnded => self.adzan_ended(),
            AppEvent::Log(s) => pinfo!("{s}"),
            AppEvent::Bluetooth { name, connected, battery, audio } => {
                if self.st.settings.hud_bluetooth {
                    self.st.show_hud(HudKind::Bluetooth { name, connected, battery, audio }, 2.6);
                }
            }
            AppEvent::Fullscreen(on) => {
                self.front_full_screen = on;
                self.apply_fullscreen();
            }
            AppEvent::Oyenza(ev) => self.on_oyenza(ev),
        }
        if let Some(before) = media_before {
            self.after_media_change(before, now);
        }
        if !unseen {
            self.request_frame();
        }
    }

    /// Where an add-on's files stand, from the downloader.
    fn on_addon(&mut self, id: &'static str, status: AddonStatus) {
        // A late report from before a switch-off doesn't bring it back.
        if self.st.settings.addon_on(id) || status == AddonStatus::Off {
            self.st.addons.insert(id, status);
        }
        if id == PRAYER_TIMES.id {
            if status == AddonStatus::Ready && self.st.settings.addon_on(id) && self.st.prayer.places.is_none() {
                self.prayer_ready();
            } else if status == AddonStatus::Off {
                self.prayer_off();
            }
        }
        // Japanese readings arrived for the song that asked for them: read it
        // again (its lyrics come from the cache).
        if id == JAPANESE_READINGS.id && status == AddonStatus::Ready && self.st.lyrics.wants_ja_readings {
            if let (Some(l), Some(m)) = (self.lyrics.as_ref(), self.st.media.as_ref()) {
                if m.track_key == self.st.lyrics.track_key {
                    pinfo!("lyrics: Japanese readings are on this Mac; reading the song again");
                    l.request(m);
                }
            }
        }
        // Switched off and its files deleted: with no add-on on, the
        // downloader's thread ends (it starts again with the next switch-on).
        let ours = pages::addons::OFFERED.iter().any(|a| a.id == id);
        // The update file is read on the same thread: it stays while this copy looks for new versions.
        let in_use = self.updater.is_some();
        if ours && status == AddonStatus::Off && !in_use && !pages::addons::OFFERED.iter().any(|a| a.downloads() && self.st.settings.addon_on(a.id)) {
            self.addons = None;
        }
    }

    // ------------------------------------------------------------ prayer times
    //
    // As in the Windows app (`../app.rs`); only asking where the computer is
    // and playing the adzan are the Mac's own (`services::location`, `adzan`).

    /// Prayer times' files are in place: read its places, settle the place,
    /// and ask where the Mac is when following the location.
    fn prayer_ready(&mut self) {
        let path = services::addons::file_path(&PRAYER_TIMES, "places.json");
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| PlaceList::parse(&t)) {
            Ok(list) => {
                pinfo!("prayer: {} places", list.places.len());
                self.st.prayer.places = Some(std::sync::Arc::new(list));
            }
            Err(e) => {
                pwarn!("prayer: can't read the places ({e})");
                return;
            }
        }
        self.resolve_prayer_place();
        if self.st.settings.prayer.follow_location {
            self.locate();
        }
    }

    /// Prayer times switched off: nothing plays, nothing is kept.
    fn prayer_off(&mut self) {
        self.adzan.stop();
        services::location::forget();
        self.st.prayer = PrayerState::default();
    }

    /// The place the times are for: the chosen one, or until there is one,
    /// the one the Mac's time zone suggests. A place from the world list is
    /// kept as it is once settled; at start it comes from the lines settings
    /// keep of it (nothing online).
    fn resolve_prayer_place(&mut self) {
        let Some(list) = self.st.prayer.places.clone() else { return };
        let chosen = match self.st.settings.prayer.place.as_deref() {
            Some(id) if self.st.prayer.place.as_ref().is_some_and(|p| p.id == id) => self.st.prayer.place.clone(),
            Some(id) => poni_core::prayer::find(&list, self.st.settings.prayer.world_place.as_deref(), id),
            None => None,
        };
        let suggested = chosen.is_none();
        let place = chosen.or_else(|| poni_core::prayer::suggested_place_id(self.utc_offset_min()).and_then(|id| list.find(id)).cloned());
        let changed = place.as_ref().map(|p| &p.id) != self.st.prayer.place.as_ref().map(|p| &p.id);
        self.st.prayer.place = place;
        self.st.prayer.suggested = suggested;
        if changed {
            pinfo!("prayer: times for {}", self.st.prayer.place.as_ref().map_or("no place", |p| p.id.as_str()));
            // A new place: prayer times that passed there don't ring now.
            self.st.prayer.handled_until = Some(self.st.prayer_now());
        }
    }

    /// The Mac's UTC offset in minutes, from its clock.
    fn utc_offset_min(&self) -> i32 {
        let l = &self.st.local;
        let local = poni_core::datetime::days_from_civil(l.year, l.month, l.day) * 86_400 + l.hour as i64 * 3600 + l.minute as i64 * 60 + l.second as i64;
        (((local - self.st.unix) as f64 / 900.0).round() * 15.0) as i32
    }

    /// Ask macOS where the Mac is (the answer: `AppEvent::Located`).
    fn locate(&mut self) {
        if self.st.prayer.locate == Locate::Asking {
            return;
        }
        self.st.prayer.locate = Locate::Asking;
        self.st.prayer.located_at = Some(self.st.unix);
        services::location::locate(self.mtm, self.sink.clone());
    }

    /// The nearest listed place becomes the place, while following the
    /// location (`pages::prayer::located`). The point is never saved.
    fn on_located(&mut self, found: services::location::Located) {
        use services::location::Located;
        match found {
            Located::Denied => self.st.prayer.locate = Locate::Denied,
            Located::Unknown => self.st.prayer.locate = Locate::Unknown,
            // Kemenag's places, or the world's cities around it (looked up online).
            Located::At(lat, lng) => pages::prayer::located(&mut self.st, lat, lng),
        }
    }

    /// Each frame and each minute: the prayer times that just came, and
    /// asking where the Mac is again every 3 hours while following it.
    fn prayer_tick(&mut self) {
        if !self.st.prayer_on() {
            return;
        }
        if self.st.settings.prayer.follow_location && self.st.prayer.located_at.is_none_or(|t| self.st.unix - t >= 3 * 3600) {
            self.locate();
        }
        let Some(site) = self.st.prayer.site() else { return };
        let now = self.st.prayer_now();
        let Some(from) = self.st.prayer.handled_until else {
            self.st.prayer.handled_until = Some(now);
            return;
        };
        if now <= from {
            return;
        }
        self.st.prayer.handled_until = Some(now);
        for m in poni_core::prayer::prayers_between(&site, from, now) {
            match poni_core::prayer::due(m.at, now) {
                Due::Ring => self.prayer_moment(m, true),
                Due::Quiet => self.prayer_moment(m, false),
                Due::Skip => {}
            }
        }
    }

    /// A prayer's time came: the island says so and plays the chosen sound.
    /// Quiet when it's late (after a sleep) or a full-screen app hides the island.
    fn prayer_moment(&mut self, m: PrayerMoment, ring: bool) {
        let Some(site) = self.st.prayer.site() else { return };
        let place = self.st.prayer.place.as_ref().map_or(String::new(), |p| p.label.clone());
        let ring = ring && !self.fullscreen_hidden;
        let sound = if ring { self.st.settings.prayer.sound } else { PrayerSound::Silent };
        let mut adzan = false;
        // The tone plays at once, not as a command: commands run with the
        // next frame, and frames rest while the displays sleep.
        match sound {
            // The recording isn't Subuh's adzan (no "ash-shalatu khairum minan naum"): Subuh gets the tone.
            PrayerSound::Adzan if m.prayer != Prayer::Subuh => {
                adzan = self.play_adzan();
                if !adzan {
                    services::play(true);
                }
            }
            PrayerSound::Adzan | PrayerSound::Tone => services::play(true),
            PrayerSound::Silent => {}
        }
        let title = pages::prayer::moment_title(m.prayer);
        let subtitle = format!("{} \u{b7} {place}", site.hhmm(m.at));
        let secs = if adzan { 200.0 } else { 60.0 };
        self.st.notify(Icon::Moon, self.st.accent, title, subtitle, secs, Some(NoticeAction::OpenPage(PageKind::Prayer)));
        if adzan {
            self.st.set_notice_inline(InlineAction::StopAdzan);
        }
        pinfo!("prayer: {} at {}{}", m.prayer.name(), site.hhmm(m.at), if ring { "" } else { ", quietly" });
    }

    /// Start the adzan; music that plays pauses (and stays paused).
    fn play_adzan(&mut self) -> bool {
        let path = services::addons::file_path(&PRAYER_TIMES, "adzan.m4a");
        match self.adzan.play(self.mtm, &path, &self.sink) {
            Ok(()) => {
                self.st.prayer.adzan = true;
                if self.st.settings.prayer.pause_music && self.st.media.as_ref().is_some_and(|m| m.is_playing()) {
                    self.st.media_cmd(MediaCmd::PlayPause);
                }
                true
            }
            Err(e) => {
                pwarn!("prayer: the adzan didn't play ({e}); the tone instead");
                false
            }
        }
    }

    /// OK on the countdown before a prayer: the closed island goes back to
    /// what it would show (the song and its sung line) until the prayer's
    /// time. Its moment still comes then: `prayer_tick` doesn't look at the
    /// countdown.
    fn prayer_countdown_ok(&mut self) {
        self.refresh_time();
        if let (Some(m), Some(site)) = (self.st.prayer_countdown_ok(), self.st.prayer.site()) {
            pinfo!("prayer: countdown put away until {} at {}", m.prayer.name(), site.hhmm(m.at));
        }
    }

    fn adzan_ended(&mut self) {
        self.adzan.stop();
        self.st.prayer.adzan = false;
        // The notice leaves soon after the adzan.
        let soon = self.st.now + 4.0;
        if let Some(n) = self.st.notice.as_mut().filter(|n| n.inline == Some(InlineAction::StopAdzan)) {
            n.until = n.until.min(soon);
        }
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
            self.st.media_paused_at = None;
        } else if before.as_ref().is_some_and(|b| b.2) {
            // Playing -> paused: the pill lingers for a while.
            self.st.media_paused_at = Some(now);
        }
        // A new track starting while collapsed: show its title for a moment.
        // Not on a notched screen, where the title would land under the camera.
        if playing && new_track && !self.expanded && self.screen.notch.is_none() {
            self.st.ui.peek_until = now + 3.0;
        }
    }

    fn refresh_screen(&mut self) {
        let Some(s) = window::pick_screen(self.mtm) else { return };
        if s != self.screen {
            pinfo!("screen {}x{} pt at {}x, notch {:?}", s.w, s.h, s.scale, s.notch);
            self.screen = s;
            self.gfx.set_scale(s.scale);
            self.layout_surface();
            // Whether a full-screen app is in front is a question per screen.
            services::fullscreen::check();
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
        let (side, top, bottom) = self.room();
        let mut w = (body_w * OVERSHOOT + 2.0 * side).ceil();
        if w as i64 % 2 != 0 {
            w += 1.0;
        }
        let h = ((body_h + self.strip()) * OVERSHOOT + top + bottom).ceil();
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
        let (side, top, bottom) = self.room();
        let need_w = bw + 2.0 * side;
        let need_h = bh + top + bottom;
        if need_w > self.surface.0 || need_h > self.surface.1 {
            let tallest = PageKind::ALL.iter().map(|p| pages::content_height(*p, &self.st)).fold(0.0f32, f32::max);
            let body_h = (pages::HEADER_H + tallest + pages::PAD).max((bh - self.strip()) / OVERSHOOT * 1.1);
            pinfo!("island outgrew the window; growing it to fit {bw:.0}x{bh:.0} DIP");
            self.layout_surface_for(self.st.settings.expanded_width.max(bw / OVERSHOOT * 1.1), body_h);
        }
    }

    /// Room the window keeps beside, above and below the island's body: the
    /// shoulders and hit margin, and in the Pill theme the floating outline's
    /// margin and the gap above it (below the menu bar on a notch).
    fn room(&self) -> (f32, f32, f32) {
        let side = SHOULDER + HIT_MARGIN;
        if !pill::on(self.st.settings.theme) {
            return (side, 0.0, HIT_MARGIN);
        }
        let top = self.screen.notch.map_or(0.0, |n| n.h) + pill::TOP_GAP;
        (side.max(pill::shape::MARGIN), top, HIT_MARGIN.max(pill::shape::MARGIN))
    }

    /// The strip the open island keeps behind a notch over its content
    /// (none in the Pill theme, which floats below the menu bar).
    fn strip(&self) -> f32 {
        match self.screen.notch {
            Some(n) if !pill::on(self.st.settings.theme) => n.h,
            _ => 0.0,
        }
    }

    /// Where the Pill theme's island floats in `m`: `TOP_GAP` below the top
    /// edge on a screen without a notch (as on Windows); around a notch below
    /// the menu bar for what hangs under it, `None` (attached) in the menu
    /// bar's line. `None` in the other themes. The island eases to it on its
    /// own spring (island.rs), so it lets go of the notch and comes back with
    /// the outline.
    fn gap_target(&self, m: &Mode) -> Option<f32> {
        if !pill::on(self.st.settings.theme) {
            return None;
        }
        match self.screen.notch {
            Some(n) => notch::pill_gap(m, &self.st, n),
            None => Some(pill::TOP_GAP),
        }
    }

    fn target_mode(&self) -> Mode {
        if self.expanded {
            return Mode::Expanded(self.st.page);
        }
        activities::collapsed_mode(&self.st)
    }

    /// How the island paints: the theme's look, or around a notch the one
    /// that meets its black (Oyen goes black there).
    fn look(&self) -> &'static Look {
        match self.screen.notch {
            Some(_) => poni_core::look::around_notch(self.st.settings.theme),
            None => poni_core::look::of(self.st.settings.theme),
        }
    }

    /// The resting island is out of sight: hidden until hover, or inside the notch.
    fn island_hidden(&self) -> bool {
        self.mode == Mode::Idle
            && (self.st.settings.idle_style == IdleStyle::Hidden || self.screen.notch.is_some_and(|_| notch::fit(&self.mode, &self.st) == Fit::Inside))
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
            if self.gap_target(m).is_some() {
                // Floating, it swells evenly about its middle and stays a capsule.
                pill::motion::hover_lift(g)
            } else {
                Geom::new(g.w + HOVER_LIFT.0, g.h + HOVER_LIFT.1, g.r + HOVER_LIFT.2)
            }
        } else {
            g
        }
    }

    fn hit_at(&self, p: Point) -> Option<Hit> {
        self.hits.iter().rev().find(|h| h.rect.contains(p)).copied()
    }

    /// Whether `p` is over the part of the window that takes the mouse: the
    /// whole canvas while expanded (a forgiving edge), only the island
    /// collapsed (floating, with the gap above it: a pointer thrown at the top
    /// edge still lands on it).
    fn hovering_inside(&self, p: Point) -> bool {
        // Floating below a notch, the menu bar beside the notch keeps its
        // clicks: in the menu bar's line the notch takes the mouse (and the
        // island under it with the gap between them), and for a pointer that
        // already hovers, the resting island's ears too.
        if let Some(n) = self.screen.notch.filter(|_| self.island.floating()) {
            if p.y < n.h {
                return (p.x - self.cx).abs() <= self.menu_line_w(n) / 2.0;
            }
        }
        if !self.expanded {
            return self.island.hover_rect_at(self.cx).contains(p);
        }
        self.island.canvas_rect_at(self.cx).contains(p)
    }

    /// How much of the menu bar's line takes the mouse while the island
    /// floats below notch `n`: the notch itself, and while the pointer already
    /// hovers the island (or presses or drags on it) the ears the resting
    /// island has there, lifted. A card opened from an ear then stays open
    /// under the pointer that opened it, and a pointer coming from the menu
    /// bar still only meets the notch.
    fn menu_line_w(&self, n: Notch) -> f32 {
        if !(self.hovering || self.input.down || self.drag_hover) {
            return n.w;
        }
        let rest = activities::collapsed_mode(&self.st);
        if notch::floats(&rest, &self.st) {
            return n.w;
        }
        let g = notch::geom(&rest, &self.st, n, activities::geom(&rest, &self.st));
        (g.w + HOVER_LIFT.0).max(n.w)
    }

    // ------------------------------------------------------------ input

    fn on_pointer(&mut self, p: Point) {
        self.peek_at(p);
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

    /// The Eyes look around a notch follows the pointer anywhere on the
    /// screen (`notch::draw_peek`). The global monitor already brings every
    /// move here, so the eyes cost a redraw only when their side or where
    /// they look changes.
    fn peek_at(&mut self, p: Point) {
        let Some(n) = self.screen.notch.filter(|_| self.st.settings.idle_style == IdleStyle::Eyes) else { return };
        let now = clock::mono().max(self.st.now);
        let before = notch::peek_key(&self.st, n, now);
        self.st.ui.peek.point(Point::new(p.x - self.cx, p.y), now, n.w / 2.0);
        if self.mode == Mode::Idle && !self.fullscreen_hidden && notch::peek_key(&self.st, n, now) != before {
            self.request_frame();
        }
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
                self.input.just_pressed = true;
                self.input.press_pos = Some(p);
                self.input.pressed_id = self.hit_at(p).map(|h| h.id);
                if !self.expanded {
                    let inline = self.st.notice.as_ref().and_then(|n| n.inline).filter(|_| self.input.pressed_id == Some(wid("notice.act")));
                    if let Some(a) = inline {
                        // The notice's inline pill acts in place without expanding.
                        self.input.pressed_id = None;
                        self.run_inline_action(a);
                    } else if self.input.pressed_id == Some(wid(activities::PRAYER_OK_ID)) {
                        // The prayer countdown's OK, the same way.
                        self.input.pressed_id = None;
                        self.prayer_countdown_ok();
                    } else {
                        // Clicking a notice jumps to its page (a new version's
                        // may be waiting under a volume change or a game's readout).
                        let shown = self.mode == Mode::Notice;
                        if let Some(n) = self.st.notice.take_if(|_| shown) {
                            match n.action {
                                Some(NoticeAction::OpenPage(p)) => self.st.page = p,
                                Some(NoticeAction::OpenUpdate) => self.st.open_update(),
                                Some(NoticeAction::OpenApp(_)) => {}
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
                if self.input.down {
                    self.input.released = self.input.pressed_id.map(|id| (id, p));
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
                    None => {
                        self.menu_open = self.expanded && self.hovering;
                        return self.menubar.as_ref().map(|m| m.menu.clone());
                    }
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
                    126 => Some(KeyInput::Up),
                    125 => Some(KeyInput::Down),
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
            // A recorder installed or removed since the last look.
            if self.smoke.is_none() {
                self.st.recorder = services::recorder::installed();
            }
            self.check_recorder_alive(true);
            // A recording under way opens on its page: Pause and Stop are there.
            if self.st.rec_live() && self.st.record_on() {
                self.st.page = PageKind::Record;
            } else if self.st.page == PageKind::Record && !self.st.record_on() {
                self.st.page = pages::first_tab(&self.st);
            } else if self.mode == Mode::Activity(activities::Activity::Download) {
                // The download pill opens on its page.
                self.st.page = PageKind::Downloads;
            }
        }
        self.request_frame();
    }

    /// Oyenza Studio Recorder's word on its recording, or "show yourself"
    /// (`recorder_message`). True when the island takes it: it shows the
    /// recording, or opened on its Record page.
    fn on_recorder_message(&mut self, m: poni_core::recorder::Message) -> bool {
        use poni_core::recorder::{Message, Phase};
        let now = clock::mono();
        self.st.now = now;
        // Only an installed recorder that reads the Record page's links
        // writes: the Record tab can show from now on (remembered).
        self.st.recorder = true;
        if !self.st.store.recorder_island {
            self.st.store.recorder_island = true;
            self.st.cmd(Command::SaveStore);
        }
        if !self.st.record_on() {
            return false;
        }
        let taken = match m {
            // Hidden for a full-screen app, the island can't be the recorder's face.
            Message::Show if self.fullscreen_hidden => false,
            // A Mac app can't tell a link launch from a plain one, so the
            // recorder Record started says show too: taken, the island as it is.
            Message::Show if self.st.rec.opening_until > now => true,
            Message::Show => {
                self.set_page(PageKind::Record);
                self.expand();
                true
            }
            Message::State { phase, elapsed_ms, error } => {
                let rec = &mut self.st.rec;
                rec.opening_until = 0.0;
                if phase == Phase::Idle {
                    rec.phase = None;
                    rec.error = error.map(|e| (e, now + REC_ERROR_S));
                } else {
                    rec.phase = Some(phase);
                    rec.elapsed_ms = elapsed_ms;
                    rec.at = now;
                    rec.error = None;
                }
                self.rec_alive_at = now + REC_ALIVE_S;
                true
            }
        };
        self.request_frame();
        taken
    }

    /// A recorder gone mid-recording (quit, crashed) leaves no recording
    /// behind on the island. Asked of macOS only while one is under way, at
    /// most every `REC_ALIVE_S` (`now`: as the island opens).
    fn check_recorder_alive(&mut self, now: bool) {
        if !self.st.rec_live() || self.smoke.is_some() || (!now && self.st.now < self.rec_alive_at) {
            return;
        }
        self.rec_alive_at = self.st.now + REC_ALIVE_S;
        if !services::recorder::running() {
            pinfo!("recorder: it quit mid-recording");
            self.st.rec = crate::state::RecView::default();
            self.request_frame();
        }
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
            self.set_sysmon(false);
            // Oyen arrives in his tab again at the next opening.
            self.st.ui.oyen_tab = None;
            // Settings closes with the island: it opens again on the page it was opened from.
            if self.st.page == PageKind::Settings {
                self.st.page = self.st.ui.before_settings.take().filter(|p| pages::tabs(&self.st).contains(p)).unwrap_or_else(|| pages::first_tab(&self.st));
            }
            self.st.update.sheet = false;
            self.st.update.said = None;
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
                self.st.update.sheet = false;
                self.st.update.said = None;
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
        if self.expanded && !self.hovering && !self.st.pinned && !self.text_focus && !self.drag_hover && !self.menu_open && !self.phone_holds() {
            self.set_timer(Timer::Collapse, self.st.settings.collapse_delay_ms.max(50));
        }
    }

    /// The right-click menu closed: off the island, the pointer closes it as
    /// leaving does (the next pointer reading keeps it open when it's back on).
    fn menu_closed(&mut self) {
        if std::mem::take(&mut self.menu_open) && !self.hovering {
            self.maybe_schedule_collapse();
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
        if a == InlineAction::StopAdzan {
            self.adzan.stop();
            self.st.prayer.adzan = false;
        }
        if a == InlineAction::Hide {
            // Gone at once, the way a notice leaves when its time is up.
            if let Some(n) = self.st.notice.as_mut() {
                n.until = self.st.now;
            }
            // A new version's: it doesn't come back.
            if let Some(u) = self.updater.as_mut() {
                u.seen(&mut self.st);
            }
            return;
        }
        if a == InlineAction::WhatsNew && self.st.notice.as_ref().is_some_and(|n| n.icon != Icon::Check) {
            services::open(&crate::version::notes_url());
        }
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
            InlineAction::ShowAmount | InlineAction::StopAdzan | InlineAction::Hide | InlineAction::WhatsNew | InlineAction::Open => {}
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
                Command::Media(m) => {
                    if let Some(s) = self.media.as_ref() {
                        s.command(m.clone());
                    }
                    // Optimistic UI for play/pause: the player's confirmation will match.
                    if m == crate::commands::MediaCmd::PlayPause {
                        if let Some(ms) = self.st.media.as_mut() {
                            let now = self.st.now;
                            let pos = ms.timeline.position_at(now);
                            ms.timeline.position_s = pos;
                            ms.timeline.sampled_at = now;
                            ms.timeline.playing = !ms.timeline.playing;
                            self.st.media_paused_at = (!ms.timeline.playing).then_some(now);
                        }
                    }
                }
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
                Command::FocusText => self.focus_text(),
                Command::BlurText => self.blur_text(),
                Command::RecordScreen => services::recorder::record_screen(self.mtm),
                Command::Record => {
                    let rec = self.st.settings.record;
                    services::recorder::record(self.mtm, rec.target, rec.inputs());
                    self.st.rec.opening_until = self.st.now + REC_OPENING_S;
                    self.st.rec.error = None;
                    // A window or an area is picked on the recorder's overlay:
                    // the open island would sit over it.
                    if rec.target != poni_core::recorder::Target::Screen {
                        self.collapse();
                    }
                }
                Command::RecordPause => services::recorder::pause(),
                Command::RecordResume => services::recorder::resume(),
                Command::RecordStop => services::recorder::stop(),
                Command::ScriptPaste => {
                    let text = services::clipboard::read_text().map(|t| poni_core::script::clean(&t)).filter(|t| !t.is_empty());
                    match text {
                        Some(text) => pages::script::set_text(&mut self.st, text),
                        None => pages::script::note(&mut self.st, "Copy your script first"),
                    }
                }
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
                // The System page's app icons, from Finder (on the main thread). The
                // first can take a tenth of a second (Finder's icons starting up in
                // Poni), so none starts while the island moves: the page asks again
                // with its next frame.
                Command::LoadAppIcon(p) => {
                    if !self.st.app_icons.contains_key(&p) && self.island.settled() {
                        self.st.app_icons.insert(p.clone(), None);
                        services::thumbs::app_icon_async(p, pages::system::icon_px(self.gfx.scale()), self.sink.clone());
                    }
                }
                // Show over games reads Windows' frame log; the Mac has no Games view.
                Command::CheckFps | Command::AllowFps => {}
                Command::SetPinned(b) => {
                    self.st.pinned = b;
                    if !b {
                        self.maybe_schedule_collapse();
                    }
                }
                Command::Collapse => self.collapse(),
                Command::Play(s) => services::play(s == Sound::Chime),
                Command::OpenUrl(u) => services::open(&u),
                Command::Update => self.update_now(),
                Command::CheckForUpdates => self.check_for_updates(),
                Command::InstallOnNextStart => {
                    if let Some(u) = self.updater.as_mut() {
                        u.install_on_next_start(&mut self.st);
                    }
                }
                Command::WantStats(b) => self.set_stats(b),
                Command::SetAutostart(on) => {
                    if (services::login::status() == Login::On) != on {
                        self.toggle_login();
                    }
                    self.st.ui.autostart = Some(services::login::status() == Login::On);
                    self.refresh_menu();
                }
                Command::Phone(c) => self.phone_cmd(c),
                // Paired phones come to the Mac in a later slice (its Shelf
                // shows no Pair a phone yet).
                Command::Pair(_) => {}
                Command::Oyenza(c) => self.oyenza(c),
                Command::GetCities(file) => services::cities::fetch(file, self.sink.clone()),
                Command::Download(c) => self.download_cmd(c),
                // Notifications is a Windows add-on: a Mac can't read other apps' notifications.
                Command::Notify(_) => {}
                Command::OpenSettings => self.open_settings(None),
                Command::Usage(c) => self.usage_cmd(c),
                Command::Ielts(c) => self.ielts_cmd(c),
                Command::Notes(c) => self.notes_cmd(c),
            }
        }
        self.sync_phone_offer();
    }

    // ------------------------------------------------------------ AI usage

    /// The AI tab's worker reads while the tab is on screen, or every few
    /// minutes for the person who switched on its heads-up notices; both rest
    /// while nobody can see the screen, the notices also on Low Power Mode.
    /// Its thread starts the first time either wants it.
    fn set_usage_watch(&mut self) {
        let here = !(self.away.0 || self.away.1);
        let open = here && self.mode == Mode::Expanded(PageKind::Ai);
        // A keychain read may bring up macOS's prompt only while the tab is
        // on screen; what it read stays while the notices are switched on,
        // resting or not, so they go on once someone is back
        // (services::keychain). Told before the worker; nothing while it
        // stays the same.
        services::keychain::set_watch(open, self.st.settings.usage.notices);
        // The smoke run shows made-up readings (and checks the keychain on
        // its own): nothing is read, even from inside a step.
        if self.smoke_run {
            return;
        }
        let w = services::usage::Watch {
            open,
            notices: here && self.st.settings.usage.notices && !self.st.power.saver,
            near_pct: self.st.settings.usage.near_pct,
            use_24h: self.st.settings.use_24h,
            hidden: services::usage::picked(&self.st.settings.usage.hidden),
            added: services::usage::picked(&self.st.settings.usage.added),
        };
        // Already told: nothing to do, every frame.
        if w == self.usage_watch {
            return;
        }
        self.usage_watch = w;
        if self.usage.is_none() && (w.open || w.notices) {
            self.start_usage();
        }
        if let Some(s) = &self.usage {
            s.set_watch(w);
        }
    }

    fn start_usage(&mut self) {
        match services::usage::UsageService::start(self.sink.clone()) {
            Ok(s) => self.usage = Some(s),
            Err(e) => pwarn!("service usage: unavailable ({e})"),
        }
    }

    /// Hide and Show change the AI tab's settings; the rest is its worker's
    /// (started by the first one if need be).
    fn usage_cmd(&mut self, c: UsageCmd) {
        match c {
            UsageCmd::Hide(id) | UsageCmd::Show(id) => {
                let u = &mut self.st.settings.usage;
                u.hidden.retain(|k| k != id.key());
                u.added.retain(|k| k != id.key());
                let list = if matches!(c, UsageCmd::Show(_)) { &mut u.added } else { &mut u.hidden };
                list.push(id.key().into());
                let before = self.saved_settings.clone();
                self.settings_changed(&before);
                self.save_settings();
            }
            c => {
                // The person asked: the keychain may ask macOS again, even after a no.
                if matches!(c, UsageCmd::Refresh) {
                    services::keychain::ask_again();
                }
                if self.usage.is_none() {
                    self.start_usage();
                }
                if let Some(s) = &self.usage {
                    s.command(c);
                }
            }
        }
    }

    /// What the AI tab's worker found goes into `st.usage`. Its heads-up
    /// notices wait their turn for the island (`tick` puts them up one at a
    /// time once it is free); the frame `on_event` asks for looks at once.
    fn on_usage(&mut self, ev: services::usage::UsageEvent) {
        match ev {
            services::usage::UsageEvent::Alerts(alerts) => self.st.hold_usage_alerts(alerts),
            ev => self.st.usage.apply(ev),
        }
    }

    // ------------------------------------------------------------ Oyenza

    /// How the Oyenza worker should pace itself: in use (unlocked, displays
    /// awake) and the power source.
    fn oyenza_activity(&self) -> services::oyenza::Activity {
        let p = self.st.power;
        services::oyenza::Activity {
            in_use: !self.away.0 && !self.away.1,
            on_ac: p.on_ac || !p.has_battery,
            saver: p.saver,
            watching: self.oyenza_watch,
            today: self.today_watch,
            health: self.st.health_on(),
        }
    }

    /// The Oyenza page came on screen or went: the worker keeps its list live only meanwhile.
    fn set_oyenza_watch(&mut self, on: bool) {
        if on != self.oyenza_watch {
            self.oyenza_watch = on;
            self.tell_oyenza_activity();
        }
    }

    /// The Stocks page asks oyenza.trade for prices only while it is on
    /// screen and someone is at the Mac (`pages::stocks::watch`); the worker
    /// starts the first time it opens.
    fn set_stocks_watch(&mut self) {
        let away = self.away.0 || self.away.1;
        let open = self.mode == Mode::Expanded(PageKind::Stocks) && !away;
        // Closed, and the worker already told so: nothing to do, every frame.
        if !open && !self.stocks_open {
            return;
        }
        self.stocks_open = open;
        let w = pages::stocks::watch(&self.st, open);
        if self.stocks.is_none() && w.open {
            match services::stocks::StocksService::start(self.sink.clone()) {
                Ok(s) => self.stocks = Some(s),
                Err(e) => pwarn!("service stocks: unavailable ({e})"),
            }
        }
        if let Some(s) = &self.stocks {
            s.set_watch(w);
        }
    }

    /// The Today page came on screen or went: the worker asks for the day only meanwhile.
    fn set_today_watch(&mut self, on: bool) {
        if on != self.today_watch {
            self.today_watch = on;
            self.tell_oyenza_activity();
        }
    }

    /// The Health page came on screen: the steps from the phone, now (unless
    /// just asked for). Never signed in: nothing starts.
    fn set_health_shown(&mut self, on: bool) {
        if on != self.health_shown {
            self.health_shown = on;
            if let (true, Some(o)) = (on, &self.oyenza) {
                o.send(OyenzaCmd::RefreshHealth);
            }
        }
    }

    /// The IELTS, Notes or Add-ons page came on screen: whether the account has
    /// Studio Pro, now (unless just asked). Never signed in: nothing starts.
    fn set_pro_shown(&mut self, on: bool) {
        if on != self.pro_shown {
            self.pro_shown = on;
            if let (true, Some(o)) = (on, &self.oyenza) {
                o.send(OyenzaCmd::CheckPro);
            }
        }
    }

    fn tell_oyenza_activity(&self) {
        if let Some(o) = &self.oyenza {
            o.set_activity(self.oyenza_activity());
        }
    }

    fn start_oyenza(&mut self) {
        match services::oyenza::OyenzaService::start(self.sink.clone(), self.oyenza_activity()) {
            Ok(o) => self.oyenza = Some(o),
            Err(e) => pwarn!("oyenza: unavailable ({e})"),
        }
    }

    /// A sign-in, cancel, disconnect, note or undo for the worker (started by the first one).
    fn oyenza(&mut self, c: OyenzaCmd) {
        if self.oyenza.is_none() {
            self.start_oyenza();
        }
        if let Some(o) = &self.oyenza {
            o.send(c);
        }
    }

    fn on_oyenza(&mut self, ev: OyenzaEvent) {
        let now = self.st.now;
        let link_before = self.st.oyenza.link.clone();
        let pro_before = self.st.studio_pro() == Some(true);
        match ev {
            OyenzaEvent::Restored { account } => {
                self.st.oyenza.link = OyenzaLink::Connected { account };
                self.ask_pro();
            }
            OyenzaEvent::Connecting { code, url, expires_in_s } => {
                self.st.oyenza.link = OyenzaLink::Connecting { code, url, until: now + expires_in_s as f64 };
                // Keep the code on screen while the person goes to oyenza.app.
                if self.st.oyenza.pinned_before.is_none() {
                    self.st.oyenza.pinned_before = Some(self.st.pinned);
                }
                self.st.pinned = true;
                self.set_page(PageKind::Oyenza);
                self.expand();
            }
            OyenzaEvent::Connected { account } => {
                self.end_sign_in();
                self.st.notify(Icon::Check, palette::GREEN, "Connected to Oyenza", format!("as {account}"), 6.0, Some(NoticeAction::OpenPage(PageKind::Oyenza)));
                self.st.oyenza.link = OyenzaLink::Connected { account };
                self.ask_pro();
                // Signed in from the Today page: back to it, where the day comes in.
                if std::mem::take(&mut self.st.oyenza.from_today) {
                    self.st.page = PageKind::Today;
                    self.expand();
                }
            }
            OyenzaEvent::ConnectFailed(code) => {
                self.st.oyenza.from_today = false;
                self.end_sign_in();
                self.st.oyenza.link = OyenzaLink::Off;
                let (title, subtitle) = oyenza::connect_failure_text(&code);
                self.st.notify(Icon::Unlink, palette::ORANGE, title, subtitle, 6.0, None);
            }
            OyenzaEvent::ConnectCancelled => {
                self.st.oyenza.from_today = false;
                self.end_sign_in();
                self.st.oyenza.link = OyenzaLink::Off;
            }
            OyenzaEvent::Money { arrived } => {
                let o = &mut self.st.oyenza;
                o.updated_unix = Some(self.st.unix);
                let fresh = oyenza::unseen(&mut o.shown, &arrived);
                if let Some(n) = oyenza::money_notice(&fresh) {
                    let (icon, color) = match n.flow {
                        Some(oyenza::Flow::In) => (Icon::MoneyIn, palette::GREEN),
                        Some(oyenza::Flow::Out) => (Icon::MoneyOut, palette::BLUE),
                        _ => (Icon::Wallet, palette::BLUE),
                    };
                    self.oyenza_notice(icon, color, n.title, n.shown, Some(n.hidden));
                }
            }
            OyenzaEvent::Activity(a) => {
                let o = &mut self.st.oyenza;
                o.seen.note(&a.days, now);
                o.activity = Some(a);
                o.updated_unix = Some(self.st.unix);
            }
            OyenzaEvent::ActivityChecked => self.st.oyenza.updated_unix = Some(self.st.unix),
            OyenzaEvent::Bills { today, bills } => {
                let day = if today.is_empty() { self.st.day_key() } else { today };
                self.st.oyenza.bills = bills;
                self.st.oyenza.bills_today = day.clone();
                self.st.oyenza.updated_unix = Some(self.st.unix);
                self.remind_bills(&day);
            }
            OyenzaEvent::Day(day) => {
                let o = &mut self.st.oyenza;
                o.agenda = Some(day);
                o.agenda_unix = Some(self.st.unix);
                o.agenda_state = crate::state::AgendaState::Ready;
            }
            OyenzaEvent::DayUnavailable => {
                self.st.oyenza.agenda = None;
                self.st.oyenza.agenda_state = crate::state::AgendaState::Unavailable;
            }
            OyenzaEvent::DayFailed => self.st.oyenza.agenda_state = crate::state::AgendaState::Failed,
            OyenzaEvent::Pro(pro) => self.st.oyenza.pro = Some(pro),
            // Steps asked for before Health was switched off stay out.
            OyenzaEvent::Health(next) if self.st.health_on() => {
                if let Some(steps) = self.st.take_health(next) {
                    let notice = pages::health::goal_notice(steps, now + oyenza::NOTICE_S);
                    self.st.show_or_hold(notice, self.fullscreen_hidden);
                }
            }
            OyenzaEvent::Health(_) => {}
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
                self.st.health = None;
                // Signed out: an IELTS session ends, and the ladder was the account's.
                self.ielts_off();
                let (title, subtitle) =
                    if by_server { ("Oyenza disconnected", "Sign in again in Settings") } else { ("Disconnected from Oyenza", "Sign in again any time in Settings") };
                self.st.notify(Icon::Unlink, palette::TEXT_DIM, title, subtitle, 6.0, None);
            }
        }
        // The Oyenza tab came or went: the menu changes with it, and the
        // window makes room for its page (or no longer has to).
        if self.st.oyenza.link != link_before {
            self.refresh_menu();
            self.layout_surface();
        }
        if (self.st.studio_pro() == Some(true)) != pro_before {
            self.pro_changed();
        }
    }

    /// Signed in (at start, or just now) with the Downloader switched on:
    /// whether the account has Studio Pro, now, so its tab doesn't wait for
    /// the Add-ons page to open (`pages::addons::pro_wanted`).
    fn ask_pro(&self) {
        if let (true, Some(o)) = (pages::addons::pro_wanted(&self.st), &self.oyenza) {
            o.send(OyenzaCmd::CheckPro);
        }
    }

    /// Studio Pro came, or went (the server's word, or signing out): the
    /// Downloader's downloads stop as it goes, and its files come with it
    /// (`AddonService::set_pro`). Its tab follows by itself (`downloader_on`),
    /// and its page on screen leaves for Add-ons (`tick`).
    fn pro_changed(&mut self) {
        let pro = self.st.studio_pro() == Some(true);
        if !pro {
            self.downloads_off();
        }
        if let Some(a) = self.addons.as_ref() {
            a.set_pro(pro);
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

    /// A money or bill notice. Its sum waits for Show unless the menu's
    /// "Show amounts on the island" is on (`hidden` None: nothing to hide).
    /// While a full-screen app hides the island it waits, and shows when the
    /// island is back if it isn't old by then.
    fn oyenza_notice(&mut self, icon: Icon, color: Color, title: String, shown: String, hidden: Option<String>) {
        let now = self.st.now;
        let hide = hidden.is_some() && !self.st.settings.oyenza_show_amounts;
        let notice = Notice {
            icon,
            color,
            title,
            title_dim: String::new(),
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

    /// One reminder per bill, the day before it (or on the day, if the Mac was
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

    // ------------------------------------------------------------ phone drop

    /// The QR code waits for a phone, or a phone waits for an answer (Shelf
    /// page only), or a reading is under way (Script page only).
    fn phone_holds(&self) -> bool {
        self.st.page_holds_island()
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
        // A new version's notice (or "Updated to") waits for the island to be free.
        let free = !(self.away.0 || self.away.1) && !self.expanded && self.st.notice.is_none();
        if self.updater.as_mut().is_some_and(|u| u.announce(&mut self.st, free)) {
            self.request_frame();
        }
        // The AI tab's heads-up notices wait the same way, one at a time, and
        // while a full-screen app hides the island.
        if self.st.show_usage_notice(free && !self.fullscreen_hidden, self.expanded && self.st.page == PageKind::Ai) {
            self.request_frame();
        }
        if let Some(ended) = self.st.pomodoro.tick(now) {
            pages::focus::on_phase_ended(&mut self.st, ended);
        }
        // The Script page: a reading pauses while its page isn't on screen,
        // and the island may close once nothing holds it. Whether Record
        // shows is asked as the page comes on screen.
        let shown = self.expanded && self.st.page == PageKind::Script && !self.away.0 && !self.away.1;
        if shown && !self.script_shown {
            self.st.recorder = services::recorder::installed();
        }
        self.script_shown = shown;
        if !shown && self.st.script.playing() {
            self.st.script.pause(now);
        }
        let held = self.st.script_holds_island();
        if self.script_held && !held {
            self.maybe_schedule_collapse();
        }
        self.script_held = held;
        self.prayer_tick();
        self.ielts_tick();
        self.notes_tick();
        self.check_recorder_alive(false);
        // Prayer times, System, Health, Stocks, Screen recorder, IELTS or
        // Downloader switched off: its tab is gone.
        let gone = match self.st.page {
            PageKind::Prayer => !self.st.prayer_on(),
            PageKind::System => !self.st.system_on(),
            PageKind::Health => !self.st.health_on(),
            PageKind::Stocks => !self.st.stocks_on(),
            PageKind::Record => !self.st.record_on(),
            PageKind::Ielts => !self.st.ielts_on(),
            PageKind::Downloads => !self.st.downloader_on(),
            PageKind::Notes => !self.st.notes_on(),
            _ => false,
        };
        if gone {
            self.st.page = PageKind::Addons;
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
            if target_mode == Mode::Expanded(PageKind::Downloads) {
                self.downloads_page_shown();
            }
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
        // Today's tiles need the readings only while they show (signed out it has the sign-in card, signed in the day).
        self.set_stats(self.mode == Mode::Expanded(PageKind::Today) && pages::today::shows_tiles(&self.st));
        self.set_sysmon(self.mode == Mode::Expanded(PageKind::System));
        self.set_oyenza_watch(self.mode == Mode::Expanded(PageKind::Oyenza));
        self.set_today_watch(self.mode == Mode::Expanded(PageKind::Today));
        self.set_health_shown(self.mode == Mode::Expanded(PageKind::Health));
        self.set_pro_shown(matches!(self.mode, Mode::Expanded(PageKind::Ielts | PageKind::Notes | PageKind::Addons)));
        self.set_stocks_watch();
        self.set_usage_watch();
        // The Pill theme floats the island below the top edge (below the menu
        // bar for what hangs under a notch).
        self.island.set_floating(self.gap_target(&self.mode));
        self.island.step(dt, now);
        // The island let go of the notch or docked again under a pointer that
        // didn't move: take the mouse there, or let it through, for where the
        // island is now (the smoke run moves its own pointer).
        if self.island.floating() != self.was_floating {
            self.was_floating = self.island.floating();
            if self.smoke.is_none() {
                if let Some((x, y)) = window::pointer() {
                    self.on_pointer(Point::new(x, y));
                }
            }
        }
        self.ensure_room();
        self.note_motion(retarget, mono, period, dt as f64);
        // macOS's glass follows the island's outline, set before the repaint.
        let glass_at = (self.look().material == Material::Glass)
            .then(|| glass::shape(self.island.body_rect_at(self.cx), self.island.current().r, self.screen.notch, self.island_hidden()))
            .flatten();
        glass::show(glass_at);

        let canvas_r = self.island.canvas_rect_at(self.cx).inset(-2.0, -2.0);
        let full = Rect::new(0.0, 0.0, self.surface.0, self.surface.1);
        // Floating at rest, only the content changes (the equalizer, a line,
        // the clock): redraw inside the body, and the shadow and hairline
        // around it stay as drawn. Whole DIPs and one more, so no pixel on its
        // edge is half redrawn. Anything else redraws the canvas.
        let still = self.island.floating() && self.island.settled() && self.outgoing.is_none() && self.page_anim.is_none() && self.prev_canvas == Some(canvas_r);
        let dirty = if still {
            let b = self.island.body_rect_at(self.cx).inset(-1.0, -1.0);
            let (x0, y0) = (b.x.floor(), b.y.floor());
            Rect::new(x0, y0, b.right().ceil() - x0, b.bottom().ceil() - y0)
        } else {
            union_rect(self.prev_canvas.unwrap_or(full), canvas_r)
        };
        self.prev_canvas = Some(canvas_r);
        Some(dirty)
    }

    /// Draw the island (called from the view's drawRect).
    fn draw(&mut self, ctx: &CGContext, dirty: Rect) {
        let started = self.smoke.is_some().then(std::time::Instant::now);
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
        let hidden = self.island_hidden();
        let look = self.look();
        // The Pill theme's island away from the edge, and how far.
        let (floating, detach) = (self.island.floating(), notch::detach(self.island.top(), notch));
        // The smoke run's pretend notch (CI's Macs have none), painted where a
        // MacBook's camera hides the island, so its pictures show what one shows.
        let pretend_notch = notch.filter(|_| self.smoke.is_some() && window::faking_notch()).map(|n| Rect::new(self.cx - n.w / 2.0, -12.0, n.w, n.h + 12.0));
        self.gfx.set_look(look);
        let mut hits = std::mem::take(&mut self.hits);
        hits.clear();
        let mut body_s = 0.0;
        let (repaint, anim, focus, live) = {
            let mut canvas = self.gfx.canvas(ctx);
            let body_started = started.map(|_| std::time::Instant::now());
            if look.material == Material::Glass {
                glass::draw_body(&mut canvas, body, geom.r, shoulder, notch, hidden, false);
            } else {
                let border = if hidden { None } else { Some(canvas.look().border) };
                if floating {
                    // Hidden until hover, the floating island fades out as it nears
                    // its sliver (one below the edge would show); the pointer test
                    // takes the mouse there, not pixels. Inside a notch it stays black.
                    let alpha = if hidden { ((geom.h - HIDDEN_SLIVER) / HIDDEN_FADE).clamp(0.0, 1.0) } else { 1.0 };
                    canvas.with_alpha(alpha, |c| notch::draw_pill_body(c, body, geom.r, shoulder, detach, palette::ISLAND, border));
                } else {
                    canvas.notch(body, geom.r, shoulder, palette::ISLAND, border);
                }
            }
            body_s = body_started.map_or(0.0, |t| t.elapsed().as_secs_f64());
            // Content scales about the top centre, where the island hangs
            // (floating, about the middle of the pill end of the morph).
            let pivot = if floating { pill::motion::pivot(body, self.island.target()) } else { Point::new(body.center().x, body.y) };
            if morphing {
                canvas.clip(body);
                if floating {
                    canvas.push_outline_mask(&notch::pill_outline(body, geom.r, shoulder, detach));
                } else {
                    canvas.push_shape_mask(body, geom.r, shoulder);
                }
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
                    let shrink = if floating { pill::motion::ghost_scale(geom, g, GHOST_MIN_SCALE) } else { (geom.w / g.w.max(1.0)).clamp(GHOST_MIN_SCALE, 1.0) };
                    let xf = scale_about(pivot, shrink);
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
                let drop_by = if floating { pill::motion::CONTENT_DROP } else { CONTENT_DROP };
                let xf = grow * Matrix3x2::translation(0.0, -drop_by * (1.0 - content_alpha));
                let (input, st) = (&mut self.input, &mut self.st);
                canvas.layered(union_rect(lay, body), content_alpha, xf, |c| draw_content(c, input, &mut hits, st, mode, lay, body, transition, now, notch))
            } else {
                draw_content(&mut canvas, &mut self.input, &mut hits, &mut self.st, mode, lay, body, transition, now, notch)
            };
            if morphing {
                canvas.pop_shape_mask();
                canvas.unclip();
            }
            if let Some(r) = pretend_notch {
                canvas.unthemed(|c| c.fill_rrect(r, 10.0, Color::black(1.0)));
            }
            out
        };
        self.hits = hits;
        self.frames_drawn += 1;
        if let Some(t) = started.filter(|_| !morphing) {
            self.cost.frames += 1;
            self.cost.draw += t.elapsed().as_secs_f64();
            self.cost.body += body_s;
        }

        // Unconsumed wheel over the header switches pages, one per whole notch.
        let wheel_left = self.input.wheel;
        if let Mode::Expanded(p) = self.mode {
            let top = body.y + self.strip();
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
        if self.settings_ui.as_ref().is_some_and(|s| s.view.dirty) {
            settings_host::redraw();
        }
        let moving = !self.island.settled()
            || d.anim
            || d.repaint
            || !self.st.cmds.is_empty()
            || self.outgoing.is_some()
            || self.page_anim.is_some();
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
                // A lyrics lookup waits on LRCLIB (at least a second between
                // requests); the Japanese readings add-on downloads 3.8 MB.
                let limit = match name {
                    "music-check" | "ja-auto" => 25.0,
                    "ja-get" => 75.0,
                    "ja-off" | "prayer-off" => 15.0,
                    // Prayer times downloads 1.9 MB; Ashar comes 2 seconds in;
                    // macOS answers where the Mac is (or asks the person).
                    "prayer-get" => 75.0,
                    "prayer-adzan" | "prayer-locate" => 10.0,
                    // The test player starts, then perl and the adapter.
                    "now-playing" => 20.0,
                    // The browser starts and its tab plays first.
                    "now-playing-browser" => 30.0,
                    // A window sliding into its own Space and out again.
                    "fullscreen-hide" => 15.0,
                    "fullscreen-stay" | "fullscreen-back" => 10.0,
                    _ => 6.0,
                };
                // The ear step: once the card floats, the pointer moves and comes
                // up on the ear, and the card must still be open a moment later
                // (a collapse would have run by then).
                let replaying = name == "notch-pill-ear"
                    && settled
                    && ready
                    && match sm.replayed {
                        None => {
                            if let Some(p) = self.smoke_point {
                                self.on_pointer(p);
                                let _ = self.on_view_input(ViewInput::Up { x: p.x, y: p.y });
                            }
                            sm.replayed = Some(now);
                            true
                        }
                        Some(at) => now - at < 0.4,
                    };
                if ((settled && ready) || now - t0 > limit) && !replaying {
                    if !settled {
                        sm.note(name, false, format!("the island did not settle within {limit:.0} s"));
                    } else {
                        let g = self.island.current();
                        sm.note(name, true, format!("{:?}, {:.0}x{:.0} DIP, {} frames drawn", self.mode, g.w, g.h, self.frames_drawn));
                    }
                    if let Some((check, ok, detail)) = self.smoke_check(name) {
                        sm.note(check, ok, detail);
                    }
                    // The Settings window's steps picture that window.
                    let rect = if name.starts_with("settings-window") && settings_host::is_open() { settings_host::frame_top_left() } else { window::frame_top_left() };
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
            // System is an add-on, off by default: on, as its row in Add-ons
            // does. Its tab comes, and the reader starts its light readings
            // for the kept minute while the page is still closed.
            "system-on" => {
                self.st.settings.set_addon(SYSTEM.id, true);
                self.st.cmds.push(Command::SaveSettings);
            }
            // The System page on this Mac's own readings, module by module.
            "open-system" => {
                self.st.ui.system_module = Module::Cpu;
                self.st.cmds.push(Command::SetPage(PageKind::System));
            }
            "system-gpu" => self.st.ui.system_module = Module::Gpu,
            "system-memory" => self.st.ui.system_module = Module::Memory,
            "system-drives" => self.st.ui.system_module = Module::Drives,
            "system-network" => self.st.ui.system_module = Module::Network,
            // Power shows on a Mac without a battery whose SMC reports the
            // whole Mac's draw (a virtual Mac's may not); the page stays on
            // Network otherwise.
            "system-power" => self.st.ui.system_module = Module::Power,
            "timer" => {
                self.leave();
                self.collapse();
                self.st.pomodoro.start(self.st.now);
            }
            "notice" => {
                self.st.notify(Icon::CheckCircle, self.st.accent, "Oyenza Studio is on", "Point at the island to open it", 30.0, None);
            }
            "volume-hud" => {
                self.st.notice = None;
                if !self.st.volume.known {
                    self.on_event(AppEvent::Volume { level: 0.5, muted: false, switched: false });
                }
                self.on_event(AppEvent::Volume { level: 0.64, muted: false, switched: false });
            }
            // CI's Macs have no headphones to connect: a device only this app
            // sees comes and goes (the device list listener must hear it), and
            // the pop-up a pair of AirPods would bring is shown on the window.
            "headphones" => {
                self.st.hud = None;
                let before = services::audio::list_changes();
                self.smoke_devices = Some((before, services::audio::smoke_device_change()));
                self.on_event(AppEvent::Bluetooth { name: "AirPods Pro".into(), connected: true, battery: None, audio: true });
            }
            "charging-hud" => {
                self.st.hud = None;
                self.st.power = crate::state::PowerState { has_battery: true, percent: 76, charging: false, on_ac: false, saver: false };
                self.on_event(AppEvent::Power(crate::state::PowerState { has_battery: true, percent: 76, charging: true, on_ac: true, saver: false }));
            }
            "shelf-drop" => {
                let paths = smoke_shelf_files();
                self.on_event(AppEvent::ShelfAdd(paths));
            }
            // CI posts a Spotify "now playing" notification when this is ready.
            "music-notify" => {
                self.collapse();
                self.st.hud = None;
            }
            "music-check" => {}
            // What a frame costs to draw at rest while music plays: Classic's
            // attached pill here, Pill's floating one (with its shadow) later.
            "music-cost" => self.cost = DrawCost::default(),
            "pill-music-cost" => {
                // The pointer from pill-hover leaves; the timer waits aside, so
                // the music pill shows alone (Pill's music and timer pill holds still).
                self.leave();
                self.smoke_timer = Some(self.st.pomodoro.clone());
                self.st.pomodoro.reset();
                self.cost = DrawCost::default();
            }
            // A made-up Japanese song: its lyrics go in the lyrics cache, as
            // if LRCLIB had answered before, and CI announces it as Spotify
            // would. Back in Classic first, as the menu step is, without the
            // pretend notch, with the pointer from notch-eyes gone and the
            // clock at rest again.
            "ja-notify" => {
                self.leave();
                self.st.settings.idle_style = IdleStyle::Clock;
                window::fake_notch(None);
                self.refresh_screen();
                self.on_menu(MenuCmd::SetTheme(Theme::Classic));
                self.collapse();
                self.st.hud = None;
                seed_japanese_lyrics();
            }
            // Read without the Japanese readings add-on, the song switches it
            // on by itself (the lyrics handler); the Music page shows it coming.
            "ja-auto" => {
                self.st.page = PageKind::Media;
                self.expand();
            }
            // The add-on downloads from GitHub, then the song is read again.
            "ja-get" => {}
            // The Add-ons tab, the "+" at the end of the row, with it on.
            "addons-tab" => self.st.cmds.push(Command::SetPage(PageKind::Addons)),
            // What a click on its row there does (pages/addons.rs).
            "ja-off" => {
                self.st.settings.set_addon(JAPANESE_READINGS.id, false);
                self.st.cmds.push(Command::SaveSettings);
            }
            // Prayer times from the Add-ons tab: its files download from
            // GitHub, for a place picked by hand (as the picker does).
            "prayer-get" => {
                self.st.settings.prayer.follow_location = false;
                self.st.settings.prayer.place = Some("jakarta".into());
                self.st.settings.set_addon(PRAYER_TIMES.id, true);
                self.st.cmds.push(Command::SaveSettings);
            }
            "prayer-page" => self.st.cmds.push(Command::SetPage(PageKind::Prayer)),
            // 5 minutes before Ashar on the prayer clock, then the countdown's
            // OK (what a click on it does). The next step's adzan still comes.
            "prayer-ok" => {
                let unix = clock::unix_now();
                if let Some(ashar) = self.st.prayer.site().and_then(|s| poni_core::prayer::day_times(&s, s.local_day(unix)).map(|t| t.ashar)) {
                    self.st.prayer.clock_shift = ashar - 5 * 60 - unix;
                    self.st.prayer.handled_until = Some(ashar - 5 * 60);
                }
                self.prayer_countdown_ok();
            }
            // Ashar comes 2 seconds from now on the prayer clock, with the adzan chosen.
            "prayer-adzan" => {
                self.st.settings.prayer.sound = PrayerSound::Adzan;
                self.st.cmds.push(Command::SaveSettings);
                let unix = clock::unix_now();
                if let Some((site, ashar)) = self.st.prayer.site().and_then(|s| poni_core::prayer::day_times(&s, s.local_day(unix)).map(|t| (s, t.ashar))) {
                    self.st.prayer.clock_shift = ashar - 2 - unix;
                    self.st.prayer.handled_until = Some(ashar - 2);
                    pinfo!("smoke: Ashar in {} is {}", site.hhmm(ashar), self.st.prayer.place.as_ref().map_or("", |p| p.label.as_str()));
                }
            }
            // Stop in the notice; the prayer clock goes back to the Mac's.
            "prayer-stop" => {
                self.run_inline_action(InlineAction::StopAdzan);
                self.st.prayer.clock_shift = 0;
                self.st.prayer.handled_until = Some(clock::unix_now());
            }
            // "Use my location" in the place picker: macOS is asked.
            "prayer-locate" => {
                self.st.settings.prayer.follow_location = true;
                self.st.cmds.push(Command::SaveSettings);
            }
            "prayer-off" => {
                self.st.settings.set_addon(PRAYER_TIMES.id, false);
                self.st.cmds.push(Command::SaveSettings);
            }
            // A plain window goes full screen in a Space of its own (a second
            // copy of the app): with Hide, macOS leaves the island on the
            // desktop's Space, and the island knows.
            "fullscreen-hide" => {
                // From rest, with nothing to say.
                self.st.notice = None;
                self.collapse();
                self.st.settings.hide_in_fullscreen = true;
                self.st.cmds.push(Command::SaveSettings);
                match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(["--full-screen-window", "90"]).spawn()) {
                    Ok(child) => self.smoke_full_screen = Some(child),
                    Err(e) => pwarn!("smoke: no full-screen window ({e})"),
                }
            }
            // Stay on top: the island joins the full-screen app's Space.
            "fullscreen-stay" => {
                self.st.settings.hide_in_fullscreen = false;
                self.st.cmds.push(Command::SaveSettings);
            }
            // Hide again, and the full-screen app quits: the island is back.
            "fullscreen-back" => {
                self.st.settings.hide_in_fullscreen = true;
                self.st.cmds.push(Command::SaveSettings);
                if let Some(mut c) = self.smoke_full_screen.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
            // Oyenza sign-in: its worker starts (this home has no key, so it
            // asks nothing), one POST goes through the Mac's HTTP client the
            // way signing in sends it, then the island follows what the worker
            // would report: a code, a week on the page, money in, signing out.
            "oyenza-post" => {
                self.st.notice = None;
                self.collapse();
                self.start_oyenza();
                self.smoke_post = Some(smoke_post());
            }
            "oyenza-code" => {
                let url = "https://oyenza.app/connect?code=KPJX-TMNQ".to_string();
                self.on_oyenza(OyenzaEvent::Connecting { code: "KPJX-TMNQ".into(), url, expires_in_s: 600 });
            }
            "oyenza-page" => {
                let fake = crate::render::fake_oyenza(&self.st, false);
                self.on_oyenza(OyenzaEvent::Connected { account: "a\u{2022}\u{2022}\u{2022}@example.com".into() });
                if let Some(a) = fake.activity {
                    self.on_oyenza(OyenzaEvent::Activity(a));
                }
                self.on_oyenza(OyenzaEvent::Bills { today: fake.bills_today, bills: fake.bills });
                self.st.notice = None;
                self.set_page(PageKind::Oyenza);
                self.expand();
            }
            "oyenza-money" => {
                self.collapse();
                let wallet = Some(poni_core::oyenza::WalletRef { name: "BCA".into(), color: "#0060AF".into() });
                let arrived = vec![poni_core::oyenza::Arrival {
                    id: "smoke-1".into(),
                    kind: "income".into(),
                    amount: 1_250_000.0,
                    currency: "IDR".into(),
                    text: "Rp 1.250.000".into(),
                    title: "Gaji Oktober".into(),
                    wallet,
                    at: String::new(),
                }];
                self.on_oyenza(OyenzaEvent::Money { arrived });
            }
            "oyenza-off" => {
                self.st.notice = None;
                self.on_oyenza(OyenzaEvent::Disconnected { by_server: false });
                self.oyenza = None;
            }
            // The AI tab (its worker never starts in the smoke run: nothing
            // of this Mac is read), then its keychain reader on test items.
            "ai-page" => {
                self.st.notice = None;
                let now = self.st.now;
                pages::usage::picture(&mut self.st, pages::usage::Picture::Plans, now);
                self.set_page(PageKind::Ai);
                self.expand();
            }
            // With the notices on and a keychain reading in hand (the tab is
            // open), the Mac locks, then unlocks on the AI tab.
            "ai-locked" => {
                self.st.settings.usage.notices = true;
                // The reader's gate as the island has it again (the keychain
                // step set its own, and a resting island draws no frame).
                self.set_usage_watch();
                if std::env::var_os("PONI_SMOKE_KEYCHAIN").is_some() {
                    let _ = services::keychain::read(SMOKE_KEYCHAIN.0, Some("plain"));
                }
                self.set_away(None, Some(true));
            }
            "ai-back" => {
                self.set_away(None, Some(false));
                self.set_page(PageKind::Ai);
                self.expand();
            }
            // macOS's own now playing, as a browser tab reaches it: the
            // stream stops, tools/mac-smoke.sh starts the MediaRemote
            // adapter's test player, and sound (CI's Macs play none) starts
            // a fresh stream, which reads it. Last of the music steps: once
            // the service answers, it speaks for Spotify too.
            "now-playing-start" => {
                self.collapse();
                services::media::smoke_rest();
            }
            "now-playing" => services::media::smoke_sound(),
            // A real browser tab, as YouTube in Edge reaches macOS's own now
            // playing: the stream stops, tools/mac-smoke.sh stops the test
            // player and opens the runner's Edge (or Chrome) on a page that
            // plays a tone with a title and an artist, the way YouTube names
            // its video, and sound starts a fresh stream, which reads it.
            "now-playing-browser-start" => {
                self.collapse();
                services::media::smoke_rest();
            }
            "now-playing-browser" => services::media::smoke_sound(),
            // The themes on the real window: Glass, then Oyen and Glass
            // around a notch (one the screen pretends to have; CI's Macs
            // have none), open and resting in the menu bar's line.
            "glass-open" => {
                self.on_menu(MenuCmd::SetTheme(Theme::Glass));
                self.st.page = PageKind::Today;
                self.expand();
            }
            "glass-collapsed" | "notch-oyen-collapsed" | "pill-collapsed" => self.collapse(),
            // Around the notch Pill's music floats below the menu bar even with
            // no line to sing (as in an intro, a break or a pause): the song
            // plays on with its lyrics dropped, and no timer.
            "notch-pill-music" => {
                self.st.pomodoro.reset();
                self.st.lyrics = LyricsState::default();
                self.collapse();
            }
            // Around the notch Pill rests inside it: with nothing playing and
            // no timer, only the notch shows.
            "notch-pill-collapsed" => {
                self.st.pomodoro.reset();
                self.st.media = None;
                self.st.media_paused_at = None;
                self.st.lyrics = LyricsState::default();
                self.collapse();
            }
            // The pointer rests on the notch and the card opens below the menu
            // bar (as after the dwell); once it floats, the pointer moves and
            // comes up on the notch again, and the card must stay open
            // (smoke_step).
            "notch-pill-ear" => {
                if let Some(n) = self.screen.notch {
                    let p = Point::new(self.cx - n.w / 4.0, n.h / 2.0);
                    self.smoke_point = Some(p);
                    self.on_pointer(p);
                    self.cancel_hover();
                    self.expand();
                }
            }
            // The Eyes look around the notch: the pointer goes off far left of
            // the camera, and the eyes cross over behind it and peek out there.
            "notch-eyes" => {
                self.leave();
                self.st.settings.idle_style = IdleStyle::Eyes;
                self.collapse();
                if let Some(n) = self.screen.notch {
                    let p = Point::new(self.cx - n.w / 2.0 - 260.0, 140.0);
                    self.smoke_point = Some(p);
                    self.on_pointer(p);
                }
            }
            // Pill floats below the top edge, open and resting; around the
            // notch it rests inside it and opens below it.
            "pill-open" | "notch-pill-open" => {
                self.on_menu(MenuCmd::SetTheme(Theme::Pill));
                self.st.page = PageKind::Today;
                self.expand();
            }
            // The pointer in the gap above the resting pill: it hovers (the
            // pill swells), without the dwell that would open it.
            "pill-hover" => {
                self.on_pointer(Point::new(self.cx, pill::TOP_GAP / 2.0));
                self.cancel_hover();
            }
            "notch-oyen-open" => {
                // The pointer from pill-hover leaves first, and the timer set
                // aside for pill-music-cost comes back.
                self.leave();
                if let Some(p) = self.smoke_timer.take() {
                    self.st.pomodoro = p;
                }
                window::fake_notch(Some(Notch::MACBOOK));
                self.refresh_screen();
                self.on_menu(MenuCmd::SetTheme(Theme::Oyen));
                self.st.page = PageKind::Today;
                self.expand();
            }
            "notch-glass-open" => {
                self.on_menu(MenuCmd::SetTheme(Theme::Glass));
                self.st.page = PageKind::Today;
                self.expand();
            }
            // Oyenza Studio Recorder's word through the island's message port,
            // sent from another process as the recorder sends it
            // (tools/mac-smoke.sh), around a notch: the recording rests in its
            // ears; then its end.
            "record-port" => {
                self.leave();
                window::fake_notch(Some(Notch::MACBOOK));
                self.refresh_screen();
                self.on_menu(MenuCmd::SetTheme(Theme::Classic));
                self.st.hud = None;
                self.st.notice = None;
                self.collapse();
            }
            "record-check" | "record-stop" | "record-gone" => {}
            // The Settings window, from the gear's command: dark, then light, then closed.
            "settings-window" => {
                self.leave();
                self.collapse();
                self.st.cmds.push(Command::OpenSettings);
            }
            "settings-window-light" => {
                settings_host::force_appearance(true);
                if let Some(s) = self.settings_ui.as_mut() {
                    s.view.show_section(Section::Appearance);
                }
                settings_host::redraw();
            }
            "settings-window-closed" => self.pending.push(Deferred::CloseSettings),
            // The IELTS coach's voice (services::voice): a short tone as a
            // 16 kHz WAV, then, when CI passes one, a short mp3 like the
            // server's, each to its end.
            "ielts-voice" | "ielts-voice-mp3" => {
                let file = if name == "ielts-voice" {
                    let p = paths::data_dir().join("ielts").join("smoke-voice.wav");
                    smoke_tone_wav(&p, 0.5).map(|()| Some(p)).map_err(|e| e.to_string())
                } else {
                    Ok(std::env::var_os("PONI_SMOKE_MP3").map(PathBuf::from))
                };
                let started = clock::mono();
                self.smoke_voice = match file {
                    Ok(Some(file)) => {
                        let mut player = services::voice::VoicePlayer::default();
                        let played = player.play(&file, &self.sink);
                        Some(SmokeVoice { player, file, started, played, ended: None })
                    }
                    Ok(None) => None,
                    Err(e) => Some(SmokeVoice { player: Default::default(), file: PathBuf::new(), started, played: Err(e), ended: None }),
                };
            }
            // Its microphone (services::mic): whatever this Mac allows (a
            // microphone or none; macOS's yes, no or a question nobody
            // answers), start answers at once. The stop step comes after.
            "ielts-mic" => {
                self.smoke_voice = None;
                let out = paths::data_dir().join("ielts").join("smoke-answer.wav");
                self.smoke_mic = Some(services::mic::Mic::start(&out, &self.sink));
            }
            "ielts-mic-stop" => self.smoke_mic_stop = Some(self.smoke_mic_stopped()),
            // Meeting notes' recording (services::notes with capture.rs): the
            // microphone and the Mac's own sound through a Core Audio tap, for
            // two seconds, whatever this Mac allows; the stop step follows.
            "notes-capture" => {
                let started = self.st.local;
                self.smoke_notes = Some(services::notes::Recording::start(&paths::notes_audio_dir(), &started, &self.sink));
            }
            "notes-capture-stop" => self.smoke_notes_stop = Some(self.smoke_notes_stopped()),
            "menu" => {
                // The pointer from notch-pill-ear leaves.
                self.leave();
                window::fake_notch(None);
                self.refresh_screen();
                self.on_menu(MenuCmd::SetTheme(Theme::Classic));
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
            // Signed out, Today shows its sign-in card (no stat tiles, no readings).
            "open-today" => waited > 0.5,
            // Full readings list the apps (their CPU from the second one on);
            // the apps' icons follow from Finder.
            "system-on" => services::sysmon::cost()[0].0 > 0 || waited > 5.0,
            "open-system" => (self.st.system.latest.as_ref().is_some_and(|r| !r.top_cpu.is_empty()) && waited > 1.6) || waited > 5.0,
            "system-gpu" | "system-memory" | "system-drives" | "system-network" => waited > 0.6,
            "system-power" => waited > 1.3,
            "shelf-drop" => self.st.shelf_thumbs.values().filter(|t| t.is_some()).count() >= 3 || waited > 5.0,
            "music-check" => {
                let looked_up = !matches!(self.st.lyrics.status, LyricsStatus::Idle | LyricsStatus::Loading);
                (self.st.media.is_some() && looked_up) || waited > 20.0
            }
            "ja-auto" => {
                let l = &self.st.lyrics;
                let playing = self.st.media.as_ref().is_some_and(|m| m.track_key == l.track_key && m.title == SMOKE_JA_TRACK.0);
                (playing && l.status == LyricsStatus::Found && waited > 0.5) || waited > 20.0
            }
            // The files in place and the song read again with them.
            "ja-get" => match self.st.addon(JAPANESE_READINGS.id) {
                AddonStatus::Ready => (!self.st.lyrics.wants_ja_readings && self.st.lyrics.status == LyricsStatus::Found) || waited > 60.0,
                AddonStatus::Failed => true,
                _ => waited > 60.0,
            },
            "ja-off" => (self.st.addon(JAPANESE_READINGS.id) == AddonStatus::Off && !ja_folder().exists() && self.addons.is_none()) || waited > 10.0,
            "prayer-get" => match self.st.addon(PRAYER_TIMES.id) {
                AddonStatus::Ready => self.st.prayer_on() || waited > 60.0,
                AddonStatus::Failed => true,
                _ => waited > 60.0,
            },
            // The eyes are out on the camera's other side.
            "notch-eyes" => !self.st.ui.peek.along(clock::mono()).1 || waited > 3.0,
            // The countdown ticks at least once.
            "prayer-page" => waited > 1.5,
            "prayer-adzan" => self.st.prayer.adzan || waited > 6.0,
            "prayer-locate" => self.st.prayer.locate != Locate::Asking || waited > 6.0,
            "prayer-off" => (self.st.addon(PRAYER_TIMES.id) == AddonStatus::Off && !prayer_folder().exists() && self.addons.is_none()) || waited > 10.0,
            "headphones" => self.smoke_devices.as_ref().is_some_and(|(before, _)| services::audio::list_changes() > *before) || waited > 4.0,
            "fullscreen-hide" => self.fullscreen_hidden || waited > 12.0,
            "fullscreen-stay" => (!self.fullscreen_hidden && window::is_shown() && waited > 1.0) || waited > 6.0,
            "fullscreen-back" => (!self.front_full_screen && !self.fullscreen_hidden && window::is_shown() && waited > 1.0) || waited > 8.0,
            "oyenza-code" | "oyenza-page" => (self.expanded && self.st.page == PageKind::Oyenza && waited > 0.6) || waited > 4.0,
            "oyenza-money" => self.st.notice.is_some() || waited > 4.0,
            "ai-page" | "ai-back" => (self.expanded && self.st.page == PageKind::Ai && waited > 0.6) || waited > 4.0,
            "now-playing" => self.st.media.as_ref().is_some_and(|m| m.title == SMOKE_SERVICE_TRACK) || std::env::var_os("PONI_SMOKE_NOW_PLAYING").is_none() || waited > 15.0,
            "now-playing-browser" => self.st.media.as_ref().is_some_and(|m| m.title == SMOKE_BROWSER_TRACK) || std::env::var_os("PONI_SMOKE_BROWSER").is_none() || waited > 25.0,
            "menu" => waited > 1.0,
            // The line plays to its end (half a second).
            "ielts-voice" | "ielts-voice-mp3" => self.smoke_voice.as_ref().is_none_or(|v| v.ended.is_some() || v.played.is_err()) || waited > 5.0,
            // A second and a half of whatever the microphone hears.
            "ielts-mic" => waited > 1.5,
            // Two seconds of a meeting.
            "notes-capture" => waited > 2.0,
            "settings-window" | "settings-window-light" => (settings_host::is_open() && waited > 0.8) || waited > 5.0,
            "settings-window-closed" => !settings_host::is_open() || waited > 3.0,
            "record-check" => self.mode == Mode::Activity(activities::Activity::Recording) || waited > 5.0,
            "record-gone" => self.st.rec.phase.is_none() || waited > 5.0,
            // About two seconds of frames at rest.
            "music-cost" | "pill-music-cost" => waited > 2.5,
            _ => true,
        }
    }

    /// The Pill theme on the real window: where the island sits, and which
    /// points take the mouse. Without a notch it floats `TOP_GAP` below the
    /// top edge and the gap above it takes the mouse; around a notch it rests
    /// inside the notch (the eyes peek out beside it) and the music pill and
    /// the open card float below it, where the gap and the notch take the
    /// mouse and the menu bar beside the notch doesn't.
    fn pill_report(&self) -> (&'static str, bool, String) {
        let body = self.island.body_rect_at(self.cx);
        let look = *self.look() == poni_core::look::PILL && !glass::shown();
        let at = |x: f32, y: f32| self.hovering_inside(Point::new(x, y));
        let (ok, place) = match self.screen.notch {
            None => {
                let gap_takes = at(self.cx, pill::TOP_GAP / 2.0);
                let ok = self.island.floating() && (self.island.top() - pill::TOP_GAP).abs() < 0.01 && gap_takes;
                (ok, format!("floating {:.0} DIP below the top edge, the gap above it takes the mouse: {gap_takes}", self.island.top()))
            }
            Some(n) if self.expanded || notch::floats(&self.mode, &self.st) => {
                let gap_takes = at(self.cx, n.h + pill::TOP_GAP / 2.0);
                let notch_takes = at(self.cx, n.h / 2.0);
                let bar_free = !at(self.cx - n.w / 2.0 - 24.0, n.h / 2.0);
                let ok = self.island.floating() && (self.island.top() - n.h - pill::TOP_GAP).abs() < 0.01 && gap_takes && notch_takes && bar_free;
                let place = format!(
                    "floating {:.0} DIP below the top edge ({:.0} below a {:.0}x{:.0} notch); the gap takes the mouse: {gap_takes}, \
                     the notch: {notch_takes}, the menu bar beside it stays clickable: {bar_free}",
                    self.island.top(),
                    self.island.top() - n.h,
                    n.w,
                    n.h
                );
                (ok, place)
            }
            // At rest: back inside the notch, or the eyes beside it.
            Some(n) => {
                let fit = notch::fit(&self.mode, &self.st);
                let want = if self.st.settings.idle_style == IdleStyle::Eyes { Fit::Ears } else { Fit::Inside };
                let ok = self.mode == Mode::Idle && !self.island.floating() && body.y == 0.0 && fit == want;
                (ok, format!("resting in the menu bar's line ({fit:?} around a {:.0}x{:.0} notch)", n.w, n.h))
            }
        };
        let g = self.island.current();
        (
            "pill-theme",
            ok && look,
            format!("{:?}, {:.0}x{:.0} DIP r {:.0}, {place}; Pill's look: {look}", self.mode, g.w, g.h, g.r),
        )
    }

    /// What each new part of the app did on a real Mac.
    fn smoke_check(&self, name: &str) -> Option<(&'static str, bool, String)> {
        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
        match name {
            "settings-window" => {
                let open = settings_host::is_open() && self.settings_ui.is_some();
                let at = settings_host::frame_top_left();
                Some(("settings-window-open", open, format!("window {at:?}, {} mode", if settings_host::is_light() { "light" } else { "dark" })))
            }
            "settings-window-light" => Some(("settings-window-light", settings_host::is_light(), format!("section {:?}", self.settings_ui.as_ref().map(|s| s.view.section)))),
            "settings-window-closed" => {
                let gone = !settings_host::is_open() && self.settings_ui.is_none();
                Some(("settings-window-closed", gone, format!("window {}, content {}", if settings_host::is_open() { "still open" } else { "gone" }, if self.settings_ui.is_some() { "kept" } else { "dropped" })))
            }
            "record-check" | "record-gone" => {
                let showing = self.mode == Mode::Activity(activities::Activity::Recording);
                let recording = self.st.rec.phase == Some(poni_core::recorder::Phase::Recording);
                let g = self.island.current();
                let d = format!(
                    "{:?}, {:.0}x{:.0} DIP, recorder said {:?} at {}, Record tab {}, notch {:?}",
                    self.mode,
                    g.w,
                    g.h,
                    self.st.rec.phase,
                    poni_core::recorder::fmt_length(self.st.rec_elapsed()),
                    if self.st.record_on() { "on" } else { "off" },
                    self.screen.notch
                );
                if name == "record-check" {
                    Some(("recording-pill", showing && recording, d))
                } else {
                    Some(("recording-ended", !showing && self.st.rec.phase.is_none(), d))
                }
            }
            // Signed out, Today shows the sign-in card, so its tiles' once-a-second
            // readings stay off; the readings still work for when the tiles show
            // (a server without the day): one taken here.
            "open-today" => {
                let s = services::sysstats::sample();
                let ok = s.valid && s.mem_total > 0 && s.disk_total > 0 && s.uptime_s > 0 && !self.stats_on && !pages::today::shows_tiles(&self.st);
                let d = format!(
                    "one reading: memory {:.1} of {:.1} GiB, disk {:.0} GiB free of {:.0}, up {} min; Today's readings {} (sign-in card)",
                    gib(s.mem_used),
                    gib(s.mem_total),
                    gib(s.disk_free),
                    gib(s.disk_total),
                    s.uptime_s / 60,
                    if self.stats_on { "on" } else { "off" },
                );
                Some(("system-stats", ok, d))
            }
            "system-on" => {
                let tab = pages::tabs(&self.st).contains(&PageKind::System);
                let [(light, _), _] = services::sysmon::cost();
                let ok = tab && light > 0 && self.sysmon.is_some();
                Some(("system-addon", ok, format!("System tab: {tab}; light readings with the page closed: {light}")))
            }
            "open-system" => {
                let (ok, d) = system_report(&self.st);
                Some(("system-readings", ok, d))
            }
            "system-network" => {
                // What the readings cost the sampler's thread so far.
                let [(ln, light), (fnum, full)] = services::sysmon::cost();
                let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
                let d = format!("light reading {:.2} ms of CPU (average of {ln}), full reading {:.2} ms of CPU (average of {fnum})", ms(light), ms(full));
                Some(("system-cost", ln > 0 && fnum > 0, d))
            }
            // What this Mac's SMC says, and whether the page has it. Only a
            // reading that isn't a number of watts fails: a Mac without the
            // key (virtual, Intel) has no Power module.
            "system-power" => {
                let smc = services::smc::Smc::open();
                let w = smc.as_ref().and_then(|s| s.float(services::smc::SYSTEM_TOTAL));
                let page = self.st.system.latest.as_ref().and_then(|r| r.power.as_ref()).map(|p| p.total_w);
                let ok = w.is_none_or(|w| w.is_finite() && w >= 0.0);
                let d = format!("SMC {}; the whole Mac's draw: {w:?} W; on the page: {page:?} W", if smc.is_some() { "open" } else { "not available" });
                Some(("system-power", ok, d))
            }
            "volume-hud" => {
                let ok = matches!(self.st.hud.as_ref().map(|h| &h.kind), Some(HudKind::Volume { .. }));
                Some(("volume-hud", ok, format!("output device: {:?}", services::audio::current())))
            }
            "headphones" => {
                let popup = matches!(self.st.hud.as_ref().map(|h| &h.kind), Some(HudKind::Bluetooth { connected: true, .. }));
                let (before, made) = self.smoke_devices.clone().unwrap_or((0, Err(0)));
                let after = services::audio::list_changes();
                let ok = popup && made.is_ok() && after > before;
                Some(("headphones", ok, format!("pop-up shown: {popup}; device list changes heard: {before} -> {after} (test device: {made:?})")))
            }
            "charging-hud" => {
                let ok = matches!(self.st.hud.as_ref().map(|h| &h.kind), Some(HudKind::Charging { .. }));
                Some(("charging-hud", ok, format!("this Mac's power: {:?}", services::power::query())))
            }
            "shelf-drop" => {
                let made = self.st.shelf_thumbs.values().filter(|t| t.is_some()).count();
                Some(("shelf-thumbnails", made >= 3, format!("{made} of {} tiles drawn", self.st.shelf_thumbs.len())))
            }
            "glass-open" | "notch-glass-open" => {
                let notch = self.screen.notch.map_or("no notch".to_string(), |n| format!("a {:.0}x{:.0} notch with a black brow", n.w, n.h));
                let what = glass::kind().unwrap_or("no glass view");
                Some(("glass-theme", glass::shown() && self.look().material == Material::Glass, format!("{what} under the open island, {notch}")))
            }
            "notch-oyen-open" | "notch-oyen-collapsed" => {
                let look = self.look();
                let ok = self.screen.notch.is_some() && *look == poni_core::look::OYEN_NIGHT && !glass::shown();
                Some(("oyen-around-notch", ok, format!("ground {:?}, ink {:?}, font {:?}", look.ground, look.ink, look.font)))
            }
            "pill-open" | "pill-collapsed" | "notch-pill-open" | "notch-pill-collapsed" => Some(self.pill_report()),
            "notch-pill-music" => {
                let (_, placed, d) = self.pill_report();
                let line = pill::lyric::current(&self.st).is_some();
                let ok = placed && self.mode == Mode::Activity(activities::Activity::Media) && !line;
                Some(("pill-music-floats", ok, format!("{d}; a line to sing: {line}")))
            }
            "notch-eyes" => {
                let Some(n) = self.screen.notch else { return Some(("eyes-around-notch", false, "no notch".into())) };
                let g = self.island.current();
                let fit = notch::fit(&self.mode, &self.st);
                let peek = self.st.ui.peek;
                let (along, moving) = peek.along(clock::mono());
                let ok = self.mode == Mode::Idle && fit == Fit::Ears && g.w > n.w + 40.0 && peek.left && along == -1.0 && !moving;
                let d = format!(
                    "{:?} {fit:?}, {:.0}x{:.0} DIP around a {:.0}x{:.0} notch; pointer at {:?} from its middle: the eyes peek out {} of the camera ({along:.2} of the way), looking {:?}",
                    self.mode,
                    g.w,
                    g.h,
                    n.w,
                    n.h,
                    peek.pointer.map(|p| (p.x.round(), p.y.round())),
                    if peek.left { "left" } else { "right" },
                    notch::peek_key(&self.st, n, clock::mono()).1
                );
                Some(("eyes-around-notch", ok, d))
            }
            "music-cost" | "pill-music-cost" => {
                let c = self.cost;
                let per = |s: f64| if c.frames > 0 { s / c.frames as f64 * 1e6 } else { 0.0 };
                let d = format!(
                    "{:?}{}, {} frames at rest: {:.0} us a frame in draw, {:.0} us of it the body; at 120 Hz {:.1} ms of CPU a second",
                    self.mode,
                    if self.island.floating() { " floating (body = fill, hairline and shadow)" } else { "" },
                    c.frames,
                    per(c.draw),
                    per(c.body),
                    per(c.draw) * 120.0 / 1000.0
                );
                Some(("draw-cost", c.frames > 0, d))
            }
            "now-playing" => {
                if std::env::var_os("PONI_SMOKE_NOW_PLAYING").is_none() {
                    return Some(("now-playing-service", true, "skipped: no test player on this run".into()));
                }
                let m = self.st.media.as_ref();
                let ok = m.is_some_and(|m| m.title == SMOKE_SERVICE_TRACK);
                let d = m.map_or("nothing".to_string(), |m| format!("{} by {} from {:?} ({})", m.title, m.artist, m.app_id, if m.is_playing() { "playing" } else { "paused" }));
                Some(("now-playing-service", ok, format!("macOS's now playing through the adapter: {d}")))
            }
            "now-playing-browser" => {
                if std::env::var_os("PONI_SMOKE_BROWSER").is_none() {
                    return Some(("now-playing-browser", true, "skipped: no Edge or Chrome on this run".into()));
                }
                let m = self.st.media.as_ref();
                let ok = m.is_some_and(|m| m.title == SMOKE_BROWSER_TRACK && poni_core::nowplaying::is_browser(&m.app_id));
                let d = m.map_or("nothing".to_string(), |m| format!("{} by {} in {} ({:?}, {})", m.title, m.artist, m.app_name, m.app_id, if m.is_playing() { "playing" } else { "paused" }));
                Some(("now-playing-browser", ok, format!("a browser tab in macOS's now playing: {d}")))
            }
            "notch-pill-ear" => {
                let armed = self.timers[Timer::Collapse as usize].is_some();
                let ok = self.expanded && self.hovering && !armed && self.island.floating();
                let d = format!(
                    "{:?} {:.0} DIP below the top, pointer on the notch at {:?}: hovering {}, collapse armed {armed}",
                    self.mode,
                    self.island.top(),
                    self.smoke_point.map(|p| (p.x.round(), p.y.round())),
                    self.hovering
                );
                Some(("pill-ear-stays-open", ok, d))
            }
            "pill-hover" => {
                let (now, rest) = (self.island.current(), activities::geom(&self.mode, &self.st));
                let want = pill::motion::hover_lift(rest);
                let top = pill::TOP_GAP + want.dy;
                let ok = self.hovering && (now.w, now.h, now.r) == (want.w, want.h, want.r) && (self.island.top() - top).abs() < 0.01;
                let d = format!(
                    "pointer in the gap above the pill: hovering {}, {:.0}x{:.0} DIP r {:.0} at {:.0} below the top edge (resting {:.0}x{:.0} at {:.0})",
                    self.hovering,
                    now.w,
                    now.h,
                    now.r,
                    self.island.top(),
                    rest.w,
                    rest.h,
                    pill::TOP_GAP
                );
                Some(("pill-hover", ok, d))
            }
            "music-check" => {
                let playing = self.st.media.as_ref().map_or("nothing".to_string(), |m| format!("{} by {} in {}", m.title, m.artist, m.app_name));
                let l = &self.st.lyrics;
                let lines = l.lyrics.as_ref().map_or(0, |x| x.lines.len());
                let synced = l.lyrics.as_ref().is_some_and(|x| x.synced);
                let ok = self.st.media.as_ref().is_some_and(|m| m.title == SMOKE_TRACK) && l.status == LyricsStatus::Found && lines > 0;
                Some(("now-playing-lyrics", ok, format!("{playing}; lyrics {:?} from {}: {lines} lines, synced {synced}", l.status, l.source)))
            }
            "ja-auto" => {
                let l = &self.st.lyrics;
                let (read, needing) = romaji_lines(l);
                let status = self.st.addon(JAPANESE_READINGS.id);
                let on = self.st.settings.addon_on(JAPANESE_READINGS.id);
                let ok = l.status == LyricsStatus::Found && on && status != AddonStatus::Off;
                let d = format!("lyrics {:?} from {}: {read} of {needing} lines with romaji; Japanese readings switched on by the song: {on} ({status:?})", l.status, l.source);
                Some(("ja-auto", ok, d))
            }
            "ja-get" => {
                let status = self.st.addon(JAPANESE_READINGS.id);
                let checked = |f: &poni_core::addons::AddonFile| {
                    std::fs::read(services::addons::file_path(&JAPANESE_READINGS, f.name)).is_ok_and(|b| f.matches(&b))
                };
                let files: Vec<String> = JAPANESE_READINGS.files.iter().map(|f| format!("{} {}", f.name, if checked(f) { "checked" } else { "missing or different" })).collect();
                let (read, needing) = romaji_lines(&self.st.lyrics);
                let ok = status == AddonStatus::Ready && JAPANESE_READINGS.files.iter().all(checked) && read == needing && !self.st.lyrics.wants_ja_readings;
                Some(("ja-readings-addon", ok, format!("{status:?}: {}; read again: {read} of {needing} lines with romaji", files.join(", "))))
            }
            "addons-tab" => {
                let tabs = pages::tabs(&self.st);
                let status = self.st.addon(JAPANESE_READINGS.id);
                let ok = self.st.page == PageKind::Addons && tabs.last() == Some(&PageKind::Addons) && status == AddonStatus::Ready;
                let d = format!("{} tabs, the last {:?}; Japanese readings: {}", tabs.len(), tabs.last(), status.line(&JAPANESE_READINGS, true));
                Some(("addons-tab", ok, d))
            }
            "prayer-get" => {
                let status = self.st.addon(PRAYER_TIMES.id);
                let checked =
                    |f: &poni_core::addons::AddonFile| std::fs::read(services::addons::file_path(&PRAYER_TIMES, f.name)).is_ok_and(|b| f.matches(&b));
                let files: Vec<String> = PRAYER_TIMES.files.iter().map(|f| format!("{} {}", f.name, if checked(f) { "checked" } else { "missing or different" })).collect();
                let places = self.st.prayer.places.as_ref().map_or(0, |l| l.places.len());
                let place = self.st.prayer.place.as_ref().map_or("none", |p| p.id.as_str());
                let ok = status == AddonStatus::Ready && PRAYER_TIMES.files.iter().all(checked) && places > 500 && place == "jakarta";
                Some(("prayer-addon", ok, format!("{status:?}: {}; {places} places, times for {place}", files.join(", "))))
            }
            "prayer-page" => {
                let tabs = pages::tabs(&self.st);
                let now = self.st.prayer_now();
                let next = self.st.prayer.site().and_then(|s| poni_core::prayer::next_prayer(&s, now).map(|m| format!("{} at {}", m.prayer.name(), s.hhmm(m.at))));
                let ok = self.st.page == PageKind::Prayer && tabs.contains(&PageKind::Prayer) && next.is_some();
                Some(("prayer-page", ok, format!("{} tabs; next: {}", tabs.len(), next.unwrap_or_else(|| "none".into()))))
            }
            "prayer-ok" => {
                let running = self.st.prayer.site().and_then(|s| poni_core::prayer::countdown(&s, self.st.prayer_now(), self.st.settings.prayer.remind_before_min));
                let shown = activities::prayer_countdown(&self.st);
                let ok = running.is_some() && shown.is_none() && self.st.prayer.countdown_ok == running.map(|(m, _)| m.at);
                Some(("prayer-ok", ok, format!("countdown {:?}, on the island: {shown:?}", running.map(|(m, left)| (m.prayer, left)))))
            }
            "prayer-adzan" => {
                let playing = self.adzan.is_playing();
                let notice = self.st.notice.as_ref().map(|n| format!("{} / {}", n.title, n.subtitle)).unwrap_or_default();
                let stop = self.st.notice.as_ref().is_some_and(|n| n.inline == Some(InlineAction::StopAdzan));
                let ok = playing && self.st.prayer.adzan && stop;
                Some(("prayer-adzan", ok, format!("adzan playing: {playing}; notice \"{notice}\", Stop in it: {stop}")))
            }
            "prayer-stop" => {
                let playing = self.adzan.is_playing();
                Some(("prayer-stop", !playing && !self.st.prayer.adzan, format!("adzan playing: {playing}")))
            }
            "prayer-locate" => {
                let l = self.st.prayer.locate;
                Some(("prayer-locate", l != Locate::Idle, format!("location: {l:?}")))
            }
            "prayer-off" => {
                let status = self.st.addon(PRAYER_TIMES.id);
                let gone = !prayer_folder().exists();
                let tab = pages::tabs(&self.st).contains(&PageKind::Prayer);
                let ok = status == AddonStatus::Off && gone && self.st.prayer.places.is_none() && !tab && self.addons.is_none();
                Some(("prayer-off", ok, format!("{status:?}; files {}; Prayer tab: {tab}", if gone { "deleted" } else { "still there" })))
            }
            "fullscreen-hide" => {
                let ok = self.front_full_screen && self.fullscreen_hidden && !window::is_shown();
                let d = format!("full-screen app in front: {}; island {}", self.front_full_screen, if window::is_shown() { "still on the screen" } else { "off the screen" });
                Some(("fullscreen-hide", ok, d))
            }
            "fullscreen-stay" => {
                let ok = self.front_full_screen && !self.fullscreen_hidden && window::is_shown();
                Some(("fullscreen-stay", ok, format!("full-screen app in front: {}; island on the screen: {}", self.front_full_screen, window::is_shown())))
            }
            "fullscreen-back" => {
                let ok = !self.front_full_screen && !self.fullscreen_hidden && window::is_shown();
                Some(("fullscreen-back", ok, format!("full-screen app in front: {}; island on the screen: {}", self.front_full_screen, window::is_shown())))
            }
            "oyenza-post" => {
                let (ok, d) = self.smoke_post.clone().unwrap_or((false, "not run".into()));
                Some(("oyenza-post", ok && self.oyenza.is_some(), format!("worker {}; {d}", if self.oyenza.is_some() { "running" } else { "not started" })))
            }
            "oyenza-code" => {
                let code = matches!(&self.st.oyenza.link, OyenzaLink::Connecting { code, .. } if code == "KPJX-TMNQ");
                let ok = code && self.st.pinned && self.st.page == PageKind::Oyenza && pages::tabs(&self.st).contains(&PageKind::Oyenza);
                Some(("oyenza-code", ok, format!("code shown: {code}; island kept open: {}; page {:?}", self.st.pinned, self.st.page)))
            }
            "oyenza-page" => {
                let o = &self.st.oyenza;
                let rows = o.activity.as_ref().map_or(0, |a| a.days.iter().map(|d| d.rows.len()).sum::<usize>());
                let ok = matches!(o.link, OyenzaLink::Connected { .. }) && rows > 0 && !o.bills.is_empty() && self.st.oyenza.pinned_before.is_none();
                Some(("oyenza-page", ok, format!("{:?}; {rows} transactions, {} bills; island no longer held open: {}", o.link, o.bills.len(), o.pinned_before.is_none())))
            }
            "oyenza-money" => {
                let n = self.st.notice.as_ref();
                let ok = n.is_some_and(|n| n.title == "Money in \u{b7} BCA" && n.inline == Some(InlineAction::ShowAmount) && n.reveal.is_some());
                Some(("oyenza-money", ok, format!("notice: {:?} / {:?}, sum held back for Show: {}", n.map(|n| &n.title), n.map(|n| &n.subtitle), n.is_some_and(|n| n.reveal.is_some()))))
            }
            "ai-page" => {
                let tab = pages::tabs(&self.st).contains(&PageKind::Ai);
                let asked = services::keychain::asked();
                let ok = tab && self.expanded && self.st.page == PageKind::Ai && self.usage.is_none() && asked == 0;
                let worker = if self.usage.is_none() { "not started" } else { "running" };
                Some(("ai-page", ok, format!("AI tab: {tab}; page {:?}; worker {worker}; keychain asked {asked} times", self.st.page)))
            }
            "ai-keychain" => {
                let (ok, d) = smoke_keychain();
                Some(("keychain", ok, d))
            }
            // Locked, the notices rest, but what the keychain read stays for
            // when someone is back; macOS is asked nothing meanwhile.
            "ai-locked" => {
                let (ask, keep) = services::keychain::watching();
                let before = services::keychain::asked();
                let read = services::keychain::read(SMOKE_KEYCHAIN.0, Some("plain"));
                let asked = services::keychain::asked() - before;
                let kept = std::env::var_os("PONI_SMOKE_KEYCHAIN").is_none() || read == studio_usage::Got::Text(SMOKE_KEYCHAIN.1.into());
                let ok = self.away.1 && !ask && keep && kept && asked == 0;
                Some(("keychain-locked", ok, format!("locked, notices on: may ask macOS {ask}, keeps what it read {keep}; a read now: {read:?}, macOS asked {asked} times")))
            }
            "ai-back" => {
                let (ask, keep) = services::keychain::watching();
                services::keychain::ask_again();
                // Locking and unlocking went through set_usage_watch: still no worker.
                let ok = !self.away.1 && self.mode == Mode::Expanded(PageKind::Ai) && ask && keep && self.usage.is_none();
                let worker = if self.usage.is_none() { "not started" } else { "running" };
                Some(("keychain-back", ok, format!("unlocked, on the AI tab: may ask macOS {ask}, keeps what it read {keep}; worker {worker}")))
            }
            "ai-secret" => {
                let (ok, d) = smoke_secret();
                Some(("secret-backups", ok, d))
            }
            "oyenza-off" => {
                // Signed out, the tab stays: it is where to sign in again.
                let tab = pages::tabs(&self.st).contains(&PageKind::Oyenza);
                let ok = self.st.oyenza.link == OyenzaLink::Off && tab;
                Some(("oyenza-off", ok, format!("{:?}; Oyenza tab: {tab}", self.st.oyenza.link)))
            }
            "ja-off" => {
                let status = self.st.addon(JAPANESE_READINGS.id);
                let gone = !ja_folder().exists();
                let ok = status == AddonStatus::Off && gone && self.addons.is_none();
                let d = format!("{status:?}; files {}; downloader {}", if gone { "deleted" } else { "still there" }, if self.addons.is_none() { "stopped" } else { "still running" });
                Some(("ja-readings-off", ok, d))
            }
            "ielts-voice" | "ielts-voice-mp3" => {
                let check = if name == "ielts-voice" { "ielts-voice-wav" } else { "ielts-voice-mp3" };
                let Some(v) = &self.smoke_voice else { return Some((check, true, "skipped: no mp3 on this run".into())) };
                let ended = v.ended.map_or("never".to_string(), |t| format!("after {:.2} s", t - v.started));
                let ok = v.played.is_ok() && v.ended.is_some() && !v.player.playing();
                Some((check, ok, format!("{}: play {:?}, VoiceEnded {ended}", v.file.display(), v.played)))
            }
            "ielts-mic" => {
                use services::mic::MicError;
                let (ok, gave) = match &self.smoke_mic {
                    Some(Ok(mic)) => (true, format!("Ok ({mic:?}, level {:.3})", mic.level())),
                    Some(Err(MicError::NoDevice)) => (true, "NoDevice (no microphone on this Mac)".to_string()),
                    Some(Err(MicError::Denied)) => (true, "Denied (macOS says no)".to_string()),
                    Some(Err(MicError::Failed(e))) => (false, format!("Failed: {e}")),
                    None => (false, "nothing".to_string()),
                };
                Some(("ielts-mic-start", ok, format!("Mic::start gave {gave}")))
            }
            "ielts-mic-stop" => {
                let (ok, d) = self.smoke_mic_stop.clone().unwrap_or((false, "not run".into()));
                Some(("ielts-mic-stop", ok, d))
            }
            "notes-capture-stop" => {
                let (ok, d) = self.smoke_notes_stop.clone().unwrap_or((false, "not run".into()));
                Some(("notes-capture", ok, d))
            }
            _ => None,
        }
    }

    /// The microphone's stop step: what `Mic::stop` gave, and whether that's
    /// right for what `start` gave.
    fn smoke_mic_stopped(&mut self) -> (bool, String) {
        match self.smoke_mic.take() {
            Some(Ok(mic)) => {
                let was = format!("{mic:?}");
                match mic.stop() {
                    Ok((path, secs)) => match smoke_wav_format(&path) {
                        Some((rate, channels, bits, at, bytes)) => {
                            let ok = rate == 16_000 && channels == 1 && bits == 16 && secs > 1.0 && bytes >= 32_000;
                            (ok, format!("{secs:.2} s in {}: {rate} Hz, {channels} channel(s), {bits}-bit, {bytes} bytes of samples from byte {at}", path.display()))
                        }
                        None => (false, format!("{secs:.2} s, but {} isn't a WAV file", path.display())),
                    },
                    // macOS's question still up (nobody answers it on CI), or
                    // the person said no meanwhile: nothing was recorded.
                    Err(e) if was == "asking macOS" || was == "turned down" => (true, format!("{e:?} (it was {was})")),
                    Err(e) => (false, format!("{e:?} (it was {was})")),
                }
            }
            Some(Err(e)) => (true, format!("nothing to stop: start gave {e:?}")),
            None => (false, "Mic::start didn't run".into()),
        }
    }

    /// Meeting notes' stop step: a recording that started stops into WAV
    /// parts of 16 kHz mono; one that couldn't start says why. Either is
    /// fine on a runner (no microphone, nobody to answer macOS's questions);
    /// what fails is a crash, a hang, or parts in the wrong format.
    fn smoke_notes_stopped(&mut self) -> (bool, String) {
        match self.smoke_notes.take() {
            Some(Ok((rec, mic, pc))) => {
                let (lm, lp) = rec.levels();
                match rec.stop() {
                    Ok(take) => {
                        let formats: Vec<_> = take.parts.iter().map(|p| smoke_wav_format(p).map(|(r, c, b, _, n)| (r, c, b, n))).collect();
                        let ok = formats.iter().all(|f| matches!(f, Some((16_000, 1, 16, _))));
                        let _ = std::fs::remove_dir_all(&take.dir);
                        (ok, format!("microphone {mic}, the Mac's sound {pc}; levels {lm:.3}/{lp:.3}; {:.2} s in {} part(s) {formats:?}", take.secs, take.parts.len()))
                    }
                    Err(issue) => (true, format!("microphone {mic}, the Mac's sound {pc}; stop gave {issue:?}")),
                }
            }
            // No microphone, or macOS saying no, is the runner's business; a
            // recording that fails to start (a source hanging) is ours.
            Some(Err(issue)) => (issue != poni_core::notes::CaptureIssue::Failed, format!("no recording: {issue:?}")),
            None => (false, "Recording::start didn't run".into()),
        }
    }
}

/// The IELTS coach's voice step: the player, its file, when the line
/// started, what `play` said and when `AppEvent::VoiceEnded` came.
struct SmokeVoice {
    player: services::voice::VoicePlayer,
    file: PathBuf,
    started: f64,
    played: Result<(), String>,
    ended: Option<f64>,
}

/// The voice step's line: a quiet 440 Hz tone, `secs` long, as a 16 kHz mono
/// 16-bit WAV (the microphone's format too).
fn smoke_tone_wav(path: &std::path::Path, secs: f32) -> std::io::Result<()> {
    let n = (16_000.0 * secs) as u32;
    let mut b = Vec::with_capacity(44 + 2 * n as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + 2 * n).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&16_000u32.to_le_bytes());
    b.extend_from_slice(&32_000u32.to_le_bytes()); // bytes a second
    b.extend_from_slice(&2u16.to_le_bytes()); // bytes a frame
    b.extend_from_slice(&16u16.to_le_bytes()); // bits
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(2 * n).to_le_bytes());
    for i in 0..n {
        let s = (i as f32 / 16_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.1 * i16::MAX as f32;
        b.extend_from_slice(&(s as i16).to_le_bytes());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, b)
}

/// A WAV file's rate, channels, bits, and where its samples start and how
/// many bytes they take (chunks such as Apple's FLLR padding skipped).
fn smoke_wav_format(path: &std::path::Path) -> Option<(u32, u16, u16, usize, usize)> {
    let b = std::fs::read(path).ok()?;
    if b.get(0..4)? != b"RIFF" || b.get(8..12)? != b"WAVE" {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?));
    let (mut at, mut fmt) = (12, None);
    while at + 8 <= b.len() {
        let (id, len) = (&b[at..at + 4], u32_at(at + 4)? as usize);
        if id == b"fmt " {
            fmt = Some((u32_at(at + 12)?, u16_at(at + 10)?, u16_at(at + 22)?));
        } else if id == b"data" {
            let (rate, channels, bits) = fmt?;
            return Some((rate, channels, bits, at + 8, len.min(b.len() - at - 8)));
        }
        at += 8 + len + (len & 1);
    }
    None
}

/// The keychain items tools/mac-smoke.sh adds for the AI tab's reader: their
/// service, and the passwords of the accounts "plain" and "accent" (an accent
/// and a line break, which `security` prints as hex). "absent" is never added.
const SMOKE_KEYCHAIN: (&str, &str, &str) = ("Oyenza Studio smoke", "{\"claudeAiOauth\":{\"accessToken\":\"test-token\"}}", "caf\u{e9}\nnext");

/// The smoke test of the AI tab's keychain reader (`services::keychain`) on
/// the items above, with the tab closed, open, then closed with the notices
/// on and with nothing watching: whether each read gave what it should and
/// how often macOS was asked. The worker never runs in the smoke run, so the
/// reader's gate is this test's alone.
fn smoke_keychain() -> (bool, String) {
    use studio_usage::Got;
    use services::keychain as k;
    if std::env::var_os("PONI_SMOKE_KEYCHAIN").is_none() {
        return (true, "skipped: no test items in the keychain".into());
    }
    let (svc, plain, accent) = SMOKE_KEYCHAIN;
    let text = |s: &str| Got::Text(s.to_string());
    k::set_watch(false, false);
    k::ask_again();
    let before = k::asked();
    let asked = || k::asked() - before;
    // Closed and never read: nothing, and macOS isn't asked.
    let closed = k::read(svc, Some("plain"));
    let closed_ok = closed == Got::Missing && asked() == 0;
    k::set_watch(true, false);
    let t = std::time::Instant::now();
    let open = [k::read(svc, Some("plain")), k::read(svc, Some("accent")), k::read(svc, Some("absent"))];
    let ms = t.elapsed().as_millis();
    let open_ok = open == [text(plain), text(accent), Got::Missing] && asked() == 3;
    // Once an opening: the next read is the one kept.
    let again = k::read(svc, Some("plain"));
    let again_ok = again == text(plain) && asked() == 3;
    // Closed with the notices on: what was read while it was open.
    k::set_watch(false, true);
    let notices = k::read(svc, Some("plain"));
    let notices_ok = notices == text(plain) && asked() == 3;
    // Nobody watching: forgotten.
    k::set_watch(false, false);
    let gone = k::read(svc, Some("plain"));
    let gone_ok = gone == Got::Missing && asked() == 3;
    k::ask_again();
    let ok = closed_ok && open_ok && again_ok && notices_ok && gone_ok;
    // Got's Debug shows a text's length, never the text.
    let d = format!("tab closed: {closed:?}; open: {open:?} in {ms} ms; again: {again:?}; notices only: {notices:?}; nobody watching: {gone:?}; macOS asked {} times", asked());
    (ok, d)
}

/// The smoke test of a key saved the way the AI tab saves one (into this
/// run's own home): it reads back, and Time Machine leaves it out (tmutil's
/// own answer, and the attribute that says so).
fn smoke_secret() -> (bool, String) {
    let path = paths::usage_key_path("smoke-test");
    if let Err(e) = services::secret::save(&path, "test-key") {
        return (false, format!("not saved: {e}"));
    }
    let out = |cmd: &str, args: &[&std::ffi::OsStr]| {
        std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    let tm = out("/usr/bin/tmutil", &["isexcluded".as_ref(), path.as_os_str()]);
    let attr = out("/usr/bin/xattr", &[path.as_os_str()]).contains("com.apple.metadata:com_apple_backup_excludeItem");
    let back = services::secret::load(&path);
    let _ = services::secret::delete(&path);
    let verdict = if tm.contains("[Excluded]") {
        "excluded"
    } else if tm.contains("[Included]") {
        "included"
    } else {
        "no answer"
    };
    let ok = attr && verdict != "included" && back.as_deref() == Some("test-key");
    (ok, format!("tmutil: {verdict}; exclusion attribute: {}; read back: {}", if attr { "set" } else { "missing" }, back.is_some()))
}

/// The smoke test: one POST with a JSON body and a key through the Mac's
/// HTTP client (what signing in to Oyenza and saving a spend send), to a
/// listener of its own on this Mac. Whether it arrived whole and its answer
/// came back, and what happened.
fn smoke_post() -> (bool, String) {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => return (false, format!("no listener ({e})")),
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let _ = listener.set_nonblocking(true);
    let server = std::thread::spawn(move || -> String {
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut conn = None;
        while conn.is_none() && Instant::now() < deadline {
            match listener.accept() {
                Ok((c, _)) => conn = Some(c),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let Some(mut c) = conn else { return String::new() };
        let _ = c.set_nonblocking(false);
        let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        // The headers, then as much body as Content-Length says.
        loop {
            match c.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            if let Some(end) = text.find("\r\n\r\n") {
                let len = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                if buf.len() >= end + 4 + len {
                    break;
                }
            }
        }
        let body = "{\"ok\":true}";
        let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        String::from_utf8_lossy(&buf).into_owned()
    });
    let json = "{\"app\":\"poni\",\"name\":\"Smoke Mac\"}";
    let answer = services::http::Client::new("OyenzaStudio smoke").and_then(|c| {
        c.send(services::http::Method::Post, &format!("http://127.0.0.1:{port}/connect/start"), &[("Authorization", "Bearer oyzd_smoke")], Some(json), Duration::from_secs(5))
    });
    let seen = server.join().unwrap_or_default();
    let arrived = seen.starts_with("POST /connect/start ") && seen.contains("Bearer oyzd_smoke") && seen.to_ascii_lowercase().contains("content-type: application/json") && seen.ends_with(json);
    match answer {
        Ok(r) => (arrived && r.status == 200 && r.body.contains("\"ok\":true"), format!("request arrived whole: {arrived}; answer {} {}", r.status, r.body)),
        Err(e) => (false, format!("request arrived whole: {arrived}; no answer ({e})")),
    }
}

/// Lines of these lyrics that have romaji, and lines that need it.
fn romaji_lines(l: &LyricsState) -> (usize, usize) {
    let needing = l.lyrics.as_ref().map_or(0, |x| x.lines.iter().filter(|line| poni_core::romanize::needs_roman(line.text.trim())).count());
    (l.ruby.iter().filter(|p| p.iter().any(|r| !r.roman.is_empty())).count(), needing)
}

/// The Japanese readings add-on's folder (every version).
fn ja_folder() -> PathBuf {
    paths::addons_dir().join(JAPANESE_READINGS.id)
}

fn prayer_folder() -> PathBuf {
    paths::addons_dir().join(PRAYER_TIMES.id)
}

/// The smoke run's Japanese song (`render::LYRICS_JA`) in the lyrics cache,
/// as LRCLIB's answer: the lyrics worker finds it there as it would a song
/// it looked up before. The entry is `services/lyrics.rs`'s `CacheEntry`
/// (format 1); a new format makes the ja-auto check fail.
fn seed_japanese_lyrics() {
    use poni_core::lrclib::{self, LrclibRecord, TrackQuery};
    let (title, artist, album, duration_s) = SMOKE_JA_TRACK;
    let q = lrclib::normalize_query(&TrackQuery { title: title.into(), artist: artist.into(), album: album.into(), duration_s });
    let record = LrclibRecord {
        track_name: title.into(),
        artist_name: artist.into(),
        album_name: Some(album.into()),
        duration: Some(duration_s as f64),
        synced_lyrics: Some(crate::render::LYRICS_JA.into()),
        ..Default::default()
    };
    let entry = serde_json::json!({ "v": 1, "lookup": 2, "fetched_at": clock::unix_now(), "record": record });
    let path = paths::lyrics_cache_dir().join(format!("{}.json", lrclib::cache_key(&q)));
    let saved = std::fs::create_dir_all(paths::lyrics_cache_dir()).and_then(|_| std::fs::write(&path, entry.to_string()));
    if let Err(e) = saved {
        pwarn!("smoke: couldn't put the Japanese song's lyrics in {} ({e})", path.display());
    }
}

/// The smoke test's look at the System page's real readings: CPU cores,
/// memory, a drive and a network interface must be there; the GPU and the
/// battery are reported (CI's Mac is a virtual one, and has no battery).
fn system_report(st: &AppState) -> (bool, String) {
    use poni_core::sysmon::{fmt_app_cpu, fmt_bytes, fmt_link, fmt_rate, pressure_word, AppUse};
    let Some(r) = st.system.latest.as_ref() else { return (false, "no reading arrived".into()) };
    let cpu = r.cpu.as_ref().filter(|c| c.logical > 0 && !c.cores.is_empty() && (0.0..=1.0).contains(&c.load));
    let mem = r.memory.as_ref().filter(|m| m.total > 0 && m.used <= m.total);
    let net = r.network.as_ref().filter(|n| !n.adapter.is_empty());
    let ok = cpu.is_some() && mem.is_some() && !r.drives.is_empty() && net.is_some();
    let pct = |f: f32| format!("{:.0}%", f * 100.0);
    let cores = cpu.map_or("missing".into(), |c| {
        let kinds = match c.hybrid() {
            Some((p, e)) => {
                let load = |k: CoreKind| pct(c.kind_load(k).unwrap_or(0.0));
                format!(" ({p} P at {}, {e} E at {})", load(CoreKind::Performance), load(CoreKind::Efficiency))
            }
            None => String::new(),
        };
        format!("{} of {} cores{kinds}, system {}", pct(c.load), c.cores.len(), pct(c.kernel))
    });
    let memory = mem.map_or("missing".into(), |m| {
        let parts = m.parts().map(|(_, b)| fmt_bytes(b));
        format!(
            "{} of {} (app {}, wired {}, compressed {}, cached {}, free {}), pressure {}, swap {} of {}",
            fmt_bytes(m.used),
            fmt_bytes(m.total),
            parts[0],
            parts[1],
            parts[2],
            parts[3],
            parts[4],
            if m.commit_limit > 0 { pressure_word(m.pressure()) } else { "not reported" },
            fmt_bytes(m.page_used),
            fmt_bytes(m.page_total)
        )
    });
    let drives: Vec<String> =
        r.drives.iter().map(|d| format!("{}{} {} free of {}", d.root, if d.system { " (startup)" } else { "" }, fmt_bytes(d.free), fmt_bytes(d.total))).collect();
    let network = r.network.as_ref().map_or("missing".into(), |n| {
        format!(
            "{} {} {} MAC {} link {} down {} up {} received {} sent {}",
            if n.adapter.is_empty() { "(no name)" } else { &n.adapter },
            if n.online { "connected" } else { "offline" },
            n.ipv4.as_deref().unwrap_or("no IPv4"),
            if n.mac.is_empty() { "none" } else { &n.mac },
            if n.link_bps > 0 { fmt_link(n.link_bps) } else { "not reported".into() },
            fmt_rate(n.down_bps),
            fmt_rate(n.up_bps),
            fmt_bytes(n.received),
            fmt_bytes(n.sent)
        )
    });
    let gpu = r.gpu.as_ref().map_or("none".into(), |g| {
        format!("{} {} (renderer {}, tiler {}), {} of {}", g.name, pct(g.util), pct(g.engine_3d), pct(g.tiler), fmt_bytes(g.mem_used), fmt_bytes(g.mem_total))
    });
    let battery = r.battery.as_ref().map_or("none".into(), |b| format!("{}%", b.percent));
    let apps = |list: &[AppUse], value: &dyn Fn(&AppUse) -> String| {
        let one = |a: &AppUse| {
            let count = if a.processes == 1 { "1 process".to_string() } else { format!("{} processes", a.processes) };
            format!("{} {} ({count}{})", a.name, value(a), if a.path.is_some() { ", icon path" } else { "" })
        };
        list.iter().take(4).map(one).collect::<Vec<_>>().join(", ")
    };
    let wanted: Vec<&String> = r.top_cpu.iter().chain(&r.top_memory).filter_map(|a| a.path.as_ref()).collect();
    let drawn = wanted.iter().filter(|p| st.app_icons.get(p.as_str()).is_some_and(|i| i.is_some())).count();
    let detail = format!(
        "{} · CPU {cores}, up {} min · memory {memory} · drives {} · disk {} · network {network} · GPU {gpu} · battery {battery} \
         · top CPU {} · top memory {} · icons {drawn} of {} drawn",
        services::sysmon::mac_model().unwrap_or_else(|| "model not reported".into()),
        cpu.map_or(0, |c| c.uptime_s / 60),
        if drives.is_empty() { "none".into() } else { drives.join("; ") },
        r.disk.map_or("not reported".into(), |d| format!("read {} write {}", fmt_rate(d.read_bps), fmt_rate(d.write_bps))),
        apps(&r.top_cpu, &|a| fmt_app_cpu(a.cpu)),
        apps(&r.top_memory, &|a| fmt_bytes(a.memory)),
        wanted.len()
    );
    (ok, detail)
}

/// Oyen's orange in his theme, the Mac's accent colour otherwise.
pub(crate) fn theme_accent(theme: Theme) -> Color {
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

const SMOKE_STEPS: [&str; 77] = [
    "idle",
    "hover",
    "open-focus",
    "open-today",
    "system-on",
    "open-system",
    "system-gpu",
    "system-memory",
    "system-drives",
    "system-network",
    "system-power",
    "timer",
    "notice",
    "volume-hud",
    "charging-hud",
    "headphones",
    "shelf-drop",
    "music-notify",
    "music-check",
    "music-cost",
    "glass-open",
    "glass-collapsed",
    "pill-open",
    "pill-collapsed",
    "pill-hover",
    "pill-music-cost",
    "notch-oyen-open",
    "notch-oyen-collapsed",
    "notch-glass-open",
    "notch-pill-open",
    "notch-pill-music",
    "notch-pill-collapsed",
    "notch-pill-ear",
    "notch-eyes",
    "ja-notify",
    "ja-auto",
    "ja-get",
    "addons-tab",
    "ja-off",
    "prayer-get",
    "prayer-page",
    "prayer-ok",
    "prayer-adzan",
    "prayer-stop",
    "prayer-locate",
    "prayer-off",
    "fullscreen-hide",
    "fullscreen-stay",
    "fullscreen-back",
    "oyenza-post",
    "oyenza-code",
    "oyenza-page",
    "oyenza-money",
    "oyenza-off",
    "ai-page",
    "ai-keychain",
    "ai-locked",
    "ai-back",
    "ai-secret",
    "now-playing-start",
    "now-playing",
    "now-playing-browser-start",
    "now-playing-browser",
    "record-port",
    "record-check",
    "record-stop",
    "record-gone",
    "settings-window",
    "settings-window-light",
    "settings-window-closed",
    "ielts-voice",
    "ielts-voice-mp3",
    "ielts-mic",
    "ielts-mic-stop",
    "notes-capture",
    "notes-capture-stop",
    "menu",
];
/// The track CI announces as Spotify's own notification would.
const SMOKE_TRACK: &str = "Blinding Lights";
/// The made-up song with Japanese lyrics CI announces after it: title,
/// artist, album and length (tools/mac-smoke.sh sends the same).
const SMOKE_JA_TRACK: (&str, &str, &str, u32) = ("Paper Boats", "The Lanterns", "Harbor Lights", 201);
/// The track the MediaRemote adapter's test player reports in macOS's own
/// now playing (third_party/mediaremote-adapter/src/test/NowPlayingTest.m).
const SMOKE_SERVICE_TRACK: &str = "Is It Broken Yet?";
/// The title the browser's page gives its tab, as YouTube names a video
/// (tools/mac-browser-player.html).
const SMOKE_BROWSER_TRACK: &str = "Robbers";

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
    /// When the ear step moved the pointer on the floating card's ear.
    replayed: Option<f64>,
}

impl Smoke {
    fn new(dir: PathBuf) -> Smoke {
        let _ = std::fs::create_dir_all(&dir);
        Smoke { dir, step: 0, phase: SmokePhase::Apply, checks: Vec::new(), failed: false, replayed: None }
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

// The Downloads page's commands and yt-dlp's reports.
#[path = "app_downloads.rs"]
mod downloads;

// The IELTS coach's page, worker, microphone and voice.
#[path = "app_ielts.rs"]
mod ielts;

// Meeting notes: the recording and the handing over.
#[path = "app_notes.rs"]
mod notes;
