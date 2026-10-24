//! `poni --render <dir> [--theme classic|oyen|glass] [--notch]`: draw the island's
//! states with sample data into PNGs, offscreen, through the Mac renderer.
//! CI runs it on a Mac with no notch and no person at the screen, so this is
//! how the Mac drawing gets looked at: every page and activity, each theme,
//! with and without a (simulated) MacBook notch.
//!
//! Each picture is the island over a desktop: a soft wallpaper gradient and a
//! light menu bar. With `--notch` the camera notch is painted on top, black,
//! the way it hides that part of the island on a real MacBook. Glass is
//! macOS's own view on the real window, so here a smoked stand-in takes its
//! place (the smoke run's screenshots show the real one).
//!
//! The music scenes use made-up data to test the drawing; the Mac app shows
//! music once the media slice is built. The System page's scenes show a
//! made-up minute of a MacBook too, with real app icons from Finder.

use crate::activities::{self, Mode};
use crate::gfx::{Bitmap, Gfx, ImageData};
use crate::icons::Icon;
use crate::island::{Island, SHOULDER};
use crate::notch::{self, Notch};
use crate::pages::{self, PageKind};
use crate::state::{AppState, Hud, HudKind, LyricsState, LyricsStatus, MediaSession, PowerState, SysStats, VolumeState};
use crate::ui::{Hit, Input};
use poni_core::datetime::LocalDateTime;
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::health::{HealthDay, HealthSnapshot, HealthSource};
use poni_core::lrc::parse_lrc;
use poni_core::pomodoro::{Phase, RunState};
use poni_core::settings::{IdleStyle, Settings, Theme};
use poni_core::store::Store;
use poni_core::sysmon::{
    level_pressure, AppUse, BatteryReading, CoreKind, CoreLoad, CpuReading, DiskActivity, Drive, GpuReading, MemoryReading, Module, Monitor, NetReading,
    PressureLevel, Reading, HISTORY_LEN,
};
use poni_core::timeline::Timeline;
use std::path::{Path, PathBuf};

/// Pixels per DIP (a Retina screen).
const SCALE: f32 = 2.0;
/// Room around the island in each picture (DIPs).
const MARGIN_X: f32 = 36.0;
const MARGIN_BOTTOM: f32 = 40.0;
/// The menu bar of a Mac without a notch (DIPs).
const MENU_BAR: f32 = 24.0;
/// A fixed moment for every picture: 24 Oct 2026, 09:41.
const NOW_MONO: f64 = 1000.0;
const NOW_UNIX: i64 = 1_792_834_860;

