//! The Downloads page's commands (`commands::DownloadCmd`) and yt-dlp's
//! reports (`services::downloader`). A child of `app`, so it reaches the
//! app's state: jobs start here when a place is free (`poni_core::downloads`),
//! finished files go to the Shelf, and the island's pill follows them.

use super::App;
use crate::commands::DownloadCmd;
use poni_core::ytdlp::Line;

impl App {
    /// The Downloads page came up: the island opened on it or its tab was
    /// picked (`App::frame`). A link copied before then waits above the list
    /// (`pages::downloads_offer`), unless it is in the list or the link field
    /// already, or was the clipboard's link last time (so a dismissed one
    /// doesn't come back). The only read of the clipboard for it: once each
    /// time the page comes up, never on a timer or while the island is closed.
    pub(super) fn downloads_page_shown(&mut self) {
        // The self-test's scenes show what they set up, not this PC's clipboard.
        if self.selftest.is_some() || !self.st.downloader_on() {
            return;
        }
        let Some(link) = crate::services::clipboard::read_text(self.hwnd).and_then(|t| poni_core::downloads::link_in(&t)) else { return };
        let ui = &mut self.st.ui.downloads;
        let known = ui.link.trim() == link || self.st.downloads.list.iter().any(|j| j.url == link);
        if !known && ui.offered != link {
            ui.offer = Some(link.clone());
        }
        ui.offered = link;
    }

    pub(super) fn download_cmd(&mut self, cmd: DownloadCmd) {
        let _ = cmd;
    }

    pub(super) fn download_line(&mut self, id: u64, line: Line) {
        self.st.downloads.apply(id, line);
    }

    pub(super) fn download_ended(&mut self, id: u64, ok: bool) {
        self.st.downloads.ended(id, ok);
    }
}
