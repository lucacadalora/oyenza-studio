//! `poni --render <dir> [--theme classic|oyen|glass|pill] [--notch]`: draw the island's
//! states with sample data into PNGs, offscreen, through the Mac renderer.
//! CI runs it on a Mac with no notch and no person at the screen, so this is
//! how the Mac drawing gets looked at: every page and activity, each theme,
//! with and without a (simulated) MacBook notch.
//!
//! Each picture is the island over a desktop: a soft wallpaper gradient and a
//! light menu bar. With `--notch` the camera notch is painted on top, black,
//! the way it hides that part of the island on a real MacBook. Glass is
//! macOS's own view on the real window, so here a smoked stand-in takes its
//! place (the smoke run's screenshots show the real one). Pill floats below
//! the top edge, or around a notch rests inside it and floats below the menu
//! bar for everything else (notch.rs). Around a
//! notch, the last picture is the Eyes look's hide and seek with the camera
//! as it moves, a frame a row (`render_peek`).
//!
//! The music scenes use made-up data to test the drawing; the Mac app shows
//! music once the media slice is built. The System page's scenes show a
//! made-up minute of a MacBook too, with real app icons from Finder. The
//! Japanese song is read by the Mac's own reading code, without the Japanese
//! readings add-on and then with the repo's copy of its dictionary in the
//! add-on's place (so run it from the repo).

use crate::activities::{self, Mode};
use crate::gfx::{Bitmap, Gfx, ImageData};
use crate::icons::Icon;
use crate::island::{Island, SHOULDER};
use crate::notch::{self, Notch};
use crate::pages::{self, PageKind};
use crate::services::romanize::{self, Pronounced};
use crate::state::{
    AppState, CityFile, Hud, HudKind, InlineAction, Locate, LyricsState, LyricsStatus, MediaSession, NoticeAction, OyenzaLink, OyenzaState, PowerState, SysStats, VolumeState,
};
use crate::ui::{Hit, Input};
use poni_core::addons::{Status as AddonStatus, JAPANESE_READINGS, PRAYER_TIMES};
use poni_core::datetime::LocalDateTime;
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::health::{HealthDay, HealthSnapshot, HealthSource};
use poni_core::lrc::parse_lrc;
use poni_core::oyenza::{Activity, Bill, Day, Seen, Txn, WalletRef};
use poni_core::pomodoro::{Phase, RunState};
use poni_core::power::PowerReading;
use poni_core::prayer::{PlaceList, Prayer, World};
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
/// The same song in Japanese (the Windows self-test's, written for the test;
/// the smoke run plays it too).
pub const LYRICS_JA: &str = "\
[00:38.00]朝をたたんで小さな形にした
[00:43.50]港の壁のそばで水に浮かべた
[00:49.00]端ににじんだインク、昔の名前
[00:54.50]流れを見つけて、遠くへ行った
[01:00.00]紙の舟よ、言えなかった言葉を運んで
[01:05.50]灰色の海をこえて湾の向こうへ
[01:11.00]潮がやさしければ光を見つける
[01:16.50]桟橋のランタンは夜通し灯る
[01:22.00]小さな帆はどれも出せない手紙
[01:27.50]小さな波はどれも誰かの想い
[01:33.00]だから また ひとつ おって ながすよ
[01:38.50]紙の舟よ、いつかそこで会おう OK
";
/// The Japanese readings add-on's dictionary in the repo (what the add-on's
/// release holds), relative to the repo's root.
const JA_DICT: &str = "crates/poni-core/data/ja-readings.dic.zst";

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
    /// Pill's status bar look at rest.
    IdleStatus,
    /// Music with no line to sing (lyrics off): the song in Pill's pill,
    /// floating below a notch too.
    MediaNoLine,
    /// Music with the Japanese song, the Japanese readings add-on at this
    /// status: Ready reads every line; otherwise its line shows under the
    /// lyrics (Off offers it).
    Japanese(AddonStatus),
    /// The Add-ons tab, Japanese readings at this status.
    Addons(AddonStatus),
    /// Prayer times on, in Kota Jakarta, 8 minutes before Ashar: the page,
    /// its place picker, the resting island's countdown, and the notice at
    /// the time with the adzan playing.
    Prayer,
    PrayerPicker,
    PrayerCountdown,
    PrayerNotice,
    /// Prayer times abroad: the place picker with "edin" typed, and the page
    /// for Edinburgh picked by hand (its method beside its name).
    PrayerPickerWorld,
    PrayerWorld,
    /// Bluetooth headphones connected (the headphone pop-up).
    HeadphonesHud,
    /// Oyenza, signed in as a made-up account: its page (today's and
    /// yesterday's transactions, bills due), the sign-in code, a money
    /// notice with the sum held back, and Settings signed in.
    Oyenza,
    OyenzaCode,
    /// The Oyenza tab signed out: what signing in brings, and Sign in.
    OyenzaSignedOut,
    OyenzaNotice,
    SettingsSignedIn,
    /// The Today page signed in to Oyenza: the day from Google Calendar and
    /// Gmail (`agenda::sample`), no app switched on, and a server without the
    /// day (the stat tiles, as before). Signed out, `Page(Today)` above shows
    /// the sign-in card.
    TodayDay,
    TodayNoApps,
    TodayTiles,
    /// Google Calendar switched off for Studio (Gmail on), and a date ten
    /// days on clicked in the grid.
    TodayOff,
    TodayPicked,
    /// The Stocks add-on on a made-up market (`pages::stocks::picture`):
    /// the IHSG's day, Bank BCA's year, the search and edit mode.
    Stocks(crate::pages::stocks::Picture),
    /// The Update sheet for a later version: while it downloads (or where
    /// this copy can't replace itself), with the DMG to download, and once
    /// its app is out of the DMG and checked, Install now and Install on
    /// next start.
    UpdateSheet,
    UpdateReady,
    /// Oyenza Studio Recorder installed and heard from: the Record page's
    /// choices, the page while it records (0:42), the closed island's
    /// recording (12:34; around a notch its dot and time sit in the ears),
    /// paused, and the Add-ons tab with Screen recorder's row.
    Record,
    RecordLive,
    RecordActivity,
    RecordPaused,
    AddonsRecorder,
    /// The AI tab with made-up readings (`pages::usage::picture`): plans
    /// and limits, a trouble mix, nothing found, the first look, connecting
    /// and connected Oyenza Inference, the Services list.
    Ai(crate::pages::usage::Picture),
    /// The IELTS add-on signed in: on the Mac, for now, the coach is on its
    /// way (its microphone and voice come with branch ielts-coach-mac).
    Ielts,
    /// The Downloader, signed in with Studio Pro: the Downloads page with a
    /// download coming in, one waiting, one done and one failed; the closed
    /// island while it downloads (around a notch, the ring and the percent
    /// sit in the ears); and the Add-ons tab with its row.
    Downloads,
    DownloadActivity,
    AddonsDownloader,
    /// Meeting notes, signed in with Studio Pro: the page while Oyen listens
    /// (42 s in), the closed island 12 minutes in (around a notch, Oyen in
    /// one ear and the time in the other), and the Add-ons tab with every
    /// Pro add-on listed.
    NotesListening,
    NotesActivity,
    AddonsPro,
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
    // In the Clipboard page's place (it left the app), so the numbers stay.
    Scene::OyenzaSignedOut,
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

/// Pill's own looks, after its frames strip (number 26), so every other
/// picture keeps its number.
const PILL_SCENES: [Scene; 2] = [Scene::IdleStatus, Scene::MediaNoLine];

/// The Japanese song, after Pill's pictures (numbers 26-28, in that theme
/// only), so every picture keeps its number.
const JA_SCENES: [Scene; 4] = [
    Scene::Japanese(AddonStatus::Ready),
    Scene::Japanese(AddonStatus::Off),
    // Of the add-on's size (filled in when drawn).
    Scene::Japanese(AddonStatus::Downloading { done: 1_700_000, total: 0 }),
    Scene::Japanese(AddonStatus::Failed),
];

