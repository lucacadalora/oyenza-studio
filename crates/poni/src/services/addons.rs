//! Add-on files (`poni_core::addons`): downloads them when an add-on is
//! switched on, checks each one against the size and SHA-256 built into the
//! app, and deletes them when the add-on is switched off.
//!
//! Files live in `paths::addons_dir()/<id>/<tag>/<name>`; a finished set
//! clears out any older tag's folder. At start an add-on that is on only
//! has its files read and checked: no network unless one is missing or
//! damaged. A download that can't reach GitHub waits and tries again (5 s,
//! doubling to 10 minutes); one whose bytes don't match is thrown away and
//! not retried until the add-on is switched off and on again.
//!
//! One worker thread, asleep on a condition variable whenever nothing is
//! wanted, with its own HTTP client. Other one-off downloads ride on it as
//! jobs ([`AddonService::run`]: the update file), so they need no thread of
//! their own. The Mac app runs this file as is, on its own HTTP client
//! (`mac/services/http.rs`), for the add-ons it offers.

use crate::events::{AppEvent, EventSink};
use crate::services::http::{Client, FetchError, MtaGuard};
use poni_core::addons::{self, Addon, AddonFile, Status};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");
/// Budget for one file: slow connections still bring 2 MB in this time.
const FILE_TIMEOUT: Duration = Duration::from_secs(180);
const FIRST_RETRY: Duration = Duration::from_secs(5);
const LAST_RETRY: Duration = Duration::from_secs(600);
/// Progress reaches the island at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(120);

pub struct AddonService {
    shared: Arc<Shared>,
}

/// One-off work for the worker's HTTP client, run once, in order. It gets
/// the client, or why there is none, so it can always answer its owner.
pub type Job = Box<dyn FnOnce(Result<&Client, &str>) + Send>;

impl AddonService {
    /// `on`: the add-ons switched on in settings.
    pub fn start(sink: EventSink, on: Vec<&'static str>) -> io::Result<Self> {
        let want = addons::ALL.iter().map(|a| (a.id, on.contains(&a.id))).collect();
        let shared = Arc::new(Shared { q: Mutex::new(Queue { want, jobs: Vec::new(), shutdown: false }), cv: Condvar::new() });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("poni-addons".into())
            .spawn(move || worker_main(worker, sink, crate::paths::addons_dir()))
            .map_err(|e| io::Error::other(format!("could not start the add-ons thread: {e}")))?;
        Ok(Self { shared })
    }

    /// Switch an add-on on or off; returns at once (UI thread safe).
    pub fn set(&self, id: &'static str, on: bool) {
        self.shared.lock().want.insert(id, on);
        self.shared.cv.notify_all();
    }

    /// Run `job` on the worker after what it is doing now; returns at once.
    pub fn run(&self, job: Job) {
        self.shared.lock().jobs.push(job);
        self.shared.cv.notify_all();
    }
}

