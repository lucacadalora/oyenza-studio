//! All state shared by pages, activities and the app loop.
//!
//! Owned by the UI thread. Services never touch it directly; they send
//! [`crate::events::AppEvent`]s that the app applies here.

use crate::commands::{Command, MediaCmd, Sound};
use crate::gfx::Image;
use crate::icons::Icon;
use crate::pages::PageKind;
use poni_core::addons::Status as AddonStatus;
use poni_core::datetime::LocalDateTime;
use poni_core::geom::{palette, Color};
use poni_core::health::HealthSnapshot;
use poni_core::lrc::Lyrics;
use poni_core::oyenza::{Activity, Bill, DayPick, Saved, Seen};
use poni_core::phonedrop::QrMatrix;
use poni_core::pomodoro::Pomodoro;
use poni_core::recorder::Phase as RecPhase;
use poni_core::prayer::{Place, PlaceList, PrayerMoment, Site, World};
use poni_core::romanize::Ruby;
use poni_core::settings::Settings;
use poni_core::store::Store;
use poni_core::timeline::Timeline;
use studio_usage::alerts::{Alert, AlertCfg, AlertKind};
use studio_usage::{ProviderId, Snapshot, Trouble};
use std::collections::{BTreeMap, BTreeSet, HashMap};

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
    /// The song's kanji went unread because Windows has no Japanese reading
    /// data and the Japanese readings add-on isn't on this PC: the app
    /// switches the add-on on the first time, and the Music page shows how
    /// it comes in (or offers it, after a switch-off by hand).
    pub wants_ja_readings: bool,
}

impl Default for LyricsState {
    fn default() -> Self {
        Self { track_key: String::new(), status: LyricsStatus::Idle, lyrics: None, source: String::new(), ruby: Vec::new(), wants_ja_readings: false }
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
    /// Open the Update sheet in Settings (a new version's notice).
    OpenUpdate,
    /// Open the app another app's notification came from (its
    /// AppUserModelID; the Notifications add-on). The island stays closed.
    OpenApp(String),
}

/// A one-click action shown as a small pill inside a notice (doesn't expand).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineAction {
    /// Start the idle Pomodoro phase.
    StartFocus,
    /// Put the notice's hidden words in place: an Oyenza sum (and sender),
    /// or a message and who it's from (Notifications).
    ShowAmount,
    /// Stop the adzan (Prayer times).
    StopAdzan,
    /// Put the notice away now (a new version's: the menu and Settings
    /// keep offering it).
    Hide,
    /// Open this version's release notes ("Updated to").
    WhatsNew,
    /// Open the app a notification came from (the notice's `OpenApp`).
    Open,
}

/// Whether this copy may read other apps' notifications (the Notifications
/// add-on, `services::notifications`), for its page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotifyAccess {
    /// Not looked at yet: the add-on is off, or just came on.
    #[default]
    Unknown,
    /// This copy has no package identity (the download from GitHub), and
    /// Windows lets only a packaged app read notifications.
    NoPackage,
    /// Windows hasn't been asked yet, or its question was closed unanswered.
    Ask,
    /// Windows' question is on screen.
    Asking,
    /// Asked, and Windows came back without an answer (the question was
    /// closed, or never showed): ask again, or turn it on in Windows' settings.
    Unanswered,
    /// Turned off for Oyenza Studio in Windows' settings.
    Denied,
    /// Windows lets it read them: new ones show on the island.
    Allowed,
    /// Windows' notification listener didn't answer (an older Windows, or a
    /// failure the log names).
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub icon: Icon,
    pub color: Color,
    pub title: String,
    /// Dimmer words after the title, on its line ("0.2.5" after "Update
    /// available"); empty for most notices.
    pub title_dim: String,
    pub subtitle: String,
    pub until: f64,
    pub action: Option<NoticeAction>,
    pub inline: Option<InlineAction>,
    /// The subtitle Show puts in place of `subtitle` ([`InlineAction::ShowAmount`]).
    pub reveal: Option<String>,
}

impl Notice {
    /// No time limit (a new version's): it stays until clicked or put away,
    /// and lets a volume change or a game's readout show over it meanwhile.
    pub fn stays(&self) -> bool {
        self.until.is_infinite()
    }
}

/// What the app knows about new versions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateView {
    /// A later release than this copy, with a download for this system.
    pub offer: Option<poni_core::update::Release>,
    /// Settings shows the Update sheet in place of its rows (until the
    /// island closes or the page changes).
    pub sheet: bool,
    /// Its Setup is downloaded and checked (Windows): the sheet offers
    /// Install now and Install on next start.
    pub ready: bool,
    /// Install on next start was picked for it.
    pub next_start: bool,
    /// This copy reads the update file itself (not a Microsoft Store copy):
    /// Settings has Check now.
    pub looks: bool,
    /// What Settings says in Check now's place since it was clicked, until
    /// the island closes or the page changes.
    pub said: Option<Said>,
}

/// Check now's answer in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    Checking,
    UpToDate,
    Failed,
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

/// What the phone panel shows on its left and right.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PhoneView {
    /// Today's same-Wi-Fi QR code.
    #[default]
    Lan,
    /// A pairing QR code (`services::pairdrop`): the link, its code and
    /// until when (app clock) it works.
    Pairing { url: String, qr: Option<QrMatrix>, until: f64 },
    /// The paired phones, each with Remove.
    Paired,
}

/// A paired phone as the panel lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedPhone {
    /// Its room on the meeting server (what Remove names).
    pub room: String,
    pub name: String,
    /// Unix seconds it last connected (0 = not yet).
    pub seen: i64,
}

/// Phone drop while the Shelf's phone panel is open: the link, its QR code
/// and what the phone is doing (`services::phonedrop` runs the server).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhoneDrop {
    pub view: PhoneView,
    /// A second line under `outcome` (None = the usual hint).
    pub outcome_line: Option<String>,
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
    /// The island stays open while a QR code waits for its first phone and
    /// while a phone waits for an answer. Once the panel has shown a result
    /// (a phone paired, files arrived), or on the list of paired phones,
    /// nothing waits: the island closes when the pointer leaves.
    pub fn holds_island(&self) -> bool {
        let waiting = !matches!(self.view, PhoneView::Paired) && self.outcome.is_none();
        self.ask.is_some() || (waiting && self.error.is_none() && self.visitor.is_none() && self.transfer.is_none())
    }
}

