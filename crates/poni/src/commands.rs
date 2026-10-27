//! Side effects requested by pages; executed by the app after each frame.

use crate::pages::PageKind;
use studio_usage::ProviderId;

#[derive(Clone, Debug, PartialEq)]
pub enum MediaCmd {
    PlayPause,
    Next,
    Prev,
    /// Seek to absolute position in seconds.
    Seek(f64),
    /// The pointer reached the progress bar or the back button: get ready
    /// (Apple Music's own controls are looked up before the press, not during it).
    Prepare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    /// Gentle chime (timer finished, reminder).
    Chime,
    /// Short tick (button feedback) — currently unused.
    Tick,
}

/// Phone drop (the Shelf's phone panel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneCmd {
    /// Start it (or try again after an error).
    Open,
    /// Stop it; files still coming in are dropped.
    Close,
    /// Answer the phone's question.
    Answer { id: u64, accept: bool },
}

/// Paired phones (also in the Shelf's phone panel; `services::pairdrop`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairCmd {
    /// Show a pairing QR code in the panel.
    Begin,
    /// Back to the same-Wi-Fi code (a pairing code stops working).
    Back,
    /// Show the paired phones.
    List,
    /// Forget a paired phone (by its room on the meeting server).
    Remove(String),
}

/// Requests for the Oyenza worker (`services::oyenza`).
#[derive(Clone, Debug, PartialEq)]
pub enum OyenzaCmd {
    /// Ask for a sign-in code to show.
    SignIn,
    /// Stop waiting for the code's approval.
    Cancel,
    /// Sign this PC out (and tell the server).
    Disconnect,
    /// Save a spend typed in the island.
    Note(String),
    /// Take back a spend just saved (its id).
    Undo(String),
    /// The Oyenza tab opened: today's money and the bills, now.
    Refresh,
    /// The Health tab opened: the steps from the phone, now (unless just asked for).
    RefreshHealth,
    /// Today's Try again: the day, now (unless just asked for).
    RefreshDay,
}

/// What the AI tab asks for. Hide and Show change `settings.usage`; the rest
/// go to its worker (`services::usage`).
#[derive(Clone, PartialEq)]
pub enum UsageCmd {
    /// Read every service now.
    Refresh,
    /// Keep a key typed for a service, sealed (DPAPI / the keychain), under
    /// its secret name (`usage::KeyAsk::secret`).
    SetKey { secret: &'static str, key: String },
    /// Forget a typed key.
    ClearKey(&'static str),
    /// Connect inference.oyenza.com: ask for a code to show.
    Connect,
    /// Stop waiting for the code's approval.
    CancelConnect,
    /// Disconnect inference.oyenza.com (its key stops working there too).
    Disconnect,
    /// Take a service's card off the tab.
    Hide(ProviderId),
    /// Put it back, even when nothing of it is on this computer (a service
    /// that takes a typed key).
    Show(ProviderId),
}

/// Never shows a typed key (a command may end up in the log).
impl std::fmt::Debug for UsageCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsageCmd::Refresh => f.write_str("Refresh"),
            UsageCmd::SetKey { secret, .. } => write!(f, "SetKey({secret})"),
            UsageCmd::ClearKey(secret) => write!(f, "ClearKey({secret})"),
            UsageCmd::Connect => f.write_str("Connect"),
            UsageCmd::CancelConnect => f.write_str("CancelConnect"),
            UsageCmd::Disconnect => f.write_str("Disconnect"),
            UsageCmd::Hide(id) => write!(f, "Hide({})", id.key()),
            UsageCmd::Show(id) => write!(f, "Show({})", id.key()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Media(MediaCmd),
    /// Open a file/folder with its default handler.
    OpenPath(String),
    /// Show a file in Explorer.
    RevealPath(String),
    /// Begin an OLE drag of these paths out of the island.
    DragOut(Vec<String>),
    /// Request a thumbnail for a shelf item (async; arrives as `AppEvent::ShelfThumb`).
    LoadThumb(String),
    /// Give the island keyboard focus for a text field.
    FocusText,
    /// The Script page's Paste: the clipboard's text, line breaks kept,
    /// becomes the script.
    ScriptPaste,
    /// The Script page's Record: Oyenza Studio Recorder records the screen
    /// the island is on (`services::recorder`).
    RecordScreen,
    /// Drop keyboard focus.
    BlurText,
    /// Persist `AppState::store` soon.
    SaveStore,
    /// Persist `AppState::settings` now (after a page toggled a setting).
    SaveSettings,
    SetPage(PageKind),
    /// Keep the island open even when the cursor leaves.
    SetPinned(bool),
    Collapse,
    Play(Sound),
    /// Open a URL in the default browser.
    OpenUrl(String),
    /// Update to the version the update file offers: install it (Windows,
    /// once its Setup is downloaded), or its release page or DMG.
    Update,
    /// Install it the next time the app starts (Windows).
    InstallOnNextStart,
    /// Start/stop the 1 Hz system-stats sampler.
    WantStats(bool),
    /// Start with Windows on or off (the Settings page's switch).
    SetAutostart(bool),
    /// Open the System page on this module (a Today ring was clicked).
    OpenSystem(poni_core::sysmon::Module),
    /// Request an app's icon for the System page (arrives in `AppState::app_icons`).
    LoadAppIcon(String),
    /// Show over games: find out whether Windows lets Poni read frames
    /// (the answer lands in `AppState::game.access`).
    CheckFps,
    /// Show over games: ask Windows once for the right to read frames.
    AllowFps,
    Phone(PhoneCmd),
    Pair(PairCmd),
    Oyenza(OyenzaCmd),
    /// Get one of Prayer times' online city files ("ed", "near/55_-5"; the
    /// answer: `AppEvent::Cities`).
    GetCities(String),
    Usage(UsageCmd),
}
