//! Meeting notes on this PC (the Meeting notes add-on): the recording and
//! the handing over. The page and its moments are `poni_core::notes`. The
//! Mac app runs this file too, with its own two sources
//! (`mac/services/capture.rs`: the microphone, and the Mac's own sound
//! through a Core Audio tap); everything else here is the same.
//!
//! The recording takes the default microphone and the default playback
//! device's own sound (WASAPI loopback: whatever the PC plays, so the voices
//! of a call in Meet, Zoom or Teams, with no bot joining it), each in shared
//! mode as `services/mic.rs` opens them, mixed onto one 16 kHz mono track
//! (`poni_core::notes::Mixer`) and written as five-minute WAV parts in
//! `%LOCALAPPDATA%\Poni\notes\<meeting>\`. One source missing is fine (no
//! microphone, or nothing to play through); both missing is an error. When
//! Windows' default device changes mid-meeting (a headset plugged in), the
//! recording moves to the new one.
//!
//! Its thread, `poni-notes`, exists only while recording, waking every 40 ms
//! (a 500 ms device buffer leaves room for a late wake-up). After Stop, a
//! second thread, `poni-notes-send`, exists only while a meeting is handed
//! over: it sends the parts, says it's over, asks every few seconds until
//! the notes are written (`poni_core::notes::POLL_S`), saves them as
//! Markdown in `Documents\Oyenza Studio\Notes` and deletes the audio. The
//! calls carry the PC's Oyenza key, read from its sealed file on that thread
//! only, never the UI's or the log.

use crate::events::{AppEvent, EventSink};
#[cfg(target_os = "macos")]
use crate::services::capture::Source;
use crate::services::http::{Client, Method, MtaGuard};
use crate::services::mic::MicError;
#[cfg(windows)]
use crate::services::mic::{self, Device};
use poni_core::datetime::LocalDateTime;
#[cfg(windows)]
use poni_core::ielts::peak;
use poni_core::ielts::WavWriter;
use poni_core::notes::{self, CaptureIssue, Lang, Mixer, Problem, Reply, Saved, View, PART_SECS, RATE};
use poni_core::oyenza;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows::Win32::Media::Audio::{eCapture, eConsole, eRender, EDataFlow, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT};
#[cfg(windows)]
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");
/// How long `start` waits for the devices to open.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the recording checks whether Windows' default devices changed.
const DEVICE_CHECK: Duration = Duration::from_secs(2);
/// How often the recording empties its sources (a 500 ms device buffer, or
/// the Mac's queues, leave room for a late wake-up).
const POLL: Duration = Duration::from_millis(40);
const PART_SAMPLES: u64 = PART_SECS as u64 * RATE as u64;
/// The level meters: the loudest sample of the last ~100 ms, eased down.
#[cfg(windows)]
const METER_FALL: f32 = 0.82;
const CALL_TIMEOUT: Duration = Duration::from_secs(15);
/// A five-minute part (9.6 MB) on a slow uplink.
const PART_TIMEOUT: Duration = Duration::from_secs(180);
/// Tries for each call before the meeting counts as not sent.
const TRIES: u32 = 3;

struct Shared {
    stop: AtomicBool,
    /// f32 bits: 0..1.
    mic: AtomicU32,
    pc: AtomicU32,
}

/// A meeting's audio, once stopped.
#[derive(Clone, Debug)]
pub struct Take {
    pub dir: PathBuf,
    pub parts: Vec<PathBuf>,
    pub secs: f64,
}

/// A recording in progress.
pub struct Recording {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Result<Take, CaptureIssue>>>,
}