/// The Add-ons tab, after the Japanese song.
const ADDON_SCENES: [Scene; 3] = [
    Scene::Addons(AddonStatus::Off),
    Scene::Addons(AddonStatus::Downloading { done: 1_700_000, total: 0 }),
    Scene::Addons(AddonStatus::Ready),
];

/// Prayer times, after the Add-ons tab.
const PRAYER_SCENES: [Scene; 4] = [Scene::Prayer, Scene::PrayerPicker, Scene::PrayerCountdown, Scene::PrayerNotice];
/// Prayer times abroad, after Today (so every other picture keeps its number).
const PRAYER_WORLD_SCENES: [Scene; 2] = [Scene::PrayerPickerWorld, Scene::PrayerWorld];
/// The AI tab in each of its sample states (the page's own list), last of all.
fn ai_scenes() -> impl Iterator<Item = Scene> {
    crate::pages::usage::Picture::ALL.into_iter().map(Scene::Ai)
}

/// The Settings page (behind the gear), after Prayer times.
const SETTINGS_SCENES: [Scene; 1] = [Scene::Page(PageKind::Settings)];
/// The headphone pop-up, after Settings.
const HEADPHONE_SCENES: [Scene; 1] = [Scene::HeadphonesHud];
/// Oyenza sign-in, after the headphone pop-up.
const OYENZA_SCENES: [Scene; 4] = [Scene::Oyenza, Scene::OyenzaCode, Scene::OyenzaNotice, Scene::SettingsSignedIn];
/// The System page's Power module on a Mac without a battery, after Oyenza.
const POWER_SCENES: [Scene; 1] = [Scene::System(Module::Power)];
/// The Today page signed in, after Power.
const TODAY_SCENES: [Scene; 5] = [Scene::TodayDay, Scene::TodayNoApps, Scene::TodayTiles, Scene::TodayOff, Scene::TodayPicked];
/// The Stocks page, after Prayer times abroad.
const STOCKS_SCENES: [Scene; 4] = [
    Scene::Stocks(crate::pages::stocks::Picture::Day),
    Scene::Stocks(crate::pages::stocks::Picture::Year),
    Scene::Stocks(crate::pages::stocks::Picture::Search),
    Scene::Stocks(crate::pages::stocks::Picture::Edit),
];
/// The Update sheet, after Stocks (so every other picture keeps its number).
const UPDATE_SCENES: [Scene; 2] = [Scene::UpdateSheet, Scene::UpdateReady];
/// The Record page and the recording on the closed island, every theme,
/// after the Update sheet (so every other picture keeps its number).
const RECORD_SCENES: [Scene; 5] = [Scene::Record, Scene::RecordLive, Scene::RecordActivity, Scene::RecordPaused, Scene::AddonsRecorder];
/// The IELTS page, after the Update sheet.
const IELTS_SCENES: [Scene; 1] = [Scene::Ielts];
/// The Downloader, last of all (so every other picture keeps its number).
const DOWNLOAD_SCENES: [Scene; 3] = [Scene::Downloads, Scene::DownloadActivity, Scene::AddonsDownloader];
/// Meeting notes and every Pro add-on, after the Downloader.
const NOTES_SCENES: [Scene; 3] = [Scene::NotesListening, Scene::NotesActivity, Scene::AddonsPro];

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
            Scene::IdleStatus => "idle-status".into(),
            Scene::MediaNoLine => "media-no-line".into(),
            Scene::Japanese(AddonStatus::Ready) => "page-media-japanese".into(),
            Scene::Japanese(AddonStatus::Off) => "page-media-japanese-offer".into(),
            Scene::Japanese(AddonStatus::Downloading { .. }) => "page-media-japanese-getting".into(),
            Scene::Japanese(s) => format!("page-media-japanese-{}", format!("{s:?}").to_lowercase()),
            Scene::Addons(AddonStatus::Off) => "page-addons".into(),
            Scene::Addons(AddonStatus::Downloading { .. }) => "page-addons-getting".into(),
            Scene::Addons(s) => format!("page-addons-{}", format!("{s:?}").to_lowercase()),
            Scene::Prayer => "page-prayer".into(),
            Scene::PrayerPicker => "page-prayer-picker".into(),
            Scene::PrayerCountdown => "prayer-countdown".into(),
            Scene::PrayerNotice => "prayer-notice".into(),
            Scene::PrayerPickerWorld => "page-prayer-picker-world".into(),
            Scene::PrayerWorld => "page-prayer-world".into(),
            Scene::HeadphonesHud => "headphones-hud".into(),
            Scene::Oyenza => "page-oyenza".into(),
            Scene::OyenzaSignedOut => "page-oyenza-signed-out".into(),
            Scene::OyenzaCode => "page-oyenza-code".into(),
            Scene::OyenzaNotice => "oyenza-notice".into(),
            Scene::SettingsSignedIn => "page-settings-signed-in".into(),
            Scene::TodayDay => "page-today-day".into(),
            Scene::TodayNoApps => "page-today-no-apps".into(),
            Scene::TodayTiles => "page-today-tiles".into(),
            Scene::TodayOff => "page-today-off".into(),
            Scene::TodayPicked => "page-today-picked".into(),
            Scene::Stocks(crate::pages::stocks::Picture::Day) => "page-stocks".into(),
            Scene::Stocks(p) => format!("page-stocks-{}", format!("{p:?}").to_lowercase()),
            Scene::UpdateSheet => "page-update-sheet".into(),
            Scene::UpdateReady => "page-update-ready".into(),
            Scene::Record => "page-record".into(),
            Scene::RecordLive => "page-record-live".into(),
            Scene::RecordActivity => "record-activity".into(),
            Scene::RecordPaused => "record-paused-activity".into(),
            Scene::AddonsRecorder => "page-addons-recorder".into(),
            Scene::Ai(p) => p.name().into(),
            Scene::Ielts => "page-ielts".into(),
            Scene::Downloads => "page-downloads".into(),
            Scene::DownloadActivity => "download-activity".into(),
            Scene::AddonsDownloader => "page-addons-downloader".into(),
            Scene::NotesListening => "page-notes-listening".into(),
            Scene::NotesActivity => "notes-activity".into(),
            Scene::AddonsPro => "page-addons-pro".into(),
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
        Some("pill") => Theme::Pill,
        _ => Theme::Classic,
    };
    let notch = args.iter().any(|a| a == "--notch").then_some(Notch::MACBOOK);
    let name = match theme {
        Theme::Classic => "",
        Theme::Oyen => "-oyen",
        Theme::Glass => "-glass",
        Theme::Pill => "-pill",
    };
    let suffix = format!("{name}{}", if notch.is_some() { "-notch" } else { "" });
    // A home of its own, so add-ons on this Mac don't change the pictures.
    let home = std::env::temp_dir().join("poni-render-home");
    let _ = std::fs::remove_dir_all(&home);
    std::env::set_var("PONI_HOME", &home);
    let mut failed = 0;
    let ja = read_japanese().map_err(|e| eprintln!("render: the Japanese song: {e}")).ok();
    let mut gfx = Gfx::new(SCALE);
    for (i, scene) in SCENES.into_iter().chain(SYSTEM_SCENES).enumerate() {
        let path = out.join(format!("{:02}-{}{suffix}.png", i + 1, scene.name()));
        match render_scene(&mut gfx, scene, theme, notch, ja.as_ref(), &path) {
            Ok(()) => println!("render: {}", path.display()),
            Err(e) => {
                eprintln!("render: {}: {e}", path.display());
                failed += 1;
            }
        }
    }
    // Pill: how the floating outline moves (stills can't show it), then its own looks.
    if crate::pill::on(theme) {
        let n = SCENES.len() + SYSTEM_SCENES.len() + 1;
        let path = out.join(format!("{n:02}-open-close-frames{suffix}.png"));
        match render_morph(&mut gfx, theme, notch, &path) {
            Ok(()) => println!("render: {}", path.display()),
            Err(e) => {
                eprintln!("render: {}: {e}", path.display());
                failed += 1;
            }
        }
        for (k, scene) in PILL_SCENES.into_iter().enumerate() {
            let path = out.join(format!("{:02}-{}{suffix}.png", n + 1 + k, scene.name()));
            match render_scene(&mut gfx, scene, theme, notch, ja.as_ref(), &path) {
                Ok(()) => println!("render: {}", path.display()),
                Err(e) => {
                    eprintln!("render: {}: {e}", path.display());
                    failed += 1;
                }
            }
        }
    }
    // The Japanese song, the Add-ons tab, Prayer times, Settings, the headphone pop-up, Oyenza, Power, Today signed in, Prayer times abroad, Stocks, the Update sheet, Record, the AI tab, IELTS and the Downloader in every theme, after Pill's.
    let first = SCENES.len() + SYSTEM_SCENES.len() + 2 + PILL_SCENES.len();
    let mut next = first;
    let later = SETTINGS_SCENES.into_iter().chain(HEADPHONE_SCENES).chain(OYENZA_SCENES).chain(POWER_SCENES).chain(TODAY_SCENES).chain(PRAYER_WORLD_SCENES).chain(STOCKS_SCENES).chain(UPDATE_SCENES).chain(RECORD_SCENES).chain(ai_scenes()).chain(IELTS_SCENES).chain(DOWNLOAD_SCENES).chain(NOTES_SCENES);
    for (k, scene) in JA_SCENES.into_iter().chain(ADDON_SCENES).chain(PRAYER_SCENES).chain(later).enumerate() {
        let path = out.join(format!("{:02}-{}{suffix}.png", first + k, scene.name()));
        match render_scene(&mut gfx, scene, theme, notch, ja.as_ref(), &path) {
            Ok(()) => println!("render: {}", path.display()),
            Err(e) => {
                eprintln!("render: {}: {e}", path.display());
                failed += 1;
            }
        }
        next = first + k + 1;
    }
    // The Settings window's sections, dark, and two of them light.
    for (k, (section, light)) in settings_shots().into_iter().enumerate() {
        let mode = if light { "-light" } else { "" };
        let path = out.join(format!("{:02}-settings-window-{}{mode}{suffix}.png", next + k, section.key()));
        match render_settings(&mut gfx, section, theme, light, &path) {
            Ok(()) => println!("render: {}", path.display()),
            Err(e) => {
                eprintln!("render: {}: {e}", path.display());
                failed += 1;
            }
        }
    }
    let next = next + settings_shots().len();
    // Around a notch, last: the Eyes look's hide and seek with the camera.
    if let Some(n) = notch {
        let path = out.join(format!("{next:02}-peek-frames{suffix}.png"));
        match render_peek(&mut gfx, theme, n, &path) {
            Ok(()) => println!("render: {}", path.display()),
            Err(e) => {
                eprintln!("render: {}: {e}", path.display());
                failed += 1;
            }
        }
    }
    let _ = std::fs::remove_dir_all(&home);
    if failed > 0 {
        1
    } else {
        0
    }
}

