//! Messages from service threads to the UI thread.
//!
//! Services hold an [`EventSink`]. `send` pushes onto an mpsc channel and posts
//! `WM_APP_EVENT` to the island window; the UI thread drains the channel.

use crate::gfx::Image;
use crate::state::{MediaSession, PowerState, SysStats};
use poni_core::lrc::Lyrics;
use poni_core::oyenza::{Arrival, Bill, Saved};
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
    Found { lyrics: Lyrics, source: String, ruby: Vec<Vec<Ruby>> },
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
    /// Money that just arrived (oldest first), and today's when asked for.
    Money { arrived: Vec<Arrival>, today: Option<Vec<Arrival>> },
    /// Bills due in the next 7 days, and the server's today (YYYY-MM-DD).
    Bills { today: String, bills: Vec<Bill> },
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
    /// A full-screen app took over (or left) the island's monitor.
    Fullscreen(bool),
    /// Files dropped on the island.
    ShelfAdd(Vec<String>),
    /// Files are being dragged over the island (true) or left (false).
    DragHover(bool),
    ShelfThumb { path: String, image: Option<Image> },
    Stats(SysStats),
    /// A System page reading (`services::sysmon`, only while the page is open).
    System(Box<poni_core::sysmon::Reading>),
    ClipboardText(String),
    /// Phone drop's server: a phone opened the page, asked, sent, gave up.
    Phone(poni_core::phonedrop::Event),
    Oyenza(OyenzaEvent),
    /// Start with Windows is on (true) or off, read for the Settings page.
    Autostart(bool),
    /// Free-form log line from a service thread.
    Log(String),
}