/// Transient per-page UI state (scroll offsets, text being typed, …).
#[derive(Clone, Debug, Default)]
pub struct PageUiState {
    pub task_input: String,
    pub task_scroll: f32,
    pub shelf_scroll: f32,
    /// The Today page's list: how far it's scrolled (DIPs), the date clicked
    /// in its grid (days since 1970-01-01), and when it was last drawn (mono
    /// time; a page drawn again after a while starts at today).
    pub today_scroll: f32,
    pub today_pick: Option<i64>,
    /// The next draw scrolls the list to `today_pick` (or back to today).
    pub today_jump: bool,
    pub today_drawn: f64,
    /// Smoothly animated lyric scroll position (line index as float).
    pub lyric_pos: f32,
    pub lyric_track: String,
    /// Volume HUD bar position, gliding toward the real level.
    pub hud_level: f32,
    /// Collapsed media pill shows the track title until this mono time.
    pub peek_until: f64,
    /// Header tab under the pointer and since when (for hover labels).
    pub tab_hover: Option<(usize, f64)>,
    /// Oyen in the Oyenza tab: his last move of his own and when it started
    /// (`oyen::tab`). None while the island is closed: the next opening is
    /// his arrival.
    pub oyen_tab: Option<(crate::oyen::TabMove, f64)>,
    /// The module the System page shows under its strip.
    pub system_module: poni_core::sysmon::Module,
    /// The last System reading had a GPU (keeps its chip while the next one is on its way).
    pub system_gpu: bool,
    /// The last full System reading had the whole PC's power (keeps its chip
    /// while the page's first, light reading shows).
    pub system_power: bool,
    /// The page the Settings button was pressed on (pressing it again goes back).
    pub before_settings: Option<PageKind>,
    /// Start with Windows, as last read off the UI thread (None until Settings first opens).
    pub autostart: Option<bool>,
    /// Oyen theme: the music notes floating up in the media pill.
    pub notes: crate::oyen::Notes,
    /// The Eyes look on a MacBook's notch: the pointer the eyes follow and
    /// the side of the camera they peek out from (`mac/notch.rs`).
    #[cfg(target_os = "macos")]
    pub peek: poni_core::peek::Peek,
    /// The Stocks page: its scroll, search, edit mode and chart animation.
    pub stocks: crate::pages::stocks::StocksUi,
    /// The AI tab's own state while it shows (`pages::usage`).
    pub usage: crate::pages::usage::UsageUi,
    /// The Notes page: when the last notes were saved (Oyen's tick), a short
    /// line beside them until a time ("Copied"), and whether a recording is
    /// kept on this PC for Try again.
    pub notes_saved_at: f64,
    pub notes_note: Option<(String, f64)>,
    pub notes_kept: bool,
}

/// Whether Windows lets Oyenza Studio read a game's frames. Its event tracing
/// is open to admins and to the Performance Log Users group; the one-time
/// OK adds the account to that group, which counts from the next sign-in.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum FpsAccess {
    /// Not asked yet.
    #[default]
    Unknown,
    /// Being checked.
    Checking,
    /// Frames can be read.
    Ready,
    /// The one-time OK is needed.
    NeedsOk,
    /// Windows' prompt for it is open.
    Asking,
    /// The account is in the group; Windows applies it at the next sign-in.
    SignOut,
    /// The OK didn't go through (why, in a few words).
    Failed(String),
}

/// Show over games: the readout in the island over a full-screen app
/// (`activities::Activity::Game`) and the System page's Games view.
#[derive(Clone, Debug, Default)]
pub struct GameState {
    /// The readout is on screen: Show over games is on and a full-screen
    /// app is in front.
    pub active: bool,
    /// The full-screen app ("cs2.exe").
    pub app: Option<String>,
    /// Its frame rate, while it draws and the frames can be read.
    pub rate: Option<poni_core::frames::FrameRate>,
    /// Frames per second over the last minute in front.
    pub history: poni_core::sysmon::Series,
    pub access: FpsAccess,
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

/// Where the Today page's day stands (`OyenzaState::agenda`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgendaState {
    /// Not asked yet since signing in or opening.
    #[default]
    Waiting,
    /// `agenda` holds the latest answer.
    Ready,
    /// The last ask failed; `agenda` may still hold an older answer.
    Failed,
    /// The server has no day for Today (from before it, or switched off there).
    Unavailable,
}

/// What the Oyenza tab and its notices show (`pages::oyenza`), and the Today
/// page's day (`pages::today`). Oyen's moods never read any of it: money
/// never makes him happier or a bill sadder.
#[derive(Clone, Debug, Default)]
pub struct OyenzaState {
    pub link: OyenzaLink,
    /// The last 7 days of transactions (asked while the page is open).
    pub activity: Option<Activity>,
    /// When each row was first seen, so what lands while you look glows.
    pub seen: Seen,
    /// The day the page shows (Today when it opens).
    pub day: DayPick,
    /// How far that day's list is scrolled (DIPs).
    pub scroll: f32,
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
    /// The Today page's day (`poni_core::agenda`), kept in memory only, and
    /// the unix time it came.
    pub agenda: Option<poni_core::agenda::Answer>,
    pub agenda_unix: Option<i64>,
    pub agenda_state: AgendaState,
    /// Signing in started from the Today page: back to it once connected.
    pub from_today: bool,
    /// Whether the account has Studio Pro, as the server last said (None:
    /// not asked yet). Signing out forgets it with the rest.
    pub pro: Option<bool>,
}


/// Where finding the PC's place stands.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Locate {
    /// Not asked (following the location is off, or not yet).
    #[default]
    Idle,
    Asking,
    /// Picked the nearest place, this far from the PC (km).
    Found { km: f64 },
    /// The nearest place in either list is too far away to use (km).
    Far { km: f64 },
    /// Away from Kemenag's places, and the cities around couldn't be looked
    /// up (offline): the place stays as it was.
    Offline,
    /// Windows' location is off, or not allowed for Oyenza Studio.
    Denied,
    /// Windows couldn't tell where the PC is.
    Unknown,
}

/// One of Prayer times' online city files, while it's wanted.
#[derive(Clone, Debug)]
pub enum CityFile {
    Asking,
    Here(std::sync::Arc<World>),
    /// Offline, or the website didn't answer.
    Failed,
}

/// The Stocks add-on's prices, kept in memory only (the list itself is
/// `store.stocks`). Filled by `services::stocks` while the page is open.
#[derive(Clone, Debug, Default)]
pub struct StocksState {
    /// The last answer, and the Unix time it came or was last confirmed unchanged.
    pub board: Option<poni_core::stocks::Board>,
    pub checked: Option<i64>,
    /// Why the last ask failed, until one goes through.
    pub trouble: Option<crate::services::stocks::Trouble>,
    /// Every listed stock's name, once the search has asked for them.
    pub names: Option<std::sync::Arc<poni_core::stocks::Names>>,
    pub names_failed: bool,
    /// Years of daily closes read this run, by code (the IHSG's too).
    pub years: HashMap<String, poni_core::stocks::History>,
    /// Codes whose year couldn't be read.
    pub no_year: Vec<String>,
}

