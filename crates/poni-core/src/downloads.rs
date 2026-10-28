//! The Downloads page's list (`addons::DOWNLOADER`): each link the person
//! gave the downloader, where it stands, the line under it and what the
//! island's pill shows while something downloads. yt-dlp's lines
//! (`ytdlp::Line`) move a job along; at most [`AT_ONCE`] run together, the
//! rest wait their turn. Kept in memory: finished files also go to the Shelf.

use crate::phonedrop::fmt_bytes;
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
    /// All its parts together ([`Jobs::apply`]); `speed` and `eta_s` are the
    /// part coming in now.
    Downloading { done: u64, total: Option<u64>, speed: Option<f64>, eta_s: Option<u64> },
    /// The bytes are in; yt-dlp is joining or tidying the file.
    Finishing(Stage),
    /// `size` is 0 until the app reads it from the file ([`Jobs::set_size`]).
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

    /// Its yt-dlp runs: starting, downloading or finishing.
    fn running(&self) -> bool {
        matches!(self, State::Starting | State::Downloading { .. } | State::Finishing(_))
    }
}

/// What the closed island's pill shows while downloads run.
#[derive(Clone, Debug, PartialEq)]
pub struct Pill {
    /// Of everything running, by bytes (None while no size is known yet).
    pub fraction: Option<f32>,
    /// Downloads not finished yet, queued ones included.
    pub count: usize,
    /// The running download that started first (else the next to start):
    /// its title, or its link's host until yt-dlp gives one.
    pub title: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Jobs {
    /// Newest first.
    pub list: Vec<Job>,
    next_id: u64,
    /// The bytes of each job yt-dlp is downloading.
    parts: Vec<Parts>,
}

/// One job's bytes. yt-dlp fetches a video's picture and then its sound,
/// each counted from 0, so the page adds the parts up and its bar never goes
/// back when the sound starts.
#[derive(Clone, Debug, Default, PartialEq)]
struct Parts {
    id: u64,
    /// The parts already in.
    before: u64,
    /// The part coming in now, as yt-dlp last gave it.
    done: u64,
    total: Option<u64>,
    /// The fullest the bar has been, 0..1 (None until a size is known).
    bar: Option<f32>,
}

impl Parts {
    /// Every part's bytes, the one coming in counted whole when its size is known.
    fn size(&self) -> u64 {
        self.before + self.done.max(self.total.unwrap_or(0))
    }
}

impl Jobs {
    /// A new job for `url`, queued; returns its id (from 1, never used twice).
    pub fn add(&mut self, url: String, mode: Mode, now: i64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.list.insert(0, Job { id, url, mode, title: None, site: None, thumbnail: None, state: State::Queued, added: now });
        id
    }

    pub fn get(&self, id: u64) -> Option<&Job> {
        self.list.iter().find(|j| j.id == id)
    }

    /// One of yt-dlp's lines for job `id`. Lines for a job that ended
    /// already (stopped while yt-dlp was still talking) change nothing.
    pub fn apply(&mut self, id: u64, line: Line) {
        let Some(job) = self.list.iter_mut().find(|j| j.id == id && j.state.live()) else { return };
        match line {
            Line::Info { title, site, thumbnail, .. } => {
                job.title = Some(title).filter(|t| !t.is_empty());
                job.site = Some(site).filter(|s| !s.is_empty());
                job.thumbnail = thumbnail;
                if job.state == State::Queued {
                    job.state = State::Starting;
                }
            }
            Line::Progress { done, total, speed, eta_s } => {
                // 0 bytes is no size: yt-dlp doesn't know it yet.
                let total = total.filter(|t| *t > 0);
                let i = self.parts.iter().position(|p| p.id == id).unwrap_or_else(|| {
                    self.parts.push(Parts { id, ..Parts::default() });
                    self.parts.len() - 1
                });
                let p = &mut self.parts[i];
                // Back to fewer bytes: the next part. The same size again is
                // this part downloading again from its start (it couldn't resume).
                if done < p.done && (total.is_none() || total != p.total) {
                    p.before = p.size();
                }
                p.done = done;
                p.total = total;
                let total = total.map(|t| p.before + t.max(done));
                if let Some(t) = total {
                    let now = ((p.before + done) as f64 / t as f64) as f32;
                    p.bar = Some(p.bar.map_or(now, |bar| bar.max(now)));
                }
                job.state = State::Downloading { done: p.before + done, total, speed, eta_s };
            }
            Line::Stage(stage) => job.state = State::Finishing(stage),
            Line::Done { path } => {
                job.state = State::Done { path, size: 0 };
                self.parts.retain(|p| p.id != id);
            }
            Line::Failed(problem) => {
                job.state = State::Failed(problem.message());
                self.parts.retain(|p| p.id != id);
            }
        }
    }

