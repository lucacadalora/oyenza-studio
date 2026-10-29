//! The Downloader add-on's runner: one yt-dlp per download
//! (`poni_core::ytdlp::args`), started without a console window inside a
//! job object, so stopping a download (or the app quitting) ends yt-dlp and
//! the FFmpeg and QuickJS it started. Each job has a reader thread for its
//! output, which `poni_core::ytdlp::parse` turns into `AppEvent::Download`;
//! the thread ends with the process (`AppEvent::DownloadEnded`). Nothing runs
//! at rest: the service starts at the first download and holds no thread of
//! its own between downloads.
//!
//! yt-dlp.exe is a PyInstaller bundle: a small loader that unpacks Python
//! into a temporary folder and runs it as a second process. So yt-dlp starts
//! suspended and only runs once it is in the job, which nothing it starts
//! can leave; the job also ends all of it when its last handle closes, even
//! if Poni itself is ended. The loader only deletes what it unpacked (about
//! 30 MB) when it ends by itself, so each download unpacks into its own
//! folder, deleted when the download ends.
//!
//! When the person stops a download, its thread deletes the files yt-dlp was
//! writing in the Downloads folder (`….part`, `….ytdl` and the fragments,
//! `poni_core::ytdlp_partial`) once yt-dlp has ended, found by the video's id
//! from yt-dlp's info line. Not when the app quits: those stay, as after a
//! failure.

use crate::events::{AppEvent, EventSink};
use poni_core::addons::{DOWNLOADER, FFMPEG_EXE, QJS_EXE, YTDLP_EXE};
use poni_core::ytdlp::{self, Line, Problem, Request, Tools};
use poni_core::ytdlp_partial;
use std::collections::HashMap;
use std::fs;
use std::io::{self, PipeReader, Read};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Threading::{OpenThread, ResumeThread, BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME};

/// Progress reaches the island at most this often per download (yt-dlp
/// prints a line for every block it reads); the newest one waiting goes next.
const PROGRESS_EVERY: Duration = Duration::from_millis(125);
/// yt-dlp's lines are a few hundred bytes; this only bounds a runaway one.
const LINE_MAX: usize = 64 * 1024;
/// What the page shows when yt-dlp couldn't start (the log says why).
const NOT_STARTED: &str = "yt-dlp couldn't start. Try switching the Downloader off and on.";

pub struct DownloaderService {
    sink: EventSink,
    runs: Arc<Runs>,
}

impl DownloaderService {
    pub fn start(sink: EventSink) -> io::Result<Self> {
        Ok(Self { sink, runs: Arc::default() })
    }

    /// Start yt-dlp for `req`; returns at once (UI thread safe).
    pub fn run(&self, req: Request) {
        let id = req.id;
        let started = Job::new().map_err(io::Error::from).and_then(|job| {
            let run = Arc::new(Run::new(job));
            self.runs.lock().insert(id, run.clone());
            let (runs, sink) = (self.runs.clone(), self.sink.clone());
            std::thread::Builder::new().name(format!("poni-dl-{id}")).spawn(move || {
                let mut ended = Ended { id, ok: false, runs: &runs, sink: &sink };
                ended.ok = download(&req, &run, &runs, &sink);
            })
        });
        if let Err(e) = started {
            crate::pwarn!("downloader: {id}: could not start ({e})");
            self.runs.lock().remove(&id);
            self.sink.send(not_started(id));
            self.sink.send(AppEvent::DownloadEnded { id, ok: false });
        }
    }

    /// Stop download `id`; returns at once. Its thread reports the end (not
    /// ok) once yt-dlp and what it started are gone, and deletes the files
    /// yt-dlp left unfinished.
    pub fn stop(&self, id: u64) {
        if let Some(run) = self.runs.lock().get(&id) {
            run.tidy.store(true, Ordering::SeqCst);
            run.stop();
        }
    }
}

impl Drop for DownloaderService {
    fn drop(&mut self) {
        // The app is quitting: every download ends now, and each thread goes
        // when its output does. Never join from the UI thread.
        for run in self.runs.lock().values() {
            run.stop();
        }
    }
}

/// The downloads whose yt-dlp runs, by id.
#[derive(Default)]
struct Runs(Mutex<HashMap<u64, Arc<Run>>>);

