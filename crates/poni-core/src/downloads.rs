//! The Downloads page's list (`addons::DOWNLOADER`): each link the person
//! gave the downloader, where it stands, the line under it and what the
//! island's pill shows while something downloads. yt-dlp's lines
//! (`ytdlp::Line`) move a job along; at most [`AT_ONCE`] run together, the
//! rest wait their turn. Kept in memory: finished files also go to the Shelf.

use crate::ytdlp::{Line, Mode, Stage};

/// How many downloads run at once; the rest queue.
pub const AT_ONCE: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    pub url: String,
    pub mode: Mode,
    /// From yt-dlp's first line about it; None until then.
    pub title: Option<String>,
    pub site: Option<String>,
    pub thumbnail: Option<String>,
    pub state: State,
    /// When it was asked for (unix seconds).
    pub added: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Waiting for a free place ([`AT_ONCE`]).
    Queued,
    /// yt-dlp is reading the link.
    Starting,
    Downloading { done: u64, total: Option<u64>, speed: Option<f64>, eta_s: Option<u64> },
    /// The bytes are in; yt-dlp is joining or tidying the file.
    Finishing(Stage),
    Done { path: String, size: u64 },
    /// In the words the page shows (`ytdlp::Problem::message`).
    Failed(String),
    /// Stopped by the person.
    Stopped,
}

impl State {
    /// Still to finish: queued, starting, downloading or finishing.
    pub fn live(&self) -> bool {
        matches!(self, State::Queued | State::Starting | State::Downloading { .. } | State::Finishing(_))
    }
}

/// What the closed island's pill shows while downloads run.
#[derive(Clone, Debug, PartialEq)]
pub struct Pill {
    /// Of everything running (None while no size is known yet).
    pub fraction: Option<f32>,
    /// Downloads not finished yet, queued ones included.
    pub count: usize,
    /// The first running download's title (or its link).
    pub title: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Jobs {
    /// Newest first.
    pub list: Vec<Job>,
    next_id: u64,
}

impl Jobs {
    /// A new job for `url`, queued; returns its id.
    pub fn add(&mut self, url: String, mode: Mode, now: i64) -> u64 {
        let _ = (url, mode, now);
        todo!("dl-jobs")
    }

    pub fn get(&self, id: u64) -> Option<&Job> {
        self.list.iter().find(|j| j.id == id)
    }

    /// One of yt-dlp's lines for job `id`.
    pub fn apply(&mut self, id: u64, line: Line) {
        let _ = (id, line);
        todo!("dl-jobs")
    }

    /// yt-dlp for job `id` ended (`ok`: exit code 0).
    pub fn ended(&mut self, id: u64, ok: bool) {
        let _ = (id, ok);
        todo!("dl-jobs")
    }

    /// The person stopped job `id`.
    pub fn stop(&mut self, id: u64) {
        let _ = id;
        todo!("dl-jobs")
    }

    /// Take job `id` off the list (a finished, failed or stopped one).
    pub fn remove(&mut self, id: u64) {
        let _ = id;
        todo!("dl-jobs")
    }

    /// Take every finished, failed and stopped job off the list.
    pub fn clear_done(&mut self) {
        todo!("dl-jobs")
    }

    /// The queued jobs that may start now, oldest first, given those running.
    /// The caller starts them and marks them `Starting` ([`Jobs::starting`]).
    pub fn to_start(&self) -> Vec<u64> {
        todo!("dl-jobs")
    }

    pub fn starting(&mut self, id: u64) {
        let _ = id;
        todo!("dl-jobs")
    }

    /// Jobs whose yt-dlp runs now (starting, downloading or finishing).
    pub fn running(&self) -> usize {
        todo!("dl-jobs")
    }

    /// Anything left to finish (the pill shows while this is true).
    pub fn busy(&self) -> bool {
        self.list.iter().any(|j| j.state.live())
    }

    pub fn pill(&self) -> Option<Pill> {
        todo!("dl-jobs")
    }
}

/// The line under a job on the Downloads page: "12 MB of 45 MB · 2.1 MB/s ·
/// 14 s left", "Joining picture and sound", "Done · 45 MB", the failure...
pub fn line(job: &Job) -> String {
    let _ = job;
    todo!("dl-jobs")
}

/// The link in `text` (the clipboard, a paste) when it is one: a single
/// http(s) URL, trimmed. None for anything else.
pub fn link_in(text: &str) -> Option<String> {
    let _ = text;
    todo!("dl-jobs")
}
