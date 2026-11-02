//! Synced lyrics from LRCLIB (https://lrclib.net), and from Apple Music when
//! the user saved their Apple Music token, with a disk cache.
//!
//! LRCLIB is a free, open lyrics database (no key). One worker thread does
//! all the work; [`LyricsService::request`] only swaps the queued job and
//! returns. The latest request wins: a newer track replaces any queued job,
//! aborts the running lookup at its next step, and results for a track that
//! is no longer the newest are dropped.
//!
//! Lookup for a track:
//! 1. clean the metadata (`lrclib::normalize_query`);
//! 2. with an Apple Music token: the cache `lyrics\am-<cache_key>.json`, then
//!    Apple's own synced lyrics (`services::applemusic`); anything short of
//!    synced lyrics from Apple falls through to LRCLIB;
//! 3. the disk cache `lyrics\<cache_key>.json` (hits, and misses for 3 days
//!    unless a better lookup has shipped since: `CacheEntry::LOOKUP`);
//! 4. `/api/get` with title, artist, album and duration;
//! 5. `/api/get` without the album;
//! 6. `/api/search` by title + primary artist, then `lrclib::best_match`
//!    (also after a 5xx from `/api/get`; a miss then is retried, not cached);
//! 7. for a browser tab, `/api/search` again reading the title as
//!    "Artist - Song", then as "Song - Artist" (`lrclib::title_guesses`).
//!    A 5xx from one search doesn't stop the next.
//!
//! A plain-only answer doesn't end steps 4-7: it is kept while the later
//! steps look for a synced copy of the song.
//!
//! Synced lyrics whose recording is much longer or shorter than what plays
//! (a music video's intro, a radio edit) are shown unsynced.
//!
//! Politeness: at least 1 s between requests, `429 Retry-After` honoured for
//! every later request, a descriptive User-Agent, and at most a couple of
//! delayed retries after network errors.

use crate::events::{AppEvent, AppleMusicStatus, EventSink, LyricsResult};
use crate::services::applemusic::{AppleMusic, Check, Miss};
use crate::services::http::{self, Client, FetchError, MtaGuard};
use crate::state::MediaSession;
use poni_core::lrclib::{self, LrclibRecord, TrackQuery};
use poni_core::nowplaying;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pure::{duration_secs, is_fresh, lyrics_for, prune_victims, retry_delay, usable, CacheEntry};

// LRCLIB asks apps to say who they are.
const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");
const SOURCE: &str = "LRCLIB";
const APPLE_SOURCE: &str = "Apple Music";
/// Minimum gap between two requests to LRCLIB.
const MIN_GAP: Duration = Duration::from_secs(1);
/// 429 without a usable `Retry-After`.
const DEFAULT_BACKOFF: Duration = Duration::from_secs(60);
/// Upper bound for any server-requested back-off.
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// Waits up to this long happen inline; longer ones fail the lookup now and
/// retry it later, so the UI isn't left on "loading".
const MAX_INLINE_WAIT: Duration = Duration::from_secs(10);
/// Delayed retries after transient failures (while the track is still current).
const MAX_RETRIES: u32 = 2;
/// Cache size bounds: prune to `CACHE_KEEP` files once over `CACHE_MAX`.
const CACHE_MAX: usize = 2000;
const CACHE_KEEP: usize = 1600;
/// Re-check the cache size after this many writes (long-running sessions).
const PRUNE_EVERY_WRITES: u32 = 200;

const MSG_BUSY: &str = "LRCLIB is busy right now";
const MSG_TIMEOUT: &str = "LRCLIB didn't answer in time";
const MSG_UNREACHABLE: &str = "Can't reach LRCLIB";
/// "LRCLIB error 503": its HTTP status follows.
const MSG_HTTP: &str = "LRCLIB error";

/// A lookup that failed with `msg` failed on LRCLIB's side: it was busy,
/// didn't answer in time, couldn't be reached or had a server error. The
/// service asks again later on its own; nothing was wrong with the song or
/// with this copy.
#[cfg_attr(windows, allow(dead_code))] // only the Mac's smoke run asks (it looks a real song up)
pub fn lrclib_trouble(msg: &str) -> bool {
    [MSG_BUSY, MSG_TIMEOUT, MSG_UNREACHABLE].contains(&msg) || msg.strip_prefix(MSG_HTTP).and_then(|s| s.trim().parse::<u16>().ok()).is_some_and(|s| s >= 500)
}

pub struct LyricsService {
    shared: Arc<Shared>,
}