/// The Settings window's pictures: every section dark, General and
/// Appearance light too.
fn settings_shots() -> Vec<(crate::settings_window::Section, bool)> {
    use crate::settings_window::Section;
    let mut v: Vec<_> = Section::ALL.into_iter().filter(|s| *s != Section::Prayer).map(|s| (s, false)).collect();
    v.extend([(Section::General, true), (Section::Appearance, true)]);
    v
}

/// One section of the Settings window as the Mac shows it, light or dark,
/// with the system's window buttons where macOS puts them (the real window
/// draws those itself; the smoke test pictures that one).
fn render_settings(gfx: &mut Gfx, section: crate::settings_window::Section, theme: Theme, light: bool, path: &Path) -> Result<(), String> {
    use crate::settings_host::{HEIGHT, TITLEBAR, WIDTH};
    let (w, h) = (WIDTH as f32, HEIGHT as f32);
    let (w_px, h_px) = ((w * SCALE) as usize, (h * SCALE) as usize);
    let bmp = Bitmap::new(w_px, h_px, SCALE).ok_or("no bitmap")?;
    let mut st = AppState::new(Settings { theme, ..Settings::default() }, Store::default());
    st.accent = crate::app::theme_accent(theme);
    st.ui.autostart = Some(true);
    let mut view = crate::settings_window::View::new(section);
    let frame = crate::settings_window::Frame { card: Rect::new(0.0, 0.0, w, h), own_chrome: false, top: TITLEBAR };
    gfx.set_look(poni_core::look::glass(light));
    {
        let mut c = gfx.canvas(bmp.ctx());
        let _ = view.draw(&mut c, &mut st, NOW_MONO, frame, light);
        // Close, minimise and zoom, 12 points across and 20 apart from 20 in.
        c.unthemed(|c| {
            for (i, rgb) in [0xFF5F57u32, 0xFEBC2E, 0x28C840].into_iter().enumerate() {
                c.fill_circle(Point::new(20.0 + 6.0 + i as f32 * 20.0, TITLEBAR / 2.0), 6.0, Color::hex(rgb));
            }
        });
    }
    let px = bmp.pixels();
    crate::png::save_pbgra(path, w_px as u32, h_px as u32, &px)
}

