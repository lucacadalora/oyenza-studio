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
//! if Poni itself is ended. The loader only deletes what it unpacked when it
//! ends by itself, so each download unpacks into its own folder, deleted
//! when the download ends.

use crate::events::{AppEvent, EventSink};
use poni_core::addons::{DOWNLOADER, FFMPEG_EXE, QJS_EXE, YTDLP_EXE};
use poni_core::ytdlp::{self, Line, Problem, Request, Tools};
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
            let run = Arc::new(Run { job, stopped: AtomicBool::new(false) });
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
    /// ok) once yt-dlp and what it started are gone.
    pub fn stop(&self, id: u64) {
        if let Some(run) = self.runs.lock().get(&id) {
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

/// One download's job, and whether the person stopped it.
struct Run {
    job: Job,
    stopped: AtomicBool,
}

impl Run {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.job.end();
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
    let cache = crate::paths::addons_dir().join(DOWNLOADER.id).join("cache");
    // In the add-on's version folder: switching it off deletes them, its
    // check at start (which clears other folders) doesn't.
    let temps = crate::services::addons::file_path(&DOWNLOADER, "temp");
    sweep(&temps, runs);
    let temp = temps.join(id.to_string());
    let args = ytdlp::args(req, &Tools { ytdlp: &ytdlp, qjs: &qjs, ffmpeg: &ffmpeg, cache: &cache.to_string_lossy() });
    let began = Instant::now();
    let spawned = fs::create_dir_all(&cache).and_then(|_| fs::create_dir_all(&temp)).and_then(|_| spawn(&ytdlp, &args, &temp, run));
    let ok = match spawned {
        Ok((out, mut child)) => {
            crate::pinfo!("downloader: {id}: yt-dlp started");
            read(out, id, &run.stopped, sink);
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
fn read(mut out: PipeReader, id: u64, stopped: &AtomicBool, sink: &EventSink) {
    let mut lines = Lines { id, stopped, sink, line: Vec::new(), held: None, sent: None };
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
    stopped: &'a AtomicBool,
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
        let parsed = ytdlp::parse(&String::from_utf8_lossy(&self.line));
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

    /// Held-back progress goes before any other line, and at the end.
    fn flush(&mut self) {
        if let Some(p) = self.held.take() {
            self.send(p);
        }
    }

    fn send(&self, line: Line) {
        // Stopped: the page shows that already; yt-dlp's last words are moot.
        if !self.stopped.load(Ordering::SeqCst) {
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