impl Runs {
    /// Plain data: recover from poisoning instead of panicking.
    fn lock(&self) -> MutexGuard<'_, HashMap<u64, Arc<Run>>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// One download's job, and whether it was stopped (by the person, or the
/// app quitting).
struct Run {
    job: Job,
    stopped: AtomicBool,
    /// The person stopped it: the files yt-dlp left unfinished go once it has
    /// ended ([`tidy`]).
    tidy: AtomicBool,
    /// The site's id for the video, from yt-dlp's info line, which the
    /// video's files carry in their names.
    video: Mutex<Option<String>>,
}

impl Run {
    fn new(job: Job) -> Self {
        Run { job, stopped: AtomicBool::new(false), tidy: AtomicBool::new(false), video: Mutex::new(None) }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.job.end();
    }

    fn video(&self) -> Option<String> {
        self.video.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn set_video(&self, video: String) {
        *self.video.lock().unwrap_or_else(|p| p.into_inner()) = Some(video);
    }
}

/// Takes a download off the list and reports its end (not ok unless set)
/// when its thread ends, even by a panic: the page never waits forever.
struct Ended<'a> {
    id: u64,
    ok: bool,
    runs: &'a Runs,
    sink: &'a EventSink,
}

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        self.runs.lock().remove(&self.id);
        self.sink.send(AppEvent::DownloadEnded { id: self.id, ok: self.ok });
    }
}

/// Run yt-dlp for `req` in `run`'s job and pass its lines on until it ends.
/// True when it finished well: exit code 0, not stopped.
fn download(req: &Request, run: &Run, runs: &Runs, sink: &EventSink) -> bool {
    let id = req.id;
    let tool = |name: &str| crate::services::addons::file_path(&DOWNLOADER, name).to_string_lossy().into_owned();
    let (ytdlp, qjs, ffmpeg) = (tool(YTDLP_EXE), tool(QJS_EXE), tool(FFMPEG_EXE));
    // yt-dlp's cache (YouTube's solved players: QuickJS is slow at them)
    // sits with this version's tools, so it lasts while the add-on stays on
    // and goes when it is switched off or its tools change.
    let cache = crate::services::addons::file_path(&DOWNLOADER, "cache");
    // Outside the version folder: the add-on's check at start clears it, and
    // switching the add-on off deletes it.
    let temps = crate::paths::addons_dir().join(DOWNLOADER.id).join("tmp");
    sweep(&temps, runs);
    let temp = temps.join(id.to_string());
    let args = ytdlp::args(req, &Tools { ytdlp: &ytdlp, qjs: &qjs, ffmpeg: &ffmpeg, cache: &cache.to_string_lossy() });
    let began = Instant::now();
    let spawned = fs::create_dir_all(&cache).and_then(|_| fs::create_dir_all(&temp)).and_then(|_| spawn(&ytdlp, &args, &temp, run));
    let ok = match spawned {
        Ok((out, mut child)) => {
            crate::pinfo!("downloader: {id}: yt-dlp started");
            read(out, id, run, sink);
            let code = child.wait().ok().and_then(|s| s.code());
            let stopped = run.stopped.load(Ordering::SeqCst);
            let secs = began.elapsed().as_secs();
            if stopped {
                crate::pinfo!("downloader: {id}: stopped after {secs} s");
            } else {
                crate::pinfo!("downloader: {id}: yt-dlp ended after {secs} s (exit code {})", code.unwrap_or(-1));
            }
            code == Some(0) && !stopped
        }
        Err(e) => {
            crate::pwarn!("downloader: {id}: yt-dlp didn't start ({e})");
            sink.send(not_started(id));
            false
        }
    };
    // Whatever is left in the job ends, then what the loader unpacked goes.
    run.job.end();
    remove(&temp);
    // Stopped by the person: what yt-dlp wrote of the video goes too.
    if run.tidy.load(Ordering::SeqCst) {
        if let Some(video) = run.video() {
            tidy(Path::new(&req.folder), &video, id, runs);
        }
    }
    ok
}

