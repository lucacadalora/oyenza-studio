//! Add-on files (`poni_core::addons`): downloads them when an add-on is
//! switched on, checks each one against the size and SHA-256 built into the
//! app, and deletes them when the add-on is switched off.
//!
//! Files live in `paths::addons_dir()/<id>/<tag>/<name>`; a finished set
//! clears out any older tag's folder. At start an add-on that is on only
//! has its files checked: no network unless one is missing or damaged, and
//! no reading either for a file the folder's stamp vouches for ([`STAMP`]).
//! A download that can't reach GitHub waits and tries again (5 s,
//! doubling to 10 minutes); one whose bytes don't match is thrown away and
//! not retried until the add-on is switched off and on again. An add-on
//! whose files this build doesn't pin yet downloads nothing at all. A Studio
//! Pro add-on's files (`Addon::pro`: the Downloader's tools) are checked and
//! fetched only for an account the server says has Pro
//! ([`AddonService::set_pro`]). Without it (at start before the answer,
//! signed out, a no, or an app that never tells, as the Mac's for now) they
//! stay as they are, neither checked, fetched nor deleted, so no start,
//! sign-out or wrong answer costs their download again; switching the
//! add-on off deletes them as for any other.
//!
//! GitHub answers a release download (the add-ons' own pre-releases, and
//! the Downloader's yt-dlp and QuickJS from theirs) with a redirect to its
//! file host, release-assets.githubusercontent.com. Both HTTP clients follow
//! it by themselves: WinRT's `HttpClient` through the `HttpBaseProtocolFilter`
//! it makes by default (`AllowAutoRedirect` is on unless switched off), the
//! Mac's URLSession task by default.
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
use std::time::{Duration, Instant, UNIX_EPOCH};