impl LyricsService {
    pub fn start(sink: EventSink) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("poni-lyrics".into())
            .spawn(move || worker_main(worker, sink))
            .map_err(|e| io::Error::other(format!("could not start the lyrics thread: {e}")))?;
        Ok(Self { shared })
    }

    /// Queue a lookup for `s` and return immediately (UI thread safe). The
    /// answer arrives as `AppEvent::Lyrics` with `s.track_key`.
    pub fn request(&self, s: &MediaSession) {
        let job = Job {
            track_key: s.track_key.clone(),
            query: TrackQuery {
                title: s.title.clone(),
                artist: s.artist.clone(),
                album: s.album.clone(),
                duration_s: duration_secs(s.timeline.duration_s),
            },
            browser: nowplaying::is_browser(&s.app_id),
            attempt: 0,
        };
        {
            let mut q = self.shared.lock();
            q.newest_key = job.track_key.clone();
            q.pending = Some(job);
            q.retry = None;
        }
        self.shared.cv.notify_all();
    }

    /// The Apple Music token on disk was saved or removed: the worker reloads
    /// it, checks it with Apple and reports `AppEvent::AppleMusic`.
    pub fn apple_token_changed(&self) {
        self.shared.lock().apple_changed = true;
        self.shared.cv.notify_all();
    }
}

impl Drop for LyricsService {
    fn drop(&mut self) {
        // The worker exits at its next wait; a request in flight finishes
        // (bounded by the HTTP timeout) on its own. Never join from the UI thread.
        self.shared.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

// ------------------------------------------------------------------ queue

#[derive(Clone, Debug)]
struct Job {
    track_key: String,
    query: TrackQuery,
    /// A browser tab: the title is a video title ("Artist - Song").
    browser: bool,
    /// 0 for a fresh request, n for the n-th delayed retry.
    attempt: u32,
}

/// What the worker should do next.
enum Work {
    Lookup(Job),
    /// Reload and check the Apple Music token.
    AppleToken,
}

#[derive(Default)]
struct Queue {
    /// The next job; a new request replaces it (latest wins).
    pending: Option<Job>,
    /// A delayed retry of the current track, run once its time comes.
    retry: Option<(Instant, Job)>,
    /// Track key of the newest request; anything else is stale.
    newest_key: String,
    /// The Apple Music token was saved or removed (handled before any job).
    apple_changed: bool,
    shutdown: bool,
}

#[derive(Default)]
struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
}

impl Shared {
    /// The queue holds plain data, so a panic elsewhere can't leave it
    /// inconsistent: recover from poisoning instead of panicking.
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.q.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn wait_for<'a>(&self, g: MutexGuard<'a, Queue>, timeout: Option<Duration>) -> MutexGuard<'a, Queue> {
        match timeout {
            Some(d) => match self.cv.wait_timeout(g, d) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            },
            None => self.cv.wait(g).unwrap_or_else(|p| p.into_inner()),
        }
    }
}

// ----------------------------------------------------------------- worker

/// Why a lookup stopped without an answer.
enum Halt {
    /// Superseded by a newer track, or shutting down: say nothing.
    Abandoned,
    /// Show `msg`; retry at `retry_at` while the track is still current.
    Failed { msg: String, retry_at: Option<Instant> },
}

enum Fetched {
    Found(LrclibRecord),
    Missing,
}