/// Start yt-dlp suspended, put it in the job, then let it run: what it
/// starts (the loader starts Python at once) is in the job from the start.
/// Its stdout and stderr share one pipe, so one thread reads both in order.
fn spawn(exe: &str, args: &[String], temp: &Path, run: &Run) -> io::Result<(PipeReader, Child)> {
    let (out, into) = io::pipe()?;
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        // Where the loader unpacks Python, and Python's own temporary files.
        .env("TEMP", temp)
        .env("TMP", temp)
        .stdin(Stdio::null())
        .stdout(into.try_clone()?)
        .stderr(into)
        // No console window (what yt-dlp starts shares its hidden console),
        // below normal priority (so is what it starts), and suspended until
        // it is in the job.
        .creation_flags((CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS | CREATE_SUSPENDED).0);
    let mut child = cmd.spawn()?;
    // `cmd` held the pipe's writing end: with it gone, the output ends once
    // yt-dlp and what inherited it from yt-dlp have closed theirs.
    drop(cmd);
    // SAFETY: the process handle stays open as long as `child`.
    if let Err(e) = unsafe { AssignProcessToJobObject(run.job.0, HANDLE(child.as_raw_handle())) } {
        let _ = child.kill();
        return Err(e.into());
    }
    if run.stopped.load(Ordering::SeqCst) {
        // Stopped while it started: it ends before running.
        run.job.end();
    } else if let Err(e) = resume(child.id()) {
        run.job.end();
        return Err(e);
    }
    Ok((out, child))
}

/// Let suspended process `pid` run. std keeps no handle to its first thread,
/// so it is found in a snapshot of the system's threads (a new process has
/// only that one).
fn resume(pid: u32) -> io::Result<()> {
    // SAFETY: plain calls on handles opened and closed here.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let mut t = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut resumed = false;
        let mut more = Thread32First(snap, &mut t).is_ok();
        while more {
            if t.th32OwnerProcessID == pid {
                if let Ok(h) = OpenThread(THREAD_SUSPEND_RESUME, false, t.th32ThreadID) {
                    resumed |= ResumeThread(h) != u32::MAX;
                    let _ = CloseHandle(h);
                }
            }
            more = Thread32Next(snap, &mut t).is_ok();
        }
        let _ = CloseHandle(snap);
        if resumed {
            Ok(())
        } else {
            Err(io::Error::other("its first thread wasn't found"))
        }
    }
}

/// Pass yt-dlp's output on, line by line, until it ends.
fn read(mut out: PipeReader, id: u64, run: &Run, sink: &EventSink) {
    let mut lines = Lines { id, run, sink, line: Vec::new(), held: None, sent: None };
    let mut buf = [0u8; 4096];
    loop {
        match out.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => lines.feed(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    lines.end_line();
    lines.flush();
}

/// Turns yt-dlp's bytes into `AppEvent::Download`s. A line ends at `\n` or
/// `\r` (a console's progress line is redrawn with `\r`); progress goes at
/// most every PROGRESS_EVERY, the newest one held back going next.
struct Lines<'a> {
    id: u64,
    run: &'a Run,
    sink: &'a EventSink,
    line: Vec<u8>,
    /// The newest progress not sent yet, and when progress last went.
    held: Option<Line>,
    sent: Option<Instant>,
}

impl Lines<'_> {
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                self.end_line();
            } else if self.line.len() < LINE_MAX {
                self.line.push(b);
            }
        }
    }

    fn end_line(&mut self) {
        if self.line.is_empty() {
            return;
        }
        let text = String::from_utf8_lossy(&self.line);
        let parsed = ytdlp::parse(&text);
        // The video's id comes with its info line (`Line::Info` leaves it out).
        if let Some(video) = matches!(parsed, Some(Line::Info { .. })).then(|| ytdlp_partial::video_id(&text)).flatten() {
            self.run.set_video(video);
        }
        self.line.clear();
        match parsed {
            Some(p @ Line::Progress { .. }) if self.sent.is_some_and(|t| t.elapsed() < PROGRESS_EVERY) => self.held = Some(p),
            Some(p @ Line::Progress { .. }) => {
                self.held = None;
                self.sent = Some(Instant::now());
                self.send(p);
            }
            Some(other) => {
                self.flush();
                self.send(other);
            }
            None => {}
        }
    }

    /// Held-back progress goes before any other line and at the end, so the
    /// count before what follows (the finished size, say) is the last one.
    fn flush(&mut self) {
        if let Some(p) = self.held.take() {
            self.send(p);
        }
    }

    fn send(&self, line: Line) {
        // Stopped: the page shows that already; yt-dlp's last words are moot.
        if !self.run.stopped.load(Ordering::SeqCst) {
            self.sink.send(AppEvent::Download { id: self.id, line });
        }
    }
}

/// The line the page shows when yt-dlp couldn't start.
fn not_started(id: u64) -> AppEvent {
    AppEvent::Download { id, line: Line::Failed(Problem::Other(NOT_STARTED.into())) }
}