/// The island opening from rest onto the Today page (left column) and
/// closing again (right), a frame every few sixtieths of a second, stepped on
/// the app's own springs: for the Pill theme, how the floating outline moves
/// and, around a notch, how it lets go of the edge and comes back.
fn render_morph(gfx: &mut Gfx, theme: Theme, notch: Option<Notch>, path: &Path) -> Result<(), String> {
    /// Sixtieths of a second after the change, one row each.
    const FRAMES: [usize; 12] = [0, 2, 4, 6, 8, 10, 13, 16, 20, 25, 30, 45];
    const DT: f32 = 1.0 / 60.0;
    /// The strip is drawn at one pixel per DIP (it is long).
    const STRIP_SCALE: f32 = 1.0;
    let mut st = base_state(theme);
    st.page = PageKind::Today;
    let (rest, open) = (Mode::Idle, Mode::Expanded(PageKind::Today));
    let size = |m: &Mode, st: &AppState| {
        let g = activities::geom(m, st);
        notch.map_or(g, |n| notch::geom(m, st, n, g))
    };
    let gap_at = |m: &Mode, st: &AppState| notch.map_or(Some(crate::pill::TOP_GAP), |n| notch::pill_gap(m, st, n));
    let ends = [(rest, size(&rest, &st), gap_at(&rest, &st)), (open, size(&open, &st), gap_at(&open, &st))];
    let menu_h = notch.map_or(MENU_BAR, |n| n.h);
    let cell_w = st.settings.expanded_width + 2.0 * MARGIN_X;
    let cell_h = (notch.map_or(0.0, |n| n.h) + crate::pill::TOP_GAP + ends[1].1.h * 1.06 + 28.0).ceil();
    let (w_px, h_px) = ((2.0 * cell_w * STRIP_SCALE).ceil() as usize, (FRAMES.len() as f32 * cell_h * STRIP_SCALE).ceil() as usize);
    let bmp = Bitmap::new(w_px, h_px, STRIP_SCALE).ok_or("no bitmap context")?;
    let look = if notch.is_some() { poni_core::look::around_notch(theme) } else { poni_core::look::of(theme) };
    for (col, (from, to)) in [(0usize, 1usize), (1, 0)].into_iter().enumerate() {
        let (_, g_from, gap_from) = ends[from];
        let (to_mode, g_to, gap_to) = ends[to];
        let mut island = Island::new(g_from);
        island.set_floating(gap_from);
        island.snap(g_from);
        // A first (still) frame, so the new gap below slides instead of snapping.
        island.step(0.0, NOW_MONO);
        island.set_target(g_to, NOW_MONO, true);
        island.set_floating(gap_to);
        let (mut k, mut now) = (0usize, NOW_MONO);
        for (row, &f) in FRAMES.iter().enumerate() {
            while k < f {
                island.step(DT, now);
                now += DT as f64;
                k += 1;
            }
            let (x, y) = (col as f32 * cell_w, row as f32 * cell_h);
            let cx = cell_w / 2.0;
            let body = island.body_rect_at(cx);
            let geom = island.current();
            let shoulder = notch.map_or(SHOULDER, |n| notch::shoulder(n, geom.w));
            let detach = notch::detach(island.top(), notch);
            gfx.set_look(&poni_core::look::CLASSIC);
            {
                let mut c = gfx.canvas(bmp.ctx());
                c.translate(x, y);
                c.clip(Rect::new(0.0, 0.0, cell_w, cell_h));
                let steps = 24;
                for i in 0..steps {
                    let t = i as f32 / (steps - 1) as f32;
                    let shade = Color::lerp(Color::hex(0x2B4F86), Color::hex(0xF0708A), t);
                    c.fill_rect(Rect::new(0.0, cell_h * i as f32 / steps as f32, cell_w, cell_h / steps as f32 + 0.5), shade);
                }
                c.fill_rect(Rect::new(0.0, 0.0, cell_w, menu_h), Color::rgba(0.96, 0.96, 0.97, 0.78));
                c.unclip();
                c.reset_transform();
            }
            gfx.set_look(look);
            {
                let mut c = gfx.canvas(bmp.ctx());
                c.translate(x, y);
                // Each frame keeps to its cell (the notch is drawn from above it).
                c.clip(Rect::new(0.0, 0.0, cell_w, cell_h));
                let border = Some(c.look().border);
                if island.floating() {
                    notch::draw_pill_body(&mut c, body, geom.r, shoulder, detach, palette::ISLAND, border);
                    c.push_outline_mask(&notch::pill_outline(body, geom.r, shoulder, detach));
                } else {
                    c.notch(body, geom.r, shoulder, palette::ISLAND, border);
                    c.push_shape_mask(body, geom.r, shoulder);
                }
                // The incoming content as it fades in (the app also fades the old one out).
                let lay = match to_mode {
                    Mode::Expanded(_) => Rect::new(body.center().x - g_to.w / 2.0, body.y, g_to.w, g_to.h),
                    _ => body,
                };
                let (mut input, mut hits) = (Input::default(), Vec::<Hit>::new());
                let alpha = island.content_alpha();
                c.with_alpha(alpha, |c| crate::app::draw_content(c, &mut input, &mut hits, &mut st, to_mode, lay, body, None, now, notch));
                c.pop_shape_mask();
                if let Some(n) = notch {
                    c.unthemed(|c| c.fill_rrect(Rect::new(cx - n.w / 2.0, -12.0, n.w, n.h + 12.0), 10.0, Color::black(1.0)));
                }
                let label = format!("{} {:.0} ms", if to == 1 { "open" } else { "close" }, f as f32 * 1000.0 / 60.0);
                c.unthemed(|c| c.text(&label, Rect::new(10.0, cell_h - 22.0, 160.0, 16.0), &crate::gfx::TextStyle::caption().color(Color::white(0.92))));
                c.unclip();
                c.reset_transform();
            }
        }
    }
    let px = bmp.pixels();
    crate::png::save_pbgra(path, w_px as u32, h_px as u32, &px)
}

/// The Eyes look around notch `n` as it moves (stills can't show it), a
/// frame a row, a fifteenth of a second apart: the pointer (drawn, so each
/// frame says what the eyes follow) wanders right of the camera and crosses
/// over; the eyes (Oyen, in his theme) duck behind the camera and come out
/// on its left, follow it up into the menu bar and back under the camera.
fn render_peek(gfx: &mut Gfx, theme: Theme, n: Notch, path: &Path) -> Result<(), String> {
    /// The pointer from the island's top centre (DIPs) at each time
    /// (seconds), in a straight line from one to the next.
    const WAY: [(f64, f32, f32); 7] = [
        (0.0, 250.0, 118.0),
        (0.6, 170.0, 70.0),
        (1.2, -60.0, 95.0),
        (1.5, -130.0, 105.0),
        (2.4, -235.0, 80.0),
        (3.0, -215.0, 14.0),
        (3.6, -40.0, 118.0),
    ];
    const FPS: f64 = 15.0;
    const CELL: (f32, f32) = (560.0, 140.0);
    let mut st = base_state(theme);
    st.settings.idle_style = IdleStyle::Eyes;
    let mode = Mode::Idle;
    let g = notch::geom(&mode, &st, n, activities::geom(&mode, &st));
    let frames = (WAY[WAY.len() - 1].0 * FPS).round() as usize + 1;
    let (w_px, h_px) = ((CELL.0 * SCALE).ceil() as usize, (frames as f32 * CELL.1 * SCALE).ceil() as usize);
    let bmp = Bitmap::new(w_px, h_px, SCALE).ok_or("no bitmap context")?;
    let look = poni_core::look::around_notch(theme);
    let at = |t: f64| {
        let i = WAY.iter().rposition(|w| w.0 <= t).unwrap_or(0).min(WAY.len() - 2);
        let (a, b) = (WAY[i], WAY[i + 1]);
        let k = ((t - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) as f32;
        Point::new(a.1 + (b.1 - a.1) * k, a.2 + (b.2 - a.2) * k)
    };
    for f in 0..frames {
        let t = f as f64 / FPS;
        st.now = NOW_MONO + t;
        let p = at(t);
        st.ui.peek.point(p, st.now, n.w / 2.0);
        let (x, y) = (0.0, f as f32 * CELL.1);
        let cx = CELL.0 / 2.0;
        let mut island = Island::new(g);
        island.snap(g);
        let body = island.body_rect_at(cx);
        gfx.set_look(&poni_core::look::CLASSIC);
        {
            let mut c = gfx.canvas(bmp.ctx());
            c.translate(x, y);
            c.clip(Rect::new(0.0, 0.0, CELL.0, CELL.1));
            let steps = 24;
            for i in 0..steps {
                let k = i as f32 / (steps - 1) as f32;
                let shade = Color::lerp(Color::hex(0x2B4F86), Color::hex(0xF0708A), k);
                c.fill_rect(Rect::new(0.0, CELL.1 * i as f32 / steps as f32, CELL.0, CELL.1 / steps as f32 + 0.5), shade);
            }
            c.fill_rect(Rect::new(0.0, 0.0, CELL.0, n.h), Color::rgba(0.96, 0.96, 0.97, 0.78));
            c.unclip();
            c.reset_transform();
        }
        gfx.set_look(look);
        {
            let mut c = gfx.canvas(bmp.ctx());
            c.translate(x, y);
            c.clip(Rect::new(0.0, 0.0, CELL.0, CELL.1));
            let shoulder = notch::shoulder(n, g.w);
            if look.material == poni_core::look::Material::Glass {
                crate::glass::draw_body(&mut c, body, g.r, shoulder, Some(n), false, true);
            } else {
                let border = Some(c.look().border);
                c.notch(body, g.r, shoulder, palette::ISLAND, border);
            }
            let (mut input, mut hits) = (Input::default(), Vec::<Hit>::new());
            let now = st.now;
            crate::app::draw_content(&mut c, &mut input, &mut hits, &mut st, mode, body, body, None, now, Some(n));
            c.unthemed(|c| {
                c.fill_rrect(Rect::new(cx - n.w / 2.0, -12.0, n.w, n.h + 12.0), 10.0, Color::black(1.0));
                pointer_arrow(c, Point::new(cx + p.x, p.y));
            });
            c.unclip();
            c.reset_transform();
        }
    }
    let px = bmp.pixels();
    crate::png::save_pbgra(path, w_px as u32, h_px as u32, &px)
}

/// The Mac's arrow pointer, its tip at `tip`: black, outlined in white.
fn pointer_arrow(c: &mut crate::gfx::Canvas, tip: Point) {
    use poni_core::svgpath::Seg;
    const ARROW: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 16.0), (4.0, 12.3), (6.9, 18.6), (9.6, 17.4), (6.8, 11.3), (12.0, 11.3)];
    let mut segs: Vec<Seg> = ARROW
        .iter()
        .enumerate()
        .map(|(i, (dx, dy))| if i == 0 { Seg::MoveTo(tip.x + dx, tip.y + dy) } else { Seg::LineTo(tip.x + dx, tip.y + dy) })
        .collect();
    segs.push(Seg::Close);
    c.path(&segs, Some(Color::white(1.0)), Some((Color::white(1.0), 2.4)));
    c.path(&segs, Some(Color::black(1.0)), None);
}