    /// yt-dlp for job `id` ended (`ok`: exit code 0). A job it left
    /// unfinished failed: without the finished file's path there is nothing
    /// to open, even after a clean exit.
    pub fn ended(&mut self, id: u64, ok: bool) {
        let why = if ok { "It stopped without a file" } else { "The download stopped" };
        self.end(id, State::Failed(why.to_string()));
    }

    /// The person stopped job `id` (one that ended already stays as it
    /// ended). One still queued never started: its Cancel takes it off the
    /// list, as there is nothing of it to show.
    pub fn stop(&mut self, id: u64) {
        self.list.retain(|j| j.id != id || j.state != State::Queued);
        self.end(id, State::Stopped);
    }

    fn end(&mut self, id: u64, state: State) {
        if let Some(job) = self.list.iter_mut().find(|j| j.id == id && j.state.live()) {
            job.state = state;
        }
        self.parts.retain(|p| p.id != id);
    }

    /// The finished file's size, which the app reads from the file.
    pub fn set_size(&mut self, id: u64, size: u64) {
        if let Some(Job { state: State::Done { size: s, .. }, .. }) = self.list.iter_mut().find(|j| j.id == id) {
            *s = size;
        }
    }

    /// Take job `id` off the list (a finished, failed or stopped one).
    pub fn remove(&mut self, id: u64) {
        self.list.retain(|j| j.id != id || j.state.live());
    }

    /// Take every finished, failed and stopped job off the list.
    pub fn clear_done(&mut self) {
        self.list.retain(|j| j.state.live());
    }

    /// The queued jobs that may start now, oldest first, given those running.
    /// The caller starts them and marks them `Starting` ([`Jobs::starting`]).
    pub fn to_start(&self) -> Vec<u64> {
        let free = AT_ONCE.saturating_sub(self.running());
        self.list.iter().rev().filter(|j| j.state == State::Queued).take(free).map(|j| j.id).collect()
    }

    /// Only a queued job starts: one stopped meanwhile stays stopped.
    pub fn starting(&mut self, id: u64) {
        if let Some(job) = self.list.iter_mut().find(|j| j.id == id && j.state == State::Queued) {
            job.state = State::Starting;
        }
    }

    /// Jobs whose yt-dlp runs now (starting, downloading or finishing).
    pub fn running(&self) -> usize {
        self.list.iter().filter(|j| j.state.running()).count()
    }

    /// Anything left to finish (the pill shows while this is true).
    pub fn busy(&self) -> bool {
        self.list.iter().any(|j| j.state.live())
    }

    /// The bar under job `id`, 0..1, all its parts together. While it
    /// downloads it never goes back, not even when the sound starts from 0
    /// after the picture; full while finishing. None until a size is known,
    /// and once it ended.
    pub fn progress(&self, id: u64) -> Option<f32> {
        self.get(id).and_then(|j| self.bar(j))
    }

    fn bar(&self, job: &Job) -> Option<f32> {
        match job.state {
            State::Downloading { .. } => self.parts.iter().find(|p| p.id == job.id)?.bar,
            State::Finishing(_) => Some(1.0),
            _ => None,
        }
    }