const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");
/// Budget for a small file: slow connections still bring 2 MB in this time.
/// A bigger one gets more ([`file_timeout`]).
const FILE_TIMEOUT: Duration = Duration::from_secs(180);
/// What a slow line (half a megabit a second) brings in a second.
const SLOW_LINE: u64 = 64 * 1024;
/// The most any one file gets.
const LONGEST_FILE: Duration = Duration::from_secs(20 * 60);
/// The stamp a checked set of files leaves in its folder ([`present`]).
const STAMP: &str = ".ok";
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
        let shared = Arc::new(Shared { q: Mutex::new(Queue { want, pro: false, jobs: Vec::new(), shutdown: false }), cv: Condvar::new() });
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

    /// Whether the account has Studio Pro, as the server last said, for the
    /// Pro add-ons' files ([`wanted`]); returns at once.
    pub fn set_pro(&self, pro: bool) {
        let mut q = self.shared.lock();
        if q.pro != pro {
            q.pro = pro;
            drop(q);
            self.shared.cv.notify_all();
        }
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
    pro: bool,
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

/// What becomes of `addon`'s files, switched `on` or off: kept (true) or
/// deleted (false). None leaves them as they are: a Pro add-on switched on
/// for an account without Pro (as far as the island knows).
fn wanted(addon: &Addon, on: bool, pro: bool) -> Option<bool> {
    if on && addon.pro && !pro {
        None
    } else {
        Some(on)
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
        let (want, pro, jobs, shutdown) = {
            let mut q = shared.lock();
            (q.want.clone(), q.pro, std::mem::take(&mut q.jobs), q.shutdown)
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
            // A Pro add-on's files wait for Pro.
            let Some(on) = wanted(addon, want.get(addon.id).copied().unwrap_or(false), pro) else { continue };
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
        if q.want == want && q.pro == pro && q.jobs.is_empty() && !q.shutdown {
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

/// Budget for downloading a file of `size` bytes: [`FILE_TIMEOUT`] and a
/// second more for every 64 KB, so a big file still comes over a slow line
/// (the Downloader's yt-dlp, 24 MB, gets 9 minutes; 3 minutes would ask for
/// 1 Mbit/s). Tight costs more than it saves: a file cut off is thrown away,
/// and the next try starts again from its first byte. The cap is only for a
/// transfer that stalls without failing, which holds the add-ons' thread (and
/// the update check queued behind it) until then.
fn file_timeout(size: u64) -> Duration {
    (FILE_TIMEOUT + Duration::from_secs(size / SLOW_LINE)).min(LONGEST_FILE)
}

/// Is the file on disk exactly the one expected? `stamp` is its folder's
/// [`STAMP`]: a line for each file once it was read and checked in full, with
/// its pin and the size and last write time it had then. A file whose line
/// still matches isn't read again, so an add-on that is on costs a few
/// metadata reads at start instead of a read of every byte (the Downloader's
/// tools are about 30 MB, and the app often starts with Windows). Any other
/// file is read in full, and once it checks out its line goes in `lines` for
/// the next stamp ([`save_stamp`]).
///
/// Just as safe: whatever writes to a file moves its last write time, so a
/// changed file no longer matches its line and is read in full, and a line
/// made for another pin or another version of the app matches nothing (a new
/// version reads every file once, which also finds damage the disk did with no
/// write). Only a change that sets the time back on purpose gets by, and
/// whatever does that could as well change the app itself.
fn present(path: &Path, file: &AddonFile, stamp: &str, lines: &mut Vec<String>) -> bool {
    let Some(m) = fs::metadata(path).ok().filter(|m| m.len() == file.size) else { return false };
    // Taken before the bytes are read: a write during the read moves the time past it.
    let line = stamp_line(file, &m);
    let right = line.as_ref().is_some_and(|l| stamp.lines().any(|s| s == l)) || fs::File::open(path).is_ok_and(|f| file.matches_read(f));
    if right {
        lines.extend(line);
    }
    right
}

/// A file's line in the stamp: this version, its pin, and the size and last
/// write time (in nanoseconds) it has on disk. None where the system keeps no
/// such time: that file is read in full every time.
fn stamp_line(file: &AddonFile, m: &fs::Metadata) -> Option<String> {
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(format!("{} {} {} {} {}", env!("CARGO_PKG_VERSION"), file.sha256, m.len(), t.as_nanos(), file.name))
}

/// Keep `lines` ([`present`]) as the stamp in `folder`, which held `stamp`,
/// and return it: written only when they changed, so a start that finds
/// everything in place writes nothing.
fn save_stamp(folder: &Path, stamp: &str, lines: &mut [String]) -> String {
    // Unstable: the lines are distinct, and its code is a few KB smaller.
    lines.sort_unstable();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    if text != stamp {
        if let Err(e) = fs::write(folder.join(STAMP), &text) {
            crate::pinfo!("add-ons: no stamp in {} ({e}); its files are read in full next time", folder.display());
        }
    }
    text
}

/// Make every file of `addon` present: check the ones on disk, download the
/// rest. Reports `Checking`, `Downloading`, `Waiting`, `Ready` or `Failed`,
/// or `Unpinned` for one this build can't get.
fn ensure(shared: &Shared, client: &Client, sink: &EventSink, dir: &Path, addon: &'static Addon, first: bool, backoff: Duration) -> Have {
    // A build made while one of its files is a placeholder: nothing it could
    // check, so nothing is fetched, however the switch came on (the Add-ons
    // page keeps it off). Off and on again gets the same answer.
    if !addon.pinned() {
        crate::pwarn!("add-ons: {}: this build doesn't pin all of its files; nothing downloaded", addon.id);
        sink.send(AppEvent::Addon { id: addon.id, status: Status::Unpinned });
        return Have::Failed;
    }
    if first {
        sink.send(AppEvent::Addon { id: addon.id, status: Status::Checking });
    }
    let folder = version_dir(dir, addon);
    let total = addon.size();
    let stamp = fs::read_to_string(folder.join(STAMP)).unwrap_or_default();
    let mut lines = Vec::new();
    let missing: Vec<&AddonFile> = addon.files.iter().filter(|f| !present(&folder.join(f.name), f, &stamp, &mut lines)).collect();
    // Kept now, so tries while the rest can't come don't read these again.
    let stamp = save_stamp(&folder, &stamp, &mut lines);
    // A Mac runs the Downloader's tools only with their execute bit, which
    // each gets as it is saved (below); one that lost it gets it back.
    for f in addon.files.iter().filter(|f| runs(addon, f) && !missing.contains(f)) {
        if let Err(e) = make_runnable(&folder.join(f.name)) {
            crate::pwarn!("add-ons: {}: {} can't be made runnable ({e})", addon.id, f.name);
        }
    }
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
        // Switched off meanwhile, or Pro went: stop here (the caller deletes
        // the files, or leaves them for Pro).
        let wants = {
            let q = shared.lock();
            wanted(addon, q.want.get(addon.id).copied().unwrap_or(false), q.pro)
        };
        if wants != Some(true) {
            return Have::Unknown;
        }
        let url = addon.url(file);
        let base = done;
        let progress_sink = sink.clone();
        let mut last = Instant::now();
        let id = addon.id;
        let started = Instant::now();
        let got = client.get_bytes(&url, file_timeout(file.size), file.size + 64 * 1024, move |n| {
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                progress_sink.send(AppEvent::Addon { id, status: Status::Downloading { done: base + n.min(file.size), total } });
            }
        });
        match got {
            Ok((200, bytes)) if file.matches(&bytes) => {
                let path = folder.join(file.name);
                let part = folder.join(format!("{}.part", file.name));
                let saved = fs::write(&part, &bytes).and_then(|_| if runs(addon, file) { make_runnable(&part) } else { Ok(()) }).and_then(|_| fs::rename(&part, &path));
                if let Err(e) = saved {
                    let _ = fs::remove_file(&part);
                    crate::pwarn!("add-ons: {}: could not save {} ({e})", addon.id, file.name);
                    sink.send(AppEvent::Addon { id: addon.id, status: Status::Failed });
                    return Have::Failed;
                }
                // Checked once more as saved (just written, so from the
                // system's cache): the stamp only vouches for a file on disk
                // that was read in full.
                if !present(&path, file, "", &mut lines) {
                    crate::pwarn!("add-ons: {}: {} isn't the same once saved", addon.id, file.name);
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
    save_stamp(&folder, &stamp, &mut lines);
    clear_old_versions(dir, addon);
    crate::pinfo!("add-ons: {}: ready", addon.id);
    sink.send(AppEvent::Addon { id: addon.id, status: Status::Ready });
    Have::Complete
}

/// Whether `file` is a program the app runs (the Downloader's tools).
fn runs(addon: &Addon, file: &AddonFile) -> bool {
    addon.id == addons::DOWNLOADER.id && addons::is_downloader_tool(file.name)
}

/// Make a downloaded program runnable. A file the app writes on a Mac can be
/// read but not run (0644), and yt-dlp, QuickJS and FFmpeg only start with
/// the execute bit; setting it changes the file's mode, not its bytes or its
/// last write time, so the stamp still vouches for it. On Windows an .exe
/// runs as it is.
#[cfg(unix)]
pub fn make_runnable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if fs::metadata(path)?.permissions().mode() & 0o111 == 0o111 {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
pub fn make_runnable(_: &Path) -> io::Result<()> {
    Ok(())
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

/// Switched off: delete all of the add-on's files. A program of the add-on
/// that was just stopped (the Downloader's yt-dlp) can hold its file for a
/// moment after the stop, so a failed delete is tried again for about 2 s
/// (this runs on the add-on thread, never the UI's).
fn remove(dir: &Path, addon: &Addon) {
    let folder = dir.join(addon.id);
    for tries in 0.. {
        if !folder.exists() {
            return;
        }
        match fs::remove_dir_all(&folder) {
            Ok(()) => return crate::pinfo!("add-ons: {}: switched off, files deleted", addon.id),
            Err(_) if tries < 10 => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return crate::pwarn!("add-ons: {}: could not delete {} ({e})", addon.id, folder.display()),
        }
    }
}

/// The self-test's real download (Windows CI): every file of `addon` into
/// `dir`, checked. Returns a line for the report. A file this build doesn't
/// pin can't be checked, so its add-on fails here, naming it.
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
    for file in addon.files.iter().filter(|f| f.pinned()) {
        match client.get_bytes(&addon.url(file), file_timeout(file.size), file.size + 64 * 1024, |_| {}) {
            Ok((200, bytes)) if file.matches(&bytes) => {
                if let Err(e) = fs::write(folder.join(file.name), &bytes) {
                    return (false, format!("{}: could not save ({e})", file.name));
                }
            }
            Ok((status, bytes)) => return (false, format!("{}: HTTP {status}, {} bytes, not the expected file", file.name, bytes.len())),
            Err(e) => return (false, format!("{}: {e}", file.name)),
        }
    }
    let unpinned: Vec<&str> = addon.files.iter().filter(|f| !f.pinned()).map(|f| f.name).collect();
    let ok = unpinned.is_empty() && addon.files.iter().all(|f| present(&folder.join(f.name), f, "", &mut Vec::new()));
    let mut said = format!("{} files, {} bytes from {} in {} ms", addon.files.len(), addon.size(), addon.tag, started.elapsed().as_millis());
    if !unpinned.is_empty() {
        said += &format!("; not pinned in this build: {}", unpinned.join(", "));
    }
    (ok, said)
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::addons::JAPANESE_READINGS;

    #[test]
    fn a_pro_add_on_s_files_come_only_with_pro() {
        use poni_core::addons::{DOWNLOADER, PRAYER_TIMES};
        // Switched on: kept (and fetched) with Pro, left as they are without it.
        assert_eq!(wanted(&DOWNLOADER, true, true), Some(true));
        assert_eq!(wanted(&DOWNLOADER, true, false), None);
        for pro in [true, false] {
            // Switched off: deleted, Pro or not.
            assert_eq!(wanted(&DOWNLOADER, false, pro), Some(false));
            // Every other add-on goes by its switch alone.
            assert_eq!(wanted(&PRAYER_TIMES, true, pro), Some(true));
            assert_eq!(wanted(&PRAYER_TIMES, false, pro), Some(false));
        }
    }

    #[test]
    fn a_big_file_gets_more_time() {
        assert_eq!(file_timeout(869), FILE_TIMEOUT, "a small file: three minutes, as before");
        assert_eq!(file_timeout(23_575_744), Duration::from_secs(539), "yt-dlp: 9 minutes");
        assert_eq!(file_timeout(10_000_000_000), LONGEST_FILE);
    }

    #[test]
    fn a_file_read_in_full_is_stamped_and_a_changed_one_is_caught() {
        let file = JAPANESE_READINGS.files[1];
        let bytes: &[u8] = include_bytes!("../../../../addons/ja/NOTICE.txt");
        let dir = std::env::temp_dir().join(format!("poni-addon-stamp-{}", std::process::id()));
        let path = dir.join(file.name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, bytes).unwrap();
        let set_time = |t| fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(t)).unwrap();
        // No stamp yet: read in full, then stamped.
        let mut lines = Vec::new();
        assert!(present(&path, &file, "", &mut lines));
        let stamp = save_stamp(&dir, "", &mut lines);
        assert_eq!(fs::read_to_string(dir.join(STAMP)).unwrap(), stamp);
        assert!(stamp.contains(file.sha256) && stamp.ends_with(" NOTICE.txt\n"), "{stamp}");
        // The next start takes the stamp's word, and has nothing to write.
        let mut again = Vec::new();
        assert!(present(&path, &file, &stamp, &mut again));
        assert_eq!(save_stamp(&dir, &stamp, &mut again), stamp);
        // A byte changed by a later write: the line no longer matches, and
        // the full read finds it.
        let t = fs::metadata(&path).unwrap().modified().unwrap();
        let mut changed = bytes.to_vec();
        changed[0] ^= 1;
        fs::write(&path, &changed).unwrap();
        set_time(t + Duration::from_secs(2));
        assert!(!present(&path, &file, &stamp, &mut Vec::new()));
        // The stamp is all that stands in for the read: only a change that
        // puts the time back on purpose gets by.
        set_time(t);
        assert!(present(&path, &file, &stamp, &mut Vec::new()));
        // A line made for another pin matches nothing.
        let other = AddonFile { sha256: JAPANESE_READINGS.files[0].sha256, ..file };
        assert!(!present(&path, &other, &stamp, &mut Vec::new()));
        let _ = fs::remove_dir_all(&dir);
    }
}