/// The Japanese song read the way the Mac reads it: without the Japanese
/// readings add-on (kana lines only, and the offer), then with the repo's
/// copy of its dictionary where the add-on keeps it (every line).
fn read_japanese() -> Result<[Pronounced; 2], String> {
    let lyrics = parse_lrc(LYRICS_JA);
    let needing = lyrics.lines.iter().filter(|l| poni_core::romanize::needs_roman(l.text.trim())).count();
    let read = |p: &Pronounced| p.ruby.iter().filter(|r| r.iter().any(|piece| !piece.roman.is_empty())).count();
    let without = romanize::pronounce(&lyrics);
    let file = &JAPANESE_READINGS.files[0];
    let bytes = std::fs::read(JA_DICT).map_err(|e| format!("{JA_DICT}: {e} (render from the repo's root)"))?;
    if !file.matches(&bytes) {
        return Err(format!("{JA_DICT}: {} bytes, not the dictionary the add-on pins", bytes.len()));
    }
    let to = crate::services::addons::file_path(&JAPANESE_READINGS, file.name);
    to.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&to, &bytes)).map_err(|e| format!("{}: {e}", to.display()))?;
    let with = romanize::pronounce(&lyrics);
    println!("render: Japanese song without the add-on: {} of {needing} lines with romaji, offer {}", read(&without), without.wants_ja_readings);
    println!("render: Japanese song with the add-on: {} of {needing} lines with romaji, offer {}", read(&with), with.wants_ja_readings);
    // Without it every kanji line is missing a reading, and the page offers it.
    if !without.wants_ja_readings || read(&with) != needing || with.wants_ja_readings {
        return Err("the Mac's readings aren't what the add-on should make them".into());
    }
    Ok([with, without])
}

/// Prayer times on, in Kota Jakarta (found by the location), its clock 8
/// minutes before today's Ashar; `picking`: the place picker with "band"
/// typed. Returns Jakarta's site and today's Ashar (UTC seconds).
fn seed_prayer(st: &mut AppState, picking: bool) -> Result<(poni_core::prayer::Site, i64), String> {
    st.settings.set_addon(PRAYER_TIMES.id, true);
    st.addons.insert(PRAYER_TIMES.id, AddonStatus::Ready);
    let list = PlaceList::parse(SAMPLE_PLACES)?;
    st.prayer.place = list.find("jakarta").cloned();
    st.prayer.places = Some(std::sync::Arc::new(list));
    st.prayer.suggested = false;
    st.prayer.locate = Locate::Found { km: 2.0 };
    st.prayer.located_at = Some(st.unix);
    st.prayer.picking = picking;
    st.prayer.query = if picking { "band".into() } else { String::new() };
    // The website's "ed" file, as if it had come.
    let ed = World::parse(poni_core::prayer::world::SAMPLE.into())?;
    st.prayer.cities = [("ed".to_string(), CityFile::Here(std::sync::Arc::new(ed)))].into();
    let site = st.prayer.site().ok_or("no site for Kota Jakarta")?;
    let ashar = poni_core::prayer::day_times(&site, site.local_day(st.unix)).ok_or("no times today")?.ashar;
    st.prayer.clock_shift = ashar - 8 * 60 - st.unix;
    st.prayer.handled_until = Some(st.prayer_now());
    Ok((site, ashar))
}

/// Prayer times abroad: the place picker with "edin" typed (`picking`), or
/// the page for Edinburgh, picked by hand, 8 minutes before its Ashar.
fn seed_prayer_world(st: &mut AppState, picking: bool) -> Result<(), String> {
    seed_prayer(st, picking)?;
    if picking {
        st.prayer.query = "edin".into();
        return Ok(());
    }
    st.prayer.place = match st.prayer.cities.get("ed") {
        Some(CityFile::Here(w)) => w.place(0),
        _ => None,
    };
    st.settings.prayer.world_place = st.prayer.place.as_ref().and_then(|p| p.world_lines.clone());
    st.settings.prayer.follow_location = false;
    let site = st.prayer.site().ok_or("no site for Edinburgh")?;
    let ashar = poni_core::prayer::day_times(&site, site.local_day(st.unix)).ok_or("no times in Edinburgh today")?.ashar;
    st.prayer.clock_shift = ashar - 8 * 60 - st.unix;
    st.prayer.handled_until = Some(st.prayer_now());
    Ok(())
}

/// A few places from the add-on's list (as the Windows self-test has them).
const SAMPLE_PLACES: &str = r#"{"places":[
    {"id":"jakarta","label":"Kota Jakarta","kind":"kota","prov":"DKI Jakarta","lat":-6.1833,"lng":106.8333,"tz":"Asia/Jakarta"},
    {"id":"banda-aceh","label":"Kota Banda Aceh","kind":"kota","prov":"Aceh","lat":5.55,"lng":95.3167,"tz":"Asia/Jakarta"},
    {"id":"bandar-lampung","label":"Kota Bandar Lampung","kind":"kota","prov":"Lampung","lat":-5.4167,"lng":105.2667,"tz":"Asia/Jakarta"},
    {"id":"kab-bandung","label":"Kab. Bandung","kind":"kab","prov":"Jawa Barat","lat":-7.0167,"lng":107.5333,"tz":"Asia/Jakarta","h0":-2},
    {"id":"bandung","label":"Kota Bandung","kind":"kota","prov":"Jawa Barat","lat":-6.9,"lng":107.6167,"tz":"Asia/Jakarta","h0":-2},
    {"id":"kab-bandung-barat","label":"Kab. Bandung Barat","kind":"kab","prov":"Jawa Barat","lat":-6.85,"lng":107.5167,"tz":"Asia/Jakarta","h0":-2},
    {"id":"makkah","label":"Makkah","kind":"abroad","prov":"Arab Saudi","lat":21.4225,"lng":39.8261,"tz":"Asia/Riyadh","method":"ummalqura"}
]}"#;

/// The Downloads page's list, newest first, as on Windows (`selftest.rs`):
/// a song waiting its turn, a film coming in from PeerTube, a private video
/// that failed and a finished film, moved along by yt-dlp's lines as the app
/// moves them, so the bar and the island's pill count the bytes as they do
/// for real. No pictures, so nothing is fetched.
fn seed_downloads(st: &mut AppState) {
    use poni_core::downloads::{Jobs, State};
    use poni_core::ytdlp::{Line, Mode as Get};
    let unix = st.unix;
    let info = |title: &str, site: &str| Line::Info { title: title.into(), site: site.into(), thumbnail: None, duration_s: None };
    let mut jobs = Jobs::default();
    // Oldest first: each one goes on top of the list.
    let bunny = jobs.add("https://archive.org/details/BigBuckBunny_124".into(), Get::Video, unix - 25 * 60);
    jobs.starting(bunny);
    jobs.apply(bunny, info("Big Buck Bunny", "Internet Archive"));
    jobs.apply(bunny, Line::Done { path: "/Users/sample/Downloads/Big Buck Bunny.mp4".into() });
    jobs.set_size(bunny, 332_000_000);
    let private = jobs.add("https://video.blender.org/w/4e0c1f7a-93b2-4d58-a6e1-2f8b9c3d7a51".into(), Get::Video, unix - 4 * 60);
    // yt-dlp's private video (`ytdlp::Problem::Private`), in the page's words.
    if let Some(job) = jobs.list.iter_mut().find(|j| j.id == private) {
        job.state = State::Failed("This video is private".into());
    }
    let film = jobs.add("https://video.blender.org/w/b29290cc-dc51-4a12-bcb2-2aa5fece7605".into(), Get::Video, unix - 50);
    jobs.starting(film);
    jobs.apply(film, info("OVERGROWN: a feature film project by Blender Studio", "PeerTube"));
    jobs.apply(film, Line::Progress { done: 8_600_000, total: Some(23_300_000), speed: Some(2_100_000.0), eta_s: Some(7) });
    jobs.add("https://archive.org/details/ElephantsDream".into(), Get::Audio, unix - 20);
    st.downloads = jobs;
}

