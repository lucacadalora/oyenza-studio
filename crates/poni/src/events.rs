//! Messages from service threads to the UI thread.
//!
//! Services hold an [`EventSink`]. `send` pushes onto an mpsc channel and posts
//! `WM_APP_EVENT` to the island window; the UI thread drains the channel.

use crate::gfx::Image;
use crate::state::{MediaSession, PowerState, SysStats};
use poni_core::health::HealthSnapshot;
use poni_core::lrc::Lyrics;
use poni_core::agenda;
use poni_core::oyenza::{Activity, Arrival, Bill, Saved};
use poni_core::romanize::Ruby;
use poni_core::timeline::Timeline;
use std::sync::mpsc::Sender;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_EVENT: u32 = WM_APP + 1;
pub const WM_APP_FRAME: u32 = WM_APP + 2;
pub const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_APPBAR: u32 = WM_APP + 4;

/// An HWND that can cross threads (only used for PostMessage).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendHwnd(pub isize);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

impl SendHwnd {
    pub fn new(h: HWND) -> Self {
        Self(h.0 as isize)
    }
    pub fn hwnd(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }
}

#[derive(Clone)]
pub struct EventSink {
    tx: Sender<AppEvent>,
    hwnd: SendHwnd,
}

impl EventSink {
    pub fn new(tx: Sender<AppEvent>, hwnd: SendHwnd) -> Self {
        Self { tx, hwnd }
    }

    pub fn send(&self, ev: AppEvent) {
        if self.tx.send(ev).is_ok() {
            unsafe {
                let _ = PostMessageW(self.hwnd.hwnd(), WM_APP_EVENT, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn hwnd(&self) -> SendHwnd {
        self.hwnd
    }
}

/// What Apple Music said about the user's token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppleMusicStatus {
    /// The token works; Apple's synced lyrics come first from now on.
    Accepted,
    /// Apple refuses the token (expired, signed out, copied wrong).
    Rejected,
    /// Apple couldn't be reached to check; lookups will try again.
    Unreachable,
    /// The token works, but the account gets no lyrics (no subscription?).
    NoAccess,
}

#[derive(Clone, Debug)]
pub enum LyricsResult {
    /// `ruby`: each line's pieces with their readings (empty = none needed).
    /// `wants_ja_readings`: kanji went unread for want of the Japanese
    /// readings add-on (`services::romanize::Pronounced`).
    Found { lyrics: Lyrics, source: String, ruby: Vec<Vec<Ruby>>, wants_ja_readings: bool },
    /// The song is marked instrumental (no words to show).
    Instrumental,
    NotFound,
    Error(String),
}

/// What the Oyenza worker reports (`services::oyenza`).
#[derive(Clone, Debug)]
pub enum OyenzaEvent {
    /// The sign-in saved on this PC was found at start (no notice).
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
    /// Whether the account has Studio Pro (a server from before Pro: no).
    Pro(bool),
    /// The day couldn't be read just now (offline, a server error).
    DayFailed,
    NoteSaved(Vec<Saved>),
    /// The note wasn't saved; the island puts `text` back and says why (`code`).
    NoteFailed { text: String, code: String },
    Undone,
    UndoFailed(String),
    /// Signed out: from the tray, or by the server (disconnected in oyenza.app).
    Disconnected { by_server: bool },
}

pub enum AppEvent {
    /// The current media session changed (None = nothing playing / no session).
    Media(Option<MediaSession>),
    /// Only the timeline/playback state of the current track changed.
    MediaTimeline { track_key: String, timeline: Timeline },
    /// Album art finished decoding for a track.
    MediaArt { track_key: String, art: Option<Image> },
    Lyrics { track_key: String, result: LyricsResult },
    /// The outcome of checking or using the user's Apple Music token.
    AppleMusic(AppleMusicStatus),
    /// Master volume changed (0..=1).
    Volume { level: f32, muted: bool },
    Power(PowerState),
    Bluetooth { name: String, connected: bool, battery: Option<u8>, audio: bool },
    /// A full-screen app took over (or left) the island's monitor. `true` is
    /// sent again when another window comes to the foreground while the
    /// monitor stays full-screen, so the island can be raised back on top.
    Fullscreen(bool),
    /// Another window came to the foreground on a monitor that isn't
    /// full-screen. It may have made itself topmost and now cover the island.
    Foreground,
    /// Step aside: the app windows at the top edge of the island's monitor
    /// changed (a browser maximized or restored, a window snapped or dragged
    /// to the top). Screen pixels (left, top, right, bottom), each clipped
    /// to the thin band along the edge.
    TopWindows(Vec<[i32; 4]>),
    /// Files dropped on the island.
    ShelfAdd(Vec<String>),
    /// Files are being dragged over the island (true) or left (false).
    DragHover(bool),
    ShelfThumb { path: String, image: Option<Image> },
    Stats(SysStats),
    /// The System page's latest reading and the minute behind it
    /// (`services::sysmon`, each second while the page is open).
    System(Box<poni_core::sysmon::Monitor>),
    /// Show over games: whether Windows lets Poni read frames (a check, or
    /// the one-time OK's outcome).
    FpsAccess(crate::state::FpsAccess),
    /// Phone drop's server: a phone opened the page, asked, sent, gave up.
    Phone(poni_core::phonedrop::Event),
    /// Paired phones: pairing done, the list changed, files from a phone.
    Paired(poni_core::pairdrop::MeetEvent),
    Oyenza(OyenzaEvent),
    /// Where an add-on's files stand (`services::addons`).
    Addon { id: &'static str, status: poni_core::addons::Status },
    /// What the update file said (`services::update`).
    Update(crate::services::update::UpdateEvent),
    /// Where Windows says the PC is (`services::location`), for prayer times.
    Located(crate::services::location::Located),
    /// One of prayer times' online city files (`services::cities`).
    Cities { file: String, got: Result<poni_core::prayer::World, String> },
    /// The Stocks page's prices, names and years (`services::stocks`).
    Stocks(crate::services::stocks::StocksEvent),
    /// The Notifications add-on: what Windows allows, and new notifications
    /// (`services::notifications`).
    Notify(crate::services::notifications::NotifyEvent),
    /// The AI tab's readings, typed keys, inference.oyenza.com's link and
    /// heads-up notices (`services::usage`).
    Usage(crate::services::usage::UsageEvent),
    /// The adzan finished playing, or couldn't play on.
    AdzanEnded,
    /// Start with Windows is on (true) or off, read for the Settings page.
    Autostart(bool),
    /// Free-form log line from a service thread.
    Log(String),
    /// The Glass theme's screen copy has new pixels (or stopped).
    Backdrop,
    /// The IELTS worker's answer (`services::ielts`).
    Ielts(Box<poni_core::ielts::Reply>),
    /// Oyen's clip finished playing, or couldn't play on (`services::voice`).
    VoiceEnded,
    /// The microphone stopped mid-answer (`services::mic`): why, for the log.
    MicFailed(String),
    /// The meeting being handed over, as it goes (`services::notes`).
    Notes(Box<poni_core::notes::Reply>),
    /// The meeting's recording stopped by itself: the longest meeting taken,
    /// or the disk wouldn't take more (`services::notes`).
    NotesStopped(poni_core::notes::CaptureIssue),
}
