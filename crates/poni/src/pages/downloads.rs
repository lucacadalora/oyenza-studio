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

/// What the page keeps between frames.
#[derive(Clone, Debug, Default)]
pub struct DownloadsUi {
    /// The link being typed or pasted.
    pub link: String,
    pub mode: Mode,
    pub scroll: f32,
}

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let _ = (ui, st, area);
}