fn worker_main(shared: Arc<Shared>, sink: EventSink) {
    // Declared first so it is dropped last, after the worker's HttpClient.
    let _com = MtaGuard::new();
    let cache_dir = crate::paths::lyrics_cache_dir();
    prune_cache(&cache_dir);
    let apple = AppleMusic::new();
    let mut w = Worker { shared, sink, cache_dir, apple, client: None, last_request: None, blocked_until: None, writes: 0 };
    while let Some(work) = w.next_job() {
        match work {
            Work::Lookup(job) => w.process(job),
            Work::AppleToken => w.apple_token_changed(),
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    sink: EventSink,
    cache_dir: PathBuf,
    apple: AppleMusic,
    /// Created on first use; reused so connections are pooled.
    client: Option<Client>,
    /// When the last request to LRCLIB finished.
    last_request: Option<Instant>,
    /// No requests before this (server asked us to back off).
    blocked_until: Option<Instant>,
    /// Cache writes since the last prune.
    writes: u32,
}

impl Worker {
    /// Sleep until there is work: a changed Apple Music token first, then a
    /// queued job or a delayed retry that is due; `None` on shutdown.
    fn next_job(&self) -> Option<Work> {
        let mut q = self.shared.lock();
        loop {
            if q.shutdown {
                return None;
            }
            if std::mem::take(&mut q.apple_changed) {
                return Some(Work::AppleToken);
            }
            if let Some(job) = q.pending.take() {
                return Some(Work::Lookup(job));
            }
            let timeout = match &q.retry {
                Some((at, _)) => {
                    let now = Instant::now();
                    if *at <= now {
                        return q.retry.take().map(|(_, job)| Work::Lookup(job));
                    }
                    Some(*at - now)
                }
                None => None,
            };
            q = self.shared.wait_for(q, timeout);
        }
    }

    fn is_current(&self, job: &Job) -> bool {
        let q = self.shared.lock();
        !q.shutdown && q.newest_key == job.track_key
    }

    fn deliver(&self, job: &Job, result: LyricsResult) {
        if self.is_current(job) {
            self.sink.send(AppEvent::Lyrics { track_key: job.track_key.clone(), result });
        }
    }

    fn process(&mut self, job: Job) {
        match self.lookup(&job) {
            Ok(LyricsResult::Found { lyrics, source, .. }) => {
                // Readings for Japanese / Korean / Chinese lines, worked out
                // here so the UI thread never waits on the analyzer.
                let read = super::romanize::pronounce(&lyrics);
                self.deliver(&job, LyricsResult::Found { lyrics, source, ruby: read.ruby, wants_ja_readings: read.wants_ja_readings })
            }
            Ok(result) => self.deliver(&job, result),
            Err(Halt::Abandoned) => {}
            Err(Halt::Failed { msg, retry_at }) => {
                self.deliver(&job, LyricsResult::Error(msg));
                if let Some(at) = retry_at {
                    self.schedule_retry(job, at);
                }
            }
        }
    }

    fn schedule_retry(&self, job: Job, at: Instant) {
        if job.attempt >= MAX_RETRIES {
            return;
        }
        let mut q = self.shared.lock();
        if q.shutdown || q.newest_key != job.track_key || q.pending.is_some() {
            return;
        }
        q.retry = Some((at, Job { attempt: job.attempt + 1, ..job }));
        // The worker itself is the only waiter and it is not waiting right
        // now, so no notify is needed.
    }

    /// The user saved or removed their Apple Music token: reload it and tell
    /// the UI whether Apple accepts it.
    fn apple_token_changed(&mut self) {
        self.apple.reload_token();
        if !self.apple.has_token() {
            return;
        }
        let shared = self.shared.clone();
        let live = move || !shared.lock().shutdown;
        let status = match self.apple.check(&live) {
            Check::Accepted => AppleMusicStatus::Accepted,
            Check::Rejected => AppleMusicStatus::Rejected,
            Check::Unreachable => AppleMusicStatus::Unreachable,
        };
        self.sink.send(AppEvent::AppleMusic(status));
    }

    fn lookup(&mut self, job: &Job) -> Result<LyricsResult, Halt> {
        let q = lrclib::normalize_query(&job.query);
        if q.title.trim().is_empty() {
            // Nothing to search for; answer so the UI doesn't sit on "loading".
            return Ok(LyricsResult::NotFound);
        }
        if self.apple.enabled() {
            if let Some(result) = self.lookup_apple(job, &q)? {
                return Ok(result);
            }
        }
        self.lookup_lrclib(job, &q)
    }

    /// Apple Music's synced lyrics; `Ok(None)` hands the song to LRCLIB.
    fn lookup_apple(&mut self, job: &Job, q: &TrackQuery) -> Result<Option<LyricsResult>, Halt> {
        let key = format!("am-{}", lrclib::cache_key(q));
        let path = self.cache_dir.join(format!("{key}.json"));
        let cached = read_cache(&path);
        if let Some(entry) = &cached {
            if is_fresh(entry, crate::clock::unix_now()) {
                return Ok(entry.record.as_ref().map(|r| to_result(Some(r), job, APPLE_SOURCE)));
            }
        }
        let (shared, track_key) = (self.shared.clone(), job.track_key.clone());
        let live = move || {
            let q = shared.lock();
            !q.shutdown && q.newest_key == track_key
        };
        match self.apple.find(q, &live) {
            Ok(Some(record)) => {
                crate::pinfo!("lyrics {key}: synced");
                self.store(&path, &CacheEntry::new(Some(record.clone()), crate::clock::unix_now()));
                Ok(Some(to_result(Some(&record), job, APPLE_SOURCE)))
            }
            Ok(None) => {
                crate::pinfo!("lyrics {key}: no synced lyrics on Apple Music");
                self.store(&path, &CacheEntry::new(None, crate::clock::unix_now()));
                Ok(None)
            }
            Err(Miss::Abandoned) => Err(Halt::Abandoned),
            Err(miss) => {
                match miss {
                    Miss::Rejected => self.sink.send(AppEvent::AppleMusic(AppleMusicStatus::Rejected)),
                    Miss::NoAccess => self.sink.send(AppEvent::AppleMusic(AppleMusicStatus::NoAccess)),
                    Miss::Unreachable(why) => crate::pwarn!("lyrics: Apple Music unavailable ({why}); asking LRCLIB"),
                    Miss::Abandoned => {}
                }
                // An expired hit still beats asking LRCLIB.
                Ok(cached.as_ref().and_then(|e| e.record.as_ref()).map(|r| to_result(Some(r), job, APPLE_SOURCE)))
            }
        }
    }

    fn lookup_lrclib(&mut self, job: &Job, q: &TrackQuery) -> Result<LyricsResult, Halt> {
        let key = lrclib::cache_key(q);
        let path = self.cache_dir.join(format!("{key}.json"));
        let cached = read_cache(&path);
        if let Some(entry) = &cached {
            if is_fresh(entry, crate::clock::unix_now()) {
                return Ok(to_result(entry.record.as_ref(), job, SOURCE));
            }
        }
        match self.fetch(job, q) {
            Ok(Fetched::Found(record)) => {
                crate::pinfo!(
                    "lyrics {key}: {}",
                    if record.instrumental {
                        "instrumental"
                    } else if record.has_synced() {
                        "synced"
                    } else {
                        "plain"
                    }
                );
                self.store(&path, &CacheEntry::new(Some(record.clone()), crate::clock::unix_now()));
                Ok(to_result(Some(&record), job, SOURCE))
            }
            Ok(Fetched::Missing) => {
                crate::pinfo!("lyrics {key}: not on LRCLIB");
                self.store(&path, &CacheEntry::new(None, crate::clock::unix_now()));
                Ok(LyricsResult::NotFound)
            }
            Err(Halt::Failed { msg, retry_at }) => match cached.as_ref().and_then(|e| e.record.as_ref()) {
                // An expired hit beats an error.
                Some(record) => Ok(to_result(Some(record), job, SOURCE)),
                None => Err(Halt::Failed { msg, retry_at }),
            },
            Err(Halt::Abandoned) => Err(Halt::Abandoned),
        }
    }

    fn store(&mut self, path: &Path, entry: &CacheEntry) {
        write_cache(path, entry);
        self.writes += 1;
        if self.writes >= PRUNE_EVERY_WRITES {
            self.writes = 0;
            prune_cache(&self.cache_dir);
        }
    }

    fn fetch(&mut self, job: &Job, q: &TrackQuery) -> Result<Fetched, Halt> {
        // Set when /api/get or a search failed on LRCLIB's side. The other
        // lookups still run, but a miss is then no longer a sure "no lyrics"
        // and isn't cached.
        let mut server_failed = None;
        // Plain lyrics found on the way. One signature can hold a plain-only
        // copy while another copy of the song is synced, so the looser
        // lookups still run before settling for these.
        let mut plain: Option<LrclibRecord> = None;
        // /api/get needs an artist; try the full signature, then without album.
        if !q.artist.is_empty() {
            let mut urls = vec![lrclib::get_url(q)];
            if !q.album.is_empty() {
                urls.push(lrclib::get_url(&TrackQuery { album: String::new(), ..q.clone() }));
            }
            for url in urls {
                let resp = self.call(job, &url)?;
                match resp.status {
                    200 => match lrclib::parse_get(&resp.body) {
                        Ok(record) if complete(&record) => return Ok(Fetched::Found(record)),
                        Ok(record) if usable(&record) => {
                            plain.get_or_insert(record);
                        }
                        Ok(_) => {}
                        Err(e) => return Err(self.bad_reply(job, "/api/get", &e)),
                    },
                    // No such signature: try the next, looser one.
                    404 => {}
                    // LRCLIB rejected the query shape (e.g. no duration); the
                    // album-less /api/get would be rejected too, so search.
                    400 => break,
                    // A signature LRCLIB doesn't hold sends it to outside
                    // sources, which can answer 503 while busy; the search
                    // only reads its own database and usually still works.
                    s if s >= 500 => {
                        server_failed = Some(self.http_error(job, s));
                        break;
                    }
                    s => return Err(self.http_error(job, s)),
                }
            }
        }
        let mut searches = vec![q.clone()];
        // A browser tab whose channel isn't the artist (a lyric-video
        // channel, a record label): read the title as "Artist - Song", then
        // as "Song - Artist", and search again.
        if job.browser {
            searches.extend(lrclib::title_guesses(q));
        }
        for sq in &searches {
            if let Some(record) = self.search(job, sq, &mut server_failed)? {
                if complete(&record) {
                    return Ok(Fetched::Found(record));
                }
                plain.get_or_insert(record);
            }
        }
        match plain {
            Some(record) => Ok(Fetched::Found(record)),
            None => server_failed.map_or(Ok(Fetched::Missing), Err),
        }
    }

    /// `/api/search` by title + primary artist; the best usable match. A
    /// 5xx is noted in `server_failed` and counts as no match here: LRCLIB
    /// can answer 503 to a search it holds nothing for (a video title read
    /// the wrong way) while the next reading of the title works.
    fn search(&mut self, job: &Job, q: &TrackQuery, server_failed: &mut Option<Halt>) -> Result<Option<LrclibRecord>, Halt> {
        let search = TrackQuery {
            title: q.title.clone(),
            artist: lrclib::primary_artist(&q.artist),
            album: String::new(),
            duration_s: q.duration_s,
        };
        let resp = self.call(job, &lrclib::search_url(&search))?;
        match resp.status {
            200 => match lrclib::parse_search(&resp.body) {
                Ok(records) => Ok(lrclib::best_match(&records, q).filter(|r| usable(r)).cloned()),
                Err(e) => Err(self.bad_reply(job, "/api/search", &e)),
            },
            400 | 404 => Ok(None),
            s if s >= 500 => {
                let failed = self.http_error(job, s);
                server_failed.get_or_insert(failed);
                Ok(None)
            }
            s => Err(self.http_error(job, s)),
        }
    }

    /// One paced request. 429s set the shared back-off; a short one is waited
    /// out and the request repeated once.
    fn call(&mut self, job: &Job, url: &str) -> Result<http::Response, Halt> {
        for _ in 0..2 {
            self.pace(job)?;
            let client = match self.client.take() {
                Some(c) => c,
                None => Client::new(USER_AGENT).map_err(|e| {
                    crate::pwarn!("lyrics: {e}");
                    Halt::Failed { msg: "Lyrics are unavailable".into(), retry_at: Some(Instant::now() + retry_delay(job.attempt)) }
                })?,
            };
            let res = client.get(url, http::DEFAULT_TIMEOUT);
            self.client = Some(client);
            self.last_request = Some(Instant::now());
            match res {
                Ok(r) if r.status == 429 => {
                    let wait = r.retry_after_s.map(Duration::from_secs).unwrap_or(DEFAULT_BACKOFF).min(MAX_BACKOFF);
                    crate::pwarn!("lyrics: LRCLIB rate limit, backing off {} s", wait.as_secs());
                    self.blocked_until = Some(Instant::now() + wait);
                    // `pace` waits a short back-off out, or fails with a retry.
                }
                Ok(r) => return Ok(r),
                Err(e) => {
                    crate::pwarn!("lyrics: request failed: {e}");
                    let msg = match e {
                        FetchError::Timeout => MSG_TIMEOUT,
                        FetchError::Network(_) => MSG_UNREACHABLE,
                        FetchError::Other(_) => "Lyrics are unavailable",
                    };
                    return Err(Halt::Failed { msg: msg.into(), retry_at: Some(Instant::now() + retry_delay(job.attempt)) });
                }
            }
        }
        Err(self.busy())
    }

    /// Wait for the rate limit / back-off. Aborts when the job goes stale or
    /// the service stops; fails (with a retry) when the wait would be long.
    fn pace(&self, job: &Job) -> Result<(), Halt> {
        let mut until = self.last_request.map(|t| t + MIN_GAP);
        if let Some(b) = self.blocked_until {
            until = Some(until.map_or(b, |u| u.max(b)));
        }
        let Some(until) = until else {
            return if self.is_current(job) { Ok(()) } else { Err(Halt::Abandoned) };
        };
        if until.saturating_duration_since(Instant::now()) > MAX_INLINE_WAIT {
            return Err(self.busy());
        }
        let mut q = self.shared.lock();
        loop {
            if q.shutdown || q.newest_key != job.track_key {
                return Err(Halt::Abandoned);
            }
            let now = Instant::now();
            if now >= until {
                return Ok(());
            }
            q = self.shared.wait_for(q, Some(until - now));
        }
    }

    fn busy(&self) -> Halt {
        let at = self.blocked_until.unwrap_or_else(Instant::now).max(Instant::now() + MIN_GAP);
        Halt::Failed { msg: MSG_BUSY.into(), retry_at: Some(at) }
    }

    fn http_error(&self, job: &Job, status: u16) -> Halt {
        crate::pwarn!("lyrics: LRCLIB answered HTTP {status}");
        // Server trouble is worth a later retry; other client errors are not.
        let retry_at = (status >= 500).then(|| Instant::now() + retry_delay(job.attempt));
        Halt::Failed { msg: format!("{MSG_HTTP} {status}"), retry_at }
    }

    /// A 200 that isn't LRCLIB JSON (captive portal, proxy page, API change):
    /// never cache it as a miss.
    fn bad_reply(&self, job: &Job, endpoint: &str, err: &str) -> Halt {
        crate::pwarn!("lyrics: unreadable {endpoint} reply: {err}");
        Halt::Failed {
            msg: "Unexpected reply from LRCLIB".into(),
            retry_at: Some(Instant::now() + retry_delay(job.attempt)),
        }
    }
}

/// A record that answers the lookup for good: synced lyrics, or a definite
/// "instrumental".
fn complete(r: &LrclibRecord) -> bool {
    r.instrumental || r.has_full_synced()
}

fn to_result(record: Option<&LrclibRecord>, job: &Job, source: &str) -> LyricsResult {
    if record.is_some_and(|r| r.instrumental) {
        return LyricsResult::Instrumental;
    }
    match record.and_then(|r| lyrics_for(r, job.query.duration_s)) {
        Some(lyrics) => LyricsResult::Found { lyrics, source: source.into(), ruby: Vec::new(), wants_ja_readings: false },
        None => LyricsResult::NotFound,
    }
}

// ------------------------------------------------------------------ cache

fn read_cache(path: &Path) -> Option<CacheEntry> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            crate::pwarn!("lyrics cache: cannot read {}: {e}", path.display());
            // Invalid UTF-8 means a damaged file; anything else may be transient.
            if e.kind() == io::ErrorKind::InvalidData {
                let _ = fs::remove_file(path);
            }
            return None;
        }
    };
    match serde_json::from_str::<CacheEntry>(&text) {
        Ok(entry) if entry.v == CacheEntry::VERSION => Some(entry),
        Ok(_) => None,
        Err(e) => {
            crate::pwarn!("lyrics cache: dropping unreadable {}: {e}", path.display());
            let _ = fs::remove_file(path);
            None
        }
    }
}