impl Drop for AddonService {
    fn drop(&mut self) {
        // The worker exits at its next wait; a download in flight ends on its
        // own timeout. Never join from the UI thread.
        self.shared.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

struct Queue {
    want: BTreeMap<&'static str, bool>,
    jobs: Vec<Job>,
    shutdown: bool,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.q.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// What the worker knows about one add-on.
#[derive(Clone, Copy, PartialEq)]
enum Have {
    /// Nothing checked yet (or it was just switched on again).
    Unknown,
    Complete,
    /// Try the download again at this time.
    RetryAt(Instant, Duration),
    /// A file came back wrong; wait for off and on.
    Failed,
    /// Switched off and its files deleted.
    Removed,
}

fn worker_main(shared: Arc<Shared>, sink: EventSink, dir: PathBuf) {
    let _com = MtaGuard::new();
    let mut client: Option<Client> = None;
    let mut have: BTreeMap<&'static str, Have> = addons::ALL.iter().map(|a| (a.id, Have::Unknown)).collect();
    let mut was_on: BTreeMap<&'static str, bool> = BTreeMap::new();
    loop {
        let (want, jobs, shutdown) = {
            let mut q = shared.lock();
            (q.want.clone(), std::mem::take(&mut q.jobs), q.shutdown)
        };
        if shutdown {
            return;
        }
        let mut next_wake: Option<Instant> = None;
        for addon in addons::ALL {
            // Already in the app (Script reader): nothing to fetch or delete.
            if !addon.downloads() {
                continue;
            }
            let on = want.get(addon.id).copied().unwrap_or(false);
            let h = have.get(addon.id).copied().unwrap_or(Have::Unknown);
            // Switched on again after a failure: a fresh start.
            let h = if on && !was_on.get(addon.id).copied().unwrap_or(false) && matches!(h, Have::Failed | Have::Removed) { Have::Unknown } else { h };
            was_on.insert(addon.id, on);
            let now_have = if !on {
                if h != Have::Removed {
                    remove(&dir, addon);
                    sink.send(AppEvent::Addon { id: addon.id, status: Status::Off });
                }
                Have::Removed
            } else {
                match h {
                    Have::Complete | Have::Failed => h,
                    Have::RetryAt(at, _) if Instant::now() < at => h,
                    _ => {
                        let backoff = if let Have::RetryAt(_, d) = h { d } else { FIRST_RETRY };
                        if client.is_none() {
                            match Client::new(USER_AGENT) {
                                Ok(c) => client = Some(c),
                                Err(e) => crate::pwarn!("add-ons: no HTTP client ({e})"),
                            }
                        }
                        match &client {
                            Some(c) => ensure(&shared, c, &sink, &dir, addon, h == Have::Unknown, backoff),
                            None => wait_retry(&sink, addon, backoff, "no HTTP client"),
                        }
                    }
                }
            };
            if let Have::RetryAt(at, _) = now_have {
                next_wake = Some(next_wake.map_or(at, |w| w.min(at)));
            }
            have.insert(addon.id, now_have);
        }
        for job in jobs {
            let mut why = String::new();
            if client.is_none() {
                match Client::new(USER_AGENT) {
                    Ok(c) => client = Some(c),
                    Err(e) => {
                        crate::pwarn!("add-ons: no HTTP client for a job ({e})");
                        why = e.to_string();
                    }
                }
            }
            job(client.as_ref().ok_or(why.as_str()));
        }
        // Sleep until something changes or a retry is due.
        let q = shared.lock();
        if q.want == want && q.jobs.is_empty() && !q.shutdown {
            let _q = match next_wake {
                Some(at) => shared.cv.wait_timeout(q, at.saturating_duration_since(Instant::now())).map(|r| r.0).unwrap_or_else(|p| p.into_inner().0),
                None => shared.cv.wait(q).unwrap_or_else(|p| p.into_inner()),
            };
        }
    }
}

/// The folder of this version's files.
fn version_dir(dir: &Path, addon: &Addon) -> PathBuf {
    dir.join(addon.id).join(addon.tag)
}

/// Where one of an add-on's files lives once it's on this PC (it is only
/// there, checked, while the add-on is on).
pub fn file_path(addon: &Addon, name: &str) -> PathBuf {
    version_dir(&crate::paths::addons_dir(), addon).join(name)
}

/// Is the file on disk exactly the one expected?
fn present(path: &Path, file: &AddonFile) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.len() == file.size => fs::read(path).is_ok_and(|b| file.matches(&b)),
        _ => false,
    }
}

/// Make every file of `addon` present: check the ones on disk, download the
/// rest. Reports `Checking`, `Downloading`, `Waiting`, `Ready` or `Failed`.
fn ensure(shared: &Shared, client: &Client, sink: &EventSink, dir: &Path, addon: &'static Addon, first: bool, backoff: Duration) -> Have {
    if first {
        sink.send(AppEvent::Addon { id: addon.id, status: Status::Checking });
    }
    let folder = version_dir(dir, addon);
    let total = addon.size();
    let missing: Vec<&AddonFile> = addon.files.iter().filter(|f| !present(&folder.join(f.name), f)).collect();
    let mut done = total - missing.iter().map(|f| f.size).sum::<u64>();
    if !missing.is_empty() {
        if let Err(e) = fs::create_dir_all(&folder) {
            crate::pwarn!("add-ons: {}: could not make {} ({e})", addon.id, folder.display());
            sink.send(AppEvent::Addon { id: addon.id, status: Status::Failed });
            return Have::Failed;
        }
        sink.send(AppEvent::Addon { id: addon.id, status: Status::Downloading { done, total } });
    }
    for file in missing {
        // Switched off meanwhile: stop here (the caller deletes the files).
        if !shared.lock().want.get(addon.id).copied().unwrap_or(false) {
            return Have::Unknown;
        }
        let url = addon.url(file);
        let base = done;
        let progress_sink = sink.clone();
        let mut last = Instant::now();
        let id = addon.id;
        let started = Instant::now();
        let got = client.get_bytes(&url, FILE_TIMEOUT, file.size + 64 * 1024, move |n| {
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                progress_sink.send(AppEvent::Addon { id, status: Status::Downloading { done: base + n.min(file.size), total } });
            }
        });
        match got {
            Ok((200, bytes)) if file.matches(&bytes) => {
                let path = folder.join(file.name);
                let part = folder.join(format!("{}.part", file.name));
                if let Err(e) = fs::write(&part, &bytes).and_then(|_| fs::rename(&part, &path)) {
                    let _ = fs::remove_file(&part);
                    crate::pwarn!("add-ons: {}: could not save {} ({e})", addon.id, file.name);
                    sink.send(AppEvent::Addon { id: addon.id, status: Status::Failed });
                    return Have::Failed;
                }
                done += file.size;
                crate::pinfo!("add-ons: {}: {} downloaded ({} bytes, {} ms)", addon.id, file.name, file.size, started.elapsed().as_millis());
                sink.send(AppEvent::Addon { id: addon.id, status: Status::Downloading { done, total } });
            }
            Ok((200, bytes)) => {
                crate::pwarn!("add-ons: {}: {} came back different ({} bytes, expected {}); not saved", addon.id, file.name, bytes.len(), file.size);
                sink.send(AppEvent::Addon { id: addon.id, status: Status::Failed });
                return Have::Failed;
            }
            // GitHub busy or limiting: like no internet, try again later.
            Ok((status, _)) if status >= 500 || status == 429 || status == 408 => return wait_retry(sink, addon, backoff, &format!("HTTP {status}")),
            Ok((status, _)) => {
                crate::pwarn!("add-ons: {}: {} answered HTTP {status}", addon.id, file.name);
                sink.send(AppEvent::Addon { id: addon.id, status: Status::Failed });
                return Have::Failed;
            }
            Err(e @ (FetchError::Network(_) | FetchError::Timeout)) => return wait_retry(sink, addon, backoff, &e.to_string()),
            Err(e) => {
                crate::pwarn!("add-ons: {}: {} failed ({e})", addon.id, file.name);
                return wait_retry(sink, addon, backoff, &e.to_string());
            }
        }
    }
    clear_old_versions(dir, addon);
    crate::pinfo!("add-ons: {}: ready", addon.id);
    sink.send(AppEvent::Addon { id: addon.id, status: Status::Ready });
    Have::Complete
}