impl Recording {
    /// Start recording into a new folder under `root` (the meeting's local
    /// start written beside its parts, for [`kept`]): the recording, and
    /// whether the microphone and the PC's sound each opened.
    pub fn start(root: &Path, started: &LocalDateTime, sink: &EventSink) -> Result<(Recording, bool, bool), CaptureIssue> {
        let dir = root.join(format!("meeting-{}", crate::clock::unix_now()));
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(STARTED), fmt_started(started))).map_err(|e| {
            crate::pwarn!("notes: no folder for the recording ({e})");
            CaptureIssue::Failed
        })?;
        let shared = Arc::new(Shared { stop: AtomicBool::new(false), mic: AtomicU32::new(0), pc: AtomicU32::new(0) });
        let (ready_tx, ready_rx) = mpsc::channel();
        let (worker, folder, sink) = (shared.clone(), dir.clone(), sink.clone());
        let thread = std::thread::Builder::new()
            .name("poni-notes".into())
            .spawn(move || capture(&folder, &worker, &sink, ready_tx))
            .map_err(|_| CaptureIssue::Failed)?;
        match ready_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok((mic, pc))) => Ok((Recording { shared, thread: Some(thread) }, mic, pc)),
            Ok(Err(e)) => {
                let _ = thread.join();
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
            Err(_) => {
                shared.stop.store(true, Ordering::SeqCst);
                Err(CaptureIssue::Failed)
            }
        }
    }

    /// The microphone's and the PC's sound's loudness, 0..1.
    pub fn levels(&self) -> (f32, f32) {
        (f32::from_bits(self.shared.mic.load(Ordering::Relaxed)), f32::from_bits(self.shared.pc.load(Ordering::Relaxed)))
    }

    pub fn stop(mut self) -> Result<Take, CaptureIssue> {
        self.shared.stop.store(true, Ordering::SeqCst);
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(r)) => r,
            _ => Err(CaptureIssue::Failed),
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One source: its device while it works, and when to look again when not.
#[cfg(windows)]
struct Source {
    flow: EDataFlow,
    what: &'static str,
    dev: Option<Device>,
    /// The default device it opened (Windows' id), to notice a new default.
    id: Option<String>,
    pcm: Vec<i16>,
    level: f32,
}

#[cfg(windows)]
impl Source {
    fn mic() -> Source {
        Source::new(eCapture, "notes: microphone")
    }

    fn pc() -> Source {
        Source::new(eRender, "notes: the PC's sound")
    }

    fn new(flow: EDataFlow, what: &'static str) -> Source {
        Source { flow, what, dev: None, id: None, pcm: Vec::with_capacity(RATE as usize / 4), level: 0.0 }
    }

    /// Not open, or Windows' default device changed (a headset plugged in, or
    /// the one it had went away): open the default again.
    fn moved(&self) -> bool {
        self.dev.is_none() || default_id(self.flow) != self.id
    }

    fn open(&mut self) -> Result<(), MicError> {
        if self.flow == eCapture && mic::privacy_denies() {
            return Err(MicError::Denied);
        }
        let d = mic::open_endpoint(self.flow, self.what)?;
        unsafe { d.client.Start() }.map_err(|e| mic::mic_error("the device didn't start", &e))?;
        self.id = default_id(self.flow);
        self.dev = Some(d);
        Ok(())
    }

    fn close(&mut self) {
        if let Some(d) = self.dev.take() {
            unsafe {
                let _ = d.client.Stop();
            }
        }
    }

    /// Everything the device has, as 16 kHz mono; on a failure the device
    /// is let go (looked for again at the next device check).
    fn read(&mut self) {
        self.pcm.clear();
        let Some(d) = self.dev.as_mut() else { return };
        let r = (|| -> windows::core::Result<()> {
            loop {
                let frames = unsafe { d.capture.GetNextPacketSize() }?;
                if frames == 0 {
                    return Ok(());
                }
                let (mut data, mut n, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                unsafe { d.capture.GetBuffer(&mut data, &mut n, &mut flags, None, None) }?;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    d.downmix.silence(n as usize, &mut self.pcm);
                } else {
                    let bytes = unsafe { std::slice::from_raw_parts(data, n as usize * d.block) };
                    d.downmix.push(bytes, &mut self.pcm);
                }
                unsafe { d.capture.ReleaseBuffer(n) }?;
            }
        })();
        if let Err(e) = r {
            crate::pwarn!("notes: {} went away (0x{:08X})", self.what, e.code().0 as u32);
            self.close();
        }
        self.level = (self.level * METER_FALL).max(peak(&self.pcm));
    }
}

/// Windows' default device of `flow` now, as its id.
#[cfg(windows)]
fn default_id(flow: EDataFlow) -> Option<String> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let p = devices.GetDefaultAudioEndpoint(flow, eConsole).ok()?.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Why neither source opened, going by the microphone's reason: a blocked
/// microphone is the one the person can fix.
fn issue(mic: &Result<(), MicError>) -> CaptureIssue {
    match mic {
        Err(MicError::Denied) => CaptureIssue::MicDenied,
        Err(MicError::NoDevice) => CaptureIssue::NoSound,
        _ => CaptureIssue::Failed,
    }
}

/// The parts written so far: `part-000.wav`, `part-001.wav`, …
struct Parts {
    dir: PathBuf,
    done: Vec<PathBuf>,
    open: Option<WavWriter>,
    /// Samples in the open part, and in all of them.
    in_part: u64,
    total: u64,
}