    /// What the pill shows; None once nothing is left to finish.
    pub fn pill(&self) -> Option<Pill> {
        let live = || self.list.iter().rev().filter(|j| j.state.live());
        let first = live().find(|j| j.state.running()).or_else(|| live().next())?;
        let (mut got, mut of) = (0.0, 0.0);
        for j in live().filter(|j| j.state.running()) {
            if let (Some(bar), Some(p)) = (self.bar(j), self.parts.iter().find(|p| p.id == j.id)) {
                got += f64::from(bar) * p.size() as f64;
                of += p.size() as f64;
            }
        }
        let title = match &first.title {
            Some(t) => t.clone(),
            None => host(&first.url).unwrap_or(&first.url).to_string(),
        };
        Some(Pill { fraction: (of > 0.0).then(|| (got / of) as f32), count: live().count(), title })
    }
}

/// How far, in whole percents rounded to the nearest (36.9% is 37), for the
/// island's pill and the Downloads page's bars alike, so the two agree. 100
/// waits for the last byte: 99.6% is 99.
pub fn percent(fraction: f32) -> u32 {
    let p = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
    if fraction < 1.0 {
        p.min(99)
    } else {
        p
    }
}

/// The line under a job on the Downloads page: "12 MB of 45 MB · 2.1 MB/s ·
/// 14 s left", "Joining picture and sound", "Done · 45 MB", the failure...
pub fn line(job: &Job) -> String {
    match &job.state {
        State::Queued => "Waiting".to_string(),
        State::Starting => "Starting".to_string(),
        State::Downloading { done, total, speed, eta_s } => {
            let mut bits = vec![match total {
                Some(total) => format!("{} of {}", fmt_bytes(*done), fmt_bytes(*total)),
                None if *done == 0 => "Downloading".to_string(),
                None => fmt_bytes(*done),
            }];
            // Under a byte a second reads as nothing.
            if let Some(speed) = speed.filter(|s| *s >= 1.0) {
                bits.push(format!("{}/s", fmt_bytes(speed as u64)));
            }
            if let Some(eta) = eta_s {
                bits.push(time_left(*eta));
            }
            bits.join(" · ")
        }
        State::Finishing(Stage::Merging) => "Joining picture and sound".to_string(),
        State::Finishing(Stage::Converting) => "Converting".to_string(),
        State::Finishing(Stage::Fixing) => "Finishing".to_string(),
        State::Done { size: 0, .. } => "Done".to_string(),
        State::Done { size, .. } => format!("Done · {}", fmt_bytes(*size)),
        State::Failed(why) => why.clone(),
        State::Stopped => "Stopped".to_string(),
    }
}

/// "14 s left", "3 min left", "1 h 5 min left".
fn time_left(secs: u64) -> String {
    let min = secs / 60;
    match (min / 60, min % 60) {
        (0, 0) => format!("{secs} s left"),
        (0, m) => format!("{m} min left"),
        (h, 0) => format!("{h} h left"),
        (h, m) => format!("{h} h {m} min left"),
    }
}

/// The link in `text` (the clipboard, a paste) when it is one: a single
/// http(s) URL, trimmed. None for anything else.
///
/// One from a mail or a chat may come in <> or quotes; one that starts
/// "www." gets https://. Anything else needs its http:// or https://, and
/// nothing with a space in it or longer than 2,000 characters is one link.
pub fn link_in(text: &str) -> Option<String> {
    let mut t = text.trim();
    for (open, close) in [('<', '>'), ('"', '"'), ('\'', '\''), ('\u{201c}', '\u{201d}'), ('\u{2018}', '\u{2019}')] {
        if let Some(inner) = t.strip_prefix(open).and_then(|t| t.strip_suffix(close)) {
            t = inner.trim();
            break;
        }
    }
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let link = if let Some(rest) = strip_any_case(t, "https://") {
        format!("https://{rest}")
    } else if let Some(rest) = strip_any_case(t, "http://") {
        format!("http://{rest}")
    } else if strip_any_case(t, "www.").is_some() {
        format!("https://{t}")
    } else {
        return None;
    };
    (link.chars().count() <= 2000 && host(&link).is_some()).then_some(link)
}

/// `s` after `prefix`, which may be in any case ("HTTPS://").
fn strip_any_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|head| head.eq_ignore_ascii_case(prefix)).map(|_| &s[prefix.len()..])
}