impl StocksState {
    pub fn apply(&mut self, ev: crate::services::stocks::StocksEvent, unix: i64) {
        use crate::services::stocks::StocksEvent;
        match ev {
            StocksEvent::Board(b) => {
                self.board = Some(b);
                self.checked = Some(unix);
                self.trouble = None;
            }
            StocksEvent::Same => {
                self.checked = Some(unix);
                self.trouble = None;
            }
            StocksEvent::Failed(why) => self.trouble = Some(why),
            StocksEvent::Names(n) => {
                self.names = Some(n);
                self.names_failed = false;
            }
            StocksEvent::NoNames => self.names_failed = true,
            StocksEvent::History(h) => {
                self.no_year.retain(|c| *c != h.code);
                self.years.insert(h.code.clone(), h);
            }
            StocksEvent::NoHistory(code) => {
                if !self.no_year.contains(&code) {
                    self.no_year.push(code);
                }
            }
        }
    }
}

/// The Prayer times add-on's live state.
#[derive(Clone, Debug, Default)]
pub struct PrayerState {
    /// The add-on's place list, read once its files are in place.
    pub places: Option<std::sync::Arc<PlaceList>>,
    /// The online city files asked for (`poni_core::prayer::world`): the
    /// picker's, dropped when it closes, and a location's squares, dropped
    /// once its place is found.
    pub cities: HashMap<String, CityFile>,
    /// A location waiting for its squares: the point (never saved) and the
    /// files it needs.
    pub fix: Option<(f64, f64, Vec<String>)>,
    /// The last location that went online and the place it found: the next
    /// one within a kilometre takes it again without asking.
    pub last_fix: Option<(f64, f64, Option<(Place, f64)>)>,
    /// The picker's last search: what was typed, and what it found.
    pub found: Option<(String, Vec<Place>)>,
    /// The place the times are for: the chosen one, or the time zone's
    /// suggestion while none is chosen.
    pub place: Option<Place>,
    /// `place` is the time zone's suggestion.
    pub suggested: bool,
    pub locate: Locate,
    /// When the PC's location was last asked (UTC seconds).
    pub located_at: Option<i64>,
    /// The place picker shows (Change), and what's typed in it.
    pub picking: bool,
    pub query: String,
    /// Prayer times up to this moment (UTC seconds) have had their moment.
    pub handled_until: Option<i64>,
    /// The adzan is playing.
    pub adzan: bool,
    /// OK on the countdown put it away until this prayer's time (UTC
    /// seconds): the closed island shows what it would otherwise (the song
    /// and its sung line) until then. Kept in memory only.
    pub countdown_ok: Option<i64>,
    /// Self-test only: moves the prayer clock by this many seconds.
    pub clock_shift: i64,
}

impl PrayerState {
    pub fn site(&self) -> Option<Site> {
        self.place.as_ref().and_then(|p| p.site())
    }
}

/// What Oyenza Studio Recorder last said about its recording
/// (`poni_core::recorder::Message::State`).
#[derive(Clone, Debug, Default)]
pub struct RecView {
    /// None: no recording heard of (none yet, or the last one ended).
    pub phase: Option<RecPhase>,
    /// Its length when the message came (paused time left out)...
    pub elapsed_ms: u64,
    /// ...and when that was (mono seconds), to count on from while it records.
    pub at: f64,
    /// The recorder's window, to notice a recorder gone mid-recording (0: none).
    pub sender: isize,
    /// Record was pressed and the recorder hasn't answered yet: the page says
    /// "Opening the recorder" until this time (mono seconds).
    pub opening_until: f64,
    /// Why the last start failed, shown on the Record page until a time.
    pub error: Option<(String, f64)>,
}

/// The AI tab's readings, kept in memory only (its choices are
/// `settings.usage`). Filled by `services::usage` while the tab is open, or
/// every few minutes for the person who switched on its heads-up notices.
#[derive(Clone, Debug, Default)]
pub struct UsageState {
    /// Each service's last reading.
    pub readings: BTreeMap<ProviderId, Reading>,
    /// inference.oyenza.com's balance and spend, once connected.
    pub inference: InferenceLink,
    /// The secret names (`usage::KeyAsk::secret`) that hold a typed key.
    pub keys: BTreeSet<&'static str>,
    /// A refresh is under way.
    pub refreshing: bool,
    /// Heads-up notices waiting for the island, one per service, most
    /// urgent first, each with when it came (mono seconds):
    /// [`AppState::hold_usage_alerts`].
    pub waiting: Vec<(Alert, f64)>,
}

/// One service's last reading on the AI tab.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// The last good one, kept through troubles.
    pub snapshot: Option<Snapshot>,
    /// The latest refresh's trouble; None when it worked.
    pub trouble: Option<Trouble>,
    /// When the latest refresh ended, unix seconds.
    pub checked_at: i64,
}

/// inference.oyenza.com on the AI tab: connected by a code the person
/// approves in its dashboard, after which Studio holds a read-only key.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum InferenceLink {
    #[default]
    Off,
    /// Asking for a code.
    Starting,
    /// A code on screen, waiting for approval at `url`, until unix `expires_at`.
    Code { code: String, url: String, expires_at: i64 },
    /// Connected: its balance and spend are read.
    On { account: String },
    /// Connecting didn't work (why, in a few words).
    #[allow(dead_code)] // until the AI tab's worker lands: it says why connecting stopped
    Failed(String),
}

impl UsageState {
    /// What the worker found. Its heads-up notices are the app's to show. A
    /// trouble keeps the last numbers (offline, busy, expired, refused, not
    /// read, the keychain's no) unless the person took the way in away: the
    /// app gone, signed out, the key removed, the account switched off.
    pub fn apply(&mut self, ev: crate::services::usage::UsageEvent) {
        use crate::services::usage::UsageEvent;
        match ev {
            UsageEvent::Reading { id, outcome, at } => {
                let r = self.readings.entry(id).or_insert(Reading { snapshot: None, trouble: None, checked_at: at });
                match outcome {
                    Ok(snap) => {
                        r.snapshot = Some(snap);
                        r.trouble = None;
                    }
                    Err(t) => {
                        if matches!(t, Trouble::NotFound | Trouble::SignedOut | Trouble::NeedsKey | Trouble::Disabled) {
                            r.snapshot = None;
                        }
                        r.trouble = Some(t);
                    }
                }
                r.checked_at = at;
            }
            UsageEvent::Keys(keys) => self.keys = keys,
            UsageEvent::Link(link) => self.inference = link,
            UsageEvent::Refreshing(on) => self.refreshing = on,
            UsageEvent::Alerts(_) => {}
        }
    }
}

