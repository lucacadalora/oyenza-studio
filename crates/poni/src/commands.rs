//! Side effects requested by pages; executed by the app after each frame.

use crate::pages::PageKind;

#[derive(Clone, Debug, PartialEq)]
pub enum MediaCmd {
    PlayPause,
    Next,
    Prev,
    /// Seek to absolute position in seconds.
    Seek(f64),
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
    /// Put text on the clipboard (not re-recorded into history).
    CopyText(String),
    /// Give the island keyboard focus for a text field.
    FocusText,
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
    /// Start/stop the 1 Hz system-stats sampler.
    WantStats(bool),
    /// Start with Windows on or off (the Settings page's switch).
    SetAutostart(bool),
    /// Open the System page on this module (a Today ring was clicked).
    OpenSystem(poni_core::sysmon::Module),
    /// Request an app's icon for the System page (arrives in `AppState::app_icons`).
    LoadAppIcon(String),
    OpenTaskManager,
    Phone(PhoneCmd),
    Oyenza(OyenzaCmd),
}