/// The Health add-on switched on, as its row in Add-ons does.
fn health_on(st: &mut AppState) {
    st.settings.set_addon(poni_core::addons::HEALTH.id, true);
    st.addons.insert(poni_core::addons::HEALTH.id, AddonStatus::Ready);
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
    // Oyen in the Oyenza tab long since arrived: at rest in every picture.
    st.ui.oyen_tab = Some((crate::oyen::TabMove::Arrive, f64::NEG_INFINITY));
    seed(&mut st);
    st
}

/// A later version than this build, with its DMG, for the Update sheet.
fn fake_release() -> poni_core::update::Release {
    let mut parts: Vec<u64> = crate::version::VERSION.split('.').filter_map(|p| p.parse().ok()).collect();
    if let Some(last) = parts.last_mut() {
        *last += 1;
    }
    let version = parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
    let repo = "https://github.com/lucacadalora/oyenza-studio/releases";
    poni_core::update::Release {
        date: "2026-10-27".into(),
        headline: "Updates install themselves on a Mac".into(),
        notes: format!("{repo}/tag/v{version}"),
        windows: None,
        mac: Some(poni_core::update::Download { url: format!("{repo}/download/v{version}/Oyenza-Studio-{version}-mac.dmg"), size: 4_300_000, sha256: "0".repeat(64) }),
        version,
    }
}

/// Oyenza for its scenes, all made up: today's and yesterday's
/// transactions (today's newest just landed, so it glows), bills due
/// tomorrow and later; or, signing in, the code to type in oyenza.app.
pub(crate) fn fake_oyenza(st: &AppState, connecting: bool) -> OyenzaState {
    let iso = |t: i64| {
        let (y, m, d) = poni_core::datetime::civil_from_days(t.div_euclid(86_400));
        let s = t.rem_euclid(86_400);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
    };
    let midnight = st.unix - (st.local.hour as i64 * 3600 + st.local.minute as i64 * 60 + st.local.second as i64);
    let at = |days_ago: i64, h: i64, m: i64| iso(midnight - days_ago * 86_400 + h * 3600 + m * 60);
    let txn = |id: &str, kind: &str, title: &str, text: &str, wallet: (&str, &str), at: String, auto: bool| Txn {
        id: id.into(),
        kind: kind.into(),
        title: title.into(),
        amount: 1.0,
        text: text.into(),
        category: None,
        wallet: Some(WalletRef { name: wallet.0.into(), color: wallet.1.into() }),
        to_wallet: None,
        at,
        auto,
    };
    let (bca, gopay, ovo) = (("BCA", "#0060AF"), ("GoPay", "#00AED6"), ("OVO", "#4C3494"));
    let day = |n: i64| st.local.add_days(n).day_key();
    let today = Day {
        date: day(0),
        count: 3,
        in_sum: 1_250_000.0,
        out: 63_000.0,
        in_text: "Rp 1.250.000".into(),
        out_text: "Rp 63.000".into(),
        rows: vec![
            txn("t3", "income", "Gaji Oktober", "Rp 1.250.000", bca, at(0, 9, 38), true),
            txn("t2", "expense", "Kopi susu", "Rp 25.000", gopay, at(0, 8, 51), false),
            txn("t1", "expense", "Grab ke kantor", "Rp 38.000", ovo, at(0, 8, 2), false),
        ],
    };
    let yesterday = Day {
        date: day(-1),
        count: 2,
        in_sum: 0.0,
        out: 390_000.0,
        in_text: "Rp 0".into(),
        out_text: "Rp 390.000".into(),
        rows: vec![txn("y2", "expense", "Makan siang", "Rp 40.000", gopay, at(1, 12, 40), false), txn("y1", "expense", "Listrik PLN", "Rp 350.000", bca, at(1, 10, 2), true)],
    };
    let empty = |n: i64| Day { date: day(-n), count: 0, in_sum: 0.0, out: 0.0, in_text: "Rp 0".into(), out_text: "Rp 0".into(), rows: Vec::new() };
    let days = vec![today, yesterday, empty(2), empty(3), empty(4), empty(5), empty(6)];
    // Everything was on the page a while ago except today's newest, which just landed.
    let mut seen = Seen::default();
    let mut before = days.clone();
    before[0].rows.remove(0);
    seen.note(&before, st.now - 120.0);
    seen.note(&days, st.now - 12.0);
    let link = if connecting {
        OyenzaLink::Connecting { code: "KPJX-TMNQ".into(), url: "https://oyenza.app/connect?code=KPJX-TMNQ".into(), until: st.now + 581.0 }
    } else {
        OyenzaLink::Connected { account: "a\u{2022}\u{2022}\u{2022}@example.com".into() }
    };
    let bill = |kind: &str, name: &str, due: String, text: Option<&str>| Bill { kind: kind.into(), name: name.into(), due, amount: text.map(|_| 1.0), text: text.map(Into::into) };
    OyenzaState {
        link,
        activity: Some(Activity { cursor: "42".into(), today: day(0), days }),
        seen,
        bills: vec![
            bill("subscription", "Spotify", day(1), Some("Rp 54.990")),
            bill("card", "Kartu Kredit BCA", day(1), None),
            bill("subscription", "Netflix", day(5), Some("Rp 54.000")),
        ],
        bills_today: st.day_key(),
        updated_unix: Some(st.unix - 8),
        ..OyenzaState::default()
    }
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
    st.lyrics = LyricsState { track_key: MediaSession::make_track_key(TITLE, ARTIST, ALBUM), status: LyricsStatus::Found, lyrics: Some(parse_lrc(LYRICS_LRC)), source: "Sample".into(), ruby: Vec::new(), wants_ja_readings: false };
}

/// Music playing the Japanese song with its `readings`, the Japanese
/// readings add-on at `status`. Each status is a song of its own (an album
/// the page doesn't show): the page keeps a song's lyric scroll from one
/// frame to the next, and these pictures follow each other.
fn media_japanese(st: &mut AppState, readings: &Pronounced, status: AddonStatus) {
    let status = ja_status(st, status);
    media(st);
    let key = match status {
        AddonStatus::Ready => MediaSession::make_track_key(TITLE, ARTIST, ALBUM),
        s => MediaSession::make_track_key(TITLE, ARTIST, &format!("{ALBUM} ({s:?})")),
    };
    if let Some(m) = st.media.as_mut() {
        m.track_key = key.clone();
    }
    st.lyrics.track_key = key;
    st.lyrics.lyrics = Some(parse_lrc(LYRICS_JA));
    st.lyrics.ruby = readings.ruby.clone();
    st.lyrics.wants_ja_readings = readings.wants_ja_readings;
}

/// The Japanese readings add-on at `status` (a download's total filled in),
/// switched on for every status but Off.
fn ja_status(st: &mut AppState, status: AddonStatus) -> AddonStatus {
    let status = match status {
        AddonStatus::Downloading { done, .. } => AddonStatus::Downloading { done, total: JAPANESE_READINGS.size() },
        s => s,
    };
    st.settings.set_addon(JAPANESE_READINGS.id, status.is_on());
    st.addons.insert(JAPANESE_READINGS.id, status);
    status
}

