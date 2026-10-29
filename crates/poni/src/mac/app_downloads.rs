//! The Downloads page's commands (`commands::DownloadCmd`) and yt-dlp's
//! reports (`services::downloader`), as in the Windows app
//! (`../app_downloads.rs`): jobs start here when a place is free
//! (`poni_core::downloads`), finished files go to the Shelf, and the island's
//! pill follows them. Only the clipboard, opening a file and starting the
//! runner are the Mac's own. A child of `app`, so it reaches the app's state.

use super::App;
use crate::commands::{Command, DownloadCmd};
use crate::icons::Icon;
use crate::pages::{self, PageKind};
use crate::state::NoticeAction;
use crate::{paths, services};
use poni_core::downloads::{link_in, State};
use poni_core::geom::palette;
use poni_core::ytdlp::{Line, Request};

impl App {
    /// The Downloads page came up: the island opened on it or its tab was
    /// picked. A link copied before then waits above the list
    /// (`pages::downloads_offer`), unless it is in the list or the link field
    /// already, or was the clipboard's link last time (so a dismissed one
    /// doesn't come back). The only read of the clipboard for it: once each
    /// time the page comes up, never on a timer or while the island is closed.
    pub(super) fn downloads_page_shown(&mut self) {
        // The smoke run shows what it set up, not this Mac's clipboard.
        if self.smoke_run || !self.st.downloader_on() {
            return;
        }
        let Some(link) = services::clipboard::read_text().and_then(|t| link_in(&t)) else { return };
        let ui = &mut self.st.ui.downloads;
        let known = ui.link.trim() == link || self.st.downloads.list.iter().any(|j| j.url == link);
        if !known && ui.offered != link {
            ui.offer = Some(link.clone());
        }
        ui.offered = link;
    }

    pub(super) fn download_cmd(&mut self, cmd: DownloadCmd) {
        match cmd {
            DownloadCmd::Start { url, mode } => {
                if !self.st.downloader_on() {
                    return;
                }
                let Some(url) = link_in(&url) else {
                    pages::downloads::note(&mut self.st, "Paste a link first");
                    return;
                };
                // The same link twice at once would write the same file.
                if self.st.downloads.list.iter().any(|j| j.url == url && j.mode == mode && j.state.live()) {
                    pages::downloads::note(&mut self.st, "Already downloading");
                    return;
                }
                self.st.downloads.add(url, mode, self.st.unix);
                self.st.ui.downloads.link.clear();
                self.st.ui.downloads.note = None;
            }
            DownloadCmd::Stop(id) => self.stop_job(id),
            DownloadCmd::Remove(id) => {
                // The page offers Remove once a download ended; a running one stops first.
                if self.st.downloads.get(id).is_some_and(|j| j.state.live()) {
                    self.stop_job(id);
                }
                self.st.downloads.remove(id);
            }
            DownloadCmd::ClearDone => self.st.downloads.clear_done(),
            DownloadCmd::Open(id) => {
                if let Some(path) = self.saved_path(id) {
                    services::open(&path);
                }
            }
            DownloadCmd::Reveal(id) => {
                if let Some(path) = self.saved_path(id) {
                    services::reveal(&path);
                }
            }
            DownloadCmd::PasteLink => match services::clipboard::read_text().as_deref().and_then(link_in) {
                Some(link) => {
                    // The field has the keyboard: its caret goes after the link.
                    if self.input.focused.is_some() {
                        self.input.caret = link.chars().count();
                    }
                    self.st.ui.downloads.link = link;
                    self.st.ui.downloads.note = None;
                }
                None => pages::downloads::note(&mut self.st, "Copy a link first"),
            },
        }
        // A new download, or a place that came free.
        self.start_ready();
    }

    pub(super) fn download_line(&mut self, id: u64, line: Line) {
        let done = match &line {
            Line::Done { path } => Some(path.clone()),
            _ => None,
        };
        self.st.downloads.apply(id, line);
        if let Some(path) = done {
            self.saved(id, path);
        }
    }

    pub(super) fn download_ended(&mut self, id: u64, ok: bool) {
        self.st.downloads.ended(id, ok);
        self.start_ready();
    }

    /// The Downloader switched off, or Studio Pro went: its downloads stop
    /// before its tools are deleted, and the list goes with the tab.
    pub(super) fn downloads_off(&mut self) {
        let live: Vec<u64> = self.st.downloads.list.iter().filter(|j| j.state.live()).map(|j| j.id).collect();
        for id in live {
            self.stop_job(id);
        }
        self.st.downloads.clear_done();
    }

    /// Start the queued downloads that have a place now (at most
    /// `downloads::AT_ONCE` run together). The runner starts with the first.
    fn start_ready(&mut self) {
        let ids = self.st.downloads.to_start();
        // The smoke run starts no downloads: what it asks for stays queued.
        if ids.is_empty() || self.smoke_run {
            return;
        }
        if self.downloader.is_none() {
            match services::downloader::DownloaderService::start(self.sink.clone()) {
                Ok(d) => self.downloader = Some(d),
                Err(e) => return pwarn!("downloader: unavailable ({e})"),
            }
        }
        let Some(svc) = self.downloader.as_ref() else { return };
        let folder = paths::media_downloads_dir().to_string_lossy().into_owned();
        for id in ids {
            let Some(job) = self.st.downloads.get(id) else { continue };
            svc.run(Request { id, url: job.url.clone(), mode: job.mode, max_height: None, folder: folder.clone() });
            self.st.downloads.starting(id);
        }
    }

    /// Stop download `id`: its yt-dlp and whatever yt-dlp started.
    fn stop_job(&mut self, id: u64) {
        if let Some(d) = self.downloader.as_ref() {
            d.stop(id);
        }
        self.st.downloads.stop(id);
    }

    /// Download `id`'s file, once it is finished.
    fn saved_path(&self, id: u64) -> Option<String> {
        match &self.st.downloads.get(id)?.state {
            State::Done { path, .. } => Some(path.clone()),
            _ => None,
        }
    }

    /// yt-dlp finished download `id`'s file: its size from the disk (yt-dlp
    /// doesn't say it), the file onto the Shelf as a dropped one goes, and a
    /// notice while the island is closed. Not for a download stopped in the
    /// meantime, or a file that isn't where yt-dlp said.
    fn saved(&mut self, id: u64, path: String) {
        let Some(job) = self.st.downloads.get(id) else { return };
        if !matches!(&job.state, State::Done { path: done, .. } if *done == path) {
            return;
        }
        let title = job.title.clone().unwrap_or_else(|| job.url.clone());
        let Ok(meta) = std::fs::metadata(&path) else {
            pwarn!("downloads: {id} finished, but its file isn't where yt-dlp said");
            return;
        };
        self.st.downloads.set_size(id, meta.len());
        pinfo!("downloads: {id} saved ({} bytes)", meta.len());
        self.st.store.shelf.add_paths(&[&path], self.st.unix);
        self.st.cmds.push(Command::LoadThumb(path));
        self.st.save();
        if !self.expanded {
            self.st.notify(Icon::Download, palette::GREEN, "Saved", title, 6.0, Some(NoticeAction::OpenPage(PageKind::Shelf)));
        }
    }
}