/// Delete the temporary folders of downloads that aren't running: left when
/// Poni was ended during one, or when deleting one failed.
fn sweep(temps: &Path, runs: &Runs) {
    let Ok(entries) = fs::read_dir(temps) else { return };
    for e in entries.flatten() {
        let running = e.file_name().to_str().and_then(|n| n.parse::<u64>().ok()).is_some_and(|id| runs.lock().contains_key(&id));
        if !running {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Delete a download's temporary folder. Ended processes take a moment to
/// let go of their files, so it tries for about a second.
fn remove(dir: &Path) {
    for left in (0..10).rev() {
        match fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) if left == 0 => crate::pwarn!("downloader: could not delete {} ({e})", dir.display()),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Delete the files yt-dlp left unfinished of `video` in `folder`
/// (`ytdlp_partial::is_partial`), once the person stopped download `id`.
/// Not while another download of the same video runs (its sound alone, say):
/// they may be that one's. Like [`remove`], it tries for about a second.
fn tidy(folder: &Path, video: &str, id: u64, runs: &Runs) {
    if runs.lock().iter().any(|(&other, run)| other != id && run.video().as_deref() == Some(video)) {
        crate::pinfo!("downloader: {id}: stopped; its video downloads again, so its unfinished files stay");
        return;
    }
    let mut gone = 0;
    for left in (0..10).rev() {
        let Ok(entries) = fs::read_dir(folder) else { return };
        let mut busy = false;
        for e in entries.flatten() {
            let ours = e.file_name().to_str().is_some_and(|n| ytdlp_partial::is_partial(n, video));
            if !ours || !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            match fs::remove_file(e.path()) {
                Ok(()) => gone += 1,
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) if left == 0 => crate::pwarn!("downloader: {id}: could not delete an unfinished file ({err})"),
                Err(_) => busy = true,
            }
        }
        if !busy {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    crate::pinfo!("downloader: {id}: {gone} unfinished file(s) deleted");
}

/// A job object whose processes all end with it: when `end` is called, or
/// when its last handle closes (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), even if
/// Poni itself is ended. What yt-dlp starts joins it on its own.
struct Job(HANDLE);

// SAFETY: a kernel handle, usable from any thread; closed once, in Drop.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    fn new() -> windows::core::Result<Self> {
        // SAFETY: plain calls; the Job owns the handle from here on.
        unsafe {
            let job = Job(CreateJobObjectW(None, PCWSTR::null())?);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            // A tool that crashes ends quietly, without Windows' error dialog.
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size)?;
            Ok(job)
        }
    }

    /// End every process in the job (none left: nothing happens).
    fn end(&self) {
        // SAFETY: our handle, open until Drop.
        unsafe {
            let _ = TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: closed once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const FILM: &str = "b29290cc-dc51-4a12-bcb2-2aa5fece7605";

    /// An empty folder of the test's own, as the Downloads folder.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("poni-downloader-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp folder");
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_stopped_download_leaves_no_unfinished_files() {
        let dir = folder("tidy");
        // Stopped during Blender's HLS film: the file, two fragments and the note of them.
        let film = format!("OVERGROWN： a feature film project by Blender Studio [{FILM}].mp4");
        for end in [".part", ".part-Frag3", ".part-Frag4.part", ".ytdl"] {
            fs::write(dir.join(format!("{film}{end}")), b"x").unwrap();
        }
        // The same film saved before, another video's download, and a folder.
        let kept = [film.clone(), "Big Buck Bunny [BigBuckBunny_124].ogv.part".to_string(), format!("odd [{FILM}].part")];
        fs::write(dir.join(&kept[0]), b"x").unwrap();
        fs::write(dir.join(&kept[1]), b"x").unwrap();
        fs::create_dir(dir.join(&kept[2])).unwrap();
        tidy(&dir, FILM, 1, &Runs::default());
        let mut want = kept.to_vec();
        want.sort();
        assert_eq!(names(&dir), want);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_download_of_the_same_video_keeps_them() {
        let dir = folder("same-video");
        let sound = format!("OVERGROWN [{FILM}] (audio).mp4.part");
        fs::write(dir.join(&sound), b"x").unwrap();
        let runs = Runs::default();
        let other = Arc::new(Run::new(Job::new().expect("a job object")));
        other.set_video(FILM.to_string());
        runs.lock().insert(2, other);
        tidy(&dir, FILM, 1, &runs);
        assert_eq!(names(&dir), [sound], "download 2, the sound alone, still writes it");
        runs.lock().remove(&2);
        tidy(&dir, FILM, 1, &runs);
        assert!(names(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
