//! All state shared by pages, activities and the app loop.
//!
//! Owned by the UI thread. Services never touch it directly; they send
//! [`crate::events::AppEvent`]s that the app applies here.

use crate::commands::{Command, MediaCmd, Sound};
use crate::gfx::Image;
use crate::icons::Icon;
use crate::pages::PageKind;
use poni_core::cliphist::ClipHistory;
use poni_core::datetime::LocalDateTime;
use poni_core::geom::{palette, Color};
use poni_core::health::HealthSnapshot;
use poni_core::lrc::Lyrics;
use poni_core::oyenza::{Arrival, Bill, Saved};
use poni_core::phonedrop::QrMatrix;
use poni_core::pomodoro::Pomodoro;
use poni_core::romanize::Ruby;
use poni_core::settings::Settings;
use poni_core::store::Store;
use poni_core::timeline::Timeline;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct MediaSession {
    /// AppUserModelId of the player (e.g. `Spotify.exe`, `Chrome`).
    pub app_id: String,
    /// Friendly player name ("Spotify", "Chrome", …).
    pub app_name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: Option<Image>,
    /// Accent derived from the art (for progress bars / the activity glow).
    pub art_color: Option<Color>,
    pub timeline: Timeline,
    pub can_prev: bool,
    pub can_next: bool,
    pub can_play_pause: bool,
    pub can_seek: bool,
    /// Identity of the track: `title\u{1}artist\u{1}album`.
    pub track_key: String,
}