/// A link's host ("https://www.example.com/a" gives "www.example.com"),
/// without a name or password written into it.
fn host(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ytdlp::Problem;

    const SPRITE_FRIGHT: &str = "https://video.blender.org/w/b29290cc-dc51-4a12-bcb2-2aa5fece7605";
    const BUNNY: &str = "https://archive.org/details/BigBuckBunny_124";
    const FLICKERMOOD: &str = "https://soundcloud.com/forss/flickermood";

    fn bytes(done: u64, total: Option<u64>) -> Line {
        Line::Progress { done, total, speed: None, eta_s: None }
    }

    fn info(title: &str) -> Line {
        Line::Info { title: title.into(), site: "PeerTube".into(), thumbnail: Some("https://video.blender.org/static/thumbnails/sprite.jpg".into()), duration_s: Some(629.0) }
    }

    fn line_of(jobs: &Jobs, id: u64) -> String {
        line(jobs.get(id).unwrap())
    }

    fn ids(jobs: &Jobs) -> Vec<u64> {
        jobs.list.iter().map(|j| j.id).collect()
    }

    #[test]
    fn downloads_wait_their_turn_two_at_a_time() {
        let mut jobs = Jobs::default();
        assert_eq!(jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 100), 1, "ids start at 1");
        assert_eq!(jobs.add(BUNNY.into(), Mode::Video, 101), 2);
        assert_eq!(jobs.add(FLICKERMOOD.into(), Mode::Audio, 102), 3);
        assert_eq!(ids(&jobs), [3, 2, 1], "newest first");
        let first = jobs.get(1).unwrap();
        assert_eq!((first.url.as_str(), first.mode, first.added, first.title.as_deref()), (SPRITE_FRIGHT, Mode::Video, 100, None));
        assert!(jobs.list.iter().all(|j| j.state == State::Queued && line(j) == "Waiting"));
        assert!(jobs.busy() && jobs.running() == 0);

        assert_eq!(jobs.to_start(), [1, 2], "the oldest two");
        jobs.starting(1);
        assert_eq!(jobs.to_start(), [2]);
        jobs.starting(2);
        assert_eq!((jobs.running(), jobs.to_start()), (2, vec![]));
        assert_eq!(line_of(&jobs, 1), "Starting");
        jobs.apply(1, Line::Done { path: "C:\\Users\\Luca\\Downloads\\Sprite Fright.mp4".into() });
        assert_eq!(jobs.to_start(), [3], "a place came free");
        jobs.starting(3);
        assert_eq!((jobs.running(), jobs.to_start()), (2, vec![]));
        // Only a queued job starts.
        jobs.starting(1);
        assert!(matches!(jobs.get(1).unwrap().state, State::Done { .. }));
        jobs.starting(42);
        assert_eq!(jobs.list.len(), 3);
    }

    #[test]
    fn the_first_line_names_the_download() {
        let mut jobs = Jobs::default();
        let id = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        jobs.apply(id, info("Sprite Fright - Blender Open Movie"));
        let job = jobs.get(id).unwrap();
        assert_eq!(job.title.as_deref(), Some("Sprite Fright - Blender Open Movie"));
        assert_eq!(job.site.as_deref(), Some("PeerTube"));
        assert_eq!(job.thumbnail.as_deref(), Some("https://video.blender.org/static/thumbnails/sprite.jpg"));
        assert_eq!(job.state, State::Starting, "queued, now starting");
        jobs.apply(id, bytes(1_024, Some(2_048)));
        jobs.apply(id, info("Sprite Fright"));
        assert_eq!(jobs.get(id).unwrap().title.as_deref(), Some("Sprite Fright"));
        assert!(matches!(jobs.get(id).unwrap().state, State::Downloading { .. }), "it doesn't go back to starting");
        // An empty title or site isn't one.
        let other = jobs.add(BUNNY.into(), Mode::Video, 0);
        jobs.starting(other);
        jobs.apply(other, Line::Info { title: String::new(), site: String::new(), thumbnail: None, duration_s: None });
        let job = jobs.get(other).unwrap();
        assert_eq!((job.title.as_deref(), job.site.as_deref(), &job.state), (None, None, &State::Starting));
    }

    #[test]
    fn lines_say_where_a_download_stands() {
        let job = |state| Job { id: 1, url: SPRITE_FRIGHT.into(), mode: Mode::Video, title: None, site: None, thumbnail: None, state, added: 0 };
        let at = |done, total, speed, eta_s| line(&job(State::Downloading { done, total, speed, eta_s }));
        assert_eq!(line(&job(State::Queued)), "Waiting");
        assert_eq!(line(&job(State::Starting)), "Starting");
        assert_eq!(at(12_300_000, Some(45_000_000), Some(2_100_000.0), Some(14)), "12 MB of 45 MB · 2.1 MB/s · 14 s left");
        assert_eq!(at(820_400, Some(4_120_000), Some(850_000.0), Some(200)), "820 KB of 4.1 MB · 850 KB/s · 3 min left");
        assert_eq!(at(1_260_000_000, Some(4_000_000_000), Some(512.0), Some(3_900)), "1.3 GB of 4.0 GB · 512 bytes/s · 1 h 5 min left");
        assert_eq!(at(0, Some(45_000_000), None, None), "0 bytes of 45 MB");
        // The size not known (yet).
        assert_eq!(at(4_120_000, None, Some(2_100_000.0), None), "4.1 MB · 2.1 MB/s");
        assert_eq!(at(0, None, None, None), "Downloading");
        assert_eq!(at(1, None, Some(0.4), None), "1 byte", "under a byte a second: no speed");
        // Time left.
        assert_eq!(at(1, Some(2), None, Some(0)), "1 byte of 2 bytes · 0 s left");
        assert_eq!(at(1, Some(2), None, Some(59)), "1 byte of 2 bytes · 59 s left");
        assert_eq!(at(1, Some(2), None, Some(60)), "1 byte of 2 bytes · 1 min left");
        assert_eq!(at(1, Some(2), None, Some(3_599)), "1 byte of 2 bytes · 59 min left");
        assert_eq!(at(1, Some(2), None, Some(3_600)), "1 byte of 2 bytes · 1 h left");
        assert_eq!(at(1, Some(2), None, Some(93_780)), "1 byte of 2 bytes · 26 h 3 min left");

        assert_eq!(line(&job(State::Finishing(Stage::Merging))), "Joining picture and sound");
        assert_eq!(line(&job(State::Finishing(Stage::Converting))), "Converting");
        assert_eq!(line(&job(State::Finishing(Stage::Fixing))), "Finishing");
        assert_eq!(line(&job(State::Done { path: "C:\\x.mp4".into(), size: 45_000_000 })), "Done · 45 MB");
        assert_eq!(line(&job(State::Done { path: "C:\\x.mp4".into(), size: 0 })), "Done", "the size not read yet");
        assert_eq!(line(&job(State::Failed("This video is private".into()))), "This video is private");
        assert_eq!(line(&job(State::Stopped)), "Stopped");
    }

    #[test]
    fn picture_then_sound_add_up_and_the_bar_never_goes_back() {
        let mut jobs = Jobs::default();
        let id = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        jobs.starting(id);
        assert_eq!(jobs.progress(id), None, "no size yet");
        // The picture: 32 MB.
        jobs.apply(id, bytes(8_000_000, Some(32_000_000)));
        assert_eq!(jobs.progress(id), Some(0.25));
        jobs.apply(id, bytes(32_000_000, Some(32_000_000)));
        assert_eq!(jobs.progress(id), Some(1.0));
        // Then the sound, from 0: 8 MB more.
        jobs.apply(id, Line::Progress { done: 1_024, total: Some(8_000_000), speed: Some(850_000.0), eta_s: Some(9) });
        let state = State::Downloading { done: 32_001_024, total: Some(40_000_000), speed: Some(850_000.0), eta_s: Some(9) };
        assert_eq!(jobs.get(id).unwrap().state, state);
        assert_eq!(line_of(&jobs, id), "32 MB of 40 MB · 850 KB/s · 9 s left");
        assert_eq!(jobs.progress(id), Some(1.0), "the bar stays where the picture left it");
        jobs.apply(id, bytes(8_000_000, Some(8_000_000)));
        assert_eq!(line_of(&jobs, id), "40 MB of 40 MB");
        jobs.apply(id, Line::Stage(Stage::Merging));
        assert_eq!(jobs.get(id).unwrap().state, State::Finishing(Stage::Merging));
        assert_eq!(line_of(&jobs, id), "Joining picture and sound");
        assert_eq!(jobs.progress(id), Some(1.0));
        jobs.apply(id, Line::Stage(Stage::Fixing));
        assert_eq!(line_of(&jobs, id), "Finishing");
    }

    #[test]
    fn a_part_left_short_of_its_size_counts_whole() {
        // yt-dlp's last word on the picture was 24 of 32 MB, then the sound began.
        let mut jobs = Jobs::default();
        let id = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        jobs.apply(id, bytes(24_000_000, Some(32_000_000)));
        assert_eq!(jobs.progress(id), Some(0.75));
        jobs.apply(id, bytes(0, Some(8_000_000)));
        assert_eq!(jobs.get(id).unwrap().state, State::Downloading { done: 32_000_000, total: Some(40_000_000), speed: None, eta_s: None });
        assert_eq!(jobs.progress(id), Some(0.8));
        // A part whose size isn't known: the bytes so far, the bar where it was.
        jobs.apply(id, bytes(8_000_000, Some(8_000_000)));
        jobs.apply(id, bytes(300_000, None));
        assert_eq!(jobs.get(id).unwrap().state, State::Downloading { done: 40_300_000, total: None, speed: None, eta_s: None });
        assert_eq!(line_of(&jobs, id), "40 MB");
        assert_eq!(jobs.progress(id), Some(1.0));
        assert_eq!(jobs.pill().unwrap().fraction, Some(1.0));
    }

    #[test]
    fn the_same_size_again_is_the_same_part_starting_over() {
        // yt-dlp couldn't resume, so it downloads the file again from its start.
        let mut jobs = Jobs::default();
        let id = jobs.add(BUNNY.into(), Mode::Video, 0);
        jobs.apply(id, bytes(16_000_000, Some(32_000_000)));
        jobs.apply(id, bytes(1_024, Some(32_000_000)));
        assert_eq!(jobs.get(id).unwrap().state, State::Downloading { done: 1_024, total: Some(32_000_000), speed: None, eta_s: None });
        assert_eq!(line_of(&jobs, id), "1 KB of 32 MB");
        assert_eq!(jobs.progress(id), Some(0.5), "the bar waits where it was");
        jobs.apply(id, bytes(24_000_000, Some(32_000_000)));
        assert_eq!(jobs.progress(id), Some(0.75));
    }

    #[test]
    fn sizes_yt_dlp_doesnt_know() {
        let mut jobs = Jobs::default();
        let id = jobs.add(FLICKERMOOD.into(), Mode::Audio, 0);
        jobs.starting(id);
        jobs.apply(id, bytes(0, None));
        assert_eq!(line_of(&jobs, id), "Downloading");
        jobs.apply(id, Line::Progress { done: 4_120_000, total: None, speed: Some(2_100_000.0), eta_s: None });
        assert_eq!(line_of(&jobs, id), "4.1 MB · 2.1 MB/s");
        assert_eq!(jobs.progress(id), None);
        // 0 is yt-dlp's "not known", not a size.
        jobs.apply(id, bytes(4_200_000, Some(0)));
        assert_eq!(jobs.get(id).unwrap().state, State::Downloading { done: 4_200_000, total: None, speed: None, eta_s: None });
        assert_eq!(jobs.pill(), Some(Pill { fraction: None, count: 1, title: "soundcloud.com".into() }));
        // A size smaller than the bytes in (a guess yt-dlp made) grows to them.
        jobs.apply(id, bytes(5_000_000, Some(4_500_000)));
        assert_eq!(line_of(&jobs, id), "5.0 MB of 5.0 MB");
        assert_eq!(jobs.progress(id), Some(1.0));
        jobs.apply(id, Line::Stage(Stage::Converting));
        assert_eq!(line_of(&jobs, id), "Converting");
    }

    #[test]
    fn a_finished_download_shows_its_files_size_once_the_app_reads_it() {
        let mut jobs = Jobs::default();
        let id = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        jobs.starting(id);
        jobs.apply(id, bytes(45_000_000, Some(45_000_000)));
        jobs.apply(id, Line::Done { path: "C:\\Users\\Luca\\Downloads\\Sprite Fright.mp4".into() });
        assert_eq!(jobs.get(id).unwrap().state, State::Done { path: "C:\\Users\\Luca\\Downloads\\Sprite Fright.mp4".into(), size: 0 });
        assert_eq!(line_of(&jobs, id), "Done");
        jobs.set_size(id, 47_900_000);
        assert_eq!(line_of(&jobs, id), "Done · 48 MB");
        assert_eq!((jobs.progress(id), jobs.running(), jobs.busy()), (None, 0, false));
        // yt-dlp's exit after it, and anything it still says, change nothing.
        jobs.ended(id, true);
        jobs.ended(id, false);
        jobs.apply(id, bytes(1, Some(2)));
        jobs.apply(id, Line::Done { path: "C:\\elsewhere.mp4".into() });
        jobs.apply(id, Line::Failed(Problem::Other("unable to write the thumbnail".into())));
        jobs.stop(id);
        assert_eq!(line_of(&jobs, id), "Done · 48 MB");
        // Only a finished download has a file to measure.
        let other = jobs.add(BUNNY.into(), Mode::Video, 0);
        jobs.set_size(other, 1);
        assert_eq!(jobs.get(other).unwrap().state, State::Queued);
        assert!(jobs.parts.is_empty(), "nothing kept for jobs that ended");
    }

    #[test]
    fn a_download_that_ends_without_its_file_failed() {
        let mut jobs = Jobs::default();
        let (clean, crashed, queued) = (jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0), jobs.add(BUNNY.into(), Mode::Video, 0), jobs.add(FLICKERMOOD.into(), Mode::Audio, 0));
        jobs.starting(clean);
        jobs.starting(crashed);
        jobs.apply(clean, Line::Stage(Stage::Merging));
        jobs.ended(clean, true);
        assert_eq!(line_of(&jobs, clean), "It stopped without a file", "exit code 0, but no file");
        jobs.apply(crashed, bytes(1_000_000, Some(2_000_000)));
        jobs.ended(crashed, false);
        assert_eq!(jobs.get(crashed).unwrap().state, State::Failed("The download stopped".into()));
        assert_eq!(jobs.progress(crashed), None);
        // Ending twice keeps the first reason.
        jobs.ended(clean, false);
        assert_eq!(line_of(&jobs, clean), "It stopped without a file");
        jobs.ended(queued, false);
        assert_eq!(line_of(&jobs, queued), "The download stopped");
        jobs.ended(42, true);
        assert!(!jobs.busy() && jobs.pill().is_none() && jobs.parts.is_empty());
    }

    #[test]
    fn a_failure_says_what_went_wrong() {
        let mut jobs = Jobs::default();
        let id = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        jobs.starting(id);
        jobs.apply(id, bytes(1_000_000, Some(2_000_000)));
        jobs.apply(id, Line::Failed(Problem::Private));
        assert_eq!(jobs.get(id).unwrap().state, State::Failed(Problem::Private.message()));
        assert_eq!(line_of(&jobs, id), Problem::Private.message());
        assert_eq!(jobs.progress(id), None);
        jobs.ended(id, false);
        assert_eq!(line_of(&jobs, id), Problem::Private.message(), "yt-dlp's exit keeps the reason");
        assert!(jobs.parts.is_empty());
    }

    #[test]
    fn a_stopped_download_stays_stopped_and_a_cancelled_one_goes() {
        let mut jobs = Jobs::default();
        let running = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        let queued = jobs.add(BUNNY.into(), Mode::Video, 0);
        jobs.starting(running);
        jobs.apply(running, bytes(5_000_000, Some(10_000_000)));
        jobs.stop(running);
        assert_eq!(line_of(&jobs, running), "Stopped");
        // Cancel: the queued one never started, so it leaves the list.
        jobs.stop(queued);
        assert_eq!(ids(&jobs), [running]);
        assert_eq!(jobs.to_start(), Vec::<u64>::new(), "a stopped job never starts");
        jobs.starting(queued);
        // yt-dlp's last words before it went, and its exit, change nothing.
        jobs.apply(running, info("Sprite Fright"));
        jobs.apply(running, bytes(6_000_000, Some(10_000_000)));
        jobs.apply(running, Line::Stage(Stage::Merging));
        jobs.apply(running, Line::Done { path: "C:\\Users\\Luca\\Downloads\\Sprite Fright.mp4".into() });
        jobs.apply(running, Line::Failed(Problem::Network));
        jobs.ended(running, false);
        assert_eq!(jobs.get(running).unwrap().state, State::Stopped);
        assert_eq!(jobs.get(running).unwrap().title, None);
        assert_eq!(jobs.get(queued), None);
        assert_eq!((jobs.progress(running), jobs.running(), jobs.busy()), (None, 0, false));
        assert!(jobs.parts.is_empty());
    }

    #[test]
    fn only_ended_downloads_leave_the_list() {
        let mut jobs = Jobs::default();
        let done = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        jobs.apply(done, Line::Done { path: "C:\\Users\\Luca\\Downloads\\Sprite Fright.mp4".into() });
        let failed = jobs.add(BUNNY.into(), Mode::Video, 0);
        jobs.ended(failed, false);
        let stopped = jobs.add(FLICKERMOOD.into(), Mode::Audio, 0);
        jobs.starting(stopped);
        jobs.stop(stopped);
        let running = jobs.add(SPRITE_FRIGHT.into(), Mode::Audio, 0);
        jobs.starting(running);
        let queued = jobs.add(BUNNY.into(), Mode::Audio, 0);
        jobs.remove(running);
        jobs.remove(queued);
        jobs.remove(42);
        assert_eq!(ids(&jobs), [queued, running, stopped, failed, done], "nothing live goes");
        jobs.remove(done);
        assert_eq!(ids(&jobs), [queued, running, stopped, failed]);
        jobs.clear_done();
        assert_eq!(ids(&jobs), [queued, running]);
        assert_eq!(jobs.add(FLICKERMOOD.into(), Mode::Audio, 0), 6, "an id is never used twice");
    }

    #[test]
    fn the_pill_follows_whats_running() {
        let mut jobs = Jobs::default();
        assert_eq!(jobs.pill(), None, "nothing to show at rest");
        let a = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        let b = jobs.add("https://luca:secret@archive.org/details/BigBuckBunny_124".into(), Mode::Video, 1);
        let c = jobs.add(FLICKERMOOD.into(), Mode::Audio, 2);
        let pill = |fraction, count: usize, title: &str| Some(Pill { fraction, count, title: title.into() });
        assert_eq!(jobs.pill(), pill(None, 3, "video.blender.org"), "the next to start, by its host");
        for id in jobs.to_start() {
            jobs.starting(id);
        }
        assert_eq!(jobs.pill(), pill(None, 3, "video.blender.org"));
        jobs.apply(a, info("Sprite Fright"));
        assert_eq!(jobs.pill(), pill(None, 3, "Sprite Fright"));
        jobs.apply(a, bytes(10_000_000, Some(40_000_000)));
        assert_eq!(jobs.pill(), pill(Some(0.25), 3, "Sprite Fright"), "only the one with a size counts");
        jobs.apply(b, bytes(30_000_000, Some(60_000_000)));
        assert_eq!(jobs.pill(), pill(Some(0.4), 3, "Sprite Fright"), "by bytes: 40 of 100 MB");
        jobs.apply(a, bytes(40_000_000, Some(40_000_000)));
        jobs.apply(a, Line::Stage(Stage::Merging));
        assert_eq!(jobs.pill(), pill(Some(0.7), 3, "Sprite Fright"), "a finishing download counts whole");
        jobs.apply(a, Line::Done { path: "C:\\Users\\Luca\\Downloads\\Sprite Fright.mp4".into() });
        assert_eq!(jobs.pill(), pill(Some(0.5), 2, "archive.org"), "never the name and password in a link");
        jobs.ended(b, false);
        jobs.stop(c);
        assert_eq!(jobs.pill(), None);
    }

    #[test]
    fn the_pill_and_the_bar_say_the_same_whole_percent() {
        for (f, p) in [(0.0, 0), (0.004, 0), (0.29, 29), (0.369, 37), (0.5, 50), (0.994, 99), (1.0, 100), (1.5, 100), (-0.2, 0), (f32::NAN, 0)] {
            assert_eq!(percent(f), p, "{f}");
        }
        assert_eq!(percent(0.996), 99, "100 waits for the last byte");
        // 8.6 of 23.3 MB, 36.9%: 37 on the island's pill and on the page's bar.
        let mut jobs = Jobs::default();
        let id = jobs.add(SPRITE_FRIGHT.into(), Mode::Video, 0);
        jobs.starting(id);
        jobs.apply(id, bytes(8_600_000, Some(23_300_000)));
        let (pill, bar) = (jobs.pill().unwrap().fraction.unwrap(), jobs.progress(id).unwrap());
        assert_eq!((percent(pill), percent(bar)), (37, 37));
    }

    #[test]
    fn a_link_is_one_web_address() {
        let ok = SPRITE_FRIGHT;
        assert_eq!(link_in(ok).as_deref(), Some(ok));
        assert_eq!(link_in(&format!("  {ok}\r\n")).as_deref(), Some(ok), "trimmed");
        for wrapped in [format!("<{ok}>"), format!("\"{ok}\""), format!("'{ok}'"), format!("\u{201c}{ok}\u{201d}"), format!("\u{2018}{ok}\u{2019}"), format!(" < {ok} > ")] {
            assert_eq!(link_in(&wrapped).as_deref(), Some(ok), "{wrapped}");
        }
        assert_eq!(link_in("http://archive.org/details/BigBuckBunny_124").as_deref(), Some("http://archive.org/details/BigBuckBunny_124"));
        assert_eq!(link_in("HTTPS://SoundCloud.com/forss/flickermood").as_deref(), Some("https://SoundCloud.com/forss/flickermood"), "the scheme in any case");
        assert_eq!(link_in("www.archive.org/details/BigBuckBunny_124").as_deref(), Some("https://www.archive.org/details/BigBuckBunny_124"));
        assert_eq!(link_in("WWW.archive.org").as_deref(), Some("https://WWW.archive.org"));
        let refused = [
            "",
            "  \n ",
            "soundcloud.com/forss/flickermood",
            "youtu.be/aqz-KE-bpKQ",
            "//archive.org/details/BigBuckBunny_124",
            "ftp://archive.org/details/BigBuckBunny_124",
            "file:///C:/Users/Luca/Videos/a.mp4",
            "javascript:alert(1)",
            "mailto:hello@oyenza.com",
            "https://",
            "https:///details",
            "https://?q=1",
            "https://luca@/details",
            "https:/archive.org",
            "https://archive.org/details/Big Buck Bunny",
            "https://archive.org/a\thttps://archive.org/b",
            "https://archive.org/a https://archive.org/b",
            "watch this https://archive.org/details/BigBuckBunny_124",
            "<https://archive.org/details/BigBuckBunny_124\"",
            "\"",
            "https://archive.org/\u{0}",
        ];
        for text in refused {
            assert_eq!(link_in(text), None, "{text:?}");
        }
        let longest = format!("https://archive.org/{}", "a".repeat(1_980));
        assert_eq!(longest.len(), 2_000);
        assert_eq!(link_in(&longest).as_deref(), Some(longest.as_str()));
        assert_eq!(link_in(&format!("{longest}a")), None, "over 2,000 characters");
        // Characters, not bytes.
        let wiki = format!("https://ja.wikipedia.org/wiki/{}", "\u{65e5}".repeat(1_970));
        assert_eq!(link_in(&wiki).as_deref(), Some(wiki.as_str()));
    }
}