impl Parts {
    fn write(&mut self, mut pcm: &[i16]) -> std::io::Result<()> {
        while !pcm.is_empty() {
            if self.open.is_none() {
                self.open = Some(WavWriter::create(&self.dir.join(format!("part-{:03}.wav", self.done.len())))?);
                self.in_part = 0;
            }
            let n = ((PART_SAMPLES - self.in_part) as usize).min(pcm.len());
            self.open.as_mut().unwrap().write(&pcm[..n])?;
            (self.in_part, self.total) = (self.in_part + n as u64, self.total + n as u64);
            pcm = &pcm[n..];
            if self.in_part == PART_SAMPLES {
                self.close()?;
            }
        }
        Ok(())
    }

    fn close(&mut self) -> std::io::Result<()> {
        if let Some(w) = self.open.take() {
            let (path, _) = w.finish()?;
            self.done.push(path);
        }
        Ok(())
    }
}

/// The `poni-notes` thread: open both sources, say which did, then record
/// until told to stop (or the longest meeting the server takes).
fn capture(dir: &Path, shared: &Shared, sink: &EventSink, ready: mpsc::Sender<Result<(bool, bool), CaptureIssue>>) -> Result<Take, CaptureIssue> {
    #[cfg(windows)]
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let out = (|| {
        let mut mic = Source::mic();
        let mut pc = Source::pc();
        let (m, p) = (mic.open(), pc.open());
        if let (Err(em), Err(ep)) = (&m, &p) {
            crate::pwarn!("notes: nothing to record (microphone: {em}; the PC's sound: {ep})");
            let e = issue(&m);
            let _ = ready.send(Err(e));
            return Err(e);
        }
        if let Err(e) = &m {
            crate::pwarn!("notes: recording without the microphone ({e})");
        }
        let _ = ready.send(Ok((m.is_ok(), p.is_ok())));
        let mut parts = Parts { dir: dir.to_path_buf(), done: Vec::new(), open: None, in_part: 0, total: 0 };
        let mut mixer = Mixer::default();
        let mut track: Vec<i16> = Vec::with_capacity(RATE as usize / 4);
        let t0 = Instant::now();
        let mut checked = Instant::now();
        let max = (notes::MAX_SECS * RATE as f64) as u64;
        let mut stopped_by_length = false;
        loop {
            let last = shared.stop.load(Ordering::SeqCst);
            mic.read();
            pc.read();
            mixer.push_mic(&mic.pcm);
            mixer.push_pc(&pc.pcm);
            shared.mic.store(mic.level.to_bits(), Ordering::Relaxed);
            shared.pc.store(pc.level.to_bits(), Ordering::Relaxed);
            track.clear();
            if last {
                mixer.drain(&mut track);
            } else {
                mixer.take((t0.elapsed().as_secs_f64() * RATE as f64) as u64, &mut track);
            }
            let room = max.saturating_sub(parts.total) as usize;
            if track.len() >= room {
                track.truncate(room);
                stopped_by_length = true;
            }
            if let Err(e) = parts.write(&track) {
                crate::pwarn!("notes: can't write the recording ({e})");
                sink.send(AppEvent::NotesStopped(CaptureIssue::Failed));
                break;
            }
            if last {
                break;
            }
            if stopped_by_length {
                crate::pinfo!("notes: the recording reached the longest meeting taken");
                sink.send(AppEvent::NotesStopped(CaptureIssue::Failed));
                break;
            }
            // A new default device (a headset plugged in), or one that went away: open the default again.
            if checked.elapsed() >= DEVICE_CHECK {
                checked = Instant::now();
                for s in [&mut mic, &mut pc] {
                    if s.moved() {
                        s.close();
                        if s.open().is_ok() {
                            crate::pinfo!("notes: {} moved to the default device", s.what);
                        }
                    }
                }
            }
            std::thread::sleep(POLL);
        }
        mic.close();
        pc.close();
        shared.mic.store(0, Ordering::Relaxed);
        shared.pc.store(0, Ordering::Relaxed);
        parts.close().map_err(|_| CaptureIssue::Failed)?;
        let secs = parts.total as f64 / RATE as f64;
        crate::pinfo!("notes: recorded {secs:.0} s in {} part(s)", parts.done.len());
        Ok(Take { dir: dir.to_path_buf(), parts: parts.done, secs })
    })();
    #[cfg(windows)]
    if com.is_ok() {
        unsafe { CoUninitialize() };
    }
    out
}

