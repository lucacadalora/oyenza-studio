//! The Downloads page's commands (`commands::DownloadCmd`) and yt-dlp's
//! reports (`services::downloader`). A child of `app`, so it reaches the
//! app's state: jobs start here when a place is free (`poni_core::downloads`),
//! finished files go to the Shelf, and the island's pill follows them.

use super::App;
use crate::commands::DownloadCmd;
use poni_core::ytdlp::Line;

impl App {
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
