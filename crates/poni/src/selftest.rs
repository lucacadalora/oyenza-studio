//! `poni.exe --selftest <dir>`: drive the real app through every visual state
//! with fake (original) data, save screenshots, verify pixel invariants and
//! frame timings, and write `<dir>/report.json`.
//!
//! The app calls [`SelfTest::step`] every 50 ms (`TIMER_SELFTEST`). Each step
//! re-applies the current scene (so stray OS events can't change it), waits
//! until the island's springs are at rest, then renders one frame through the
//! normal pipeline and checks it. PNG encoding runs on short-lived worker
//! threads so a step never blocks the UI thread for long.
//!
//! Output per state: `<nn>-<name>.png` (composited over a wallpaper-like
//! gradient, the way it looks on a desktop) and `<nn>-<name>-alpha.png` (the
//! raw alpha channel). The exit code is 0 when every check passes, else 1.

use crate::app::App;
use crate::clock;
use crate::gfx::{Image, ImageData};
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::paths;
use crate::png;
use crate::state::{
    AppState, Hud, HudKind, InlineAction, LyricsState, LyricsStatus, MediaSession, Notice, NoticeAction, OyenzaLink, OyenzaState, PhoneAsk,
    PhoneDrop, PhoneTransfer, PowerState, SysStats, VolumeState,
};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::health::{HealthDay, HealthSnapshot, HealthSource};
use poni_core::lrc::parse_lrc;
use poni_core::oyenza::{Arrival, Bill, Saved, WalletRef};
use poni_core::phonedrop::QrMatrix;
use poni_core::pomodoro::{Phase as PomoPhase, RunState};
use poni_core::settings::{IdleStyle, Theme};
use poni_core::shelf::ShelfItem;
use poni_core::sysmon::{
    AppUse, BatteryReading, CpuReading, DiskActivity, Drive, MemoryReading, Module, Monitor, NetReading, Reading, HISTORY_LEN,
};
use poni_core::timeline::Timeline;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Instant;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, GetWindowRect, IsWindowVisible, GWL_EXSTYLE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

// ---------------------------------------------------------------- timing

/// A scene must stay settled for this many consecutive steps before capture.
const STABLE_STEPS: u32 = 2;
/// Minimum steps per scene (lets the content cross-fade start and finish).
const MIN_STEPS: u32 = 3;
/// Seconds a scene may take to settle before it counts as a failure.
const SETTLE_TIMEOUT: f64 = 3.0;
/// Seconds to wait for a simulated tab click to switch the page.
const CLICK_TIMEOUT: f64 = 1.5;
/// Stop visiting scenes after this many seconds (the whole run must stay under 30 s).
const SCENE_DEADLINE: f64 = 25.0;
/// Give up waiting for PNG writers at this point.
const WRITE_DEADLINE: f64 = 28.5;
/// One frame (layout + Direct2D render + present + readback) must fit in a 60 Hz frame.
const FRAME_BUDGET_MS: f64 = 16.0;
/// Frames re-rendered when the first sample misses the budget (filters scheduler noise).
const FRAME_SAMPLES: usize = 3;

// Eye-blink timing of the idle "eyes" style (activities.rs). Only used to
// avoid capturing a mid-blink screenshot; harmless if it drifts.
const BLINK_PERIOD: f64 = 5.5;
const BLINK_GUARD: (f64, f64) = (0.3, 0.3);

// ---------------------------------------------------------------- fake data (all original)

const TITLE: &str = "Paper Boats";
const ARTIST: &str = "The Lanterns";
const ALBUM: &str = "Harbor Lights";
const TRACK_LEN_S: f64 = 201.0;
const TRACK_POS_S: f64 = 64.0;
/// Pomodoro time left in the running focus session (18:24).
const TIMER_LEFT_S: f64 = 1104.5;
const ART_PX: u32 = 240;

/// Original placeholder lyrics written for the self-test (not from any real song).
const LYRICS_LRC: &str = "\
[ti:Paper Boats]
[ar:The Lanterns]
[00:38.00]Folded up the morning into something small
[00:43.50]Set it on the water by the harbor wall
[00:49.00]Ink along the edges of a name I used to know
[00:54.50]Watched it find the current, watched it go
[01:00.00]Paper boats, carry what I couldn't say
[01:05.50]Over the grey and out across the bay
[01:11.00]If the tide is gentle they will find the light
[01:16.50]Lanterns on the pier keep burning through the night
[01:22.00]Every little sail a letter never sent
[01:27.50]Every little ripple something someone meant
[01:33.00]So I fold another and I let it drift away
[01:38.50]Paper boats, I'll meet you there someday
";

/// The same song in Japanese, Korean and Chinese (written for the test, same
/// timings), for the pronunciation lines (PONI_SELFTEST_LYRICS=ja|ko|zh).
const LYRICS_JA: &str = "\
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

const LYRICS_KO: &str = "\
[00:38.00]아침을 접어서 작게 만들었어
[00:43.50]항구 벽 옆에서 물 위에 띄웠어
[00:49.00]가장자리에 번진 잉크, 옛 이름
[00:54.50]물길을 찾아서 멀리 떠나갔어
[01:00.00]종이배야, 못 한 말을 실어 가 줘
[01:05.50]회색 바다를 건너 만 너머로
[01:11.00]물결이 다정하면 빛을 찾겠지
[01:16.50]부두의 등불은 밤새 타올라
[01:22.00]작은 돛마다 부치지 못한 편지
[01:27.50]작은 물결마다 누군가의 마음
[01:33.00]그래서 또 하나 접어 흘려보내
[01:38.50]종이배야, 언젠가 거기서 만나 Oh
";

const LYRICS_ZH: &str = "\
[00:38.00]把清晨折成小小的形状
[00:43.50]放在港口的墙边水面上
[00:49.00]边缘晕开的墨水，旧日的名字
[00:54.50]看它找到水流，看它远去
[01:00.00]纸船啊，带走我说不出的话
[01:05.50]越过灰色的海，驶向海湾那边
[01:11.00]如果潮水温柔，它们会找到光
[01:16.50]码头的灯笼彻夜燃烧
[01:22.00]每一片小帆都是没寄出的信
[01:27.50]每一道涟漪都是谁的心意
[01:33.00]所以我再折一只，让它漂走
[01:38.50]纸船啊，总有一天我们在那里相见
";

const NOTES_TXT: &str = "Weekend trip\n\n- ferry leaves at 9:40\n- pack the blue jacket\n- ask about the lighthouse tour\n";

const CLIPS: [&str; 6] = [
    "Lunch: noodles from the corner place, then a walk by the river",
    "Tracking number PX-2048-7731",
    "cargo build -p poni --release",
    "#5AC8FA",
    "https://example.com/projects/poni/notes",
    "Meet at the north entrance at 6:15",
];

const GIB: u64 = 1024 * 1024 * 1024;

/// The phone-drop link in the screenshots. Made up: no server answers it.
const PHONE_URL: &str = "HTTP://192.168.1.20:53530/K7Q2M9XWZ4R8T6YB3NPV5HJC2D/";

/// Phone drop as each phone scene shows it (None = closed).
fn fake_phone(scene: Scene) -> Option<PhoneDrop> {
    let ready = PhoneDrop { url: PHONE_URL.into(), qr: QrMatrix::encode(PHONE_URL), ..Default::default() };
    let files = vec![("IMG_2041.HEIC".to_string(), 2_480_112u64), ("IMG_2042.HEIC".to_string(), 2_391_870), ("Harbor walk.mov".to_string(), 18_402_331)];
    let total = files.iter().map(|f| f.1).sum();
    match scene {
        Scene::ShelfPhone => Some(ready),
        Scene::ShelfPhoneAsk => Some(PhoneDrop {
            visitor: Some("iPhone".into()),
            ask: Some(PhoneAsk { id: 7, from: "iPhone".into(), files, total }),
            ..ready
        }),
        Scene::ShelfPhoneReceive => Some(PhoneDrop {
            visitor: Some("iPhone".into()),
            transfer: Some(PhoneTransfer { id: 7, from: "iPhone".into(), file: 2, files: 3, done: 9_100_000, total }),
            ..ready
        }),
        _ => None,
    }
}