/// Beside a meeting's parts: when it began, "2026-10-29 14:05" (local time).
const STARTED: &str = "started.txt";

fn fmt_started(t: &LocalDateTime) -> String {
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
}

fn parse_started(s: &str) -> Option<LocalDateTime> {
    let (d, t) = s.trim().split_once(' ')?;
    let mut d = d.split('-').map(|x| x.parse::<i32>().ok());
    let mut t = t.split(':').map(|x| x.parse::<u8>().ok());
    Some(LocalDateTime { year: d.next()??, month: d.next()?? as u8, day: d.next()?? as u8, hour: t.next()??, minute: t.next()??, second: 0 })
}

/// A meeting the app closed on before its notes were saved (a crash, a
/// restart, the PC turned off mid-meeting): the newest one left, with what
/// its parts hold (a part cut short has the length 0 in its header; the
/// server reads such a part to its end). Older ones, and ones with under a
/// second in them, go.
pub fn kept() -> Option<(Take, LocalDateTime)> {
    let root = crate::paths::notes_audio_dir();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    let newest = dirs.pop()?;
    for old in dirs {
        let _ = std::fs::remove_dir_all(old);
    }
    let mut parts: Vec<PathBuf> = std::fs::read_dir(&newest).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wav")).collect();
    parts.sort();
    let bytes: u64 = parts.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len().saturating_sub(44)).sum();
    let secs = bytes as f64 / 2.0 / RATE as f64;
    let started = std::fs::read_to_string(newest.join(STARTED)).ok().and_then(|s| parse_started(&s));
    match started {
        Some(started) if secs >= 1.0 => Some((Take { dir: newest, parts, secs }, started)),
        _ => {
            let _ = std::fs::remove_dir_all(newest);
            None
        }
    }
}

