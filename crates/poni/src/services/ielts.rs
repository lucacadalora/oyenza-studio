//! Oyen's IELTS coach on this PC: the calls to the server, the microphone
//! and Oyen's voice. The session itself is `poni_core::ielts::Coach` in
//! `AppState`; the app carries out what it asks for ([`Effect`]s) with a
//! [`Kit`].
//!
//! Nothing here exists at rest. The worker thread (`poni-ielts`) starts with
//! the first call (the IELTS page opening asks for the ladder) and ends once
//! the page is closed with nothing in flight; the microphone's thread runs
//! only while recording (`services/mic.rs`); the player only while a clip
//! plays (`services/voice.rs`). Oyen's clips and the answer are files in
//! `%LOCALAPPDATA%\Poni\ielts\`, each deleted once used; anything left over
//! (the app closed mid-session) goes when the next session starts or the
//! add-on is switched off.
//!
//! The calls carry the PC's Oyenza key, read from its sealed file when the
//! worker starts (`services::secret`), like the Oyenza worker's: it never
//! reaches the UI thread or the log. An answer, its upload and the server's
//! thinking, may take a while: 60 s before it counts as no answer.
//!
//! [`Effect`]: poni_core::ielts::Effect

use crate::events::{AppEvent, EventSink};
use crate::services::http::{Client, Method, MtaGuard};
use crate::services::mic::{Mic, MicError};
use crate::services::voice::VoicePlayer;
use poni_core::ielts::{self, Call, MicIssue, Problem, Reply};
use poni_core::oyenza;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");
const ME_TIMEOUT: Duration = Duration::from_secs(10);
/// A new session's first turn is voiced before it comes back.
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Upload, listening, the next turn and its voice (the server aims for ~6 s).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);
const END_TIMEOUT: Duration = Duration::from_secs(8);

/// The IELTS page's workers: the calls, the microphone and the voice, each
/// only while needed.
#[derive(Default)]
pub struct Kit {
    worker: Option<Worker>,
    mic: Option<Mic>,
    pub voice: VoicePlayer,
    /// The self-test's own server and key, in place of api.oyenza.app and
    /// this PC's sign-in.
    pub api: Option<(String, String)>,
    /// The self-test's answer: recording "records" this file instead of the
    /// microphone (no microphone on a test machine).
    pub stand_in: Option<PathBuf>,
    recording_stand_in: Option<Instant>,
    /// Answers recorded so far (their file names).
    answers: u32,
    /// Used files that wouldn't go yet (the player still letting go of a
    /// clip): tried again on the next tick.
    doomed: Vec<PathBuf>,
}

impl Kit {
    /// Send `call` to the worker, starting it first if needed.
    pub fn call(&mut self, call: Call, sink: &EventSink) {
        if self.worker.is_none() {
            self.worker = Worker::start(sink.clone(), self.api.clone());
        }
        let kind = call.kind();
        let sent = self.worker.as_ref().is_some_and(|w| {
            w.busy.fetch_add(1, Ordering::SeqCst);
            w.tx.send(call).is_ok()
        });
        if !sent {
            self.worker = None;
            sink.send(AppEvent::Ielts(Box::new(Reply::Failed { call: kind, problem: Problem::Server, say: Vec::new() })));
        }
    }

    /// The page is closed: the worker ends once nothing is in flight.
    pub fn rest(&mut self) {
        if self.worker.as_ref().is_some_and(|w| w.busy.load(Ordering::SeqCst) == 0) {
            self.worker = None;
        }
    }

    /// Start recording an answer.
    pub fn record(&mut self, sink: &EventSink) -> Result<(), MicIssue> {
        self.drop_record();
        if self.stand_in.is_some() {
            self.recording_stand_in = Some(Instant::now());
            return Ok(());
        }
        let dir = crate::paths::ielts_dir();
        let _ = std::fs::create_dir_all(&dir);
        self.answers += 1;
        let path = dir.join(format!("answer-{}.wav", self.answers));
        match Mic::start(&path, sink) {
            Ok(m) => {
                self.mic = Some(m);
                Ok(())
            }
            Err(e) => {
                crate::pwarn!("ielts: no recording ({e})");
                Err(issue(&e))
            }
        }
    }

    pub fn recording(&self) -> bool {
        self.mic.is_some() || self.recording_stand_in.is_some()
    }

    /// The microphone's level, 0..1.
    pub fn level(&self) -> f32 {
        match (&self.mic, self.recording_stand_in) {
            (Some(m), _) => m.level(),
            // The stand-in "speaks" at a steady level.
            (None, Some(_)) => 0.5,
            _ => 0.0,
        }
    }

    /// Stop recording: the answer's file and length.
    pub fn stop_record(&mut self) -> Result<(PathBuf, f32), MicIssue> {
        if self.recording_stand_in.take().is_some() {
            let path = self.stand_in.clone().unwrap_or_default();
            let secs = std::fs::read(&path).ok().and_then(|b| ielts::wav_info(&b)).map_or(0.0, |i| i.secs);
            return Ok((path, secs));
        }
        match self.mic.take().map(Mic::stop) {
            Some(Ok(answer)) => Ok(answer),
            Some(Err(e)) => {
                crate::pwarn!("ielts: the recording failed ({e})");
                Err(issue(&e))
            }
            None => Err(MicIssue::Failed),
        }
    }