/// Paths of the sample apps (keys for their icons only; never opened) and their tile colours.
const SAMPLE_ICONS: [(&str, u32); 4] = [
    ("C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe", 0x1A9FD9),
    ("C:\\Users\\Sample\\AppData\\Roaming\\Spotify\\Spotify.exe", 0x1DB954),
    ("C:\\Windows\\explorer.exe", 0xF2C14E),
    ("C:\\Users\\Sample\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe", 0x2F80ED),
];

/// A plain rounded tile standing in for an app icon, laid out like the
/// shell's icon pictures (the tile fills the middle 72 %).
fn sample_icon(rgb: u32) -> Image {
    const PX: u32 = 48;
    let (r, g, b) = ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
    let inset = PX as f32 * 0.14;
    let (lo, hi, radius) = (inset, PX as f32 - inset, PX as f32 * 0.16);
    let mut px = vec![0u8; (PX * PX * 4) as usize];
    for y in 0..PX {
        for x in 0..PX {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distance outside the rounded square, smoothed over one pixel.
            let dx = (lo + radius - fx).max(fx - (hi - radius)).max(0.0);
            let dy = (lo + radius - fy).max(fy - (hi - radius)).max(0.0);
            let a = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            let i = ((y * PX + x) * 4) as usize;
            let k = |c: u8| (f32::from(c) * a).round() as u8;
            px[i..i + 4].copy_from_slice(&[k(b), k(g), k(r), (a * 255.0).round() as u8]);
        }
    }
    ImageData::new(PX, PX, px)
}

fn sample_app(i: usize, name: &str, cpu: f32, memory: u64) -> AppUse {
    let path = SAMPLE_ICONS[i].0;
    let image = path.rsplit('\\').next().unwrap_or(path).to_lowercase();
    AppUse { image, name: name.into(), path: Some(path.into()), pid: 1000 + i as u64, cpu, memory, processes: 1 }
}

/// The System page's sample minute (all made up): a laptop, or a desktop
/// without a battery; nothing yet when the page has just opened.
fn fake_system(scene: Scene) -> Monitor {
    let mut m = Monitor::default();
    if scene == Scene::SystemWaiting {
        return m;
    }
    const MIB: f64 = 1024.0 * 1024.0;
    for i in 0..HISTORY_LEN {
        let t = i as f32;
        let last = i + 1 == HISTORY_LEN;
        let spike = if (38..45).contains(&i) { 0.22 * ((t - 38.0) / 6.0 * std::f32::consts::PI).sin() } else { 0.0 };
        let cpu = if last { 0.23 } else { (0.19 + 0.05 * (t / 4.3).sin() + 0.03 * (t / 1.7 + 1.1).sin() + spike).clamp(0.03, 0.95) };
        let burst = |at: usize, len: usize, peak: f64| if (at..at + len).contains(&i) { peak * (((i - at) as f64 + 0.5) / len as f64 * std::f64::consts::PI).sin() } else { 0.0 };
        let read = if last { 1.2 * MIB } else { 0.15 * MIB + burst(12, 6, 6.5 * MIB) + burst(41, 4, 2.8 * MIB) };
        let write = if last { 0.33 * MIB } else { 0.05 * MIB + burst(20, 9, 1.9 * MIB) };
        let down = if last { 1.2 * MIB } else { 0.08 * MIB + burst(5, 14, 3.4 * MIB) + burst(46, 10, 1.6 * MIB) };
        let up = if last { 0.082 * MIB } else { 0.02 * MIB + burst(25, 8, 0.6 * MIB) };
        m.push(sample_reading(scene != Scene::SystemMemory, cpu, 0.6 + 0.004 * (t / 6.0).sin(), read, write, down, up));
    }
    m
}

fn sample_reading(laptop: bool, cpu: f32, mem: f32, read: f64, write: f64, down: f64, up: f64) -> Reading {
    let total = GIB * 16;
    Reading {
        cpu: Some(CpuReading { load: cpu, user: cpu * 0.6, kernel: cpu * 0.4, logical: 12, mhz: Some(2_600), uptime_s: 3 * 86_400 + 4 * 3600 + 12 * 60 }),
        memory: Some(MemoryReading {
            total,
            used: (total as f64 * f64::from(mem)) as u64,
            committed: GIB * 121 / 10,
            commit_limit: GIB * 184 / 10,
            cached: GIB * 42 / 10,
        }),
        drives: vec![
            Drive { root: "C:".into(), label: "Windows".into(), total: GIB * 476, free: GIB * 212, system: true },
            Drive { root: "D:".into(), label: "Data".into(), total: GIB * 931, free: GIB * 640, system: false },
        ],
        disk: Some(DiskActivity { read_bps: read, write_bps: write, active: Some(0.03) }),
        network: Some(NetReading {
            online: true,
            adapter: "Wi-Fi".into(),
            ipv4: Some("192.168.1.42".into()),
            down_bps: down,
            up_bps: up,
            received: GIB * 34 / 10,
            sent: 410 * 1024 * 1024,
        }),
        battery: laptop.then(|| BatteryReading {
            percent: 76,
            secs_left: Some(2 * 3600 + 14 * 60),
            design_mwh: Some(57_000),
            full_mwh: Some(51_870),
            cycles: Some(193),
            rate_mw: Some(-8_200),
            ..BatteryReading::default()
        }),
        top_cpu: vec![
            sample_app(0, "Microsoft Edge", 0.091, GIB * 21 / 10),
            sample_app(1, "Spotify", 0.042, 412 * 1024 * 1024),
            sample_app(2, "Windows Explorer", 0.013, 96 * 1024 * 1024),
        ],
        top_memory: vec![
            sample_app(0, "Microsoft Edge", 0.091, GIB * 21 / 10),
            sample_app(3, "Visual Studio Code", 0.008, 940 * 1024 * 1024),
            sample_app(1, "Spotify", 0.042, 412 * 1024 * 1024),
        ],
    }
}

/// Judge one reading from the real sources: the core readings must be
/// there; the rest is reported (a build server has no battery or GPU counters).
fn system_readings(r: Reading) -> (bool, String) {
    let cpu = r.cpu.as_ref().filter(|c| (0.0..=1.0).contains(&c.load) && c.logical > 0);
    let mem = r.memory.as_ref().filter(|m| m.total > 0 && m.used <= m.total);
    let ok = cpu.is_some() && mem.is_some() && !r.drives.is_empty() && !r.top_memory.is_empty();
    let apps: Vec<String> = r.top_memory.iter().map(|a| format!("{} ({}{})", a.name, poni_core::sysmon::fmt_bytes(a.memory), if a.path.is_some() { ", path" } else { "" })).collect();
    let detail = format!(
        "cpu {} · memory {} · drives {} · disk activity {} · network {} · battery {} · top memory {}",
        cpu.map_or("missing".into(), |c| format!("{:.0}% of {} threads{}", c.load * 100.0, c.logical, c.mhz.map_or(String::new(), |m| format!(" at {m} MHz")))),
        mem.map_or("missing".into(), |m| format!("{} of {}", poni_core::sysmon::fmt_bytes(m.used), poni_core::sysmon::fmt_bytes(m.total))),
        if r.drives.is_empty() { "none".into() } else { r.drives.iter().map(|d| d.root.clone()).collect::<Vec<_>>().join(" ") },
        if r.disk.is_some() { "yes" } else { "no" },
        r.network.as_ref().map_or("unavailable".into(), |n| if n.online { format!("{} {}", n.adapter, n.ipv4.as_deref().unwrap_or("")) } else { "offline".into() }),
        r.battery.as_ref().map_or("none".into(), |b| format!("{}%", b.percent)),
        if apps.is_empty() { "none".into() } else { apps.join(", ") },
    );
    (ok, detail)
}

fn fake_stats() -> SysStats {
    SysStats {
        valid: true,
        cpu: 0.23,
        mem_used: GIB * 96 / 10,
        mem_total: GIB * 16,
        disk_free: GIB * 212,
        disk_total: GIB * 476,
        uptime_s: 3 * 86_400 + 4 * 3600 + 12 * 60,
    }
}