/// A heads-up notice from the AI tab: the service's mark and the alert's
/// words; a click opens the AI tab. Quiet, as money notices are. Running out
/// and a low balance are orange, a limit back is green, one getting close
/// takes the accent.
pub fn usage_notice(a: &Alert, accent: Color, until: f64) -> Notice {
    let color = match a.kind {
        AlertKind::Near(_) => accent,
        AlertKind::Back => palette::GREEN,
        AlertKind::Out | AlertKind::Low => palette::ORANGE,
    };
    Notice {
        icon: crate::icons::service(a.provider),
        color,
        title: a.title.clone(),
        title_dim: String::new(),
        subtitle: usage_line(&a.subtitle, a.nudge.as_deref()),
        until,
        action: Some(NoticeAction::OpenPage(PageKind::Ai)),
        inline: None,
        reveal: None,
    }
}

/// How wide a notice's second line may be, by the Pill's guess: Classic's
/// room for its words (372 - 58 - 22 = 292 DIP; Pill's is wider) less a
/// little for system fonts wider than the guess.
const NOTICE_LINE_W: f32 = 280.0;

/// A notice has two lines, so a plan that ran out while Oyenza Inference is
/// connected puts the nudge after the reset time ("Resets 14:20 · Keep going
/// on Oyenza Inference") while both fit, else the nudge alone: the AI tab,
/// a click away, has the reset time.
fn usage_line(subtitle: &str, nudge: Option<&str>) -> String {
    match nudge.map(str::trim).filter(|n| !n.is_empty()) {
        None => subtitle.to_string(),
        Some(n) if subtitle.trim().is_empty() => n.to_string(),
        Some(n) => {
            let both = format!("{} \u{b7} {n}", subtitle.trim());
            if crate::pill::notice::guess(&both, crate::gfx::ts::FOOTNOTE) <= NOTICE_LINE_W {
                both
            } else {
                n.to_string()
            }
        }
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
    pub stats: SysStats,
    /// The System page's readings and their last minute (empty while it's closed).
    pub system: poni_core::sysmon::Monitor,
    /// Show over games.
    pub game: GameState,
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
    /// Stepped aside (Settings › Over maximized windows): an app window sits
    /// under where the island rests, so the closed island rests as an
    /// invisible strip at the top edge (`activities::Mode::Aside`). Windows
    /// sets it; the Mac never does.
    pub aside: bool,
    /// Files are being dragged over the island right now.
    pub drag_hover: bool,
    /// Phone drop, while it's open.
    pub phone: Option<PhoneDrop>,
    /// Paired phones, newest first (`services::pairdrop`).
    pub paired: Vec<PairedPhone>,
    /// Whether the meeting server answers (None = not asked yet: no pairs).
    pub pair_server: Option<bool>,
    pub oyenza: OyenzaState,
    /// Where each add-on stands, by id (every one in `poni_core::addons::ALL`).
    pub addons: BTreeMap<&'static str, AddonStatus>,
    /// New versions (`crate::updates`): what Settings, the menu and the
    /// island's notice show.
    pub update: UpdateView,
    /// The Prayer times add-on, while it is on (its choices: `settings.prayer`).
    pub prayer: PrayerState,
    /// The Script page's reading (its script and pace are in `store.script`).
    pub script: poni_core::script::Reader,
    /// A message under the script until a time (mono seconds).
    pub script_note: Option<(&'static str, f64)>,
    /// Whether Windows leaves the island out of screen capture while the
    /// Script page shows (None = not asked yet, or not on Windows).
    pub script_hidden: Option<bool>,
    /// Oyenza Studio Recorder is installed: the Script page shows Record, and
    /// the Record tab can show (`record_on`). Asked at start, as the island
    /// opens and when the recorder writes, never at rest.
    pub recorder: bool,
    /// The Stocks add-on's prices (its list: `store.stocks`).
    pub stocks: StocksState,
    /// The recorder's recording, as it last told the island.
    pub rec: RecView,
    /// Whether Windows lets the Notifications add-on read notifications.
    pub notify_access: NotifyAccess,
    /// The AI tab's readings (its choices: `settings.usage`).
    pub usage: UsageState,
    /// The IELTS page's session with Oyen (the IELTS add-on).
    pub ielts: poni_core::ielts::Coach,
    /// The Notes page's meeting (the Meeting notes add-on).
    pub notes: poni_core::notes::Taker,
    /// Side effects requested during the frame.
    pub cmds: Vec<Command>,
}

impl AppState {
    /// The Shelf's phone panel keeps the open island from closing, only
    /// while the Shelf is the page on screen.
    pub fn phone_holds_island(&self) -> bool {
        self.page == PageKind::Shelf && self.phone.as_ref().is_some_and(|p| p.holds_island())
    }

    /// A reading under way on the Script page keeps the open island from
    /// closing, while that page is on screen.
    pub fn script_holds_island(&self) -> bool {
        self.page == PageKind::Script && self.script.active
    }

    /// An IELTS session under way keeps the open island from closing (a
    /// question, an answer being recorded or on its way), while that page is
    /// on screen.
    pub fn ielts_holds_island(&self) -> bool {
        self.page == PageKind::Ielts && self.ielts.holds()
    }

    /// Something on the page on screen keeps the open island from closing.
    pub fn page_holds_island(&self) -> bool {
        self.phone_holds_island() || self.script_holds_island() || self.ielts_holds_island()
    }

    pub fn new(settings: Settings, store: Store) -> Self {
        let pomodoro = Pomodoro::new(settings.pomodoro.clone());
        let page = PageKind::from_key(if settings.remember_page && !store.last_page.is_empty() {
            &store.last_page
        } else {
            &settings.start_page
        });
        let mut st = Self {
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
            stats: SysStats::default(),
            system: poni_core::sysmon::Monitor::default(),
            game: GameState::default(),
            shelf_thumbs: HashMap::new(),
            app_icons: HashMap::new(),
            ui: PageUiState::default(),
            accent: palette::BLUE,
            health: None,
            page,
            media_paused_at: None,
            pinned: false,
            aside: false,
            drag_hover: false,
            phone: None,
            paired: Vec::new(),
            pair_server: None,
            oyenza: OyenzaState::default(),
            addons: BTreeMap::new(),
            update: UpdateView::default(),
            prayer: PrayerState::default(),
            script: poni_core::script::Reader::default(),
            script_note: None,
            script_hidden: None,
            recorder: false,
            stocks: StocksState::default(),
            rec: RecView::default(),
            notify_access: NotifyAccess::Unknown,
            usage: UsageState::default(),
            ielts: Default::default(),
            notes: Default::default(),
            cmds: Vec::new(),
        };
        // A list from a hand-edited file keeps to codes, each once, ten at most.
        st.store.stocks.tidy();
        // On ones start by checking their files (the worker reports back);
        // one without files is ready at once.
        for a in poni_core::addons::ALL {
            let on = st.settings.addon_on(a.id);
            st.addons.insert(a.id, if !on { AddonStatus::Off } else if a.downloads() { AddonStatus::Checking } else { AddonStatus::Ready });
        }
        // Last open on the Script, System, Health, Stocks, IELTS or Notes
        // page, since switched off, or on a tab since hidden: open on the
        // first tab.
        let off = match st.page {
            PageKind::Script => !st.script_on(),
            PageKind::System => !st.system_on(),
            PageKind::Health => !st.health_on(),
            PageKind::Stocks => !st.stocks_on(),
            PageKind::Record => !st.record_on(),
            PageKind::Notifications => !st.notifications_on(),
            PageKind::Ielts => !st.ielts_on(),
            PageKind::Notes => !st.notes_on(),
            p => st.tab_hidden(p),
        };
        if off {
            st.page = crate::pages::first_tab(&st);
        }
        st
    }

    /// Where the add-on `id` stands (`Off` for one this version doesn't know).
    pub fn addon(&self, id: &str) -> AddonStatus {
        self.addons.get(id).copied().unwrap_or(AddonStatus::Off)
    }

    /// Screen recorder is on (`Settings::recorder_on`): the Record tab shows
    /// and the island answers the recorder, which then shows no bar or
    /// controls of its own (`WM_COPYDATA` on Windows, a message port on the Mac).
    pub fn record_on(&self) -> bool {
        self.settings.recorder_on(self.recorder_ready())
    }

    /// The recorder is installed and has written to the island before
    /// (`Store::recorder_island`): it understands the Record page.
    pub fn recorder_ready(&self) -> bool {
        self.recorder && self.store.recorder_island
    }

    /// A recording is under way: starting, recording, paused or being saved.
    pub fn rec_live(&self) -> bool {
        self.rec.phase.is_some_and(|p| p != RecPhase::Idle)
    }

    /// The recording's length now, in seconds (paused time left out).
    pub fn rec_elapsed(&self) -> f64 {
        let at_message = self.rec.elapsed_ms as f64 / 1000.0;
        if self.rec.phase == Some(RecPhase::Recording) {
            at_message + (self.now - self.rec.at).max(0.0)
        } else {
            at_message
        }
    }

    /// Seconds until the recording's time shows its next second, while it runs.
    pub fn rec_next_second(&self) -> Option<f64> {
        (self.rec.phase == Some(RecPhase::Recording)).then(|| 1.0 - self.rec_elapsed().fract())
    }

    /// The Script reader add-on is on: the Script tab shows.
    pub fn script_on(&self) -> bool {
        self.addon(poni_core::addons::SCRIPT_READER.id) == AddonStatus::Ready
    }

    /// The System add-on is on: the System tab shows.
    pub fn system_on(&self) -> bool {
        self.addon(poni_core::addons::SYSTEM.id) == AddonStatus::Ready
    }

    /// The Health add-on is on: the Health tab shows and the steps are asked for.
    pub fn health_on(&self) -> bool {
        self.addon(poni_core::addons::HEALTH.id) == AddonStatus::Ready
    }

    /// The Stocks add-on is on: the Stocks tab shows (its prices are asked
    /// for only while the page is on screen).
    pub fn stocks_on(&self) -> bool {
        self.addon(poni_core::addons::STOCKS.id) == AddonStatus::Ready
    }

    /// The Notifications add-on is on (Windows): its tab shows, and new
    /// notifications from the apps picked there show on the island.
    pub fn notifications_on(&self) -> bool {
        cfg!(windows) && self.addon(poni_core::addons::NOTIFICATIONS.id) == AddonStatus::Ready
    }

    /// Settings took this page's tab off the header's row (Music, Today,
    /// Shelf, Tasks or Focus).
    pub fn tab_hidden(&self, p: PageKind) -> bool {
        p.hideable() && self.settings.hidden_tabs.iter().any(|k| k == p.key())
    }

    /// The IELTS add-on is on: the IELTS tab shows.
    pub fn ielts_on(&self) -> bool {
        self.addon(poni_core::addons::IELTS.id) == AddonStatus::Ready
    }

    /// The signed-in account has Studio Pro, as the server last said.
    pub fn studio_pro(&self) -> Option<bool> {
        match self.oyenza.link {
            OyenzaLink::Connected { .. } => self.oyenza.pro,
            _ => Some(false),
        }
    }

    /// The Meeting notes add-on is on (Windows): the Notes tab shows.
    pub fn notes_on(&self) -> bool {
        cfg!(windows) && self.addon(poni_core::addons::MEETING_NOTES.id) == AddonStatus::Ready
    }

    /// Prayer times is on and its places are read: the tab and reminders run.
    pub fn prayer_on(&self) -> bool {
        self.addon(poni_core::addons::PRAYER_TIMES.id) == AddonStatus::Ready && self.prayer.places.is_some()
    }

    /// The prayer clock, UTC seconds (the self-test moves it).
    pub fn prayer_now(&self) -> i64 {
        self.unix + self.prayer.clock_shift
    }

    /// OK on the countdown before a prayer: it goes away until that prayer's
    /// time (`activities::prayer_countdown`). The adzan still comes then,
    /// since what rings follows the prayer clock, not the countdown. Returns
    /// the prayer put away (none when no countdown runs).
    pub fn prayer_countdown_ok(&mut self) -> Option<PrayerMoment> {
        let site = self.prayer.site()?;
        let (m, _) = poni_core::prayer::countdown(&site, self.prayer_now(), self.settings.prayer.remind_before_min)?;
        self.prayer.countdown_ok = Some(m.at);
        Some(m)
    }

    /// New steps from the phone (`None`: none to show), kept in memory only,
    /// never in data.json. Returns today's steps when they just passed the
    /// goal; the first numbers after start or sign-in only set the baseline
    /// (`poni_core::health::goal_notice`).
    pub fn take_health(&mut self, next: Option<HealthSnapshot>) -> Option<u32> {
        let today = self.local.day_key();
        let reached = next.as_ref().filter(|n| poni_core::health::goal_notice(self.health.as_ref(), n, &today)).map(|n| n.steps_on(&today));
        self.health = next;
        reached
    }

    /// Show `n` now or, while a full-screen app hides the island (`hidden`), keep
    /// it for when the island is back, the way money notices wait
    /// (`OyenzaState::held`, shown then if no older than `HELD_MAX_S`). One
    /// notice waits at a time, and a money or bill notice already waiting stays.
    pub fn show_or_hold(&mut self, n: Notice, hidden: bool) {
        if !hidden {
            self.notice = Some(n);
        } else if self.oyenza.held.as_ref().is_none_or(|(_, at)| self.now - at > poni_core::oyenza::HELD_MAX_S) {
            self.oyenza.held = Some((n, self.now));
        }
    }

    /// The AI tab's heads-up notices (its worker sends them only while they
    /// are switched on) wait for the island: one per service, its newest,
    /// most urgent first. [`AppState::show_usage_notice`] puts them up one at
    /// a time, the way a new version's notice waits for the island to be free.
    pub fn hold_usage_alerts(&mut self, alerts: Vec<Alert>) {
        if !self.settings.usage.notices {
            return;
        }
        let waiting = &mut self.usage.waiting;
        for a in alerts {
            waiting.retain(|(w, _)| w.provider != a.provider);
            waiting.push((a, self.now));
        }
        // Stable: the same urgency keeps the order they came in.
        studio_usage::sort_small_by(waiting, |(a, _), (b, _)| a.kind.urgency() < b.kind.urgency());
    }

    /// Put the next waiting AI notice up once the island is `free` (the app
    /// says: someone at the computer, nothing hides the island, it's closed)
    /// and no other notice shows. One older than `HELD_MAX_S` or for a service
    /// hidden since is dropped, and all of them once the notices are switched
    /// off or the AI tab is on screen (`ai_open`), which says it all. True
    /// when one went up. Called every frame: nothing waiting, it's one
    /// comparison.
    pub fn show_usage_notice(&mut self, free: bool, ai_open: bool) -> bool {
        if self.usage.waiting.is_empty() {
            return false;
        }
        let (u, now) = (&self.settings.usage, self.now);
        if !u.notices || ai_open {
            self.usage.waiting.clear();
            return false;
        }
        self.usage.waiting.retain(|(a, at)| now - at <= poni_core::oyenza::HELD_MAX_S && !u.hidden.iter().any(|k| k == a.provider.key()));
        if !free || self.notice.is_some() || self.usage.waiting.is_empty() {
            return false;
        }
        let (a, _) = self.usage.waiting.remove(0);
        self.notice = Some(usage_notice(&a, self.accent, now + poni_core::oyenza::NOTICE_S));
        true
    }

    /// The line the person picked for "getting close", in percent, kept to
    /// 50..=95: a hand-edited 0 would say so at every reading, 100 only once
    /// it is all used.
    pub fn usage_near_pct(&self) -> u8 {
        self.settings.usage.near_pct.clamp(50, 95)
    }

    /// What the AI tab's notices go by, for its worker (`alerts::changes`):
    /// that line, and the app's clock for the reset times ("Resets 14:20" or
    /// "2:20 PM", local). A plan that runs out mentions Oyenza Inference only
    /// while it is connected and its key works.
    pub fn usage_alert_cfg(&self) -> AlertCfg {
        let refused = self.usage.readings.get(&ProviderId::Oyenza).is_some_and(|r| matches!(r.trouble, Some(Trouble::Refused | Trouble::Disabled)));
        AlertCfg {
            near: f32::from(self.usage_near_pct()) / 100.0,
            offset_min: poni_core::oyenza::offset_minutes(&self.local, self.unix),
            use_24h: self.settings.use_24h,
            inference: matches!(self.usage.inference, InferenceLink::On { .. }) && !refused,
            ..AlertCfg::default()
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
        self.notice = Some(Notice { icon, color, title: title.into(), title_dim: String::new(), subtitle: subtitle.into(), until: self.now + secs, action, inline: None, reveal: None });
    }

    /// The Update sheet: Settings, with the sheet in place of its rows (the
    /// gear goes back to the page it was opened from).
    pub fn open_update(&mut self) {
        if self.page != PageKind::Settings {
            self.ui.before_settings = Some(self.page);
        }
        self.page = PageKind::Settings;
        self.update.sheet = true;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_panel_holds_only_while_a_code_waits_on_the_shelf() {
        let mut st = AppState::new(Settings::default(), Store::default());
        st.phone = Some(PhoneDrop::default());
        st.page = PageKind::Shelf;
        assert!(st.phone_holds_island(), "a fresh QR code waits for a phone");
        st.page = PageKind::Media;
        assert!(!st.phone_holds_island(), "another page: nothing to scan on screen");
        st.page = PageKind::Shelf;
        // A phone finished pairing: the panel is back on its Wi-Fi code with a result.
        st.phone.as_mut().unwrap().outcome = Some(("iPhone is paired".into(), true, 8.0));
        assert!(!st.phone_holds_island());
        let pd = st.phone.as_mut().unwrap();
        pd.outcome = None;
        pd.view = PhoneView::Paired;
        assert!(!st.phone_holds_island(), "the paired list has no code to scan");
        let pd = st.phone.as_mut().unwrap();
        pd.view = PhoneView::Lan;
        pd.visitor = Some("iPhone".into());
        pd.ask = Some(PhoneAsk { id: 1, from: "iPhone".into(), files: vec![], total: 0 });
        assert!(st.phone_holds_island(), "a phone waits for an answer");
    }

    #[test]
    fn the_goal_notice_is_only_for_steps_seen_passing_it() {
        use poni_core::health::{HealthDay, HealthSource};
        let mut st = AppState::new(Settings::default(), Store::default());
        st.local = LocalDateTime { year: 2026, month: 10, day: 24, hour: 15, minute: 20, second: 0 };
        let snap = |steps: u32| HealthSnapshot {
            goal: 8000,
            source: HealthSource::AppleHealth,
            updated_at: 0,
            days: vec![HealthDay { date: "2026-10-24".into(), steps, distance_m: None, active_kcal: None }],
        };
        // Over the goal in the first numbers after start: a baseline, no notice (a restart never repeats it).
        assert_eq!(st.take_health(Some(snap(9000))), None);
        assert_eq!(st.health.as_ref().map(|h| h.steps_on("2026-10-24")), Some(9000));
        // Off in Oyenza: nothing kept; the numbers after it are a baseline again.
        assert_eq!(st.take_health(None), None);
        assert!(st.health.is_none());
        assert_eq!(st.take_health(Some(snap(7900))), None);
        assert_eq!(st.take_health(Some(snap(8010))), Some(8010));
        assert_eq!(st.take_health(Some(snap(8500))), None);
    }

    #[test]
    fn the_goal_notice_waits_while_a_full_screen_app_hides_the_island() {
        let mut st = AppState::new(Settings::default(), Store::default());
        st.now = 100.0;
        let notice = |title: &str| Notice {
            icon: Icon::Steps,
            color: palette::GREEN,
            title: title.into(),
            title_dim: String::new(),
            subtitle: String::new(),
            until: 0.0,
            action: None,
            inline: None,
            reveal: None,
        };
        let held = |st: &AppState| st.oyenza.held.as_ref().map(|(n, at)| (n.title.clone(), *at));
        // Hidden: it waits for the island instead of expiring unseen.
        st.show_or_hold(notice("goal"), true);
        assert!(st.notice.is_none());
        assert_eq!(held(&st), Some(("goal".into(), 100.0)));
        // A money notice already waiting stays...
        st.oyenza.held = Some((notice("money"), 100.0));
        st.show_or_hold(notice("goal"), true);
        assert_eq!(held(&st), Some(("money".into(), 100.0)));
        // ...unless it's too old to show any more.
        st.now = 100.0 + poni_core::oyenza::HELD_MAX_S + 1.0;
        st.show_or_hold(notice("goal"), true);
        assert_eq!(held(&st).map(|h| h.0), Some("goal".into()));
        // The island on screen: it shows at once.
        st.show_or_hold(notice("now"), false);
        assert_eq!(st.notice.as_ref().map(|n| n.title.as_str()), Some("now"));
    }

    #[test]
    fn an_ai_reading_keeps_its_last_numbers_through_a_trouble() {
        use crate::services::usage::UsageEvent;
        use studio_usage::Source;
        let mut u = UsageState::default();
        let snap = Snapshot::new(ProviderId::Codex, 100, Source::Online);
        u.apply(UsageEvent::Reading { id: ProviderId::Codex, outcome: Ok(snap.clone()), at: 100 });
        u.apply(UsageEvent::Reading { id: ProviderId::Codex, outcome: Err(Trouble::Offline), at: 220 });
        let r = &u.readings[&ProviderId::Codex];
        assert_eq!((r.snapshot.as_ref(), r.trouble.as_ref(), r.checked_at), (Some(&snap), Some(&Trouble::Offline), 220));
        // The next good one clears the trouble.
        u.apply(UsageEvent::Reading { id: ProviderId::Codex, outcome: Ok(snap.clone()), at: 340 });
        assert_eq!(u.readings[&ProviderId::Codex].trouble, None);
        u.apply(UsageEvent::Link(InferenceLink::On { account: "a•••@example.com".into() }));
        assert_eq!(u.inference, InferenceLink::On { account: "a•••@example.com".into() });
        // Troubles that pass keep the numbers; the way in taken away drops them.
        let keeps = [
            Trouble::Offline,
            Trouble::Busy { retry_after_s: Some(30) },
            Trouble::Expired,
            Trouble::Refused,
            Trouble::Unreadable("HTTP 404".into()),
            Trouble::Denied,
            Trouble::NeedsOnline,
        ];
        for t in keeps {
            u.apply(UsageEvent::Reading { id: ProviderId::Codex, outcome: Ok(snap.clone()), at: 400 });
            u.apply(UsageEvent::Reading { id: ProviderId::Codex, outcome: Err(t.clone()), at: 460 });
            assert_eq!(u.readings[&ProviderId::Codex].snapshot.as_ref(), Some(&snap), "{t:?}");
            assert_eq!(u.readings[&ProviderId::Codex].trouble.as_ref(), Some(&t));
        }
        for t in [Trouble::NotFound, Trouble::SignedOut, Trouble::NeedsKey, Trouble::Disabled] {
            u.apply(UsageEvent::Reading { id: ProviderId::Codex, outcome: Ok(snap.clone()), at: 400 });
            u.apply(UsageEvent::Reading { id: ProviderId::Codex, outcome: Err(t.clone()), at: 460 });
            assert_eq!(u.readings[&ProviderId::Codex].snapshot, None, "{t:?}");
            assert_eq!(u.readings[&ProviderId::Codex].trouble.as_ref(), Some(&t));
        }
    }

    fn alert(provider: ProviderId, kind: AlertKind, title: &str) -> Alert {
        Alert { provider, kind, title: title.into(), subtitle: "Resets 14:20".into(), nudge: None }
    }

    /// The titles of the AI notices as they go up, the island free throughout.
    fn shown(st: &mut AppState) -> Vec<String> {
        let mut titles = Vec::new();
        while st.show_usage_notice(true, false) {
            titles.extend(st.notice.take().map(|n| n.title));
        }
        titles
    }

    #[test]
    fn ai_notices_wait_their_turn_one_per_service_most_urgent_first() {
        use AlertKind::*;
        let mut st = AppState::new(Settings::default(), Store::default());
        st.settings.usage.notices = true;
        st.now = 100.0;
        st.hold_usage_alerts(vec![alert(ProviderId::Claude, Near(90), "Claude Code: 90% of 5-hour"), alert(ProviderId::Gemini, Back, "Gemini CLI is back")]);
        // A later word for a service takes its place; the same urgency keeps the order they came in.
        st.now = 110.0;
        st.hold_usage_alerts(vec![alert(ProviderId::Codex, Out, "Codex: 5-hour used up"), alert(ProviderId::Claude, Out, "Claude Code: 5-hour used up")]);
        st.hold_usage_alerts(vec![alert(ProviderId::OpenRouter, Low, "OpenRouter: $0.80 left")]);
        assert_eq!(shown(&mut st), ["Codex: 5-hour used up", "Claude Code: 5-hour used up", "OpenRouter: $0.80 left", "Gemini CLI is back"]);
        assert!(st.usage.waiting.is_empty());
    }

    #[test]
    fn ai_notices_wait_for_a_free_island_and_drop_what_went_stale() {
        let mut st = AppState::new(Settings::default(), Store::default());
        st.now = 100.0;
        let codex = || alert(ProviderId::Codex, AlertKind::Near(80), "Codex: 80% of 5-hour");
        // Notices off: none is kept.
        st.hold_usage_alerts(vec![codex()]);
        assert!(st.usage.waiting.is_empty());
        st.settings.usage.notices = true;
        st.hold_usage_alerts(vec![codex()]);
        // Nobody at the computer, a full-screen app, the island open: it waits.
        assert!(!st.show_usage_notice(false, false));
        assert!(st.notice.is_none());
        // ...and for another notice to leave.
        st.notify(Icon::Check, palette::GREEN, "Connected to Oyenza", "as a•••@example.com", 6.0, None);
        assert!(!st.show_usage_notice(true, false));
        assert_eq!(st.notice.as_ref().map(|n| n.title.as_str()), Some("Connected to Oyenza"));
        st.notice = None;
        st.now = 130.0;
        assert!(st.show_usage_notice(true, false));
        let n = st.notice.take().unwrap();
        assert_eq!((n.title.as_str(), n.subtitle.as_str(), n.until), ("Codex: 80% of 5-hour", "Resets 14:20", 130.0 + poni_core::oyenza::NOTICE_S));
        assert_eq!((n.action, n.inline), (Some(NoticeAction::OpenPage(PageKind::Ai)), None));
        // Too old once the island is free: dropped.
        st.hold_usage_alerts(vec![codex()]);
        st.now += poni_core::oyenza::HELD_MAX_S + 1.0;
        assert!(shown(&mut st).is_empty());
        assert!(st.usage.waiting.is_empty());
        // A service hidden since: dropped.
        st.hold_usage_alerts(vec![alert(ProviderId::Claude, AlertKind::Out, "Claude Code: 5-hour used up"), codex()]);
        st.settings.usage.hidden.push("claude".into());
        assert_eq!(shown(&mut st), ["Codex: 80% of 5-hour"]);
        // The AI tab on screen (a notice's click opened it): it says it all.
        st.hold_usage_alerts(vec![alert(ProviderId::Gemini, AlertKind::Back, "Gemini CLI is back")]);
        assert!(!st.show_usage_notice(false, true));
        assert!(st.usage.waiting.is_empty());
        // Notices switched off: all of them.
        st.hold_usage_alerts(vec![alert(ProviderId::Gemini, AlertKind::Back, "Gemini CLI is back")]);
        st.settings.usage.notices = false;
        assert!(!st.show_usage_notice(true, false));
        assert!(st.usage.waiting.is_empty());
    }

    #[test]
    fn an_ai_notice_says_what_ran_out_and_where_to_keep_going() {
        let accent = palette::PURPLE;
        // The service's own mark.
        assert_eq!(usage_notice(&alert(ProviderId::Claude, AlertKind::Out, "t"), accent, 0.0).icon, Icon::Claude);
        let color = |kind| usage_notice(&alert(ProviderId::Claude, kind, "t"), accent, 0.0).color;
        assert_eq!(
            [color(AlertKind::Near(90)), color(AlertKind::Out), color(AlertKind::Low), color(AlertKind::Back)],
            [accent, palette::ORANGE, palette::ORANGE, palette::GREEN]
        );
        // Out with Oyenza Inference connected: the nudge follows the reset time while both fit...
        let mut a = alert(ProviderId::Claude, AlertKind::Out, "Claude Code: 5-hour used up");
        a.nudge = Some("Keep going on Oyenza Inference".into());
        assert_eq!(usage_notice(&a, accent, 0.0).subtitle, "Resets 14:20 \u{b7} Keep going on Oyenza Inference");
        a.subtitle = "Resets 2:20 PM".into();
        assert_eq!(usage_notice(&a, accent, 0.0).subtitle, "Resets 2:20 PM \u{b7} Keep going on Oyenza Inference");
        // ...else it stands alone (the AI tab has the reset time).
        for long in ["Resets 10:20 AM", "Resets Thu 09:00", "Resets Tue 3 Nov", "None left for now", "Premium requests \u{b7} resets Tue 1 Dec"] {
            a.subtitle = long.into();
            assert_eq!(usage_notice(&a, accent, 0.0).subtitle, "Keep going on Oyenza Inference", "{long}");
        }
        // Without a nudge the alert's own line stays as it is.
        a.nudge = None;
        assert_eq!(usage_notice(&a, accent, 0.0).subtitle, "Premium requests \u{b7} resets Tue 1 Dec");
    }

    #[test]
    fn ai_notices_go_by_a_sane_line_and_the_apps_clock() {
        use studio_usage::{alerts, Source, Window, WindowKind};
        let mut st = AppState::new(Settings::default(), Store::default());
        for (picked, near) in [(0, 0.5), (49, 0.5), (80, 0.8), (95, 0.95), (100, 0.95), (255, 0.95)] {
            st.settings.usage.near_pct = picked;
            assert_eq!((st.usage_near_pct(), st.usage_alert_cfg().near), ((near * 100.0f32).round() as u8, near), "{picked}");
        }
        // Tue 27 Oct 2026, noon in Jakarta (UTC+7), on a 12-hour clock.
        st.unix = 1_793_077_200;
        st.local = LocalDateTime { year: 2026, month: 10, day: 27, hour: 12, minute: 0, second: 0 };
        st.settings.use_24h = false;
        st.settings.usage.near_pct = 80;
        let cfg = st.usage_alert_cfg();
        assert_eq!((cfg.offset_min, cfg.use_24h, cfg.inference, cfg.floor_usd), (420, false, false, alerts::AlertCfg::default().floor_usd));
        st.usage.inference = InferenceLink::On { account: "a•••@example.com".into() };
        assert!(st.usage_alert_cfg().inference);
        // What the worker's alerts then say, as the island shows it.
        let five = |used| Window::new(WindowKind::Session, "5-hour", used).resets(Some(st.unix + 2 * 3600 + 20 * 60)).length(Some(18_000));
        let read = |used| Snapshot { windows: vec![five(used)], ..Snapshot::new(ProviderId::Claude, st.unix, Source::Online) };
        let said = alerts::changes(Some(&read(0.7)), &read(1.0), st.unix, &st.usage_alert_cfg());
        let n = usage_notice(&said[0], st.accent, 0.0);
        assert_eq!((n.title.as_str(), n.color), ("Claude Code: 5-hour used up", palette::ORANGE));
        assert!(n.subtitle.starts_with("Resets 2:20 PM \u{b7} "), "{}", n.subtitle);
        // ...and one getting close (the self-test's `ai-notice` reads the same way).
        let said = alerts::changes(Some(&read(0.7)), &read(0.9), st.unix, &st.usage_alert_cfg());
        let n = usage_notice(&said[0], st.accent, 0.0);
        assert_eq!((n.title.as_str(), n.subtitle.as_str(), n.color), ("Claude Code: 90% of 5-hour", "Resets 2:20 PM", st.accent));
        // A refused or switched-off inference key: no nudge to a dead end.
        for t in [Trouble::Refused, Trouble::Disabled] {
            st.usage.readings.insert(ProviderId::Oyenza, Reading { snapshot: None, trouble: Some(t), checked_at: st.unix });
            assert!(!st.usage_alert_cfg().inference);
        }
    }
}