    /// Stop recording and throw the answer away.
    pub fn drop_record(&mut self) {
        self.recording_stand_in = None;
        if let Some(Ok((path, _))) = self.mic.take().map(Mic::stop) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// The microphone said it failed mid-answer (`AppEvent::MicFailed`): stop
    /// it and say why, as `stop` tells it (on a Mac, the permission asked
    /// for at the first recording and refused reads as `Denied`).
    pub fn mic_failed(&mut self) -> MicIssue {
        self.recording_stand_in = None;
        match self.mic.take().map(Mic::stop) {
            Some(Err(e)) => issue(&e),
            Some(Ok((path, _))) => {
                let _ = std::fs::remove_file(path);
                MicIssue::Failed
            }
            None => MicIssue::Failed,
        }
    }

    /// Delete a used clip or answer (never the self-test's stand-in, which
    /// is used again).
    pub fn delete(&mut self, path: PathBuf) {
        if self.stand_in.as_ref() != Some(&path) && !remove(&path) {
            self.doomed.push(path);
        }
    }

    /// Try again what wouldn't go; true while some still wait.
    pub fn retry_deletes(&mut self) -> bool {
        self.doomed.retain(|p| !remove(p));
        !self.doomed.is_empty()
    }

    /// Everything stops: no voice, no recording, no worker once its call is
    /// back, and no files.
    pub fn close(&mut self) {
        self.voice.stop();
        self.drop_record();
        self.rest();
        self.doomed.clear();
        clear_files();
    }
}

/// Gone (or never there).
fn remove(p: &std::path::Path) -> bool {
    match std::fs::remove_file(p) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

fn issue(e: &MicError) -> MicIssue {
    match e {
        MicError::Denied => MicIssue::Denied,
        MicError::NoDevice => MicIssue::NoDevice,
        MicError::Failed(_) => MicIssue::Failed,
    }
}

/// Delete whatever is left in the IELTS folder (clips and answers).
pub fn clear_files() {
    let dir = crate::paths::ielts_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// ---------------------------------------------------------------- the worker

struct Worker {
    tx: mpsc::Sender<Call>,
    /// Calls sent and not answered yet.
    busy: Arc<AtomicUsize>,
}

impl Worker {
    fn start(sink: EventSink, api: Option<(String, String)>) -> Option<Worker> {
        let (tx, rx) = mpsc::channel::<Call>();
        let busy = Arc::new(AtomicUsize::new(0));
        let left = busy.clone();
        let spawned = std::thread::Builder::new().name("poni-ielts".into()).spawn(move || {
            let _com = MtaGuard::new();
            let base = api.as_ref().map_or(ielts::BASE.to_string(), |a| a.0.clone());
            let fixed = api.is_some();
            // The ladder changes with every session: never from Windows' cache.
            let mut w = Client::uncached(USER_AGENT)
                .map(|client| Calls { client, base, key: api.map(|a| a.1), fixed, turns: 0 })
                .map_err(|e| crate::pwarn!("ielts: no HTTP client ({e})"));
            // Ends once the page lets the worker go (its sender drops). Every
            // call gets an answer, even without a client.
            for call in rx {
                let kind = call.kind();
                let reply = w.as_mut().map_or(Reply::Failed { call: kind, problem: Problem::Offline, say: Vec::new() }, |w| w.run(call));
                left.fetch_sub(1, Ordering::SeqCst);
                sink.send(AppEvent::Ielts(Box::new(reply)));
            }
        });
        match spawned {
            Ok(_) => Some(Worker { tx, busy }),
            Err(e) => {
                crate::pwarn!("ielts: no worker thread ({e})");
                None
            }
        }
    }
}

struct Calls {
    client: Client,
    base: String,
    key: Option<String>,
    /// The key is the self-test's, not this PC's sign-in.
    fixed: bool,
    /// Turns saved so far (their clips' names).
    turns: u32,
}

impl Calls {
    fn run(&mut self, call: Call) -> Reply {
        let kind = call.kind();
        let started = Instant::now();
        let reply = self.answer(call).unwrap_or_else(|(problem, say)| Reply::Failed { call: kind, problem, say });
        let ms = started.elapsed().as_millis();
        match &reply {
            Reply::Failed { problem, .. } => crate::pwarn!("ielts: {kind:?} didn't go through ({problem:?}) after {ms} ms"),
            _ => crate::pinfo!("ielts: {kind:?} answered in {ms} ms"),
        }
        reply
    }

    fn answer(&mut self, call: Call) -> Result<Reply, (Problem, Vec<ielts::Say>)> {
        let plain = |p: Problem| (p, Vec::new());
        let tz = oyenza::tz_param(oyenza::offset_minutes(&crate::clock::local_now(), crate::clock::unix_now()));
        match call {
            Call::Me => {
                let body = self.send(Method::Get, &ielts::me_url(&self.base, &tz), None, ME_TIMEOUT)?;
                ielts::parse_me(&body).map(Reply::Me).ok_or(plain(Problem::Server))
            }
            Call::Start { level, spice } => {
                let body = self.send(Method::Post, &ielts::session_url(&self.base), Some(&ielts::session_body(level, spice, &tz)), START_TIMEOUT)?;
                let mut s = ielts::parse_started(&body).ok_or(plain(Problem::Server))?;
                self.save(&mut s.turn, None);
                // Oyen's thinking lines, kept for the session's waits.
                let dir = crate::paths::ielts_dir();
                for (i, f) in s.fillers.iter_mut().enumerate() {
                    f.save_clip(&dir, &format!("f{i}"));
                }
                crate::pinfo!("ielts: a session at level {level} began ({} thinking lines)", s.fillers.len());
                Ok(Reply::Started { id: s.id, turn: s.turn, fillers: s.fillers })
            }
            Call::Answer { id, wav, secs } => {
                let raw = std::fs::read(&wav).map_err(|_| plain(Problem::Server))?;
                // The canonical 44-byte header (a Mac recording carries a 4 KB padding chunk).
                let bytes = ielts::canonical_wav(&raw).unwrap_or(raw);
                if bytes.len() > ielts::RAW_MAX {
                    return Err(plain(Problem::TooBig));
                }
                let json = ielts::answer_body(&bytes, secs);
                drop(bytes);
                crate::pinfo!("ielts: sending a {secs:.1} s answer ({} KB)", json.len() / 1024);
                let body = self.send(Method::Post, &ielts::answer_url(&self.base, &id), Some(&json), ANSWER_TIMEOUT)?;
                drop(json);
                let mut a = ielts::parse_answered(&body).ok_or(plain(Problem::Server))?;
                self.save_reply(&mut a);
                Ok(Reply::Answered { heard: a.heard, turn: a.turn, done: a.done, result: a.result })
            }
            Call::Skip { id } => {
                let body = self.send(Method::Post, &ielts::skip_url(&self.base, &id), Some("{}"), START_TIMEOUT)?;
                // The next question, or (skipping the last) the verdict.
                let mut a = ielts::parse_skipped(&body).ok_or(plain(Problem::Server))?;
                self.save_reply(&mut a);
                Ok(Reply::Skipped { turn: a.turn, done: a.done, result: a.result })
            }
            Call::End { id } => {
                // Ended on this PC whatever the server says: it forgets idle sessions anyway.
                let _ = self.send(Method::Delete, &ielts::end_url(&self.base, &id), None, END_TIMEOUT);
                Ok(Reply::Ended)
            }
        }
    }

    /// Save an answer's clips: its turn's, and the roast's unless the
    /// verdict turn already says it.
    fn save_reply(&mut self, a: &mut ielts::Answered) {
        let roast = match a.result.as_mut() {
            Some(v) if ielts::roast_in_turn(&a.turn, v) => {
                v.roast.audio = None;
                None
            }
            Some(v) => Some(&mut v.roast),
            None => None,
        };
        self.save(&mut a.turn, roast);
    }

    /// Save a turn's clips (and the roast's) as files.
    fn save(&mut self, turn: &mut ielts::Turn, roast: Option<&mut ielts::Say>) {
        let dir = crate::paths::ielts_dir();
        let _ = std::fs::create_dir_all(&dir);
        self.turns += 1;
        let prefix = format!("t{}", self.turns);
        turn.save_clips(&dir, &prefix);
        if let Some(r) = roast {
            r.save_clip(&dir, &format!("{prefix}-roast"));
        }
    }

    /// The key: the self-test's, or this PC's sign-in (read when first needed).
    fn key(&mut self) -> Option<String> {
        if self.key.is_none() {
            self.key = crate::services::secret::load(&crate::paths::oyenza_link_path()).and_then(|s| oyenza::parse_link(&s)).map(|l| l.key);
        }
        self.key.clone()
    }

    /// The answer's body, or what went wrong with Oyen's own line about it
    /// when the server sent one (a capped day, a locked level).
    fn send(&mut self, method: Method, url: &str, json: Option<&str>, timeout: Duration) -> Result<String, (Problem, Vec<ielts::Say>)> {
        let key = self.key().ok_or((Problem::SignedOut, Vec::new()))?;
        let auth = format!("Bearer {key}");
        match self.client.send(method, url, &[("Authorization", &auth)], json, timeout) {
            Ok(r) if r.status == 200 => Ok(r.body),
            Ok(r) => {
                if r.status == 401 && !self.fixed {
                    // Refused: the next call reads the sign-in again (signed in anew, say).
                    self.key = None;
                }
                let mut say = ielts::problem_say(&r.body);
                let dir = crate::paths::ielts_dir();
                let _ = std::fs::create_dir_all(&dir);
                self.turns += 1;
                for (i, s) in say.iter_mut().enumerate() {
                    s.save_clip(&dir, &format!("x{}-{i}", self.turns));
                }
                Err((ielts::problem(r.status, &r.body, r.retry_after_s), say))
            }
            Err(_) => Err((Problem::Offline, Vec::new())),
        }
    }
}
