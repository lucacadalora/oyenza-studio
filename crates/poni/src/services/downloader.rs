//! The Downloader add-on's runner: one yt-dlp per download
//! (`poni_core::ytdlp::args`), started without a console window inside a
//! job object, so stopping a download (or the app quitting) ends yt-dlp and
//! the FFmpeg and QuickJS it started. Each job has a reader thread for its
//! output, which `poni_core::ytdlp::parse` turns into `AppEvent::Download`;
//! the thread ends with the process (`AppEvent::DownloadEnded`). Nothing runs
//! at rest: the service starts at the first download and holds no thread of
//! its own between downloads.

use crate::events::EventSink;
use poni_core::ytdlp::Request;
use std::io;

pub struct DownloaderService {
    sink: EventSink,
}

impl DownloaderService {
    pub fn start(sink: EventSink) -> io::Result<Self> {
        Ok(Self { sink })
    }

    /// Start yt-dlp for `req`; returns at once (UI thread safe).
    pub fn run(&self, req: Request) {
        let _ = (&self.sink, req);
        crate::pwarn!("downloader: not built yet");
    }

    /// Stop download `id`; returns at once.
    pub fn stop(&self, id: u64) {
        let _ = id;
    }
}
