//! Messages from service threads to the UI (main) thread.
//!
//! Services hold an [`EventSink`]. `send` pushes onto an mpsc channel and
//! queues a drain on the main dispatch queue; the main thread then applies
//! every waiting event. The event types match the Windows app's, so the
//! services that arrive in later slices plug in the same way.

use crate::gfx::Image;
use crate::state::{MediaSession, PowerState, SysStats};
use poni_core::health::HealthSnapshot;
use poni_core::lrc::Lyrics;
use poni_core::agenda;
use poni_core::oyenza::{Activity, Arrival, Bill, Saved};
use poni_core::romanize::Ruby;
use poni_core::timeline::Timeline;
use std::sync::mpsc::Sender;

#[derive(Clone)]
pub struct EventSink {
    tx: Sender<AppEvent>,
}

impl EventSink {
    pub fn new(tx: Sender<AppEvent>) -> Self {
        Self { tx }
    }

    pub fn send(&self, ev: AppEvent) {
        if self.tx.send(ev).is_ok() {
            dispatch2::DispatchQueue::main().exec_async(|| {
                crate::app::drain_events();
            });
        }
    }
}

/// What Apple Music said about the user's token.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppleMusicStatus {
    Accepted,
    Rejected,
    Unreachable,
    NoAccess,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum LyricsResult {
    /// `ruby`: each line's pieces with their readings (empty = none needed).
    /// `wants_ja_readings`: kanji went unread for want of the Japanese
    /// readings add-on (the Music page offers it).
    Found { lyrics: Lyrics, source: String, ruby: Vec<Vec<Ruby>>, wants_ja_readings: bool },
    Instrumental,
    NotFound,
    Error(String),
}

/// What the Oyenza worker reports (`services::oyenza`, shared with Windows).
#[derive(Clone, Debug)]
pub enum OyenzaEvent {
    /// The sign-in saved on this Mac was found at start (no notice).
    Restored { account: String },
    /// A sign-in code to show, the oyenza.app address that takes it, and how long it lasts.
    Connecting { code: String, url: String, expires_in_s: u64 },
    Connected { account: String },
    /// Signing in stopped without a key: `expired`, `denied`, `offline` or `other`.
    ConnectFailed(String),
    ConnectCancelled,
    /// Money that just arrived or went out by itself (oldest first).
    Money { arrived: Vec<Arrival> },
    /// The last 7 days of transactions, for the Oyenza page (while it's open).
    Activity(Activity),
    /// Asked again while the page is open: nothing changed.
    ActivityChecked,
    /// Bills due in the next 30 days, and the server's today (YYYY-MM-DD).
    Bills { today: String, bills: Vec<Bill> },
    /// The steps the phone sent, for the Health page; None: none to show (off in
    /// Oyenza on the phone, or a server from before steps).
    Health(Option<HealthSnapshot>),
    /// The day for the Today page, from the apps switched on in oyenza.app (asked
    /// while the page is on screen).
    Day(agenda::Answer),
    /// The server has no day for the Today page (a server from before it, or
    /// switched off there): the page stays as it was before signing in mattered.
    DayUnavailable,
    /// The day couldn't be read just now (offline, a server error).
    DayFailed,
    NoteSaved(Vec<Saved>),
    /// The note wasn't saved; the island puts `text` back and says why (`code`).
    NoteFailed { text: String, code: String },
    Undone,
    UndoFailed(String),
    /// Signed out: from the menu or Settings, or by the server (disconnected in oyenza.app).
    Disconnected { by_server: bool },
}

#[allow(dead_code)]
pub enum AppEvent {
    /// The current media session changed (None = nothing playing / no session).
    Media(Option<MediaSession>),
    /// Only the timeline/playback state of the current track changed.
    MediaTimeline { track_key: String, timeline: Timeline },
    /// Album art finished decoding for a track.
    MediaArt { track_key: String, art: Option<Image> },
    Lyrics { track_key: String, result: LyricsResult },
    AppleMusic(AppleMusicStatus),
    Oyenza(OyenzaEvent),
    /// Master volume changed (0..=1). `switched`: the output device changed
    /// (headphones connected, say), not its volume.
    Volume { level: f32, muted: bool, switched: bool },
    Power(PowerState),
    Bluetooth { name: String, connected: bool, battery: Option<u8>, audio: bool },
    /// A full-screen app took over (or left) the island's screen.
    Fullscreen(bool),
    /// Files dropped on the island.
    ShelfAdd(Vec<String>),
    /// Files are being dragged over the island (true) or left (false).
    DragHover(bool),
    ShelfThumb { path: String, image: Option<Image> },
    Stats(SysStats),
    /// The System page's latest reading and the minute behind it
    /// (`services::sysmon`, each second while the page is open).
    System(Box<poni_core::sysmon::Monitor>),
    /// An app's icon for the System page, by its path (`Command::LoadAppIcon`).
    AppIcon { path: String, image: Option<Image> },
    /// The displays went to sleep (true) or woke.
    ScreensAsleep(bool),
    /// The Mac was locked (true) or unlocked.
    Locked(bool),
    /// Something happened in Phone drop (a phone visited, asked, sent files).
    Phone(poni_core::phonedrop::Event),
    /// Where an add-on's files stand (`services::addons`).
    Addon { id: &'static str, status: poni_core::addons::Status },
    /// What the update file said (`services::update`).
    Update(crate::services::update::UpdateEvent),
    /// Where the Mac is, for Prayer times (`services::location`).
    Located(crate::services::location::Located),
    /// One of Prayer times' online city files (`services::cities`).
    Cities { file: String, got: Result<poni_core::prayer::World, String> },
    /// The Stocks page's prices, names and years (`services::stocks`).
    Stocks(crate::services::stocks::StocksEvent),
    /// Prayer times' adzan played to its end (`services::adzan`).
    AdzanEnded,
    /// Free-form log line from a service thread.
    Log(String),
}