fn render_scene(gfx: &mut Gfx, scene: Scene, theme: Theme, notch: Option<Notch>, ja: Option<&[Pronounced; 2]>, path: &Path) -> Result<(), String> {
    let mut st = base_state(theme);
    let mut hovering = false;
    let mode = match scene {
        Scene::IdleClock => Mode::Idle,
        // Around a notch the eyes look at the pointer: down at it in the
        // window under the menu bar (Oyen rests again a while after it
        // moved), and over the notch while it hovers there.
        Scene::IdleEyes => {
            st.settings.idle_style = IdleStyle::Eyes;
            if let Some(n) = notch {
                st.ui.peek.point(Point::new(240.0, 320.0), NOW_MONO - 10.0, n.w / 2.0);
            }
            Mode::Idle
        }
        Scene::Hover => {
            st.settings.idle_style = IdleStyle::Eyes;
            hovering = true;
            if let Some(n) = notch {
                st.ui.peek.point(Point::new(20.0, n.h / 2.0), NOW_MONO - 0.5, n.w / 2.0);
            }
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
        Scene::HeadphonesHud => {
            st.hud = Some(Hud { kind: HudKind::Bluetooth { name: "AirPods Pro".into(), connected: true, battery: None, audio: true }, until: NOW_MONO + 30.0 });
            Mode::Hud(activities::HudTag::Bluetooth)
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
            // Health is an add-on, off by default: on for its own page.
            if p == PageKind::Health {
                health_on(&mut st);
            }
            if p == PageKind::Focus {
                st.pomodoro.phase = Phase::Focus;
                st.pomodoro.run = RunState::Running { ends_at: NOW_MONO + 1104.5 };
            }
            st.page = p;
            Mode::Expanded(p)
        }
        Scene::UpdateSheet | Scene::UpdateReady => {
            st.update = crate::state::UpdateView { offer: Some(fake_release()), sheet: true, ready: scene == Scene::UpdateReady, ..Default::default() };
            st.page = PageKind::Settings;
            Mode::Expanded(PageKind::Settings)
        }
        Scene::TodayDay | Scene::TodayNoApps | Scene::TodayTiles | Scene::TodayOff | Scene::TodayPicked => {
            use poni_core::agenda::{day_num, sample, Answer, Source};
            st.oyenza = fake_oyenza(&st, false);
            let today = st.day_key();
            let now_min = st.local.hour as i32 * 60 + st.local.minute as i32;
            // The list starts at today, or at the date clicked ten days on.
            st.ui.today_pick = (scene == Scene::TodayPicked).then(|| day_num(&today).unwrap_or(0) + 10);
            st.ui.today_jump = st.ui.today_pick.is_some();
            st.ui.today_drawn = st.now;
            let o = &mut st.oyenza;
            o.agenda_unix = Some(st.unix - 120);
            match scene {
                Scene::TodayDay | Scene::TodayPicked => {
                    o.agenda = Some(sample(&today, now_min));
                    o.agenda_state = crate::state::AgendaState::Ready;
                }
                Scene::TodayOff => {
                    let mut a = sample(&today, now_min);
                    a.apps.googlecalendar = Source::Off;
                    a.events.clear();
                    o.agenda = Some(a);
                    o.agenda_state = crate::state::AgendaState::Ready;
                }
                Scene::TodayNoApps => {
                    o.agenda = Some(Answer::none(&today));
                    o.agenda_state = crate::state::AgendaState::Ready;
                }
                _ => o.agenda_state = crate::state::AgendaState::Unavailable,
            }
            st.page = PageKind::Today;
            Mode::Expanded(PageKind::Today)
        }
        Scene::Ielts => {
            // IELTS is an add-on, off by default: on for its own scene, signed in.
            st.settings.set_addon(poni_core::addons::IELTS.id, true);
            st.addons.insert(poni_core::addons::IELTS.id, AddonStatus::Ready);
            st.oyenza = fake_oyenza(&st, false);
            st.ielts = poni_core::ielts::Coach::sample(poni_core::ielts::Sample::Home, st.now);
            st.page = PageKind::Ielts;
            Mode::Expanded(PageKind::Ielts)
        }
        Scene::Downloads | Scene::DownloadActivity | Scene::AddonsDownloader => {
            // The Downloader is an add-on, off by default, for Studio Pro: on
            // for its own scenes, signed in with Pro.
            st.oyenza = fake_oyenza(&st, false);
            st.oyenza.pro = Some(true);
            st.downloader_offered = true;
            st.settings.set_addon(poni_core::addons::DOWNLOADER.id, true);
            st.addons.insert(poni_core::addons::DOWNLOADER.id, AddonStatus::Ready);
            match scene {
                Scene::AddonsDownloader => {
                    st.page = PageKind::Addons;
                    Mode::Expanded(PageKind::Addons)
                }
                Scene::DownloadActivity => {
                    seed_downloads(&mut st);
                    Mode::Activity(activities::Activity::Download)
                }
                _ => {
                    seed_downloads(&mut st);
                    st.page = PageKind::Downloads;
                    Mode::Expanded(PageKind::Downloads)
                }
            }
        }
        Scene::NotesListening | Scene::NotesActivity | Scene::AddonsPro => {
            // Signed in with Studio Pro, every Pro add-on switched on.
            st.oyenza = fake_oyenza(&st, false);
            st.oyenza.pro = Some(true);
            st.downloader_offered = true;
            for a in [poni_core::addons::MEETING_NOTES, poni_core::addons::IELTS, poni_core::addons::DOWNLOADER] {
                st.settings.set_addon(a.id, true);
                st.addons.insert(a.id, AddonStatus::Ready);
            }
            let now = st.now;
            match scene {
                Scene::AddonsPro => {
                    st.page = PageKind::Addons;
                    Mode::Expanded(PageKind::Addons)
                }
                _ => {
                    let secs = if scene == Scene::NotesListening { 42.5 } else { 754.5 };
                    st.notes.started(now - secs, true, true);
                    (st.notes.mic, st.notes.pc) = (0.32, 0.55);
                    if scene == Scene::NotesActivity {
                        Mode::Activity(activities::Activity::Notes)
                    } else {
                        st.page = PageKind::Notes;
                        Mode::Expanded(PageKind::Notes)
                    }
                }
            }
        }
        Scene::Stocks(shows) => {
            // Stocks is an add-on, off by default: on for its own scenes.
            st.settings.set_addon(poni_core::addons::STOCKS.id, true);
            st.addons.insert(poni_core::addons::STOCKS.id, AddonStatus::Ready);
            let now = st.now;
            crate::pages::stocks::picture(&mut st, shows, now);
            st.page = PageKind::Stocks;
            Mode::Expanded(PageKind::Stocks)
        }
        Scene::Record | Scene::RecordLive | Scene::RecordActivity | Scene::RecordPaused | Scene::AddonsRecorder => {
            use poni_core::recorder::Phase as RecPhase;
            st.recorder = true;
            st.store.recorder_island = true;
            let (phase, elapsed_ms) = match scene {
                Scene::RecordLive => (Some(RecPhase::Recording), 42_000),
                Scene::RecordActivity => (Some(RecPhase::Recording), 754_000),
                Scene::RecordPaused => (Some(RecPhase::Paused), 754_000),
                _ => (None, 0),
            };
            // Heard at the picture's moment, so the time shows as sent.
            st.rec = crate::state::RecView { phase, elapsed_ms, at: NOW_MONO, ..Default::default() };
            match scene {
                Scene::RecordActivity | Scene::RecordPaused => Mode::Activity(activities::Activity::Recording),
                Scene::AddonsRecorder => {
                    st.page = PageKind::Addons;
                    Mode::Expanded(PageKind::Addons)
                }
                _ => {
                    st.page = PageKind::Record;
                    Mode::Expanded(PageKind::Record)
                }
            }
        }
        Scene::Ai(shows) => {
            let now = st.now;
            crate::pages::usage::picture(&mut st, shows, now);
            st.page = PageKind::Ai;
            Mode::Expanded(PageKind::Ai)
        }
        Scene::Oyenza | Scene::OyenzaCode => {
            st.oyenza = fake_oyenza(&st, scene == Scene::OyenzaCode);
            st.page = PageKind::Oyenza;
            Mode::Expanded(PageKind::Oyenza)
        }
        Scene::OyenzaSignedOut => {
            st.page = PageKind::Oyenza;
            Mode::Expanded(PageKind::Oyenza)
        }
        Scene::OyenzaNotice => {
            st.oyenza = fake_oyenza(&st, false);
            st.notify(Icon::MoneyIn, palette::GREEN, "Money in \u{b7} BCA", "Amount and sender hidden", 30.0, Some(NoticeAction::OpenPage(PageKind::Oyenza)));
            st.set_notice_inline(InlineAction::ShowAmount);
            Mode::Notice
        }
        Scene::SettingsSignedIn => {
            st.oyenza = fake_oyenza(&st, false);
            st.page = PageKind::Settings;
            Mode::Expanded(PageKind::Settings)
        }
        Scene::HealthSignedOut => {
            health_on(&mut st);
            st.health = None;
            st.page = PageKind::Health;
            Mode::Expanded(PageKind::Health)
        }
        Scene::IdleStatus => {
            st.settings.idle_style = IdleStyle::Status;
            Mode::Idle
        }
        Scene::MediaNoLine => {
            // A long title: Pill floats it below a notch (the other themes
            // keep words out from under the camera).
            media(&mut st);
            if let Some(m) = st.media.as_mut() {
                m.title = "Paper Boats (Harbor Lights Session)".into();
            }
            st.settings.lyrics = false;
            st.lyrics = LyricsState::default();
            Mode::Activity(activities::Activity::Media)
        }
        Scene::System(m) => {
            // System is an add-on, off by default: on for its own scenes.
            st.settings.set_addon(poni_core::addons::SYSTEM.id, true);
            st.addons.insert(poni_core::addons::SYSTEM.id, AddonStatus::Ready);
            // Power is a desktop Mac's: no battery, the SMC's whole-Mac watts.
            st.system = sample_system(m == Module::Power);
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
        Scene::Japanese(status) => {
            let ja = ja.ok_or("the Japanese song wasn't read")?;
            media_japanese(&mut st, &ja[if status == AddonStatus::Ready { 0 } else { 1 }], status);
            st.page = PageKind::Media;
            Mode::Expanded(PageKind::Media)
        }
        Scene::Addons(status) => {
            ja_status(&mut st, status);
            st.page = PageKind::Addons;
            Mode::Expanded(PageKind::Addons)
        }
        Scene::Prayer | Scene::PrayerPicker => {
            seed_prayer(&mut st, scene == Scene::PrayerPicker)?;
            st.page = PageKind::Prayer;
            Mode::Expanded(PageKind::Prayer)
        }
        Scene::PrayerCountdown => {
            seed_prayer(&mut st, false)?;
            Mode::Activity(activities::Activity::Prayer)
        }
        Scene::PrayerPickerWorld | Scene::PrayerWorld => {
            seed_prayer_world(&mut st, scene == Scene::PrayerPickerWorld)?;
            st.page = PageKind::Prayer;
            Mode::Expanded(PageKind::Prayer)
        }
        Scene::PrayerNotice => {
            let (site, ashar) = seed_prayer(&mut st, false)?;
            st.prayer.adzan = true;
            let place = st.prayer.place.as_ref().map_or(String::new(), |p| p.label.clone());
            let title = crate::pages::prayer::moment_title(Prayer::Ashar);
            st.notify(Icon::Moon, st.accent, title, format!("{} \u{b7} {place}", site.hhmm(ashar)), 30.0, Some(NoticeAction::OpenPage(PageKind::Prayer)));
            st.set_notice_inline(InlineAction::StopAdzan);
            Mode::Notice
        }
    };

    // The island at rest at its size for this state (as the app sizes it).
    // The Pill theme floats it below the top edge, where the app puts it.
    let size = |st: &AppState| {
        let mut g = activities::geom(&mode, st);
        if let Some(n) = notch {
            g = notch::geom(&mode, st, n, g);
        }
        let gap = if crate::pill::on(theme) { notch.map_or(Some(crate::pill::TOP_GAP), |n| notch::pill_gap(&mode, st, n)) } else { None };
        if hovering {
            g = if gap.is_some() { crate::pill::motion::hover_lift(g) } else { crate::island::Geom::new(g.w + 12.0, g.h + 4.0, g.r + 2.0) };
        }
        (g, gap)
    };
    // Pill's surfaces size themselves from what they last drew (a measured
    // text width) and ease in over their first frames (the song handing the
    // middle to its line): draw the scene twice, a moment before the
    // picture's, so it shows them as the app does a few frames in.
    if crate::pill::on(theme) {
        for t in [NOW_MONO - 0.4, NOW_MONO - 0.2] {
            st.now = t;
            let (g, gap) = size(&st);
            warm_up(gfx, &mut st, mode, notch, g, gap, hovering)?;
        }
        st.now = NOW_MONO;
    }
    let (g, gap) = size(&st);
    let mut island = Island::new(g);
    island.set_floating(gap);
    island.snap(g);

    let menu_h = notch.map_or(MENU_BAR, |n| n.h);
    let w_dip = (st.settings.expanded_width + 2.0 * MARGIN_X).max(g.w + 2.0 * (SHOULDER + MARGIN_X));
    let h_dip = (island.top() + g.h).max(menu_h) + MARGIN_BOTTOM + if matches!(mode, Mode::Expanded(_)) { 0.0 } else { 60.0 };
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
            if island.floating() {
                crate::pill::shape::draw_body(&mut c, body, g.r, palette::ISLAND, border);
            } else {
                c.notch(body, g.r, shoulder, palette::ISLAND, border);
            }
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

/// One frame of `mode` at `st.now`, drawn offscreen and thrown away, for the
/// state the drawing leaves behind (see `render_scene`).
#[allow(clippy::too_many_arguments)]
fn warm_up(gfx: &mut Gfx, st: &mut AppState, mode: Mode, notch: Option<Notch>, g: crate::island::Geom, gap: Option<f32>, hovering: bool) -> Result<(), String> {
    let mut island = Island::new(g);
    island.set_floating(gap);
    island.snap(g);
    let w = (st.settings.expanded_width + 2.0 * MARGIN_X).max(g.w + 2.0 * (SHOULDER + MARGIN_X));
    let h = island.top() + g.h + MARGIN_BOTTOM;
    let bmp = Bitmap::new((w * SCALE).ceil() as usize, (h * SCALE).ceil() as usize, SCALE).ok_or("no bitmap context")?;
    let body = island.body_rect_at(w / 2.0);
    let theme = st.settings.theme;
    gfx.set_look(if notch.is_some() { poni_core::look::around_notch(theme) } else { poni_core::look::of(theme) });
    let mut c = gfx.canvas(bmp.ctx());
    let mut input = Input::default();
    if hovering {
        input.mouse = Some(Point::new(body.center().x + 20.0, body.center().y));
    }
    let mut hits: Vec<Hit> = Vec::new();
    let now = st.now;
    crate::app::draw_content(&mut c, &mut input, &mut hits, st, mode, body, body, None, now, notch);
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
fn sample_system(desktop: bool) -> Monitor {
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
        let mut r = sample_reading(cpu, gpu, (read, write), (down, up), t);
        if desktop {
            // What the office Mac Studio's SMC read at rest (7.8-8.2 W), rising with the graphics.
            let w = if last { 8.1 } else { 7.4 + 24.0 * (gpu - 0.2).max(0.0) + 3.0 * cpu };
            r.battery = None;
            r.power = Some(PowerReading { total_w: w, cpu_w: None, gpu_w: None, rest_w: w, psu: None });
        }
        m.push(r);
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
        power: None,
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