fn write_cache(path: &Path, entry: &CacheEntry) {
    if let Err(e) = poni_core::store::write_json_atomic(path, entry) {
        crate::pwarn!("lyrics cache: cannot write {}: {e}", path.display());
    }
}

/// Remove leftovers of interrupted writes and keep the folder bounded
/// (oldest entries go first).
fn prune_cache(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("tmp") => {
                let _ = fs::remove_file(&path);
            }
            Some("json") => {
                let modified = entry.metadata().and_then(|m| m.modified()).ok();
                files.push((modified, path));
            }
            _ => {}
        }
    }
    let victims = prune_victims(files, CACHE_MAX, CACHE_KEEP);
    if !victims.is_empty() {
        crate::pinfo!("lyrics cache: pruning {} old entries", victims.len());
    }
    for path in victims {
        let _ = fs::remove_file(path);
    }
}

/// OS-free helpers (unit-tested on any host).
mod pure {
    use poni_core::lrc::{parse_lrc, LyricLine, Lyrics};
    use poni_core::lrclib::{timing_fits, LrclibRecord, MIN_LINES};
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    /// Misses are re-checked after 3 days (LRCLIB grows every day).
    pub const NEGATIVE_TTL_S: i64 = 3 * 24 * 3600;
    /// Plain-only hits are re-checked after 30 days in case synced lyrics appeared.
    pub const PLAIN_TTL_S: i64 = 30 * 24 * 3600;