fn wait_retry(sink: &EventSink, addon: &Addon, backoff: Duration, why: &str) -> Have {
    crate::pinfo!("add-ons: {}: waiting ({why}); trying again in {} s", addon.id, backoff.as_secs());
    sink.send(AppEvent::Addon { id: addon.id, status: Status::Waiting });
    Have::RetryAt(Instant::now() + backoff, (backoff * 2).min(LAST_RETRY))
}

/// Delete every version but this one (a newer release replaced them).
fn clear_old_versions(dir: &Path, addon: &Addon) {
    let Ok(entries) = fs::read_dir(dir.join(addon.id)) else { return };
    for e in entries.flatten() {
        if e.file_name() != addon.tag {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Switched off: delete all of the add-on's files.
fn remove(dir: &Path, addon: &Addon) {
    let folder = dir.join(addon.id);
    if folder.exists() {
        match fs::remove_dir_all(&folder) {
            Ok(()) => crate::pinfo!("add-ons: {}: switched off, files deleted", addon.id),
            Err(e) => crate::pwarn!("add-ons: {}: could not delete {} ({e})", addon.id, folder.display()),
        }
    }
}

/// The self-test's real download (Windows CI): every file of `addon` into
/// `dir`, checked. Returns a line for the report.
pub fn fetch_for_selftest(addon: &'static Addon, dir: &Path) -> (bool, String) {
    let _com = MtaGuard::new();
    let client = match Client::new(USER_AGENT) {
        Ok(c) => c,
        Err(e) => return (false, format!("no HTTP client: {e}")),
    };
    let started = Instant::now();
    let folder = version_dir(dir, addon);
    if let Err(e) = fs::create_dir_all(&folder) {
        return (false, format!("could not make {}: {e}", folder.display()));
    }
    for file in addon.files {
        match client.get_bytes(&addon.url(file), FILE_TIMEOUT, file.size + 64 * 1024, |_| {}) {
            Ok((200, bytes)) if file.matches(&bytes) => {
                if let Err(e) = fs::write(folder.join(file.name), &bytes) {
                    return (false, format!("{}: could not save ({e})", file.name));
                }
            }
            Ok((status, bytes)) => return (false, format!("{}: HTTP {status}, {} bytes, not the expected file", file.name, bytes.len())),
            Err(e) => return (false, format!("{}: {e}", file.name)),
        }
    }
    let ok = addon.files.iter().all(|f| present(&folder.join(f.name), f));
    (ok, format!("{} files, {} bytes from {} in {} ms", addon.files.len(), addon.size(), addon.tag, started.elapsed().as_millis()))
}