/// Oyenza, signed in as an example account: two arrivals today, a
/// subscription tomorrow and a card bill the day after (all made up).
fn fake_oyenza(st: &AppState, scene: Scene, now: f64) -> OyenzaState {
    let at = |hours_ago: i64| {
        let t = st.unix - hours_ago * 3600;
        let (y, m, d) = poni_core::datetime::civil_from_days(t.div_euclid(86_400));
        let s = t.rem_euclid(86_400);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
    };
    let arrival = |id: &str, amount: f64, text: &str, title: &str, wallet: &str, color: &str, hours_ago: i64| Arrival {
        id: id.into(),
        amount,
        currency: "IDR".into(),
        text: text.into(),
        title: title.into(),
        wallet: Some(WalletRef { name: wallet.into(), color: color.into() }),
        at: at(hours_ago),
    };
    let day = |n: i64| st.local.add_days(n).day_key();
    let link = match scene {
        Scene::OyenzaSignIn => OyenzaLink::Connecting {
            code: "KPJX-TMNQ".into(),
            url: "https://oyenza.app/connect?code=KPJX-TMNQ".into(),
            // Pinned on every step, so the countdown reads 9:41 in the screenshot.
            until: now + 581.0,
        },
        _ => OyenzaLink::Connected { account: "a•••@example.com".into() },
    };
    OyenzaState {
        link,
        today: vec![
            arrival("t2", 50_000.0, "Rp 50.000", "Uang masuk", "GoPay", "#00AED6", 1),
            arrival("t1", 1_250_000.0, "Rp 1.250.000", "Gaji Oktober", "BCA", "#0060AF", 3),
        ],
        bills: vec![
            Bill { kind: "subscription".into(), name: "Spotify".into(), due: day(1), amount: Some(54_990.0), text: Some("Rp 54.990".into()) },
            Bill { kind: "card".into(), name: "Kartu Kredit BCA".into(), due: day(2), amount: None, text: None },
        ],
        bills_today: st.day_key(),
        updated_unix: Some(st.unix - 60),
        saved: (scene == Scene::OyenzaSaved).then(|| {
            let kopi = Saved { id: "s1".into(), title: "Kopi".into(), amount: 25_000.0, text: "Rp 25.000".into(), category: Some("Makan & Minum".into()), wallet: "GoPay".into() };
            (vec![kopi], now + 9.5)
        }),
        ..OyenzaState::default()
    }
}

fn fake_power(plugged: bool) -> PowerState {
    PowerState { has_battery: true, percent: 76, charging: plugged, on_ac: plugged, saver: false }
}

// ---------------------------------------------------------------- scenes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    IdleClock,
    IdleEyes,
    Media,
    Timer,
    MediaTimer,
    VolumeHud,
    ChargingHud,
    BluetoothHud,
    HealthNotice,
    Page(PageKind),
    /// The shelf while files are dragged over the island.
    ShelfDrop,
    /// An empty shelf: the drop zone and the Phone drop tile.
    ShelfEmpty,
    /// Phone drop open: the QR code waits for a phone.
    ShelfPhone,
    /// An iPhone asks to send three files.
    ShelfPhoneAsk,
    /// Files coming in from the iPhone.
    ShelfPhoneReceive,
    /// Click the first header tab on the Today page; expect the Music page.
    TabClick,
    /// The Health page before the person signs in with Oyenza: no numbers yet.
    HealthSignedOut,
    /// Oyenza: money arrived, its sum hidden behind Show.
    OyenzaMoney,
    /// ...and after Show.
    OyenzaMoneyShown,
    /// Oyenza: the day before a subscription renews.
    OyenzaBill,
    /// The Oyenza tab while a sign-in code waits for approval.
    OyenzaSignIn,
    /// The Oyenza tab right after a spend was saved (Undo showing).
    OyenzaSaved,
    /// The Health page signed in to Oyenza, before the phone sends any steps.
    HealthSignedIn,
    /// System page on a desktop (no battery): Memory and its apps.
    SystemMemory,
    /// System page: the drives and disk activity.
    SystemDrives,
    /// System page: the network.
    SystemNetwork,
    /// System page: the laptop's battery.
    SystemBattery,
    /// System page just opened: no reading yet.
    SystemWaiting,
}

const SCENES: [Scene; 36] = [
    Scene::IdleClock,
    Scene::IdleEyes,
    Scene::Media,
    Scene::Timer,
    Scene::MediaTimer,
    Scene::VolumeHud,
    Scene::ChargingHud,
    Scene::BluetoothHud,
    Scene::HealthNotice,
    Scene::Page(PageKind::Media),
    Scene::Page(PageKind::Focus),
    Scene::Page(PageKind::Health),
    Scene::Page(PageKind::Shelf),
    Scene::ShelfDrop,
    Scene::ShelfPhone,
    Scene::ShelfPhoneAsk,
    Scene::ShelfPhoneReceive,
    // Last of the shelf scenes: hiding the items resets their fade-in.
    Scene::ShelfEmpty,
    Scene::Page(PageKind::Tasks),
    Scene::Page(PageKind::Clipboard),
    Scene::Page(PageKind::Today),
    Scene::TabClick,
    Scene::HealthSignedOut,
    Scene::OyenzaMoney,
    Scene::OyenzaMoneyShown,
    Scene::OyenzaBill,
    Scene::Page(PageKind::Oyenza),
    Scene::OyenzaSignIn,
    Scene::OyenzaSaved,
    Scene::HealthSignedIn,
    Scene::Page(PageKind::System),
    Scene::SystemMemory,
    Scene::SystemDrives,
    Scene::SystemNetwork,
    Scene::SystemBattery,
    Scene::SystemWaiting,
];

impl Scene {
    fn name(self) -> String {
        match self {
            Scene::IdleClock => "idle-clock".into(),
            Scene::IdleEyes => "idle-eyes".into(),
            Scene::Media => "media-activity".into(),
            Scene::Timer => "timer-activity".into(),
            Scene::MediaTimer => "media-timer".into(),
            Scene::VolumeHud => "volume-hud".into(),
            Scene::ChargingHud => "charging-hud".into(),
            Scene::BluetoothHud => "bluetooth-hud".into(),
            Scene::HealthNotice => "health-notice".into(),
            Scene::Page(p) => format!("page-{}", p.key()),
            Scene::ShelfDrop => "page-shelf-drop".into(),
            Scene::ShelfEmpty => "page-shelf-empty".into(),
            Scene::ShelfPhone => "page-shelf-phone".into(),
            Scene::ShelfPhoneAsk => "page-shelf-phone-ask".into(),
            Scene::ShelfPhoneReceive => "page-shelf-phone-receive".into(),
            Scene::TabClick => "tab-click-media".into(),
            Scene::HealthSignedOut => "page-health-signed-out".into(),
            Scene::OyenzaMoney => "oyenza-money".into(),
            Scene::OyenzaMoneyShown => "oyenza-money-shown".into(),
            Scene::OyenzaBill => "oyenza-bill".into(),
            Scene::OyenzaSignIn => "page-oyenza-signin".into(),
            Scene::OyenzaSaved => "page-oyenza-saved".into(),
            Scene::HealthSignedIn => "page-health-signed-in".into(),
            Scene::SystemMemory => "page-system-memory".into(),
            Scene::SystemDrives => "page-system-drives".into(),
            Scene::SystemNetwork => "page-system-network".into(),
            Scene::SystemBattery => "page-system-battery".into(),
            Scene::SystemWaiting => "page-system-waiting".into(),
        }
    }

    fn page(self) -> Option<PageKind> {
        match self {
            Scene::Page(p) => Some(p),
            Scene::ShelfDrop | Scene::ShelfEmpty | Scene::ShelfPhone | Scene::ShelfPhoneAsk | Scene::ShelfPhoneReceive => Some(PageKind::Shelf),
            Scene::TabClick => Some(PageKind::ALL[0]),
            Scene::HealthSignedOut => Some(PageKind::Health),
            Scene::OyenzaSignIn | Scene::OyenzaSaved => Some(PageKind::Oyenza),
            Scene::HealthSignedIn => Some(PageKind::Health),
            Scene::SystemMemory | Scene::SystemDrives | Scene::SystemNetwork | Scene::SystemBattery | Scene::SystemWaiting => Some(PageKind::System),
            _ => None,
        }
    }