const TITLE: &str = "Paper Boats";
const ARTIST: &str = "The Lanterns";
const ALBUM: &str = "Harbor Lights";
const LYRICS_LRC: &str = "\
[00:38.00]Folded up the morning into something small
[00:43.50]Set it on the water by the harbor wall
[00:49.00]Ink along the edges of a name I used to know
[00:54.50]Watched it find the current, watched it go
[01:00.00]Paper boats, carry what I couldn't say
[01:05.50]Over the grey and out across the bay
[01:11.00]If the tide is gentle they will find the light
";
const CLIPS: [&str; 5] = [
    "Lunch: noodles from the corner place, then a walk by the river",
    "Tracking number PX-2048-7731",
    "#5AC8FA",
    "https://example.com/projects/poni/notes",
    "Meet at the north entrance at 6:15",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scene {
    IdleClock,
    IdleEyes,
    Hover,
    Timer,
    Media,
    FocusNotice,
    VolumeHud,
    ChargingHud,
    LowBatteryHud,
    Page(PageKind),
    HealthSignedOut,
    ShelfItems,
    ShelfPhone,
    /// The System page on one module.
    System(Module),
}

const SCENES: [Scene; 19] = [
    Scene::IdleClock,
    Scene::IdleEyes,
    Scene::Hover,
    Scene::Timer,
    Scene::Media,
    Scene::FocusNotice,
    Scene::VolumeHud,
    Scene::Page(PageKind::Focus),
    Scene::Page(PageKind::Today),
    Scene::Page(PageKind::Tasks),
    Scene::Page(PageKind::Health),
    Scene::HealthSignedOut,
    Scene::Page(PageKind::Media),
    Scene::Page(PageKind::Shelf),
    Scene::Page(PageKind::Clipboard),
    // Slice 3: the pop-ups and the Shelf with real files.
    Scene::ChargingHud,
    Scene::LowBatteryHud,
    Scene::ShelfItems,
    Scene::ShelfPhone,
];

/// The System page's modules (after the scenes above, so their numbers stay).
const SYSTEM_SCENES: [Scene; 6] = [
    Scene::System(Module::Cpu),
    Scene::System(Module::Gpu),
    Scene::System(Module::Memory),
    Scene::System(Module::Drives),
    Scene::System(Module::Network),
    Scene::System(Module::Battery),
];

impl Scene {
    fn name(self) -> String {
        match self {
            Scene::IdleClock => "idle-clock".into(),
            Scene::IdleEyes => "idle-eyes".into(),
            Scene::Hover => "idle-hover".into(),
            Scene::Timer => "timer-activity".into(),
            Scene::Media => "media-activity".into(),
            Scene::FocusNotice => "focus-notice".into(),
            Scene::VolumeHud => "volume-hud".into(),
            Scene::ChargingHud => "charging-hud".into(),
            Scene::LowBatteryHud => "low-battery-hud".into(),
            Scene::ShelfItems => "page-shelf-items".into(),
            Scene::ShelfPhone => "page-shelf-phone".into(),
            Scene::Page(p) => format!("page-{}", p.key()),
            Scene::HealthSignedOut => "page-health-signed-out".into(),
            Scene::System(Module::Cpu) => "page-system".into(),
            Scene::System(m) => format!("page-system-{}", m.title().to_lowercase()),
        }
    }
}

pub fn run(dir: &str, args: &[String]) -> i32 {
    let out = PathBuf::from(dir);
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("render: {}: {e}", out.display());
        return 2;
    }
    let theme = match args.iter().position(|a| a == "--theme").and_then(|i| args.get(i + 1)).map(|s| s.as_str()) {
        Some("oyen") => Theme::Oyen,
        Some("glass") => Theme::Glass,
        _ => Theme::Classic,
    };
    let notch = args.iter().any(|a| a == "--notch").then_some(Notch::MACBOOK);
    let name = match theme {
        Theme::Classic => "",
        Theme::Oyen => "-oyen",
        Theme::Glass => "-glass",
    };
    let suffix = format!("{name}{}", if notch.is_some() { "-notch" } else { "" });
    let mut gfx = Gfx::new(SCALE);
    let mut failed = 0;
    for (i, scene) in SCENES.into_iter().chain(SYSTEM_SCENES).enumerate() {
        let path = out.join(format!("{:02}-{}{suffix}.png", i + 1, scene.name()));
        match render_scene(&mut gfx, scene, theme, notch, &path) {
            Ok(()) => println!("render: {}", path.display()),
            Err(e) => {
                eprintln!("render: {}: {e}", path.display());
                failed += 1;
            }
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

fn base_state(theme: Theme) -> AppState {
    let settings = Settings { theme, sounds: false, show_media_activity: true, lyrics: true, ..Default::default() };
    let mut st = AppState::new(settings, Store::default());
    st.now = NOW_MONO;
    st.unix = NOW_UNIX;
    st.local = LocalDateTime { year: 2026, month: 10, day: 24, hour: 9, minute: 41, second: 0 };
    st.accent = if theme == Theme::Oyen { crate::oyen::ORANGE } else { palette::BLUE };
    st.volume = VolumeState { level: 0.64, muted: false, known: true };
    st.power = PowerState { has_battery: true, percent: 76, charging: false, on_ac: false, saver: false };
    st.stats = SysStats { valid: true, cpu: 0.23, mem_used: 9 << 30, mem_total: 16 << 30, disk_free: 212 << 30, disk_total: 476 << 30, uptime_s: 3 * 86_400 + 4 * 3600 };
    seed(&mut st);
    st
}

fn seed(st: &mut AppState) {
    let unix = st.unix;
    let day_start = st.day_start_unix();
    let done = st.store.tasks.add("Water the balcony plants", unix - 5 * 3600);
    st.store.tasks.add("Sketch the onboarding screens", unix - 3 * 3600);
    st.store.tasks.add("Book the ferry for Saturday", unix - 2 * 3600);
    if let Some(id) = done {
        st.store.tasks.toggle(id, (unix - 1800).max(day_start + 60));
    }
    for (k, text) in CLIPS.iter().enumerate() {
        st.clip.push(text, unix - (CLIPS.len() - k) as i64 * 420);
    }
    let today = st.local;
    let week = [5210u32, 9120, 7480, 3960, 10230, 6840, 8412];
    let days = week
        .iter()
        .enumerate()
        .map(|(i, &steps)| {
            let last = i + 1 == week.len();
            HealthDay { date: today.add_days(i as i64 + 1 - week.len() as i64).day_key(), steps, distance_m: last.then_some(6140), active_kcal: last.then_some(342) }
        })
        .collect();
    st.health = Some(HealthSnapshot { goal: 8000, source: HealthSource::AppleHealth, updated_at: unix - 8 * 60, days });
    st.store.focus_sessions.insert(st.day_key(), 2);
}

fn media(st: &mut AppState) {
    let art = ImageData::new(240, 240, make_art(240));
    let color = art.accent_color();
    st.media = Some(MediaSession {
        app_id: "com.apple.Music".into(),
        app_name: "Music".into(),
        title: TITLE.into(),
        artist: ARTIST.into(),
        album: ALBUM.into(),
        art: Some(art),
        art_color: Some(color),
        timeline: Timeline::new(64.0, 201.0, NOW_MONO, true),
        can_prev: true,
        can_next: true,
        can_play_pause: true,
        can_seek: true,
        track_key: MediaSession::make_track_key(TITLE, ARTIST, ALBUM),
    });
    st.lyrics = LyricsState { track_key: MediaSession::make_track_key(TITLE, ARTIST, ALBUM), status: LyricsStatus::Found, lyrics: Some(parse_lrc(LYRICS_LRC)), source: "Sample".into(), ruby: Vec::new() };
}

fn render_scene(gfx: &mut Gfx, scene: Scene, theme: Theme, notch: Option<Notch>, path: &Path) -> Result<(), String> {
    let mut st = base_state(theme);
    let mut hovering = false;
    let mode = match scene {
        Scene::IdleClock => Mode::Idle,
        Scene::IdleEyes => {
            st.settings.idle_style = IdleStyle::Eyes;
            Mode::Idle
        }
        Scene::Hover => {
            st.settings.idle_style = IdleStyle::Eyes;
            hovering = true;
            Mode::Idle
        }
        Scene::Timer => {
            st.pomodoro.phase = Phase::Focus;
            st.pomodoro.run = RunState::Running { ends_at: NOW_MONO + 1104.5 };
            Mode::Activity(activities::Activity::Timer)
        }
        Scene::Media => {
            media(&mut st);
            Mode::Activity(activities::Activity::Media)
        }
        Scene::FocusNotice => {
            st.pomodoro.phase = Phase::ShortBreak;
            st.notify(Icon::Coffee, palette::GREEN, "Focus done", "Time for a 5 minute break", 30.0, None);
            st.set_notice_inline(crate::state::InlineAction::StartFocus);
            Mode::Notice
        }
        Scene::VolumeHud => {
            st.hud = Some(Hud { kind: HudKind::Volume { level: 0.64, muted: false }, until: NOW_MONO + 30.0 });
            st.ui.hud_level = 0.64;
            Mode::Hud(activities::HudTag::Volume)
        }
        Scene::ChargingHud => {
            st.hud = Some(Hud { kind: HudKind::Charging { percent: 76, plugged: true }, until: NOW_MONO + 30.0 });
            Mode::Hud(activities::HudTag::Charging)
        }
        Scene::LowBatteryHud => {
            st.hud = Some(Hud { kind: HudKind::LowBattery { percent: 19 }, until: NOW_MONO + 30.0 });
            Mode::Hud(activities::HudTag::LowBattery)
        }
        Scene::ShelfItems => {
            // Real files, with tiles from the Mac's own thumbnail code.
            let dir = std::env::temp_dir().join("poni-render-shelf");
            let folder = dir.join("Project assets");
            let _ = std::fs::create_dir_all(&folder);
            let notes = dir.join("Trip notes.txt");
            let _ = std::fs::write(&notes, "Weekend trip\n");
            let sketch = dir.join("Harbor sketch.png");
            let _ = crate::png::save_pbgra(&sketch, 240, 240, &make_art(240));
            let px = (crate::services::imaging::THUMB_DIP * SCALE).round() as u32;
            for (k, p) in [&folder, &notes, &sketch].into_iter().enumerate() {
                let path = p.to_string_lossy().into_owned();
                st.store.shelf.add_paths(&[path.as_str()], NOW_UNIX - 3600 + k as i64 * 600);
                let tile = crate::services::thumbs::make(&path, px);
                println!("render: shelf tile for {}: {}", p.display(), if tile.is_some() { "drawn" } else { "missing" });
                st.shelf_thumbs.insert(path, tile);
            }
            st.page = PageKind::Shelf;
            Mode::Expanded(PageKind::Shelf)
        }
        Scene::ShelfPhone => {
            let url = "http://192.168.1.20:53530/QK7WZ3MRN2D4XJ5T".to_string();
            st.phone = Some(crate::state::PhoneDrop { qr: poni_core::phonedrop::QrMatrix::encode(&url), url, ..Default::default() });
            st.page = PageKind::Shelf;
            Mode::Expanded(PageKind::Shelf)
        }
        Scene::Page(p) => {
            if p == PageKind::Media {
                media(&mut st);
            }
            if p == PageKind::Focus {
                st.pomodoro.phase = Phase::Focus;
                st.pomodoro.run = RunState::Running { ends_at: NOW_MONO + 1104.5 };
            }
            st.page = p;
            Mode::Expanded(p)
        }
        Scene::HealthSignedOut => {
            st.health = None;
            st.page = PageKind::Health;
            Mode::Expanded(PageKind::Health)
        }
        Scene::System(m) => {
            st.system = sample_system();
            st.ui.system_module = m;
            if let Some(r) = st.system.latest.clone() {
                for a in r.top_cpu.iter().chain(&r.top_memory).chain(&r.top_io) {
                    if let Some(p) = &a.path {
                        let icon = app_icon(p);
                        st.app_icons.insert(p.clone(), icon);
                    }
                }
            }
            st.page = PageKind::System;
            Mode::Expanded(PageKind::System)
        }
    };

    // The island at rest at its size for this state (as the app sizes it).
    let mut g = activities::geom(&mode, &st);
    if let Some(n) = notch {
        g = notch::geom(&mode, &st, n, g);
    }
    if hovering {
        g = crate::island::Geom::new(g.w + 12.0, g.h + 4.0, g.r + 2.0);
    }
    let mut island = Island::new(g);
    island.snap(g);

    let menu_h = notch.map_or(MENU_BAR, |n| n.h);
    let w_dip = (st.settings.expanded_width + 2.0 * MARGIN_X).max(g.w + 2.0 * (SHOULDER + MARGIN_X));
    let h_dip = g.h.max(menu_h) + MARGIN_BOTTOM + if matches!(mode, Mode::Expanded(_)) { 0.0 } else { 60.0 };
    let (w_px, h_px) = ((w_dip * SCALE).ceil() as usize, (h_dip * SCALE).ceil() as usize);
    let bmp = Bitmap::new(w_px, h_px, SCALE).ok_or("no bitmap context")?;
    let cx = w_dip / 2.0;
    let body = island.body_rect_at(cx);

    // The desktop: wallpaper and the menu bar.
    gfx.set_look(&poni_core::look::CLASSIC);
    {
        let mut c = gfx.canvas(bmp.ctx());
        let steps = 48;
        for i in 0..steps {
            let t = i as f32 / (steps - 1) as f32;
            let col = Color::lerp(Color::hex(0x2B4F86), Color::hex(0xF0708A), t);
            let y = h_dip * i as f32 / steps as f32;
            c.fill_rect(Rect::new(0.0, y, w_dip, h_dip / steps as f32 + 0.5), col);
        }
        c.fill_rect(Rect::new(0.0, 0.0, w_dip, menu_h), Color::rgba(0.96, 0.96, 0.97, 0.78));
    }

    // The island, drawn by the app's own content code (around a notch in
    // the look that meets its black).
    let look = if notch.is_some() { poni_core::look::around_notch(theme) } else { poni_core::look::of(theme) };
    gfx.set_look(look);
    {
        let mut c = gfx.canvas(bmp.ctx());
        let shoulder = notch.map_or(SHOULDER, |n| notch::shoulder(n, g.w));
        let inside = notch.is_some_and(|_| notch::fit(&mode, &st) == notch::Fit::Inside);
        if look.material == poni_core::look::Material::Glass {
            crate::glass::draw_body(&mut c, body, g.r, shoulder, notch, inside, true);
        } else {
            let border = if inside { None } else { Some(c.look().border) };
            c.notch(body, g.r, shoulder, palette::ISLAND, border);
        }
        let mut input = Input::default();
        if hovering {
            input.mouse = Some(Point::new(body.center().x + 20.0, body.center().y));
        }
        let mut hits: Vec<Hit> = Vec::new();
        crate::app::draw_content(&mut c, &mut input, &mut hits, &mut st, mode, body, body, None, NOW_MONO, notch);
    }

    // The camera notch hides its part of the island on a real MacBook.
    if let Some(n) = notch {
        let mut c = gfx.canvas(bmp.ctx());
        c.unthemed(|c| c.fill_rrect(Rect::new(cx - n.w / 2.0, -12.0, n.w, n.h + 12.0), 10.0, Color::black(1.0)));
    }

    let px = bmp.pixels();
    crate::png::save_pbgra(path, w_px as u32, h_px as u32, &px)?;
    let _ = pages::HEADER_H;
    Ok(())
}

// ---------------------------------------------------------------- the System page's sample minute

thread_local! {
    /// Each sample app's icon, drawn once a run.
    static ICONS: std::cell::RefCell<std::collections::HashMap<String, Option<crate::gfx::Image>>> = Default::default();
}

/// An app's icon the way the app gets it (`Command::LoadAppIcon`).
fn app_icon(path: &str) -> Option<crate::gfx::Image> {
    ICONS.with(|c| {
        c.borrow_mut()
            .entry(path.to_string())
            .or_insert_with(|| {
                let px = pages::system::icon_px(SCALE);
                let icon = crate::services::thumbs::app_icon(path, px).map(|i| pages::system::shrink_icon(&i, px));
                println!("render: app icon for {path}: {}", if icon.is_some() { "drawn" } else { "missing" });
                icon
            })
            .clone()
    })
}

/// Apps of the sample minute: name, bundle, CPU share, memory, disk bytes/s.
const SAMPLE_APPS: [(&str, &str, f32, u64, f64); 8] = [
    ("Safari", "/Applications/Safari.app", 0.091, 2_150 << 20, 1.2e6),
    ("Music", "/System/Applications/Music.app", 0.042, 412 << 20, 4.1e5),
    ("Mail", "/System/Applications/Mail.app", 0.018, 280 << 20, 8.2e5),
    ("Finder", "/System/Library/CoreServices/Finder.app", 0.013, 96 << 20, 4.0e4),
    ("Messages", "/System/Applications/Messages.app", 0.009, 210 << 20, 1.2e4),
    ("Xcode", "/Applications/Xcode.app", 0.008, 940 << 20, 9.6e4),
    ("Photos", "/System/Applications/Photos.app", 0.004, 520 << 20, 3.1e6),
    ("Terminal", "/System/Applications/Utilities/Terminal.app", 0.002, 88 << 20, 3.0e3),
];

fn sample_app(i: usize) -> AppUse {
    let (name, path, cpu, memory, io) = SAMPLE_APPS[i];
    AppUse { image: path.to_lowercase(), name: name.into(), path: Some(path.into()), pid: 400 + i as u64, cpu, memory, io, processes: if i == 0 { 9 } else { 1 } }
}

/// The System page's sample minute (all made up): a MacBook Air with an M2,
/// its four efficiency cores numbered before its four performance cores, as
/// Apple numbers them.
fn sample_system() -> Monitor {
    const MIB: f64 = 1024.0 * 1024.0;
    let mut m = Monitor::default();
    for i in 0..HISTORY_LEN {
        let t = i as f32;
        let last = i + 1 == HISTORY_LEN;
        let spike = if (38..45).contains(&i) { 0.22 * ((t - 38.0) / 6.0 * std::f32::consts::PI).sin() } else { 0.0 };
        let cpu = if last { 0.23 } else { (0.19 + 0.05 * (t / 4.3).sin() + 0.03 * (t / 1.7 + 1.1).sin() + spike).clamp(0.03, 0.95) };
        let gpu = if last { 0.31 } else { (0.24 + 0.08 * (t / 5.1 + 0.4).sin() + 0.04 * (t / 2.3).sin() + if (18..30).contains(&i) { 0.3 } else { 0.0 }).clamp(0.02, 0.98) };
        let burst = |at: usize, len: usize, peak: f64| if (at..at + len).contains(&i) { peak * (((i - at) as f64 + 0.5) / len as f64 * std::f64::consts::PI).sin() } else { 0.0 };
        let read = if last { 1.2 * MIB } else { 0.15 * MIB + burst(12, 6, 6.5 * MIB) + burst(41, 4, 2.8 * MIB) };
        let write = if last { 0.33 * MIB } else { 0.05 * MIB + burst(20, 9, 1.9 * MIB) };
        let down = if last { 1.2 * MIB } else { 0.08 * MIB + burst(5, 14, 3.4 * MIB) + burst(46, 10, 1.6 * MIB) };
        let up = if last { 0.082 * MIB } else { 0.02 * MIB + burst(25, 8, 0.6 * MIB) };
        m.push(sample_reading(cpu, gpu, (read, write), (down, up), t));
    }
    m
}

fn sample_reading(cpu: f32, gpu: f32, (read, write): (f64, f64), (down, up): (f64, f64), t: f32) -> Reading {
    const GIB: u64 = 1 << 30;
    let total = 16 * GIB;
    let cores = (0..8)
        .map(|k| {
            let p = k >= 4;
            let wave = 0.5 + 0.5 * (t / 3.0 + k as f32 * 1.7).sin();
            let load = if p { cpu * (0.6 + 1.1 * wave) } else { cpu * (0.3 + 0.5 * wave) };
            CoreLoad { load: load.clamp(0.02, 1.0), kind: if p { CoreKind::Performance } else { CoreKind::Efficiency } }
        })
        .collect();
    let (app, wired, compressed, cached) = (66 * GIB / 10, 21 * GIB / 10, 9 * GIB / 10, 42 * GIB / 10);
    let used = app + wired + compressed;
    let pressure = level_pressure(PressureLevel::Normal, 0.41);
    let apps = |order: [usize; 8]| order.iter().map(|&i| sample_app(i)).collect::<Vec<_>>();
    Reading {
        cpu: Some(CpuReading { load: cpu, user: cpu * 0.62, kernel: cpu * 0.38, cores, logical: 8, uptime_s: 3 * 86_400 + 4 * 3600 + 12 * 60, ..CpuReading::default() }),
        gpu: Some(GpuReading {
            name: "Apple M2".into(),
            util: gpu,
            engine_3d: gpu * 0.94,
            tiler: gpu * 0.41,
            mem_used: 13 * GIB / 10,
            mem_total: total,
            integrated: true,
            ..GpuReading::default()
        }),
        memory: Some(MemoryReading {
            total,
            used,
            committed: (f64::from(pressure) * total as f64).round() as u64,
            commit_limit: total,
            cached,
            wired,
            compressed,
            free: total - used - cached,
            page_used: 3 * GIB / 10,
            page_total: GIB,
        }),
        drives: vec![
            Drive { root: "Macintosh HD".into(), label: String::new(), total: 460 * GIB, free: 212 * GIB, system: true },
            Drive { root: "Backup".into(), label: String::new(), total: 931 * GIB, free: 640 * GIB, system: false },
        ],
        disk: Some(DiskActivity { read_bps: read, write_bps: write, active: None }),
        network: Some(NetReading {
            online: true,
            adapter: "Wi-Fi".into(),
            ipv4: Some("192.168.1.42".into()),
            mac: "3C:22:FB:0A:11:9E".into(),
            link_bps: 866_700_000,
            down_bps: down,
            up_bps: up,
            received: 34 * GIB / 10,
            sent: 410 << 20,
        }),
        battery: Some(BatteryReading {
            percent: 76,
            secs_left: Some(2 * 3600 + 14 * 60),
            design_mwh: Some(52_600),
            full_mwh: Some(48_390),
            cycles: Some(193),
            rate_mw: Some(-8_200),
            ..BatteryReading::default()
        }),
        top_cpu: apps([0, 1, 2, 3, 4, 5, 6, 7]),
        top_memory: apps([0, 5, 6, 1, 2, 4, 3, 7]),
        top_io: apps([6, 0, 2, 1, 5, 3, 4, 7]),
    }
}

// ---------------------------------------------------------------- sample album art

/// Procedural album art: a harbor sunset with a paper boat (premultiplied
/// BGRA, opaque). The same picture the Windows self-test uses.
pub(crate) fn make_art(size: u32) -> Vec<u8> {
    const SS: usize = 3;
    let n = size.max(1) as usize;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let mut acc = [0.0f32; 3];
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (x as f32 + (sx as f32 + 0.5) / SS as f32) / n as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SS as f32) / n as f32;
                    for (a, c) in acc.iter_mut().zip(art_sample(u, v)) {
                        *a += c;
                    }
                }
            }
            let o = (y * n + x) * 4;
            let to8 = |c: f32| ((c / (SS * SS) as f32).clamp(0.0, 1.0) * 255.0).round() as u8;
            px[o] = to8(acc[2]);
            px[o + 1] = to8(acc[1]);
            px[o + 2] = to8(acc[0]);
            px[o + 3] = 255;
        }
    }
    px
}