    /// One file in the lyrics cache.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct CacheEntry {
        /// Format version.
        #[serde(default)]
        pub v: u32,
        /// The lookup that wrote it ([`CacheEntry::LOOKUP`]); 0 in files from
        /// before it was recorded.
        #[serde(default)]
        pub lookup: u32,
        /// Unix seconds when LRCLIB was asked.
        pub fetched_at: i64,
        /// The matched record; `None` records a miss.
        #[serde(default)]
        pub record: Option<LrclibRecord>,
    }

    impl CacheEntry {
        pub const VERSION: u32 = 1;
        /// Raised whenever the lookup learns to find songs it used to miss
        /// (2: subtitle tags and "Song - Artist" video titles), so misses
        /// from an older lookup are asked again instead of kept for 3 days.
        pub const LOOKUP: u32 = 2;

        pub fn new(record: Option<LrclibRecord>, now: i64) -> Self {
            Self { v: Self::VERSION, lookup: Self::LOOKUP, fetched_at: now, record }
        }
    }

    /// Whether `e` can be used without asking LRCLIB again.
    pub fn is_fresh(e: &CacheEntry, now: i64) -> bool {
        if let Some(r) = &e.record {
            // Synced lyrics and instrumental flags don't go stale.
            if r.has_synced() || r.instrumental {
                return true;
            }
        } else if e.lookup < CacheEntry::LOOKUP {
            // A miss from a lookup that knew fewer ways to find the song.
            return false;
        }
        let age = now - e.fetched_at;
        if age < -24 * 3600 {
            // Written "in the future": the clock was wrong; don't trust it.
            return false;
        }
        let ttl = if e.record.is_some() { PLAIN_TTL_S } else { NEGATIVE_TTL_S };
        age < ttl
    }

    /// Whether a record answers the lookup (lyrics or a definite "instrumental").
    pub fn usable(r: &LrclibRecord) -> bool {
        r.instrumental || lyrics_from_record(r).is_some()
    }

    /// The lyrics to show for a record: synced when available, else plain.
    /// `None` for instrumentals and records without real text (fewer than
    /// `MIN_LINES` lines: a title, a lone "♪").
    pub fn lyrics_from_record(r: &LrclibRecord) -> Option<Lyrics> {
        if r.instrumental {
            return None;
        }
        let synced = r.synced_lyrics.as_deref().filter(|s| !s.trim().is_empty()).map(parse_lrc);
        if let Some(l) = &synced {
            if l.synced && l.text_lines() >= MIN_LINES {
                return synced;
            }
        }
        if let Some(text) = r.plain_lyrics.as_deref().filter(|s| !s.trim().is_empty()) {
            let l = trim_blank_edges(Lyrics::plain(text));
            if l.text_lines() >= MIN_LINES {
                return Some(l);
            }
        }
        // "Synced" text without usable timestamps still reads as plain lyrics.
        synced.map(trim_blank_edges).filter(|l| l.text_lines() >= MIN_LINES)
    }

    /// The lyrics to show while a `playing_s`-second track plays. Synced
    /// lyrics stay synced only when the record is about as long; otherwise
    /// (a music video's intro, a radio edit) they would light up the wrong
    /// lines, so they're shown unsynced.
    pub fn lyrics_for(r: &LrclibRecord, playing_s: u32) -> Option<Lyrics> {
        let l = lyrics_from_record(r)?;
        if !l.synced || timing_fits(r.duration, playing_s) {
            return Some(l);
        }
        if let Some(text) = r.plain_lyrics.as_deref() {
            let plain = trim_blank_edges(Lyrics::plain(text));
            if plain.text_lines() >= MIN_LINES {
                return Some(plain);
            }
        }
        let lines = l.lines.into_iter().map(|x| LyricLine { time_s: 0.0, text: x.text }).collect();
        Some(trim_blank_edges(Lyrics { lines, synced: false }))
    }

    /// Drop blank rows at the start and end of unsynced lyrics (blank rows
    /// in between are stanza breaks and stay).
    fn trim_blank_edges(mut l: Lyrics) -> Lyrics {
        if !l.synced {
            while l.lines.last().is_some_and(|x| x.text.trim().is_empty()) {
                l.lines.pop();
            }
            let lead = l.lines.iter().take_while(|x| x.text.trim().is_empty()).count();
            l.lines.drain(..lead);
        }
        l
    }

    /// Whole seconds for LRCLIB's duration match; 0 when unknown.
    pub fn duration_secs(d: f64) -> u32 {
        if d.is_finite() && d > 0.5 {
            d.round().min(u32::MAX as f64) as u32
        } else {
            0
        }
    }

    /// Delay before the n-th delayed retry (0-based attempt that just failed).
    pub fn retry_delay(attempt: u32) -> Duration {
        Duration::from_secs(if attempt == 0 { 30 } else { 120 })
    }

    /// Which cache files to delete: none while `files.len() <= max`, else the
    /// oldest ones (unknown times first) down to `keep`.
    pub fn prune_victims(mut files: Vec<(Option<SystemTime>, PathBuf)>, max: usize, keep: usize) -> Vec<PathBuf> {
        if files.len() <= max {
            return Vec::new();
        }
        files.sort_by_key(|(t, _)| *t);
        let excess = files.len() - keep.min(files.len());
        files.into_iter().take(excess).map(|(_, p)| p).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // Fixtures are original placeholder text, not real lyrics.
        fn rec(synced: Option<&str>, plain: Option<&str>, instrumental: bool) -> LrclibRecord {
            LrclibRecord {
                id: 1,
                track_name: "Paper Boats".into(),
                artist_name: "The Lanterns".into(),
                album_name: None,
                duration: Some(201.0),
                instrumental,
                plain_lyrics: plain.map(Into::into),
                synced_lyrics: synced.map(Into::into),
            }
        }

        const SYNCED: &str = "[00:01.00]first row\n[00:04.50]second row\n[00:08.00]third row\n[00:12.00]fourth row\n";
        const PLAIN: &str = "first row\nsecond row\nthird row\nfourth row";

        #[test]
        fn prefers_synced_then_plain() {
            let r = rec(Some(SYNCED), Some(PLAIN), false);
            let l = lyrics_from_record(&r).unwrap();
            assert!(l.synced);
            assert_eq!(l.lines.len(), 4);
            assert!((l.lines[1].time_s - 4.5).abs() < 1e-9);

            let r = rec(None, Some("\n\nfirst row\n\nsecond row\nthird row\nfourth row\n\n"), false);
            let l = lyrics_from_record(&r).unwrap();
            assert!(!l.synced);
            let rows: Vec<&str> = l.lines.iter().map(|x| x.text.as_str()).collect();
            assert_eq!(rows, vec!["first row", "", "second row", "third row", "fourth row"]);

            // Blank or stub synced text falls back to plain.
            let r = rec(Some("   "), Some(PLAIN), false);
            assert!(!lyrics_from_record(&r).unwrap().synced);
            let r = rec(Some("[00:00.00]Paper Boats"), Some(PLAIN), false);
            assert!(!lyrics_from_record(&r).unwrap().synced);
            // "Synced" text without timestamps still shows as plain lyrics.
            let r = rec(Some(PLAIN), None, false);
            let l = lyrics_from_record(&r).unwrap();
            assert!(!l.synced);
            assert_eq!(l.lines[0].text, "first row");
        }

        #[test]
        fn instrumental_empty_and_stub_records_have_no_lyrics() {
            assert!(lyrics_from_record(&rec(Some(SYNCED), None, true)).is_none());
            assert!(usable(&rec(None, None, true)));
            assert!(lyrics_from_record(&rec(None, None, false)).is_none());
            assert!(!usable(&rec(None, Some("  \n "), false)));
            assert!(usable(&rec(None, Some(PLAIN), false)));
            // Fewer than MIN_LINES lines of text: a stub, not lyrics.
            assert!(!usable(&rec(None, Some("only row"), false)));
            assert!(!usable(&rec(Some("[00:00.00]Paper Boats"), None, false)));
        }

        #[test]
        fn synced_only_when_the_lengths_match() {
            // rec() is 201 s long.
            let r = rec(Some(SYNCED), Some(PLAIN), false);
            assert!(lyrics_for(&r, 203).unwrap().synced);
            assert!(lyrics_for(&r, 0).unwrap().synced);
            // A music video 40 s longer: the plain text, unsynced.
            let l = lyrics_for(&r, 241).unwrap();
            assert!(!l.synced);
            assert_eq!(l.lines.len(), 4);
            // Without plain text, the synced rows lose their times.
            let l = lyrics_for(&rec(Some(SYNCED), None, false), 160).unwrap();
            assert!(!l.synced);
            assert!(l.lines.iter().all(|x| x.time_s == 0.0));
            assert_eq!(l.lines[3].text, "fourth row");
        }

        #[test]
        fn freshness_rules() {
            let now = 1_800_000_000;
            let day = 24 * 3600;
            let miss = CacheEntry::new(None, now - 2 * day);
            assert!(is_fresh(&miss, now));
            let miss = CacheEntry::new(None, now - 3 * day);
            assert!(!is_fresh(&miss, now));
            let synced = CacheEntry::new(Some(rec(Some("[00:01.00]x"), None, false)), now - 900 * day);
            assert!(is_fresh(&synced, now));
            let instrumental = CacheEntry::new(Some(rec(None, None, true)), now - 900 * day);
            assert!(is_fresh(&instrumental, now));
            let plain = CacheEntry::new(Some(rec(None, Some("row"), false)), now - 10 * day);
            assert!(is_fresh(&plain, now));
            let plain = CacheEntry::new(Some(rec(None, Some("row"), false)), now - 31 * day);
            assert!(!is_fresh(&plain, now));
            // Timestamps far in the future are distrusted.
            let future = CacheEntry::new(None, now + 2 * day);
            assert!(!is_fresh(&future, now));
            // Misses from an older lookup are asked again; its hits stay.
            let old_miss = CacheEntry { lookup: CacheEntry::LOOKUP - 1, ..CacheEntry::new(None, now - day) };
            assert!(!is_fresh(&old_miss, now));
            let old_synced = CacheEntry { lookup: 0, ..CacheEntry::new(Some(rec(Some("[00:01.00]x"), None, false)), now - day) };
            assert!(is_fresh(&old_synced, now));
        }

        #[test]
        fn cache_entry_roundtrips() {
            let e = CacheEntry::new(Some(rec(Some("[00:01.00]x"), Some("x"), false)), 42);
            let json = serde_json::to_string(&e).unwrap();
            let back: CacheEntry = serde_json::from_str(&json).unwrap();
            assert_eq!(back, e);
            // Files from before the lookup was recorded read as lookup 0.
            let miss: CacheEntry = serde_json::from_str(r#"{"v":1,"fetched_at":7,"record":null}"#).unwrap();
            assert_eq!(miss, CacheEntry { lookup: 0, ..CacheEntry::new(None, 7) });
        }

        #[test]
        fn durations_round_or_zero() {
            assert_eq!(duration_secs(201.4), 201);
            assert_eq!(duration_secs(201.5), 202);
            assert_eq!(duration_secs(0.0), 0);
            assert_eq!(duration_secs(0.4), 0);
            assert_eq!(duration_secs(-3.0), 0);
            assert_eq!(duration_secs(f64::NAN), 0);
            assert_eq!(duration_secs(f64::INFINITY), 0);
        }

        #[test]
        fn retry_backoff_grows() {
            assert!(retry_delay(1) > retry_delay(0));
        }

        #[test]
        fn prunes_oldest_first() {
            use std::time::UNIX_EPOCH;
            let at = |s: u64| Some(UNIX_EPOCH + Duration::from_secs(s));
            let files = vec![
                (at(30), PathBuf::from("c")),
                (at(10), PathBuf::from("a")),
                (None, PathBuf::from("unknown")),
                (at(20), PathBuf::from("b")),
            ];
            assert!(prune_victims(files.clone(), 4, 2).is_empty());
            assert_eq!(prune_victims(files, 3, 2), vec![PathBuf::from("unknown"), PathBuf::from("a")]);
        }
    }
}

#[cfg(test)]
mod trouble_tests {
    use super::*;

    #[test]
    fn only_lrclibs_own_trouble_counts_as_lrclibs() {
        for msg in [MSG_BUSY, MSG_TIMEOUT, MSG_UNREACHABLE, "LRCLIB error 500", "LRCLIB error 503"] {
            assert!(lrclib_trouble(msg), "{msg}");
        }
        // A missing song, a refused request or a reply we can't read is ours to look at.
        for msg in ["LRCLIB error 404", "LRCLIB error 429", "LRCLIB error", "Unexpected reply from LRCLIB", "Lyrics are unavailable", ""] {
            assert!(!lrclib_trouble(msg), "{msg}");
        }
    }
}