    /// Scenes that show something from Oyenza (every other one is signed out,
    /// so their screenshots stay what they were before Oyenza).
    fn oyenza(self) -> bool {
        matches!(
            self,
            Scene::OyenzaMoney
                | Scene::OyenzaMoneyShown
                | Scene::OyenzaBill
                | Scene::OyenzaSignIn
                | Scene::OyenzaSaved
                | Scene::Page(PageKind::Oyenza)
                | Scene::HealthSignedIn
        )
    }

    fn expanded(self) -> bool {
        self.page().is_some()
    }

    fn media_playing(self) -> bool {
        matches!(self, Scene::Media | Scene::MediaTimer) || self.expanded()
    }

    fn timer_running(self) -> bool {
        matches!(self, Scene::Timer | Scene::MediaTimer) || self.expanded()
    }
}

/// GET the phone page from a running Phone drop server: the address must be
/// reachable and the page must come back whole. The detail never includes
/// the link's secret.
pub fn fetch_phone_page(url: &str) -> (bool, String) {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    let rest = url.split_once("//").map_or(url, |(_, r)| r);
    let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
    let Ok(addr) = host.parse::<SocketAddr>() else {
        return (false, format!("link has no address: {host}"));
    };
    let fetch = || -> std::io::Result<String> {
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        write!(s, "GET /{path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: poni-selftest\r\n\r\n")?;
        let mut out = String::new();
        s.read_to_string(&mut out)?;
        Ok(out)
    };
    match fetch() {
        Ok(resp) if resp.starts_with("HTTP/1.1 200") && resp.contains("<title>Oyen Poni") && resp.trim_end().ends_with("</html>") => {
            (true, format!("served the page at {addr} ({} bytes)", resp.len()))
        }
        Ok(resp) => (false, format!("unexpected answer from {addr}: {}", resp.lines().next().unwrap_or(""))),
        Err(e) => (false, format!("couldn't fetch the page at {addr}: {e}")),
    }
}

// ---------------------------------------------------------------- report

#[derive(Clone, Debug, Serialize)]
struct Check {
    name: String,
    ok: bool,
    detail: String,
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check { name: name.into(), ok, detail: detail.into() }
}

#[derive(Debug, Serialize)]
struct StateReport {
    index: usize,
    name: String,
    expanded: bool,
    page: Option<&'static str>,
    image: String,
    alpha_image: String,
    ok: bool,
    settle_ms: u64,
    /// Best of the sampled frame times (full frame: layout, render, present, readback).
    frame_ms: f64,
    frame_samples_ms: Vec<f64>,
    canvas_px: [i32; 2],
    window_pos_px: [i32; 2],
    /// Island body in canvas DIPs: x, y, w, h.
    body_dip: [f32; 4],
    measured_width_px: i32,
    checks: Vec<Check>,
}

#[derive(Debug, Serialize)]
struct Report {
    app: &'static str,
    version: &'static str,
    ok: bool,
    exit_code: i32,
    total_ms: u64,
    scale: f32,
    expanded_width_dip: f32,
    /// "gpu", "gpu-warp" or "software".
    renderer: String,
    frame_budget_ms: f64,
    states_passed: usize,
    states_total: usize,
    failed_checks: Vec<String>,
    /// Checks that are not tied to a single screenshot (fixtures, input, files).
    checks: Vec<Check>,
    states: Vec<StateReport>,
}

// ---------------------------------------------------------------- driver

enum Phase {
    /// Record one transition frame by frame (PONI_SELFTEST_ANIM=1).
    Anim { seq: usize },
    Settle { i: usize, since: f64, steps: u32, stable: u32 },
    Click { i: usize },
    ClickWait { i: usize, since: f64, from: PageKind },
    Finish { since: f64 },
    Done(i32),
}

pub struct SelfTest {
    dir: PathBuf,
    t0: Instant,
    phase: Phase,
    art: Option<Image>,
    art_color: Option<Color>,
    lyrics: LyricsState,
    track_key: String,
    /// The sample week every scene shows, except the signed-out Health page.
    health: Option<HealthSnapshot>,
    states: Vec<StateReport>,
    checks: Vec<Check>,
    writes: Vec<(String, JoinHandle<Vec<String>>)>,
    scale: f32,
    expanded_width: f32,
    /// PONI_SELFTEST_DPI: render at this DPI instead of the monitor's.
    dpi: Option<u32>,
    /// PONI_SELFTEST_ANIM=1: after the scenes, record transitions frame by frame.
    anim: bool,
    dpi_applied: bool,
    /// PONI_SELFTEST_THEME=oyen: run every scene in the Oyen theme.
    theme: Theme,
    /// PONI_SELFTEST_LYRICS=ja|ko|zh: the sample song in that language.
    lyrics_lang: Option<String>,
    renderer: &'static str,
    /// The seeded shelf and its thumbnails (the empty-shelf scene hides
    /// them, and the page forgets thumbnails of items that left).
    shelf_items: Vec<ShelfItem>,
    shelf_thumbs: Vec<(String, Option<Image>)>,
}

/// Transitions recorded in animation mode: (name, from, to).
const ANIMS: [(&str, Scene, Scene); 5] = [
    ("expand", Scene::Media, Scene::Page(PageKind::Media)),
    ("page", Scene::Page(PageKind::Media), Scene::Page(PageKind::Focus)),
    ("collapse", Scene::Page(PageKind::Media), Scene::Media),
    ("hud", Scene::Media, Scene::VolumeHud),
    ("notice", Scene::IdleClock, Scene::HealthNotice),
];
/// Frames recorded per transition (~16 ms apart).
const ANIM_FRAMES: usize = 40;

