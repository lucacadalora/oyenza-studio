//! Downloads page (the Downloader add-on, `poni_core::addons::DOWNLOADER`):
//! a link field with Paste, Video or Sound, and Download; under it the
//! downloads, newest first, each with its picture, title, the line under it
//! (`poni_core::downloads::line`) and Stop, Open or Show in folder. yt-dlp
//! does the work (`services::downloader`, Windows); nothing runs at rest.

use crate::state::AppState;
use crate::ui::Ui;
use poni_core::geom::Rect;
use poni_core::ytdlp::Mode;

/// The page's height: the window never grows for it.
const HEIGHT: f32 = 256.0;
/// A message from the app ("Copy a link first") shows this long.
const NOTE_S: f64 = 4.0;

/// What the page keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct DownloadsUi {
    /// The link being typed or pasted.
    pub link: String,
    pub mode: Mode,
    pub scroll: f32,
    /// A link copied before the page came up, waiting above the list until
    /// it is downloaded or dismissed (`downloads_offer`; Windows only).
    pub offer: Option<String>,
    /// The clipboard's link the last time the page came up: each link is
    /// offered once, so a dismissed one doesn't come back.
    pub offered: String,
    /// A message under the link field until a time (mono seconds).
    pub note: Option<(&'static str, f64)>,
}

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// Show a short message under the link field.
pub fn note(st: &mut AppState, text: &'static str) {
    st.ui.downloads.note = Some((text, st.now + NOTE_S));
}

/// When the message ends (the app's wake timer takes it away).
pub fn next_wake(st: &AppState) -> Option<f64> {
    st.ui.downloads.note.map(|(_, until)| until).filter(|&until| until > st.now)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let _ = (ui, st, area);
}
