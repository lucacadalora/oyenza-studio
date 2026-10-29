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
    /// A Pro add-on's page, or Add-ons, opened: whether the account has
    /// Studio Pro, now (unless just asked).
    CheckPro,
}

/// Requests for the Notifications add-on (`services::notifications`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyCmd {
    /// Ask Windows to let the island read notifications (its question).
    Ask,
    /// Look again at what Windows allows: the page opened, and the person
    /// may have changed it in Windows' settings meanwhile.
    Check,
    /// Open Windows' settings for which apps may read notifications.
    OpenWindowsSettings,
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

/// The IELTS page's buttons; `poni_core::ielts::Coach` decides what each
/// does in the moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IeltsCmd {
    /// Mulai (and Lagi on the result): a session at the picked level.
    Start,
    /// The mic button: answer now, or stop the answer.
    Mic,
    /// Lewati: the next question, this one unscored.
    Skip,
    /// Udahan or Selesai: end the session.
    Quit,
    /// Coba lagi.
    Retry,
}

/// The Notes page's buttons (the Meeting notes add-on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesCmd {
    /// Take notes: Oyen starts listening.
    Start,
    /// Stop: the meeting is over, its notes get written.
    Stop,
    /// Try again: hand the kept recording over again.
    Retry,
    /// Throw the kept recording away.
    Discard,
    /// Open the notes' file, or the notes folder.
    Open,
    Folder,
    /// Copy the notes (without the transcript) as Markdown.
    Copy,
    /// Back to Take notes.
    Done,
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
    /// The Record page's Record: a recording of `settings.record`'s target
    /// with its camera, microphone and system sound (`services::recorder`).
    Record,
    /// Pause, resume or stop the recording under way (the Record page).
    RecordPause,
    RecordResume,
    RecordStop,
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
    /// Settings' Check now: read the update file now, as the menu's Check
    /// for updates… does (`AppState::update.said` takes the answer).
    CheckForUpdates,
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
    Ielts(IeltsCmd),
    Notes(NotesCmd),
    /// Get one of Prayer times' online city files ("ed", "near/55_-5"; the
    /// answer: `AppEvent::Cities`).
    GetCities(String),
    /// The Notifications add-on.
    Notify(NotifyCmd),
    Usage(UsageCmd),
}