impl SelfTest {
    pub fn new(dir: String) -> Self {
        let track_key = MediaSession::make_track_key(TITLE, ARTIST, ALBUM);
        Self {
            dir: PathBuf::from(dir),
            t0: Instant::now(),
            phase: Phase::Settle { i: 0, since: 0.0, steps: 0, stable: 0 },
            art: None,
            art_color: None,
            lyrics: LyricsState { track_key: track_key.clone(), ..Default::default() },
            track_key,
            health: None,
            states: Vec::new(),
            checks: Vec::new(),
            writes: Vec::new(),
            scale: 1.0,
            expanded_width: 0.0,
            dpi: std::env::var("PONI_SELFTEST_DPI").ok().and_then(|v| v.trim().parse().ok()),
            anim: std::env::var("PONI_SELFTEST_ANIM").is_ok_and(|v| v.trim() == "1"),
            dpi_applied: false,
            theme: if std::env::var("PONI_SELFTEST_THEME").is_ok_and(|v| v.trim() == "oyen") { Theme::Oyen } else { Theme::Classic },
            lyrics_lang: std::env::var("PONI_SELFTEST_LYRICS").ok().map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty()),
            renderer: "software",
            shelf_items: Vec::new(),
            shelf_thumbs: Vec::new(),
        }
    }

    /// Seed the app with fake data. Called once before the first frame.
    pub fn begin(&mut self, st: &mut AppState) {
        self.t0 = Instant::now();
        crate::pinfo!("selftest: output folder {}", self.dir.display());
        match fs::create_dir_all(&self.dir) {
            Ok(()) => self.clear_old_outputs(),
            Err(e) => self.checks.push(check("output-folder", false, format!("{}: {e}", self.dir.display()))),
        }

        // Quiet, deterministic settings (defaults otherwise: the home folder is fresh).
        st.settings.sounds = false;
        st.settings.show_media_activity = true;
        st.settings.lyrics = true;
        st.settings.hud_volume = true;
        st.settings.hud_power = true;
        st.settings.hud_bluetooth = true;
        st.accent = palette::BLUE;
        st.pinned = false;
        st.volume = VolumeState { level: 0.64, muted: false, known: true };
        st.power = fake_power(false);
        st.stats = fake_stats();

        // Album art + lyrics.
        let art = ImageData::new(ART_PX, ART_PX, make_art(ART_PX));
        self.art_color = Some(art.accent_color());
        self.art = Some(art);
        let lrc = match self.lyrics_lang.as_deref() {
            Some("ja") => LYRICS_JA,
            Some("ko") => LYRICS_KO,
            Some("zh") => LYRICS_ZH,
            _ => LYRICS_LRC,
        };
        let parsed = parse_lrc(lrc);
        let lyric_ok = parsed.synced && parsed.lines.len() == 12 && parsed.line_index_at(TRACK_POS_S) == Some(4);
        self.checks.push(check(
            "lyrics-fixture",
            lyric_ok,
            format!("{} lines, synced {}, line at {TRACK_POS_S}s = {:?}", parsed.lines.len(), parsed.synced, parsed.line_index_at(TRACK_POS_S)),
        ));
        let ruby = crate::services::romanize::pronounce(&parsed);
        if let Some(lang) = &self.lyrics_lang {
            // The CI log shows how each line reads (on Windows the Japanese
            // readings come from the system's analyzer).
            let read = |p: &[poni_core::romanize::Ruby]| p.iter().filter(|r| !r.roman.is_empty()).map(|r| r.roman.as_str()).collect::<Vec<_>>().join(" ");
            let lines_read = ruby.iter().filter(|p| p.iter().any(|r| !r.roman.is_empty())).count();
            for (line, p) in parsed.lines.iter().zip(&ruby) {
                crate::pinfo!("selftest: [{lang}] {} -> {}", line.text, read(p));
            }
            self.checks.push(check("lyrics-pronunciation", lines_read > 0, format!("{lines_read} of {} {lang} lines read in Latin letters", parsed.lines.len())));
        }
        self.lyrics = LyricsState { track_key: self.track_key.clone(), status: LyricsStatus::Found, lyrics: Some(parsed), source: "Sample".into(), ruby };

        seed_tasks(st);
        self.health = Some(sample_health(st));
        seed_clipboard(st);
        let today = st.day_key();
        st.store.focus_sessions.insert(today, 2);
        self.seed_shelf(st);

        // Read-only: the Store build must reach the startup task its manifest declares.
        let (ok, detail) = match crate::autostart::describe() {
            Ok(d) => (true, d),
            Err(d) => (false, d),
        };
        self.checks.push(check("start-with-windows", ok, detail));

        // The real readers behind the System page (its scenes show sample readings).
        let (ok, detail) = system_readings(crate::services::sysmon::probe());
        self.checks.push(check("system-readings", ok, detail));
    }

    /// Called every ~50 ms; returns Some(exit code) when finished.
    pub fn step(&mut self, app: &mut App) -> Option<i32> {
        if !self.dpi_applied {
            let (ok, detail) = app.selftest_phone_drop();
            self.checks.push(check("phone-drop-server", ok, detail));
            self.dpi_applied = true;
            if let Some(dpi) = self.dpi {
                crate::pinfo!("selftest: rendering at {dpi} DPI ({:.0} %)", dpi as f32 / 96.0 * 100.0);
                app.selftest_set_dpi(dpi);
            }
        }
        self.scale = app.scale();
        self.expanded_width = app.st.settings.expanded_width;
        self.renderer = app.renderer();
        let now = clock::mono();
        let elapsed = self.t0.elapsed().as_secs_f64();
        if elapsed > SCENE_DEADLINE && !matches!(self.phase, Phase::Finish { .. } | Phase::Done(_) | Phase::Anim { .. }) {
            let at = match &self.phase {
                Phase::Settle { i, .. } | Phase::Click { i } | Phase::ClickWait { i, .. } => SCENES[*i].name(),
                _ => String::new(),
            };
            crate::pwarn!("selftest: deadline reached while at '{at}'");
            self.checks.push(check("deadline", false, format!("stopped after {elapsed:.1} s at '{at}'")));
            self.phase = Phase::Finish { since: now };
        }

        match self.phase {
            Phase::Anim { seq } => {
                self.record_anim(app, seq);
                self.phase = if seq + 1 < ANIMS.len() { Phase::Anim { seq: seq + 1 } } else { Phase::Finish { since: now } };
            }
            Phase::Settle { i, since, steps, stable } => {
                let scene = SCENES[i];
                let since = if steps == 0 { now } else { since };
                self.apply(app, scene);
                let settled = app.selftest_settled();
                let stable = if settled { stable + 1 } else { 0 };
                let steps = steps + 1;
                let waited = now - since;
                let timed_out = waited > SETTLE_TIMEOUT;
                let ready = steps >= MIN_STEPS && stable >= STABLE_STEPS && !(scene == Scene::IdleEyes && mid_blink());
                if ready || timed_out {
                    self.capture(app, i, !timed_out || settled, waited);
                    self.phase = self.next_phase(i + 1, now);
                } else {
                    self.phase = Phase::Settle { i, since, steps, stable };
                }
            }
            Phase::Click { i } => {
                let from = app.st.page;
                let p = first_tab_point(app.island_body());
                crate::pinfo!("selftest: clicking the first header tab at ({:.0}, {:.0}) DIP on page '{}'", p.x, p.y, from.key());
                app.selftest_click(p);
                self.phase = Phase::ClickWait { i, since: now, from };
            }
            Phase::ClickWait { i, since, from } => {
                let want = PageKind::ALL[0];
                let page = app.st.page;
                if page != from || now - since > CLICK_TIMEOUT {
                    let ok = from != want && page == want;
                    self.checks.push(check(
                        "tab-click",
                        ok,
                        format!("page '{}' -> '{}' after {:.0} ms (expected '{}')", from.key(), page.key(), (now - since) * 1000.0, want.key()),
                    ));
                    if !ok {
                        crate::pwarn!("selftest: tab click did not switch to '{}' (page is '{}')", want.key(), page.key());
                    }
                    self.phase = Phase::Settle { i, since: now, steps: 0, stable: 0 };
                }
            }
            Phase::Finish { since } => {
                let all_done = self.writes.iter().all(|(_, h)| h.is_finished());
                let deadline = if self.anim { WRITE_DEADLINE + 40.0 } else { WRITE_DEADLINE };
                if all_done || elapsed > deadline {
                    let code = self.finish(all_done, now - since);
                    self.phase = Phase::Done(code);
                    return Some(code);
                }
            }
            Phase::Done(code) => return Some(code),
        }
        None
    }

    fn next_phase(&self, i: usize, now: f64) -> Phase {
        match SCENES.get(i) {
            Some(Scene::TabClick) => Phase::Click { i },
            Some(_) => Phase::Settle { i, since: now, steps: 0, stable: 0 },
            None if self.anim => Phase::Anim { seq: 0 },
            None => Phase::Finish { since: now },
        }
    }

    /// Settle the `from` state, switch to `to`, and save every frame of the
    /// transition as anim-<name>-NN.png. Blocks the UI thread for ~1-3 s,
    /// which is fine in self-test mode.
    fn record_anim(&mut self, app: &mut App, seq: usize) {
        let (name, from, to) = ANIMS[seq];
        let t = Instant::now();
        loop {
            self.apply(app, from);
            let _ = app.selftest_capture();
            if (app.selftest_settled() && t.elapsed().as_millis() > 200) || t.elapsed().as_secs_f64() > 2.5 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        self.apply(app, to);
        let mut settle_frame = None;
        // Where each frame sat on screen, to check that the island's centre holds still.
        let mut placed = Vec::with_capacity(ANIM_FRAMES);
        for k in 0..ANIM_FRAMES {
            let (w, h, px, at) = app.selftest_capture();
            if settle_frame.is_none() && app.selftest_settled() {
                settle_frame = Some(k);
            }
            placed.push(format!("{{\"frame\": {k}, \"x\": {}, \"y\": {}, \"w\": {w}, \"h\": {h}}}", at.0, at.1));
            let scale = self.scale;
            self.queue_write(&format!("anim-{name}-{k:02}"), w, h, px, scale);
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        let json = format!("{{\"name\": \"{name}\", \"scale\": {}, \"frames\": [\n  {}\n]}}\n", self.scale, placed.join(",\n  "));
        if let Err(e) = fs::write(self.dir.join(format!("anim-{name}.json")), json) {
            crate::pwarn!("selftest: writing anim-{name}.json failed: {e}");
        }
        crate::pinfo!("selftest: anim '{name}' recorded, settled at frame {settle_frame:?}");
    }

    // ------------------------------------------------------------ scene state

    /// Put the app into `scene`. Idempotent: called on every step and right
    /// before each capture so clocks (track position, countdown) are pinned.
    fn apply(&self, app: &mut App, scene: Scene) {
        let now = clock::mono();
        let st = &mut app.st;
        st.settings.idle_style = if scene == Scene::IdleEyes { IdleStyle::Eyes } else { IdleStyle::Clock };
        if st.settings.theme != self.theme {
            st.settings.theme = self.theme;
            st.accent = if self.theme == Theme::Oyen { crate::oyen::ORANGE } else { palette::BLUE };
        }
        st.power = fake_power(scene == Scene::ChargingHud);
        st.stats = fake_stats();
        st.volume = VolumeState { level: 0.64, muted: false, known: true };
        st.pinned = false;
        st.drag_hover = scene == Scene::ShelfDrop;
        st.health = if matches!(scene, Scene::HealthSignedOut | Scene::HealthSignedIn) { None } else { self.health.clone() };
        st.phone = fake_phone(scene);
        st.store.shelf.items = if scene == Scene::ShelfEmpty { Vec::new() } else { self.shelf_items.clone() };
        if scene != Scene::ShelfEmpty {
            for (path, thumb) in &self.shelf_thumbs {
                st.shelf_thumbs.entry(path.clone()).or_insert_with(|| thumb.clone());
            }
        }

        if scene.media_playing() {
            let timeline = Timeline::new(TRACK_POS_S, TRACK_LEN_S, now, true);
            match st.media.as_mut() {
                Some(m) if m.track_key == self.track_key => m.timeline = timeline,
                _ => st.media = Some(self.media_session(timeline)),
            }
            if st.lyrics.track_key != self.track_key || st.lyrics.status != LyricsStatus::Found {
                st.lyrics = self.lyrics.clone();
            }
        } else {
            st.media = None;
            st.lyrics = LyricsState::default();
        }

        st.pomodoro.phase = PomoPhase::Focus;
        st.pomodoro.cycle_done = 1;
        st.pomodoro.run = if scene.timer_running() { RunState::Running { ends_at: now + TIMER_LEFT_S } } else { RunState::Idle };

        let until = now + 30.0;
        st.hud = match scene {
            Scene::VolumeHud => Some(Hud { kind: HudKind::Volume { level: 0.64, muted: false }, until }),
            Scene::ChargingHud => Some(Hud { kind: HudKind::Charging { percent: 76, plugged: true }, until }),
            Scene::BluetoothHud => {
                Some(Hud { kind: HudKind::Bluetooth { name: "Sound Buds".into(), connected: true, battery: Some(80), audio: true }, until })
            }
            _ => None,
        };
        st.notice = match scene {
            Scene::HealthNotice => {
                let steps = st.health.as_ref().map_or(0, |h| h.steps_on(&st.local.day_key()));
                Some(crate::app::steps_goal_notice(steps, until))
            }
            Scene::OyenzaMoney | Scene::OyenzaMoneyShown => {
                let hidden = scene == Scene::OyenzaMoney;
                Some(Notice {
                    icon: Icon::MoneyIn,
                    color: palette::GREEN,
                    title: "Money in · BCA".into(),
                    subtitle: if hidden { "Amount and sender hidden".into() } else { "Rp 1.250.000 · Gaji Oktober".into() },
                    until,
                    action: Some(NoticeAction::OpenPage(PageKind::Oyenza)),
                    inline: Some(InlineAction::ShowAmount),
                    reveal: hidden.then(|| "Rp 1.250.000 · Gaji Oktober".into()),
                })
            }
            Scene::OyenzaBill => Some(Notice {
                icon: Icon::Calendar,
                color: palette::ORANGE,
                title: "Spotify renews tomorrow".into(),
                subtitle: "Amount hidden".into(),
                until,
                action: Some(NoticeAction::OpenPage(PageKind::Oyenza)),
                inline: Some(InlineAction::ShowAmount),
                reveal: Some("Rp 54.990".into()),
            }),
            _ => None,
        };
        st.oyenza = if scene.oyenza() { fake_oyenza(st, scene, now) } else { OyenzaState::default() };
        st.system = fake_system(scene);
        st.ui.system_module = match scene {
            Scene::SystemMemory => Module::Memory,
            Scene::SystemDrives => Module::Drives,
            Scene::SystemNetwork => Module::Network,
            Scene::SystemBattery => Module::Battery,
            _ => Module::Cpu,
        };
        for (path, rgb) in SAMPLE_ICONS {
            st.app_icons.entry(path.to_string()).or_insert_with(|| Some(sample_icon(rgb)));
        }
        if let Some(p) = scene.page() {
            st.page = p;
        }
        app.selftest_hover(None);
        app.selftest_set_expanded(scene.expanded());
    }

    fn media_session(&self, timeline: Timeline) -> MediaSession {
        MediaSession {
            app_id: "Spotify.exe".into(),
            app_name: "Spotify".into(),
            title: TITLE.into(),
            artist: ARTIST.into(),
            album: ALBUM.into(),
            art: self.art.clone(),
            art_color: self.art_color,
            timeline,
            can_prev: true,
            can_next: true,
            can_play_pause: true,
            can_seek: true,
            track_key: self.track_key.clone(),
        }
    }

    fn seed_shelf(&mut self, st: &mut AppState) {
        let home = paths::data_dir();
        let base = std::path::absolute(&home).unwrap_or(home).join("shelf-demo");
        let txt = base.join("Trip notes.txt");
        let img = base.join("Harbor sketch.png");
        let folder = base.join("Project assets");
        let mut errors = Vec::new();
        if let Err(e) = fs::create_dir_all(&folder) {
            errors.push(format!("{}: {e}", folder.display()));
        }
        if let Err(e) = fs::write(folder.join("readme.txt"), "Drafts and exports for the harbor poster.\n") {
            errors.push(format!("readme.txt: {e}"));
        }
        if let Err(e) = fs::write(&txt, NOTES_TXT) {
            errors.push(format!("{}: {e}", txt.display()));
        }
        if let Some(art) = &self.art {
            if let Err(e) = png::save_pbgra(&img, art.width, art.height, &art.pixels) {
                errors.push(e);
            }
        }
        let made = [&folder, &img, &txt].iter().filter(|p| p.exists()).count();
        self.checks.push(check("shelf-files", errors.is_empty() && made == 3, if errors.is_empty() { format!("{made} items in {}", base.display()) } else { errors.join("; ") }));

        // Oldest first: add_paths puts each new path at the front.
        let unix = st.unix;
        for (k, p) in [&folder, &img, &txt].into_iter().enumerate() {
            let s = p.to_string_lossy().into_owned();
            st.store.shelf.add_paths(&[s.as_str()], unix - 3600 + k as i64 * 600);
            // Pre-seeded thumbnails keep the screenshots identical on every machine
            // (the app skips LoadThumb for paths it already has an entry for).
            let thumb = if p == &img { self.art.clone() } else { None };
            self.shelf_thumbs.push((s.clone(), thumb.clone()));
            st.shelf_thumbs.insert(s, thumb);
        }
        self.shelf_items = st.store.shelf.items.clone();
    }

    // ------------------------------------------------------------ capture + checks

    fn capture(&mut self, app: &mut App, i: usize, settled: bool, waited: f64) {
        let scene = SCENES[i];
        let name = format!("{:02}-{}", i + 1, scene.name());
        let mut samples = Vec::with_capacity(FRAME_SAMPLES);
        let mut shot = (0, 0, Vec::new(), (0, 0));
        for _ in 0..FRAME_SAMPLES {
            self.apply(app, scene);
            let t = Instant::now();
            shot = app.selftest_capture();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            samples.push(round2(ms));
            if ms < FRAME_BUDGET_MS {
                break;
            }
        }
        let (w, h, px, present) = shot;
        let scale = app.scale();
        let body = app.island_body_in_capture();
        let frame = Frame { w, h, px: &px };
        let best = samples.iter().copied().fold(f64::INFINITY, f64::min);

        let mut checks = vec![check("settled", settled, format!("{:.0} ms (limit {:.0} ms)", waited * 1000.0, SETTLE_TIMEOUT * 1000.0))];
        let mut measured = 0;
        if w <= 0 || h <= 0 || px.len() != (w as usize) * (h as usize) * 4 {
            checks.push(check("pixels", false, format!("{w}x{h} frame with {} bytes", px.len())));
        } else {
            let cx = ((body.x + body.w / 2.0) * scale).floor() as i32;
            let top = (3.0 * scale).round() as i32;
            let [b, g, r, a] = frame.at(cx, top);
            let ground = rgb8(poni_core::look::of(self.theme).ground);
            let off = [r, g, b].iter().zip(ground).map(|(&p, q)| p.abs_diff(q)).max().unwrap_or(0);
            checks.push(check(
                "body-opaque-ground",
                a >= 254 && off <= 6,
                format!("pixel ({cx}, {top}) = rgba({r}, {g}, {b}, {a}), ground rgb{ground:?}"),
            ));

            let corners = [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)].map(|(x, y)| frame.at(x, y)[3]);
            checks.push(check("corners-transparent", corners.iter().all(|&a| a <= 1), format!("corner alpha {corners:?}")));

            let mid = ((body.y + body.h / 2.0) * scale).floor() as i32;
            measured = frame.opaque_run(cx, mid);
            let want = body.w * scale;
            checks.push(check(
                "body-width",
                (measured as f32 - want).abs() <= 2.0,
                format!("{measured} px opaque at row {mid}, geometry {want:.1} px"),
            ));
            if scene.expanded() {
                let ew = app.st.settings.expanded_width * scale;
                checks.push(check(
                    "expanded-width",
                    (measured as f32 - ew).abs() <= 2.0,
                    format!("{measured} px vs settings.expanded_width {:.0} x {scale:.2} = {ew:.1} px", app.st.settings.expanded_width),
                ));
            }

            let lit = frame.content_pixels(body, scale, rgb8(poni_core::look::of(self.theme).ground));
            checks.push(check("content-drawn", lit >= 20, format!("{lit} content pixels inside the island")));
        }
        checks.push(check(
            "frame-time",
            best < FRAME_BUDGET_MS,
            format!("best {best:.2} ms of {samples:?} (budget {FRAME_BUDGET_MS} ms)"),
        ));
        checks.push(window_check(app.hwnd(), app.selftest_surface(), present, w, h));
        if let Some(want) = scene.page() {
            let page = app.st.page;
            checks.push(check("page", page == want, format!("showing '{}', expected '{}'", page.key(), want.key())));
        }

        let ok = checks.iter().all(|c| c.ok);
        crate::pinfo!(
            "selftest: [{:02}/{}] {}: {} (settle {:.0} ms, frame {best:.2} ms, {w}x{h} px)",
            i + 1,
            SCENES.len(),
            scene.name(),
            if ok { "ok" } else { "FAILED" },
            waited * 1000.0
        );
        for c in checks.iter().filter(|c| !c.ok) {
            crate::pwarn!("selftest:   {} failed: {}", c.name, c.detail);
        }

        if w > 0 && h > 0 && px.len() == (w as usize) * (h as usize) * 4 {
            self.queue_write(&name, w, h, px, scale);
        }
        self.states.push(StateReport {
            index: i + 1,
            name: scene.name(),
            expanded: scene.expanded(),
            page: scene.page().map(|p| p.key()),
            image: format!("{name}.png"),
            alpha_image: format!("{name}-alpha.png"),
            ok,
            settle_ms: (waited * 1000.0).round().max(0.0) as u64,
            frame_ms: round2(best),
            frame_samples_ms: samples,
            canvas_px: [w, h],
            window_pos_px: [present.0, present.1],
            body_dip: [body.x, body.y, body.w, body.h],
            measured_width_px: measured,
            checks,
        });
    }

    /// Composite + alpha PNGs, encoded off the UI thread.
    fn queue_write(&mut self, name: &str, w: i32, h: i32, px: Vec<u8>, scale: f32) {
        let dir = self.dir.clone();
        let base = name.to_string();
        let job = move || {
            let mut errors = Vec::new();
            let (cw, ch, comp) = composite(w as usize, h as usize, &px, scale);
            if let Err(e) = png::save_rgba(dir.join(format!("{base}.png")), cw, ch, &comp) {
                errors.push(e);
            }
            let alpha: Vec<u8> = px.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
            if let Err(e) = png::save_gray(dir.join(format!("{base}-alpha.png")), w as u32, h as u32, &alpha) {
                errors.push(e);
            }
            errors
        };
        match std::thread::Builder::new().name("poni-selftest-png".into()).spawn(job) {
            Ok(handle) => self.writes.push((name.to_string(), handle)),
            Err(e) => self.checks.push(check("png-writer", false, format!("{name}: could not start a writer thread: {e}"))),
        }
    }

    fn finish(&mut self, writers_done: bool, waited: f64) -> i32 {
        let mut write_errors = Vec::new();
        let mut written = 0;
        for (name, handle) in std::mem::take(&mut self.writes) {
            if !handle.is_finished() {
                write_errors.push(format!("{name}: still encoding"));
                continue; // detached; the process exits soon
            }
            match handle.join() {
                Ok(errs) if errs.is_empty() => written += 1,
                Ok(errs) => write_errors.extend(errs),
                Err(_) => write_errors.push(format!("{name}: writer thread panicked")),
            }
        }
        let detail = if write_errors.is_empty() {
            format!("{written} screenshot pairs written in {:.0} ms after the last capture", waited * 1000.0)
        } else {
            write_errors.join("; ")
        };
        self.checks.push(check("png-files", writers_done && write_errors.is_empty(), detail));
        if self.states.len() < SCENES.len() {
            self.checks.push(check("states-visited", false, format!("{} of {} states captured", self.states.len(), SCENES.len())));
        }

        let mut failed: Vec<String> = self.checks.iter().filter(|c| !c.ok).map(|c| c.name.clone()).collect();
        for s in &self.states {
            failed.extend(s.checks.iter().filter(|c| !c.ok).map(|c| format!("{}: {}", s.name, c.name)));
        }
        let ok = failed.is_empty();
        let mut code = if ok { 0 } else { 1 };
        let total_ms = self.t0.elapsed().as_millis() as u64;
        let report = Report {
            app: "Poni",
            version: env!("CARGO_PKG_VERSION"),
            ok,
            exit_code: code,
            total_ms,
            scale: self.scale,
            expanded_width_dip: self.expanded_width,
            renderer: self.renderer.to_string(),
            frame_budget_ms: FRAME_BUDGET_MS,
            states_passed: self.states.iter().filter(|s| s.ok).count(),
            states_total: SCENES.len(),
            failed_checks: failed.clone(),
            checks: std::mem::take(&mut self.checks),
            states: std::mem::take(&mut self.states),
        };
        let path = self.dir.join("report.json");
        match serde_json::to_vec_pretty(&report).map_err(|e| e.to_string()).and_then(|j| fs::write(&path, j).map_err(|e| e.to_string())) {
            Ok(()) => crate::pinfo!("selftest: report written to {}", path.display()),
            Err(e) => {
                crate::pwarn!("selftest: could not write {}: {e}", path.display());
                code = 1;
            }
        }
        if ok {
            crate::pinfo!("selftest: all {} states passed in {total_ms} ms", report.states_total);
        } else {
            crate::pwarn!("selftest: {} check(s) failed in {total_ms} ms: {}", failed.len(), failed.join(", "));
        }
        code
    }

    /// Remove screenshots and the report from an earlier run (only our own file names).
    fn clear_old_outputs(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let b = name.as_bytes();
            let ours = name == "report.json"
                || (name.ends_with(".png") && b.len() > 3 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b'-');
            if ours && e.path().is_file() {
                if let Err(err) = fs::remove_file(e.path()) {
                    crate::pwarn!("selftest: could not remove old {name}: {err}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------- fixtures

fn seed_tasks(st: &mut AppState) {
    let unix = st.unix;
    let day_start = st.day_start_unix();
    let tasks = &mut st.store.tasks;
    let done = tasks.add("Water the balcony plants", unix - 5 * 3600);
    tasks.add("Sketch the onboarding screens", unix - 3 * 3600);
    tasks.add("Book the ferry for Saturday", unix - 2 * 3600);
    if let Some(id) = done {
        tasks.toggle(id, (unix - 1800).max(day_start + 60));
    }
}

/// A week of steps as Oyenza would send it: today just over the goal.
fn sample_health(st: &AppState) -> HealthSnapshot {
    let today = st.local;
    let week = [5210u32, 9120, 7480, 3960, 10230, 6840, 8412];
    let days = week
        .iter()
        .enumerate()
        .map(|(i, &steps)| {
            let last = i + 1 == week.len();
            HealthDay {
                date: today.add_days(i as i64 + 1 - week.len() as i64).day_key(),
                steps,
                distance_m: last.then_some(6140),
                active_kcal: last.then_some(342),
            }
        })
        .collect();
    HealthSnapshot { goal: 8000, source: HealthSource::AppleHealth, updated_at: st.unix - 8 * 60, days }
}

fn seed_clipboard(st: &mut AppState) {
    let unix = st.unix;
    // Oldest first, so the last one ends up at the top.
    for (k, text) in CLIPS.iter().enumerate() {
        st.clip.push(text, unix - (CLIPS.len() - k) as i64 * 420);
    }
    // Pin the colour code (index counted from the newest).
    let pin = st.clip.iter().position(|c| c.text == "#5AC8FA");
    if let Some(i) = pin {
        st.clip.toggle_pin(i);
    }
}

/// Procedural album art: a harbor sunset with a paper boat (premultiplied BGRA, opaque).
fn make_art(size: u32) -> Vec<u8> {
    const SS: usize = 3; // 3x3 supersampling for smooth edges
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
    let mut c = if v < HORIZON {
        mix(rgb(0xFFB37A), rgb(0xE0607E), v / HORIZON)
    } else {
        mix(rgb(0x2F6F8F), rgb(0x13233F), (v - HORIZON) / (1.0 - HORIZON))
    };
    let d = ((u - sun_x).powi(2) + (v - sun_y).powi(2)).sqrt();
    if v < HORIZON && d < sun_r {
        c = mix(c, rgb(0xFFE3A3), 0.92);
    }
    // Glitter: short streaks on the water under the sun, narrowing with depth.
    if v > HORIZON + 0.015 && v < HORIZON + 0.24 {
        let depth = (v - HORIZON) / 0.24;
        let band = ((v - HORIZON) * 42.0).fract();
        if band < 0.4 && (u - sun_x).abs() < 0.12 * (1.0 - depth) {
            c = mix(c, rgb(0xFFD08A), 0.6 * (1.0 - depth));
        }
    }
    // Paper boat: a hull trapezoid and a folded sail.
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

// ---------------------------------------------------------------- pixels

/// Premultiplied BGRA frame with bounds-checked reads.
struct Frame<'a> {
    w: i32,
    h: i32,
    px: &'a [u8],
}

impl Frame<'_> {
    /// `[b, g, r, a]`, or transparent outside the frame.
    fn at(&self, x: i32, y: i32) -> [u8; 4] {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return [0; 4];
        }
        let o = (y as usize * self.w as usize + x as usize) * 4;
        match self.px.get(o..o + 4) {
            Some(p) => [p[0], p[1], p[2], p[3]],
            None => [0; 4],
        }
    }

    /// Width of the run of mostly-opaque pixels through (`x`, `y`).
    fn opaque_run(&self, x: i32, y: i32) -> i32 {
        if self.at(x, y)[3] < 128 {
            return 0;
        }
        let (mut l, mut r) = (x, x);
        while l > 0 && self.at(l - 1, y)[3] >= 128 {
            l -= 1;
        }
        while r + 1 < self.w && self.at(r + 1, y)[3] >= 128 {
            r += 1;
        }
        r - l + 1
    }

    /// Opaque, bright pixels inside the island body (text, icons, art).
    /// Opaque pixels inside the island that stand clearly apart from its
    /// ground colour (text, icons, art).
    fn content_pixels(&self, body: Rect, scale: f32, ground: [u8; 3]) -> usize {
        let x0 = (body.x * scale).floor().max(0.0) as i32;
        let x1 = ((body.x + body.w) * scale).ceil() as i32;
        let y1 = ((body.y + body.h) * scale).ceil() as i32;
        let mut n = 0;
        for y in (body.y * scale).floor().max(0.0) as i32..y1.min(self.h) {
            for x in x0..x1.min(self.w) {
                let [b, g, r, a] = self.at(x, y);
                let off = [r, g, b].iter().zip(ground).map(|(&p, q)| p.abs_diff(q)).max().unwrap_or(0);
                if a >= 200 && off >= 140 {
                    n += 1;
                }
            }
        }
        n
    }
}

/// The island window must be a visible, top-most, non-activating layered
/// tool window, laid out where the app placed it, with the frame inside it.
fn window_check(hwnd: HWND, surface: (i32, i32, i32, i32), present: (i32, i32), w: i32, h: i32) -> Check {
    let need = WS_EX_LAYERED.0 | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0;
    let (visible, ex, rect) = unsafe {
        let visible = IsWindowVisible(hwnd).as_bool();
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let mut rc = RECT::default();
        let rect = GetWindowRect(hwnd, &mut rc).ok().map(|_| rc);
        (visible, ex, rect)
    };
    let styles_ok = ex & need == need;
    let (sx, sy, sw, sh) = surface;
    let rect_ok = rect.is_some_and(|r| {
        r.left == sx && r.top == sy && r.right - r.left == sw && r.bottom - r.top == sh
            && present.0 >= r.left && present.1 >= r.top && present.0 + w <= r.right && present.1 + h <= r.bottom
    });
    let rect_s = rect.map(|r| format!("({}, {}) {}x{}", r.left, r.top, r.right - r.left, r.bottom - r.top)).unwrap_or_else(|| "unavailable".into());
    check(
        "window",
        visible && styles_ok && rect_ok,
        format!("visible {visible}, ex-style {ex:#x} (needs {need:#x}), rect {rect_s} (laid out ({sx}, {sy}) {sw}x{sh}), frame at ({}, {}) {w}x{h}", present.0, present.1),
    )
}

/// Centre of the first header tab. Mirrors `pages::draw_header`: tabs start at
/// the header's left edge and are 30 DIPs wide while inactive.
fn first_tab_point(body: Rect) -> Point {
    let header = crate::pages::header_rect(body);
    Point::new(header.x + 15.0, header.center().y)
}

fn mid_blink() -> bool {
    let phase = clock::mono() % BLINK_PERIOD;
    phase < BLINK_GUARD.0 || phase > BLINK_PERIOD - BLINK_GUARD.1
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Frame over a soft wallpaper-like gradient, with room around it, the way
/// the island hangs from the top of a desktop. Returns straight RGBA.
fn composite(w: usize, h: usize, pbgra: &[u8], scale: f32) -> (u32, u32, Vec<u8>) {
    let pad_x = (48.0 * scale).round().max(0.0) as usize;
    let pad_b = (40.0 * scale).round().max(0.0) as usize;
    let (ow, oh) = (w + 2 * pad_x, h + pad_b);
    let (tl, tr, bl, br) = (rgb(0x5D6B84), rgb(0x837C98), rgb(0x8E8C96), rgb(0xB9A596));
    let fx = 1.0 / (ow.max(2) - 1) as f32;
    let fy = 1.0 / (oh.max(2) - 1) as f32;
    let mut out = vec![0u8; ow * oh * 4];
    for y in 0..oh {
        let v = y as f32 * fy;
        let (left, right) = (mix(tl, bl, v), mix(tr, br, v));
        for x in 0..ow {
            let bg = mix(left, right, x as f32 * fx);
            let mut c = [bg[0] * 255.0, bg[1] * 255.0, bg[2] * 255.0];
            if y < h && x >= pad_x && x < pad_x + w {
                let o = (y * w + (x - pad_x)) * 4;
                if let Some(p) = pbgra.get(o..o + 4) {
                    let inv = 1.0 - p[3] as f32 / 255.0;
                    c = [p[2] as f32 + c[0] * inv, p[1] as f32 + c[1] * inv, p[0] as f32 + c[2] * inv];
                }
            }
            let o = (y * ow + x) * 4;
            out[o] = c[0].round().clamp(0.0, 255.0) as u8;
            out[o + 1] = c[1].round().clamp(0.0, 255.0) as u8;
            out[o + 2] = c[2].round().clamp(0.0, 255.0) as u8;
            out[o + 3] = 255;
        }
    }
    (ow as u32, oh as u32, out)
}

/// A colour as 8-bit (r, g, b).
fn rgb8(c: Color) -> [u8; 3] {
    [c.r, c.g, c.b].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}