impl MediaSession {
    pub fn make_track_key(title: &str, artist: &str, album: &str) -> String {
        format!("{title}\u{1}{artist}\u{1}{album}")
    }
    pub fn is_playing(&self) -> bool {
        self.timeline.playing
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LyricsStatus {
    Idle,
    Loading,
    Found,
    Instrumental,
    NotFound,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct LyricsState {
    pub track_key: String,
    pub status: LyricsStatus,
    pub lyrics: Option<Lyrics>,
    pub source: String,
    /// Each line's pieces with their readings in Latin letters (Japanese,
    /// Korean, Chinese); empty for lines that need none.
    pub ruby: Vec<Vec<Ruby>>,
}

impl Default for LyricsState {
    fn default() -> Self {
        Self { track_key: String::new(), status: LyricsStatus::Idle, lyrics: None, source: String::new(), ruby: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VolumeState {
    pub level: f32,
    pub muted: bool,
    pub known: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PowerState {
    pub has_battery: bool,
    /// 0..=100 (meaningful only with a battery).
    pub percent: u8,
    pub charging: bool,
    pub on_ac: bool,
    pub saver: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SysStats {
    pub valid: bool,
    /// 0..=1
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_free: u64,
    pub disk_total: u64,
    pub uptime_s: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HudKind {
    Volume { level: f32, muted: bool },
    Charging { percent: u8, plugged: bool },
    Bluetooth { name: String, connected: bool, battery: Option<u8>, audio: bool },
    LowBattery { percent: u8 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hud {
    pub kind: HudKind,
    /// Monotonic time when it disappears.
    pub until: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoticeAction {
    OpenPage(PageKind),
    /// Open Focus and start the (idle) timer.
    StartFocus,
}

/// A one-click action shown as a small pill inside a notice (doesn't expand).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineAction {
    /// Start the idle Pomodoro phase.
    StartFocus,
    /// Put the notice's hidden sum (and sender) in place (Oyenza).
    ShowAmount,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub icon: Icon,
    pub color: Color,
    pub title: String,
    pub subtitle: String,
    pub until: f64,
    pub action: Option<NoticeAction>,
    pub inline: Option<InlineAction>,
    /// The subtitle Show puts in place of `subtitle` ([`InlineAction::ShowAmount`]).
    pub reveal: Option<String>,
}

/// A phone wants to send these files; the Shelf asks the person at the PC.
#[derive(Clone, Debug, PartialEq)]
pub struct PhoneAsk {
    pub id: u64,
    /// "iPhone", "Android phone", …
    pub from: String,
    /// (name, size) in the order the phone sends them.
    pub files: Vec<(String, u64)>,
    pub total: u64,
}

/// Files coming in from the phone.
#[derive(Clone, Debug, PartialEq)]
pub struct PhoneTransfer {
    pub id: u64,
    pub from: String,
    /// The file being sent (0-based) and how many there are.
    pub file: usize,
    pub files: usize,
    pub done: u64,
    pub total: u64,
}

/// Phone drop while the Shelf's phone panel is open: the link, its QR code
/// and what the phone is doing (`services::phonedrop` runs the server).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhoneDrop {
    /// The link the QR code carries (empty while starting or after an error).
    pub url: String,
    pub qr: Option<QrMatrix>,
    /// Why it couldn't start, as a sentence for the island.
    pub error: Option<String>,
    /// The kind of phone that last opened the page ("iPhone").
    pub visitor: Option<String>,
    pub ask: Option<PhoneAsk>,
    pub transfer: Option<PhoneTransfer>,
    /// The last result, whether it's good news, and until when it shows.
    pub outcome: Option<(String, bool, f64)>,
}

impl PhoneDrop {
    /// The island stays open while the QR code waits for a phone and while
    /// a phone waits for an answer.
    pub fn holds_island(&self) -> bool {
        self.ask.is_some() || (self.error.is_none() && self.visitor.is_none() && self.transfer.is_none())
    }
}

/// Transient per-page UI state (scroll offsets, text being typed, …).
#[derive(Clone, Debug, Default)]
pub struct PageUiState {
    pub task_input: String,
    pub task_scroll: f32,
    pub clip_scroll: f32,
    pub shelf_scroll: f32,
    /// Smoothly animated lyric scroll position (line index as float).
    pub lyric_pos: f32,
    pub lyric_track: String,
    /// Clipboard row that just got copied (index, mono time) for a "Copied" flash.
    pub clip_flash: Option<(usize, f64)>,
    /// Volume HUD bar position, gliding toward the real level.
    pub hud_level: f32,
    /// Collapsed media pill shows the track title until this mono time.
    pub peek_until: f64,
    /// Header tab under the pointer and since when (for hover labels).
    pub tab_hover: Option<(usize, f64)>,
    /// The module the System page shows on the right.
    pub system_module: poni_core::sysmon::Module,
    /// The page the Settings button was pressed on (pressing it again goes back).
    pub before_settings: Option<PageKind>,
    /// Start with Windows, as last read off the UI thread (None until Settings first opens).
    pub autostart: Option<bool>,
}

/// Oyenza sign-in, as the island sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum OyenzaLink {
    #[default]
    Off,
    /// A code on screen, waiting for approval in oyenza.app, until mono time `until`.
    Connecting { code: String, url: String, until: f64 },
    Connected { account: String },
}

/// What the Oyenza tab and its notices show (`pages::oyenza`). Oyen's moods
/// never read any of it: money never makes him happier or a bill sadder.
#[derive(Clone, Debug, Default)]
pub struct OyenzaState {
    pub link: OyenzaLink,
    /// Today's money in, newest first.
    pub today: Vec<Arrival>,
    pub bills: Vec<Bill>,
    /// The server's today for `bills` (YYYY-MM-DD).
    pub bills_today: String,
    /// Unix time of Oyenza's last answer ("Updated 13:49").
    pub updated_unix: Option<i64>,
    /// The spend being typed.
    pub input: String,
    /// A note is on its way to the server.
    pub saving: bool,
    /// What was just saved, with Undo until mono time `.1`.
    pub saved: Option<(Vec<Saved>, f64)>,
    /// Why the last note wasn't saved (until the text changes).
    pub error: Option<String>,
    /// A short line ("Taken back") until mono time `.1`.
    pub flash: Option<(String, f64)>,
    /// "Show amounts" in the tab, until the island closes.
    pub reveal: bool,
    /// A notice that came while a full-screen app hid the island, and when.
    pub held: Option<(Notice, f64)>,
    /// Arrivals already shown as a notice (`oyenza::unseen`), so none repeats.
    pub shown: Vec<String>,
    /// Pinned before a sign-in code pinned the island open.
    pub pinned_before: Option<bool>,
}

impl OyenzaState {
    /// The Oyenza tab shows while signing in and once signed in.
    pub fn tab_visible(&self) -> bool {
        self.link != OyenzaLink::Off
    }
}

pub struct AppState {
    pub settings: Settings,
    pub store: Store,
    /// Monotonic seconds at the current frame.
    pub now: f64,
    pub local: LocalDateTime,
    pub unix: i64,
    pub media: Option<MediaSession>,
    pub lyrics: LyricsState,
    pub pomodoro: Pomodoro,
    pub volume: VolumeState,
    pub power: PowerState,
    pub hud: Option<Hud>,
    pub notice: Option<Notice>,
    pub clip: ClipHistory,
    pub stats: SysStats,
    /// The System page's readings and their last minute (empty while it's closed).
    pub system: poni_core::sysmon::Monitor,
    pub shelf_thumbs: HashMap<String, Option<Image>>,
    /// Icons of the apps the System page lists, by executable path (None = loading or none).
    pub app_icons: HashMap<String, Option<Image>>,
    pub ui: PageUiState,
    /// System accent colour (falls back to blue).
    pub accent: Color,
    /// Steps from the person's phone, sent by Oyenza (None until connected).
    pub health: Option<HealthSnapshot>,
    /// The page currently shown when expanded.
    pub page: PageKind,
    /// When playback went from playing to paused (the pill lingers 30 s).
    pub media_paused_at: Option<f64>,
    pub pinned: bool,
    /// Files are being dragged over the island right now.
    pub drag_hover: bool,
    /// Phone drop, while it's open.
    pub phone: Option<PhoneDrop>,
    pub oyenza: OyenzaState,
    /// Side effects requested during the frame.
    pub cmds: Vec<Command>,
}

impl AppState {
    pub fn new(settings: Settings, store: Store) -> Self {
        let pomodoro = Pomodoro::new(settings.pomodoro.clone());
        let page = PageKind::from_key(if settings.remember_page && !store.last_page.is_empty() {
            &store.last_page
        } else {
            &settings.start_page
        });
        Self {
            settings,
            store,
            now: 0.0,
            local: LocalDateTime::default(),
            unix: 0,
            media: None,
            lyrics: LyricsState::default(),
            pomodoro,
            volume: VolumeState::default(),
            power: PowerState::default(),
            hud: None,
            notice: None,
            clip: ClipHistory::default(),
            stats: SysStats::default(),
            system: poni_core::sysmon::Monitor::default(),
            shelf_thumbs: HashMap::new(),
            app_icons: HashMap::new(),
            ui: PageUiState::default(),
            accent: palette::BLUE,
            health: None,
            page,
            media_paused_at: None,
            pinned: false,
            drag_hover: false,
            phone: None,
            oyenza: OyenzaState::default(),
            cmds: Vec::new(),
        }
    }

    pub fn cmd(&mut self, c: Command) {
        self.cmds.push(c);
    }

    pub fn media_cmd(&mut self, c: MediaCmd) {
        self.cmds.push(Command::Media(c));
    }

    /// Mark the persisted store as changed.
    pub fn save(&mut self) {
        if !self.cmds.contains(&Command::SaveStore) {
            self.cmds.push(Command::SaveStore);
        }
    }

    pub fn chime(&mut self) {
        if self.settings.sounds {
            self.cmds.push(Command::Play(Sound::Chime));
        }
    }

    /// Show a notice pill for `secs` seconds.
    pub fn notify(&mut self, icon: Icon, color: Color, title: impl Into<String>, subtitle: impl Into<String>, secs: f64, action: Option<NoticeAction>) {
        self.notice = Some(Notice { icon, color, title: title.into(), subtitle: subtitle.into(), until: self.now + secs, action, inline: None, reveal: None });
    }

    /// Give the current notice a one-click inline action pill.
    pub fn set_notice_inline(&mut self, a: InlineAction) {
        if let Some(n) = self.notice.as_mut() {
            n.inline = Some(a);
        }
    }

    pub fn show_hud(&mut self, kind: HudKind, secs: f64) {
        self.hud = Some(Hud { kind, until: self.now + secs });
    }

    pub fn day_key(&self) -> String {
        self.local.day_key()
    }

    pub fn day_start_unix(&self) -> i64 {
        crate::clock::day_start_unix(&self.local, self.unix)
    }

    /// Accent for media UI: art colour when available, else the system accent.
    /// The Oyen theme keeps to Oyen's orange.
    pub fn media_accent(&self) -> Color {
        if self.settings.theme == poni_core::settings::Theme::Oyen {
            return self.accent;
        }
        self.media.as_ref().and_then(|m| m.art_color).unwrap_or(self.accent)
    }
}