/// Whatever is left of meetings whose notes were saved or thrown away, and
/// of ones the app closed on (they're no longer on any page): at the add-on
/// switched off, or at a new recording.
pub fn clear_audio(keep: Option<&Path>) {
    if let Ok(entries) = std::fs::read_dir(crate::paths::notes_audio_dir()) {
        for e in entries.flatten() {
            if Some(e.path().as_path()) != keep {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

// ---------------------------------------------------------------- handing it over

/// A meeting to hand over, as the sender thread needs it.
pub struct Job {
    pub take: Take,
    pub started: LocalDateTime,
    pub lang: Lang,
    /// The self-test's own server and key, in place of api.oyenza.app and
    /// this PC's sign-in.
    pub api: Option<(String, String)>,
    /// Where the notes go.
    pub out: PathBuf,
}

/// Hand `job` over on its own thread; it reports with `AppEvent::Notes`
/// and ends with the notes saved or a problem.
pub fn send(job: Job, sink: &EventSink) {
    let sink = sink.clone();
    let spawned = std::thread::Builder::new().name("poni-notes-send".into()).spawn(move || {
        let _com = MtaGuard::new();
        let reply = match Client::uncached(USER_AGENT) {
            Ok(client) => Sender { client, job: &job, sink: &sink }.run(),
            Err(e) => {
                crate::pwarn!("notes: no HTTP client ({e})");
                Reply::Failed(Problem::Offline)
            }
        };
        sink.send(AppEvent::Notes(Box::new(reply)));
    });
    if spawned.is_err() {
        crate::pwarn!("notes: no thread to send the meeting");
    }
}

struct Sender<'a> {
    client: Client,
    job: &'a Job,
    sink: &'a EventSink,
}

impl Sender<'_> {
    fn base(&self) -> String {
        self.job.api.as_ref().map_or(notes::BASE.to_string(), |a| a.0.clone())
    }

    fn key(&self) -> Option<String> {
        match &self.job.api {
            Some((_, k)) => Some(k.clone()),
            None => crate::services::secret::load(&crate::paths::oyenza_link_path()).and_then(|s| oyenza::parse_link(&s)).map(|l| l.key),
        }
    }

    fn run(&self) -> Reply {
        let started = Instant::now();
        // A server that forgot the meeting (a restart) gets it again, once.
        let mut out = self.hand_over();
        if out == Err(Problem::Gone) {
            out = self.hand_over();
        }
        match out {
            Ok(saved) => {
                crate::pinfo!("notes: {:.0} s of meeting written up in {:.0} s", self.job.take.secs, started.elapsed().as_secs_f64());
                let _ = std::fs::remove_dir_all(&self.job.take.dir);
                Reply::Saved(Box::new(saved))
            }
            Err(p) => {
                crate::pwarn!("notes: not written ({p:?}) after {:.0} s; the recording is kept", started.elapsed().as_secs_f64());
                Reply::Failed(p)
            }
        }
    }

    fn hand_over(&self) -> Result<Saved, Problem> {
        let key = self.key().ok_or(Problem::SignedOut)?;
        let auth = format!("Bearer {key}");
        let headers = [("Authorization", auth.as_str())];
        let base = self.base();
        let body = self.call(|| self.client.send(Method::Post, &notes::start_url(&base), &headers, Some(&notes::start_body(self.job.lang)), CALL_TIMEOUT))?;
        let id = notes::parse_started(&body).ok_or(Problem::Server)?;
        let parts = self.job.take.parts.len() as u32;
        for (n, path) in self.job.take.parts.iter().enumerate() {
            let bytes = std::fs::read(path).map_err(|_| Problem::Capture(CaptureIssue::Failed))?;
            let url = notes::part_url(&base, &id, n as u32);
            self.call(|| self.client.post_bytes(&url, &headers, &bytes, "audio/wav", PART_TIMEOUT))?;
            self.sink.send(AppEvent::Notes(Box::new(Reply::Sent { sent: n as u32 + 1, parts })));
        }
        self.call(|| self.client.send(Method::Post, &notes::finish_url(&base, &id), &headers, Some("{}"), CALL_TIMEOUT))?;
        self.sink.send(AppEvent::Notes(Box::new(Reply::Writing)));
        let asked = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs_f64(notes::POLL_S));
            let body = self.call(|| self.client.send(Method::Get, &notes::view_url(&base, &id), &headers, None, CALL_TIMEOUT))?;
            match notes::parse_view(&body) {
                Some(View::Done(n)) => {
                    let saved = self.save(*n)?;
                    // Fetched and saved here: the server needn't keep them for the day.
                    let _ = self.client.send(Method::Delete, &notes::view_url(&base, &id), &headers, None, CALL_TIMEOUT);
                    return Ok(saved);
                }
                Some(View::Failed(p)) => return Err(p),
                Some(View::Working) => {}
                // The finish didn't land (its answer was lost, so it was never sent again): say it again.
                Some(View::Open) => {
                    self.call(|| self.client.send(Method::Post, &notes::finish_url(&base, &id), &headers, Some("{}"), CALL_TIMEOUT))?;
                }
                None => return Err(Problem::Server),
            }
            if asked.elapsed().as_secs_f64() > notes::WRITING_MAX_S {
                return Err(Problem::TooSlow);
            }
        }
    }

    /// One call, tried again on a dropped connection or a busy server; the
    /// body of a 2xx, or what went wrong.
    fn call(&self, f: impl Fn() -> Result<crate::services::http::Response, crate::services::http::FetchError>) -> Result<String, Problem> {
        let mut last = Problem::Offline;
        for attempt in 0..TRIES {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(2 << attempt));
            }
            match f() {
                Ok(r) if (200..300).contains(&r.status) => return Ok(r.body),
                Ok(r) => {
                    last = notes::problem(r.status, &r.body, r.retry_after_s);
                    // Only a server error is worth asking again at once (a 4xx says the same next time).
                    if r.status < 500 {
                        return Err(last);
                    }
                }
                Err(_) => last = Problem::Offline,
            }
        }
        Err(last)
    }

    /// The notes as Markdown in the notes folder (a name already there gets " (2)").
    fn save(&self, mut n: notes::Notes) -> Result<Saved, Problem> {
        if n.secs == 0 {
            n.secs = self.job.take.secs.round() as u32;
        }
        let dir = &self.job.out;
        std::fs::create_dir_all(dir).map_err(|_| Problem::Server)?;
        let name = notes::file_name(&self.job.started, &n.title);
        let mut path = dir.join(&name);
        for i in 2..100 {
            if !path.exists() {
                break;
            }
            path = dir.join(name.replace(".md", &format!(" ({i}).md")));
        }
        // UTF-8 with its mark, so Notepad and Word read the Indonesian and the dashes right.
        let md = format!("\u{feff}{}", notes::markdown(&n, &self.job.started, true));
        std::fs::write(&path, md).map_err(|e| {
            crate::pwarn!("notes: can't save the notes ({e})");
            Problem::Server
        })?;
        crate::pinfo!("notes: saved \"{}\" ({} lines of transcript)", n.title, n.transcript.len());
        Ok(Saved { notes: n, file: path.to_string_lossy().into_owned(), started: self.job.started })
    }
}
