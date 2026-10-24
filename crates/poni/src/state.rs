//! All state shared by pages, activities and the app loop.
//!
//! Owned by the UI thread. Services never touch it directly; they send
//! [`crate::events::AppEvent`]s that the app applies here.

use crate::commands::{Command, MediaCmd, Sound};
use crate::gfx::Image;
use crate::icons::Icon;
use crate::pages::PageKind;
use poni_core::addons::Status as AddonStatus;
use poni_core::cliphist::ClipHistory;
use poni_core::datetime::LocalDateTime;
use poni_core::geom::{palette, Color};
use poni_core::health::HealthSnapshot;
use poni_core::lrc::Lyrics;
use poni_core::oyenza::{Activity, Bill, DayPick, Saved, Seen};
use poni_core::phonedrop::QrMatrix;
use poni_core::pomodoro::Pomodoro;
use poni_core::prayer::{Place, PlaceList, Site};
use poni_core::romanize::Ruby;
use poni_core::settings::Settings;
use poni_core::store::Store;
use poni_core::timeline::Timeline;
use std::collections::{BTreeMap, HashMap};

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
    /// data and the Japanese readings add-on isn't on this PC: the Music page
    /// offers the add-on.
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
}

/// A one-click action shown as a small pill inside a notice (doesn't expand).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineAction {
    /// Start the idle Pomodoro phase.
    StartFocus,
    /// Put the notice's hidden sum (and sender) in place (Oyenza).
    ShowAmount,
    /// Stop the adzan (Prayer times).
    StopAdzan,
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
    /// The module the System page shows under its strip.
    pub system_module: poni_core::sysmon::Module,
    /// The last System reading had a GPU (keeps its chip while the next one is on its way).
    pub system_gpu: bool,
    /// The page the Settings button was pressed on (pressing it again goes back).
    pub before_settings: Option<PageKind>,
    /// Start with Windows, as last read off the UI thread (None until Settings first opens).
    pub autostart: Option<bool>,
    /// Oyen theme: the music notes floating up in the media pill.
    pub notes: crate::oyen::Notes,
}

/// Whether Windows lets Oyen Poni read a game's frames. Its event tracing
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

/// What the Oyenza tab and its notices show (`pages::oyenza`). Oyen's moods
/// never read any of it: money never makes him happier or a bill sadder.
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
}

impl OyenzaState {
    /// The Oyenza tab shows while signing in and once signed in.
    pub fn tab_visible(&self) -> bool {
        self.link != OyenzaLink::Off
    }
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
    /// The nearest listed place is too far away to use (km).
    Far { km: f64 },
    /// Windows' location is off, or not allowed for Oyen Poni.
    Denied,
    /// Windows couldn't tell where the PC is.
    Unknown,
}

/// The Prayer times add-on's live state.
#[derive(Clone, Debug, Default)]
pub struct PrayerState {
    /// The add-on's place list, read once its files are in place.
    pub places: Option<std::sync::Arc<PlaceList>>,
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
    /// Self-test only: moves the prayer clock by this many seconds.
    pub clock_shift: i64,
}

impl PrayerState {
    pub fn site(&self) -> Option<Site> {
        self.place.as_ref().and_then(|p| p.site())
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
    /// The Prayer times add-on, while it is on (its choices: `settings.prayer`).
    pub prayer: PrayerState,
    /// Side effects requested during the frame.
    pub cmds: Vec<Command>,
}

impl AppState {
    /// The Shelf's phone panel keeps the open island from closing, only
    /// while the Shelf is the page on screen.
    pub fn phone_holds_island(&self) -> bool {
        self.page == PageKind::Shelf && self.phone.as_ref().is_some_and(|p| p.holds_island())
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
            clip: ClipHistory::default(),
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
            drag_hover: false,
            phone: None,
            paired: Vec::new(),
            pair_server: None,
            oyenza: OyenzaState::default(),
            addons: BTreeMap::new(),
            prayer: PrayerState::default(),
            cmds: Vec::new(),
        };
        // On ones start by checking their files (the worker reports back).
        for a in poni_core::addons::ALL {
            let on = st.settings.addon_on(a.id);
            st.addons.insert(a.id, if on { AddonStatus::Checking } else { AddonStatus::Off });
        }
        st
    }

    /// Where the add-on `id` stands (`Off` for one this version doesn't know).
    pub fn addon(&self, id: &str) -> AddonStatus {
        self.addons.get(id).copied().unwrap_or(AddonStatus::Off)
    }

    /// Prayer times is on and its places are read: the tab and reminders run.
    pub fn prayer_on(&self) -> bool {
        self.addon(poni_core::addons::PRAYER_TIMES.id) == AddonStatus::Ready && self.prayer.places.is_some()
    }

    /// The prayer clock, UTC seconds (the self-test moves it).
    pub fn prayer_now(&self) -> i64 {
        self.unix + self.prayer.clock_shift
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
}