fn rgb(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 0xff) as f32 / 255.0, ((hex >> 8) & 0xff) as f32 / 255.0, (hex & 0xff) as f32 / 255.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let edge = |p0: (f32, f32), p1: (f32, f32)| (p1.0 - p0.0) * (p.1 - p0.1) - (p1.1 - p0.1) * (p.0 - p0.0);
    let (d1, d2, d3) = (edge(a, b), edge(b, c), edge(c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

fn art_sample(u: f32, v: f32) -> [f32; 3] {
    const HORIZON: f32 = 0.64;
    let (sun_x, sun_y, sun_r) = (0.66, HORIZON - 0.03, 0.15);
    let mut c = if v < HORIZON { mix(rgb(0xFFB37A), rgb(0xE0607E), v / HORIZON) } else { mix(rgb(0x2F6F8F), rgb(0x13233F), (v - HORIZON) / (1.0 - HORIZON)) };
    let d = ((u - sun_x).powi(2) + (v - sun_y).powi(2)).sqrt();
    if v < HORIZON && d < sun_r {
        c = mix(c, rgb(0xFFE3A3), 0.92);
    }
    if v > HORIZON + 0.015 && v < HORIZON + 0.24 {
        let depth = (v - HORIZON) / 0.24;
        let band = ((v - HORIZON) * 42.0).fract();
        if band < 0.4 && (u - sun_x).abs() < 0.12 * (1.0 - depth) {
            c = mix(c, rgb(0xFFD08A), 0.6 * (1.0 - depth));
        }
    }
    let paper = rgb(0xF7F3EA);
    let hull = v > 0.70 && v < 0.78 && {
        let t = (v - 0.70) / 0.08;
        u > 0.17 + 0.06 * t && u < 0.51 - 0.06 * t
    };
    let sail = in_triangle((u, v), (0.34, 0.46), (0.45, 0.70), (0.24, 0.70));
    if hull {
        c = mix(paper, rgb(0xD8D0C2), ((v - 0.70) / 0.08) * 0.5);
    }
    if sail {
        c = if u > 0.34 { mix(paper, rgb(0xD9D2C4), 0.55) } else { paper };
    }
    c
}
