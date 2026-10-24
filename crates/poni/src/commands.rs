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
    Phone(PhoneCmd),
}
