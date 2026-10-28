//! Oyen's IELTS speaking coach, the IELTS add-on: Oyen plays a GoPrep-style
//! IELTS Speaking examiner. He asks the Part 1 to 3 questions out loud,
//! listens to each answer through the microphone, follows up on what was
//! said, and ends with a band score in the four criteria and a roast in
//! Jakarta gaul (Sopan, Gaul or Ngegas). Levels 1 to 5 open one after the
//! other, kept per Oyenza account.
//!
//! The thinking happens on the server (catat-server `routes/ielts.ts`, under
//! `/island/ielts` on api.oyenza.app, with the PC's Oyenza key): it hears the
//! answer, writes Oyen's next turn and voices it as mp3 clips. Everything
//! here is pure and runs anywhere:
//! - the five routes: URLs, request bodies, the answers ([`Me`], [`Turn`],
//!   [`Verdict`]) and what went wrong in plain words ([`Problem`]);
//! - base64 and a WAV writer for the answer's upload (16 kHz mono 16-bit),
//!   and [`Downmix`], which turns what the microphone gives into that;
//! - the session as a state machine ([`Coach`]): the app tells it what
//!   happened (a reply, a clip ended, a tap, the clock) and it answers with
//!   what to do ([`Effect`]). The apps own the HTTP (`services/ielts.rs`),
//!   the microphone and the player (`services/{mic,voice}.rs`).
//!
//! Nothing here runs by itself: the coach moves only when the app calls it,
//! and the app only does while the IELTS page is open or a session runs.

use crate::oyenza::API;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Where the routes live on the API (`BASE` + `PATH` + `/me`, …).
pub const BASE: &str = API;
pub const PATH: &str = "/island/ielts";
/// The answer's audio: 16 kHz mono 16-bit PCM in a WAV file.
pub const RATE: u32 = 16_000;
/// The server refuses raw audio over 8 MB (about 4 minutes of it); the mic
/// stops writing a little before.
pub const RAW_MAX: usize = 8 * 1024 * 1024;
pub const RECORD_MAX_S: f32 = 250.0;
/// Shorter than this is a slip of the finger, not an answer: nothing is sent.
pub const ANSWER_MIN_S: f32 = 1.0;
/// A recording stops by itself at the turn's `max_secs`, never sooner than this.
const RECORD_FLOOR_S: f32 = 5.0;
/// A line without a voice (TTS off, over budget or failed) shows for its
/// reading time: this many characters a second, within these bounds.
const READ_CPS: f64 = 15.0;
const READ_MIN_S: f64 = 2.0;
const READ_MAX_S: f64 = 9.0;
/// Opening the page asks for the ladder again only after this.
const ME_FRESH_S: f64 = 60.0;
/// Oyen says one of his thinking lines this long into the wait for an
/// answer's reply (the server takes about 7 to 9 s), unless the reply came.
const FILLER_AFTER_S: f64 = 1.5;
/// Oyen's clips are seconds long: one still "playing" after this has a
/// player that will never say it ended, and the session goes on without it.
const CLIP_MAX_S: f64 = 45.0;
/// Levels go 1 to 5.
pub const LEVELS: u8 = 5;

// ---------------------------------------------------------------- spice

/// How Oyen talks in Indonesian (reactions, notes, roasts). The examiner's
/// questions are always plain English. Settings keep its name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spice {
    /// Aku/kamu, no swearing.
    Sopan,
    /// Gue/lo, "anjir".
    Gaul,
    /// GoPrep-style: rants, swear words, shouting. Luca's pick for the
    /// default, and what a name from a newer version reads as.
    #[default]
    #[serde(other)]
    Ngegas,
}

impl Spice {
    pub const ALL: [Spice; 3] = [Spice::Sopan, Spice::Gaul, Spice::Ngegas];

    /// The number the server takes (0, 1, 2).
    pub fn n(self) -> u8 {
        self as u8
    }

    /// Anything out of range is the default.
    pub fn from_n(n: u8) -> Spice {
        Spice::ALL.get(n as usize).copied().unwrap_or_default()
    }

    pub fn name(self) -> &'static str {
        match self {
            Spice::Sopan => "Sopan",
            Spice::Gaul => "Gaul",
            Spice::Ngegas => "Ngegas",
        }
    }

    /// One line under the choice: what it means.
    pub fn line(self) -> &'static str {
        match self {
            Spice::Sopan => "Halus, aku-kamu, tanpa kata kasar.",
            Spice::Gaul => "Gue-lo, santai, ada \u{201c}anjir\u{201d}-nya.",
            Spice::Ngegas => "Kayak GoPrep: ngegas, pakai kata kasar.",
        }
    }
}

// ---------------------------------------------------------------- the answers
//
// Read from `serde_json::Value` by hand, as `stocks` does, rather than with
// derived deserializers: a dozen small types this way cost a fraction of the
// code (this ships in poni.exe). Anything missing or of the wrong kind reads
// as its default, so a server a little ahead still reads.

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn num(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

fn small(v: &Value) -> u8 {
    v.as_u64().unwrap_or(0).min(u8::MAX as u64) as u8
}

fn list<T>(v: &Value, f: impl Fn(&Value) -> T) -> Vec<T> {
    v.as_array().map_or_else(Vec::new, |a| a.iter().map(f).collect())
}

/// A clip of Oyen's voice, base64 in the JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Audio {
    pub mime: String,
    pub b64: String,
}

impl Audio {
    fn read(v: &Value) -> Option<Audio> {
        v.is_object().then(|| Audio { mime: text(&v["mime"]), b64: text(&v["b64"]) })
    }

    /// The file extension the player knows it by.
    pub fn ext(&self) -> &'static str {
        match self.mime.as_str() {
            "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
            "audio/mp4" | "audio/aac" | "audio/m4a" => "m4a",
            _ => "mp3",
        }
    }
}

/// One thing Oyen says. `audio` comes with the answer; the app saves it to a
/// file (`clip`) and drops the base64 at once ([`Turn::save_clips`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Say {
    pub text: String,
    /// "en" (the examiner's questions) or "id" (reactions, notes, roasts).
    pub lang: String,
    pub audio: Option<Audio>,
    /// The clip on this PC, once saved; deleted once played.
    pub clip: Option<PathBuf>,
}

impl Say {
    fn read(v: &Value) -> Say {
        let lang = v["lang"].as_str().filter(|l| !l.is_empty()).unwrap_or("en").to_string();
        Say { text: text(&v["text"]), lang, audio: Audio::read(&v["audio"]), clip: None }
    }

    /// How long a line without a voice stays on screen, in seconds.
    pub fn read_secs(&self) -> f64 {
        (self.text.chars().count() as f64 / READ_CPS).clamp(READ_MIN_S, READ_MAX_S)
    }

    /// Write the clip to `dir/<name>.<ext>`; keep only the words when it
    /// can't be decoded or written (the page shows the text then).
    pub fn save_clip(&mut self, dir: &Path, name: &str) {
        let Some(audio) = self.audio.take() else { return };
        let Some(bytes) = b64_decode(&audio.b64).filter(|b| !b.is_empty()) else { return };
        let path = dir.join(format!("{name}.{}", audio.ext()));
        if std::fs::write(&path, bytes).is_ok() {
            self.clip = Some(path);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Intro,
    #[default]
    Question,
    Followup,
    Interrupt,
    CueCard,
    Verdict,
    /// A kind this version doesn't know: a question like any other.
    Other,
}

impl Kind {
    fn read(v: &Value) -> Kind {
        match v.as_str() {
            Some("intro") => Kind::Intro,
            Some("question") | None => Kind::Question,
            Some("followup") => Kind::Followup,
            Some("interrupt") => Kind::Interrupt,
            Some("cue_card") => Kind::CueCard,
            Some("verdict") => Kind::Verdict,
            Some(_) => Kind::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Expect {
    #[default]
    Answer,
    None,
}

/// The Part 2 card: the topic, what to cover, and how to round it off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CueCard {
    pub topic: String,
    pub bullets: Vec<String>,
    pub closing: String,
}

/// One of Oyen's turns: what he says (one to three lines, in order) and
/// what he expects back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    /// 0 for the intro and the verdict, else the IELTS part.
    pub part: u8,
    pub kind: Kind,
    pub say: Vec<Say>,
    pub expect: Expect,
    /// The recording stops by itself here (Part 1 about 40, Part 2 120, Part 3 about 75).
    pub max_secs: f32,
    /// Part 2's preparation minute before the recording starts by itself.
    pub prep_secs: f32,
    pub cue_card: Option<CueCard>,
}

impl Turn {
    fn read(v: &Value) -> Turn {
        let card = &v["cue_card"];
        Turn {
            part: small(&v["part"]),
            kind: Kind::read(&v["kind"]),
            say: list(&v["say"], Say::read),
            expect: if v["expect"].as_str() == Some("none") { Expect::None } else { Expect::Answer },
            max_secs: v["max_secs"].as_f64().map_or(45.0, |s| s as f32),
            prep_secs: num(&v["prep_secs"]),
            cue_card: card.is_object().then(|| CueCard { topic: text(&card["topic"]), bullets: list(&card["bullets"], text), closing: text(&card["closing"]) }),
        }
    }

    pub fn wants_answer(&self) -> bool {
        self.expect != Expect::None
    }

    /// Save every voiced line to `dir` as `<prefix>-<n>.<ext>` (the base64
    /// leaves memory with it).
    pub fn save_clips(&mut self, dir: &Path, prefix: &str) {
        for (i, s) in self.say.iter_mut().enumerate() {
            s.save_clip(dir, &format!("{prefix}-{i}"));
        }
    }

    /// The clips still on disk.
    pub fn clips(&self) -> Vec<PathBuf> {
        self.say.iter().filter_map(|s| s.clip.clone()).collect()
    }
}

/// One criterion's band and Oyen's line about it (gaul, in the session's spice).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Band {
    pub band: f32,
    pub note: String,
}

impl Band {
    fn read(v: &Value) -> Band {
        Band { band: num(&v["band"]), note: text(&v["note"]) }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Criteria {
    pub fc: Band,
    pub lr: Band,
    pub gra: Band,
    pub p: Band,
}

impl Criteria {
    /// The four, with their names, in the order IELTS lists them.
    pub fn rows(&self) -> [(&'static str, &Band); 4] {
        [("Fluency & Coherence", &self.fc), ("Lexical Resource", &self.lr), ("Grammar", &self.gra), ("Pronunciation", &self.p)]
    }
}

/// Where the session left the person's level.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LevelChange {
    pub before: u8,
    pub after: u8,
    /// The next level just opened.
    pub unlocked: bool,
}

/// The end of a session: the band (the mean of the four, IELTS-rounded by
/// the server), each criterion, the roast, three tips, and the level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verdict {
    pub band: f32,
    pub criteria: Criteria,
    pub roast: Say,
    pub tips: Vec<String>,
    pub level: LevelChange,
    /// The person's best band at this level so far.
    pub best: Option<f32>,
}

impl Verdict {
    fn read(v: &Value) -> Option<Verdict> {
        let (c, l) = (&v["criteria"], &v["level"]);
        v.is_object().then(|| Verdict {
            band: num(&v["band"]),
            criteria: Criteria { fc: Band::read(&c["fc"]), lr: Band::read(&c["lr"]), gra: Band::read(&c["gra"]), p: Band::read(&c["p"]) },
            roast: Say::read(&v["roast"]),
            tips: list(&v["tips"], text),
            level: LevelChange { before: small(&l["before"]), after: small(&l["after"]), unlocked: l["unlocked"].as_bool().unwrap_or(false) },
            best: v["best"].as_f64().map(|b| b as f32),
        })
    }
}

/// One rung of the ladder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Level {
    pub n: u8,
    pub name: String,
    pub blurb: String,
    /// The band that opens the next level.
    pub target: f32,
    pub parts: Vec<u8>,
    pub unlocked: bool,
    pub best: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Today {
    pub used: u32,
    pub cap: u32,
}

/// `GET /me`: the ladder, the person's level, whether Oyen has a voice
/// today, and today's sessions against the cap.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Me {
    pub levels: Vec<Level>,
    pub level: u8,
    pub voice: bool,
    pub today: Today,
}

impl Me {
    pub fn rung(&self, n: u8) -> Option<&Level> {
        self.levels.iter().find(|l| l.n == n)
    }

    pub fn unlocked(&self, n: u8) -> bool {
        self.rung(n).is_some_and(|l| l.unlocked)
    }

    /// Today's sessions are all used.
    pub fn capped(&self) -> bool {
        self.today.cap > 0 && self.today.used >= self.today.cap
    }

    /// A finished session: its best, and the level it opened.
    fn note(&mut self, level: u8, v: &Verdict) {
        if let Some(l) = self.levels.iter_mut().find(|l| l.n == level) {
            l.best = Some(l.best.map_or(v.band, |b| b.max(v.band)));
        }
        if v.level.unlocked {
            if let Some(l) = self.levels.iter_mut().find(|l| l.n == v.level.after) {
                l.unlocked = true;
            }
            self.level = self.level.max(v.level.after);
        }
    }
}

fn json(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body).ok().filter(Value::is_object)
}

pub fn parse_me(body: &str) -> Option<Me> {
    let v = json(body)?;
    let level = |l: &Value| Level {
        n: small(&l["n"]),
        name: text(&l["name"]),
        blurb: text(&l["blurb"]),
        target: num(&l["target"]),
        parts: list(&l["parts"], small),
        unlocked: l["unlocked"].as_bool().unwrap_or(false),
        best: l["best"].as_f64().map(|b| b as f32),
    };
    let t = &v["today"];
    let count = |c: &Value| c.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
    Some(Me {
        levels: list(&v["levels"], level).into_iter().filter(|l| l.n > 0).collect(),
        level: v["level"].as_u64().map_or(1, |n| n.clamp(1, LEVELS as u64) as u8),
        voice: v["voice"].as_bool().unwrap_or(false),
        today: Today { used: count(&t["used"]), cap: count(&t["cap"]) },
    })
}

/// `POST /session`: the session, its first turn (the intro, then the first
/// question), and Oyen's thinking lines for the waits while an answer is
/// heard (said one a wait, in turn).
#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    pub id: String,
    pub turn: Turn,
    pub fillers: Vec<Say>,
}

pub fn parse_started(body: &str) -> Option<Started> {
    let v = json(body)?;
    let id = text(&v["id"]);
    (!id.is_empty() && v["turn"].is_object()).then(|| Started { id, turn: Turn::read(&v["turn"]), fillers: list(&v["fillers"], Say::read) })
}

/// `POST /session/:id/answer`: what the server heard and Oyen's next turn;
/// after the last answer, `done` with the verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct Answered {
    pub heard: String,
    pub turn: Turn,
    pub done: bool,
    pub result: Option<Verdict>,
}

pub fn parse_answered(body: &str) -> Option<Answered> {
    let v = json(body)?;
    let a = Answered { heard: text(&v["heard"]), turn: Turn::read(&v["turn"]), done: v["done"].as_bool().unwrap_or(false), result: Verdict::read(&v["result"]) };
    (v["turn"].is_object() && (!a.done || a.result.is_some())).then_some(a)
}

/// `POST /session/:id/skip`: the next question, this one unscored, or, when
/// skipping ends the test, `done` with the verdict (like a last answer).
pub fn parse_skipped(body: &str) -> Option<Answered> {
    parse_answered(body)
}

/// What Oyen says about a refused call (a capped day, a locked level carry
/// his own line), from the error's body.
pub fn problem_say(body: &str) -> Vec<Say> {
    json(body).map_or_else(Vec::new, |v| list(&v["say"], Say::read))
}

/// The verdict turn already says the roast (the server sends closing,
/// roast, then level-up or goodbye): the result's own copy isn't said again.
pub fn roast_in_turn(turn: &Turn, v: &Verdict) -> bool {
    !v.roast.text.is_empty() && turn.say.iter().any(|s| s.text == v.roast.text)
}

// ---------------------------------------------------------------- requests

/// `tz` as `oyenza::tz_param`: today's sessions are counted in the PC's day.
pub fn me_url(base: &str, tz: &str) -> String {
    crate::urlenc::with_query(&format!("{base}{PATH}/me"), &[("tz", tz)])
}

pub fn session_url(base: &str) -> String {
    format!("{base}{PATH}/session")
}

pub fn answer_url(base: &str, id: &str) -> String {
    format!("{base}{PATH}/session/{}/answer", crate::urlenc::encode(id))
}

pub fn skip_url(base: &str, id: &str) -> String {
    format!("{base}{PATH}/session/{}/skip", crate::urlenc::encode(id))
}

/// `DELETE` it: the session ends (Quit).
pub fn end_url(base: &str, id: &str) -> String {
    format!("{base}{PATH}/session/{}", crate::urlenc::encode(id))
}

/// `tz` as `oyenza::tz_param` ("+07:00"): the server counts today's sessions in it.
pub fn session_body(level: u8, spice: Spice, tz: &str) -> String {
    serde_json::json!({ "level": level.clamp(1, LEVELS), "spice": spice.n(), "tz": tz }).to_string()
}

/// The answer: the WAV as base64, and its length. Built by hand: the audio
/// is most of the body, and this copies it once.
pub fn answer_body(wav: &[u8], secs: f32) -> String {
    let b64 = b64_encode(wav);
    let mut out = String::with_capacity(b64.len() + 64);
    out.push_str("{\"audio\":{\"mime\":\"audio/wav\",\"b64\":\"");
    out.push_str(&b64);
    out.push_str(&format!("\"}},\"secs\":{:.1}}}", secs.max(0.0)));
    out
}

// ---------------------------------------------------------------- what went wrong

/// Why the microphone didn't record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicIssue {
    /// The system's privacy switch keeps apps (desktop apps, on Windows) from the microphone.
    Denied,
    /// No microphone is plugged in or enabled.
    NoDevice,
    Failed,
}

/// What went wrong, as the page says it: never a code or a status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// 503 `paused`: switched off on the server. Also any other 404: a
    /// server without the IELTS routes yet says what a paused one does,
    /// not "error".
    Paused,
    /// 429 `daily_cap`: today's sessions are used.
    DailyCap,
    /// 429 `rate_limited`: too many calls this hour (the server's guard).
    TooFast,
    /// 403 `level_locked`.
    LevelLocked,
    /// 404 `no_session`: it ended after 30 minutes without an answer.
    NoSession,
    /// 413 `too_big`.
    TooBig,
    /// 503 `ai_busy`, and how long to wait when the server said.
    Busy(Option<u64>),
    /// 502 `ai_failed`.
    AiFailed,
    /// No answer at all: no internet, or the server didn't answer in time.
    Offline,
    /// 401, or no Oyenza key on this PC.
    SignedOut,
    /// 409 `busy`: the server is still hearing an answer to this session.
    StillThinking,
    /// 400 `bad_level`.
    BadLevel,
    /// 400 `bad_audio`: the recording couldn't be read (record it again).
    BadAudio,
    /// Any other status, or an answer that couldn't be read.
    Server,
    Mic(MicIssue),
}

/// The problem a failed call's status and body (`{"error": "…"}`, with
/// `retry_after` seconds for `ai_busy`) stand for. `retry_after` is the
/// `Retry-After` header's, when there was one.
pub fn problem(status: u16, body: &str, retry_after: Option<u64>) -> Problem {
    let e = json(body).unwrap_or_default();
    let wait = e["retry_after"].as_f64().filter(|s| s.is_finite() && *s >= 0.0).map(|s| s.ceil() as u64).or(retry_after);
    match (status, e["error"].as_str().unwrap_or_default()) {
        (401, _) => Problem::SignedOut,
        (_, "paused") => Problem::Paused,
        (_, "daily_cap") => Problem::DailyCap,
        (_, "rate_limited") | (429, _) => Problem::TooFast,
        (_, "level_locked") => Problem::LevelLocked,
        (_, "no_session") => Problem::NoSession,
        (404, _) => Problem::Paused,
        (_, "too_big") | (413, _) => Problem::TooBig,
        (_, "ai_busy") => Problem::Busy(wait),
        (_, "ai_failed") | (502, _) => Problem::AiFailed,
        (_, "busy") | (409, _) => Problem::StillThinking,
        (_, "bad_level") => Problem::BadLevel,
        (_, "bad_audio") => Problem::BadAudio,
        (503, _) => Problem::Busy(wait),
        _ => Problem::Server,
    }
}

impl Problem {
    pub fn title(self) -> &'static str {
        match self {
            Problem::Paused => "Latihan IELTS lagi libur",
            Problem::DailyCap => "Jatah hari ini udah habis",
            Problem::TooFast => "Pelan-pelan dulu",
            Problem::LevelLocked => "Level ini masih kekunci",
            Problem::NoSession => "Sesinya udah berhenti",
            Problem::TooBig => "Jawabannya kepanjangan",
            Problem::Busy(_) => "Oyen lagi rame",
            Problem::AiFailed => "Oyen gagal mikir",
            Problem::Offline => "Nggak nyambung ke Oyenza",
            Problem::SignedOut => "Kamu udah keluar dari Oyenza",
            Problem::StillThinking => "Oyen masih mikirin jawabanmu",
            Problem::BadLevel => "Level itu nggak ada",
            Problem::BadAudio => "Rekamanmu nggak kebaca",
            Problem::Server => "Ada yang error di server",
            Problem::Mic(MicIssue::Denied) => "Mikrofonnya diblokir",
            Problem::Mic(MicIssue::NoDevice) => "Mikrofon nggak ketemu",
            Problem::Mic(MicIssue::Failed) => "Mikrofonnya error",
        }
    }

    /// What to do about it. `mac`: the Mac's words for the microphone switch.
    pub fn line(self, mac: bool) -> String {
        match self {
            Problem::Paused => "Oyen istirahat dulu. Coba lagi nanti ya.".into(),
            Problem::DailyCap => "Sesi latihan hari ini udah kepakai semua. Lanjut besok ya.".into(),
            Problem::TooFast => "Kebanyakan permintaan dalam sejam ini. Tunggu sebentar, terus coba lagi.".into(),
            Problem::LevelLocked => "Capai target band di level sebelumnya dulu.".into(),
            Problem::NoSession => "Sesi berhenti kalau 30 menit nggak ada jawaban. Mulai lagi yuk.".into(),
            Problem::TooBig => "Rekaman maksimal sekitar 4 menit. Coba jawab lebih singkat.".into(),
            Problem::Busy(Some(s)) if s > 1 => format!("Lagi banyak yang latihan. Coba lagi {s} detik lagi."),
            Problem::Busy(_) => "Lagi banyak yang latihan. Coba lagi sebentar lagi.".into(),
            Problem::AiFailed => "Jawabanmu masih aman. Kirim lagi aja.".into(),
            Problem::Offline => "Cek internetnya, terus coba lagi.".into(),
            Problem::SignedOut => "Masuk lagi ke Oyenza buat lanjut latihan.".into(),
            Problem::StillThinking => "Jawaban tadi masih diproses. Tunggu sebentar, terus coba lagi.".into(),
            Problem::BadLevel => "Pilih level lain di daftar, terus mulai lagi.".into(),
            Problem::BadAudio => "Coba rekam ulang jawabanmu ya.".into(),
            Problem::Server => "Coba lagi sebentar lagi.".into(),
            Problem::Mic(MicIssue::Denied) if mac => "Nyalain Oyenza Studio di Privasi & Keamanan › Mikrofon, terus coba lagi.".into(),
            Problem::Mic(MicIssue::Denied) => "Nyalain akses mikrofon buat aplikasi desktop di Pengaturan Windows, terus coba lagi.".into(),
            Problem::Mic(MicIssue::NoDevice) => "Colok mic atau headset dulu, terus coba lagi.".into(),
            Problem::Mic(MicIssue::Failed) => "Coba cabut-colok mic-nya, terus coba lagi.".into(),
        }
    }

    /// Try again can help (the answer is kept for it; a recording that
    /// couldn't be read is made again).
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Problem::Busy(_) | Problem::TooFast | Problem::AiFailed | Problem::Offline | Problem::Server | Problem::StillThinking | Problem::BadAudio | Problem::Mic(_)
        )
    }
}

/// The page's words for the band: one decimal, as IELTS writes it ("6.5", "7.0").
pub fn fmt_band(b: f32) -> String {
    format!("{:.1}", (b.clamp(0.0, 9.0) * 2.0).round() / 2.0)
}

/// "0:07", "1:00".
pub fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0).ceil() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The turn's part, as the page labels it.
pub fn part_label(t: &Turn) -> &'static str {
    match (t.kind, t.part) {
        (Kind::Verdict, _) => "Hasil",
        (_, 1) => "Part 1",
        (_, 2) => "Part 2",
        (_, 3) => "Part 3",
        _ => "Intro",
    }
}

// ---------------------------------------------------------------- base64

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (what the server and Gemini take).
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let v = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(v >> 18) as usize & 63] as char);
        out.push(B64[(v >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(v >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[v as usize & 63] as char } else { '=' });
    }
    out
}

/// Standard base64, padded or not.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    crate::update::base64(s)
}

// ---------------------------------------------------------------- WAV

/// A canonical 44-byte WAV header for PCM.
pub fn wav_header(rate: u32, channels: u16, bits: u16, data_len: u32) -> [u8; 44] {
    let block = channels * bits / 8;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * block as u32).to_le_bytes());
    h[32..34].copy_from_slice(&block.to_le_bytes());
    h[34..36].copy_from_slice(&bits.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

/// A WAV file's format and length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WavInfo {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
    pub secs: f32,
}

/// Read a WAV file's `fmt ` and `data` chunks (PCM only), walking the RIFF
/// chunks: recorders put others before the audio (the Mac's 4 KB `FLLR`
/// padding, for one), so the header isn't always 44 bytes.
pub fn wav_info(bytes: &[u8]) -> Option<WavInfo> {
    riff(bytes).map(|(info, _)| info)
}

/// The same audio behind the canonical 44-byte header, every other chunk
/// left out (what the answer uploads). None for a file that isn't PCM WAV.
pub fn canonical_wav(bytes: &[u8]) -> Option<Vec<u8>> {
    let (info, data) = riff(bytes)?;
    let audio = &bytes[data];
    let mut out = Vec::with_capacity(44 + audio.len());
    out.extend(wav_header(info.rate, info.channels, info.bits, audio.len() as u32));
    out.extend_from_slice(audio);
    Some(out)
}

/// The format and where the audio is.
fn riff(bytes: &[u8]) -> Option<(WavInfo, std::ops::Range<usize>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        // A recorder that never filled in its sizes: the rest of the file.
        let end = (at + 8).saturating_add(len).min(bytes.len());
        let body = &bytes[at + 8..end];
        match id {
            b"fmt " if body.len() >= 16 => {
                let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                // WAVE_FORMAT_EXTENSIBLE: the old tag is the sub-format GUID's first field.
                let tag = if u16_at(0) == 0xFFFE && body.len() >= 26 { u16_at(24) } else { u16_at(0) };
                fmt = Some((tag, u16_at(2), u32::from_le_bytes([body[4], body[5], body[6], body[7]]), u16_at(14)));
            }
            b"data" => data = Some(at + 8..end),
            _ => {}
        }
        at = end + (len & 1);
    }
    let (tag, channels, rate, bits) = fmt?;
    let data = data?;
    if tag != 1 || channels == 0 || rate == 0 || bits < 8 {
        return None;
    }
    let secs = data.len() as f32 / (rate as f32 * channels as f32 * (bits / 8) as f32);
    Some((WavInfo { rate, channels, bits, secs }, data))
}

/// Writes the answer as it is recorded: 16 kHz mono 16-bit, the header's
/// sizes filled in at the end.
pub struct WavWriter {
    out: BufWriter<File>,
    path: PathBuf,
    samples: u64,
    buf: Vec<u8>,
}

impl WavWriter {
    pub fn create(path: &Path) -> io::Result<WavWriter> {
        let mut out = BufWriter::with_capacity(64 * 1024, File::create(path)?);
        out.write_all(&wav_header(RATE, 1, 16, 0))?;
        Ok(WavWriter { out, path: path.to_path_buf(), samples: 0, buf: Vec::new() })
    }

    /// Append samples, up to [`RECORD_MAX_S`] in all (the rest is dropped).
    pub fn write(&mut self, pcm: &[i16]) -> io::Result<()> {
        let room = (RECORD_MAX_S as u64 * RATE as u64).saturating_sub(self.samples) as usize;
        let pcm = &pcm[..pcm.len().min(room)];
        if pcm.is_empty() {
            return Ok(());
        }
        self.buf.clear();
        self.buf.extend(pcm.iter().flat_map(|s| s.to_le_bytes()));
        self.out.write_all(&self.buf)?;
        self.samples += pcm.len() as u64;
        Ok(())
    }

    pub fn secs(&self) -> f32 {
        self.samples as f32 / RATE as f32
    }

    /// Fill in the sizes and close the file: its path and length in seconds.
    pub fn finish(self) -> io::Result<(PathBuf, f32)> {
        let secs = self.secs();
        let data = (self.samples * 2) as u32;
        let mut f = self.out.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&wav_header(RATE, 1, 16, data))?;
        f.flush()?;
        Ok((self.path, secs))
    }
}

/// A WAV of `secs` of a soft `hz` tone at `rate` (16-bit mono): the
/// self-test's stand-in answer and voice, so nothing needs a microphone.
pub fn tone_wav(secs: f32, hz: f32, rate: u32) -> Vec<u8> {
    let n = (secs * rate as f32) as usize;
    let mut out = wav_header(rate, 1, 16, (n * 2) as u32).to_vec();
    for i in 0..n {
        let v = (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() * 0.25;
        out.extend(((v * 32767.0) as i16).to_le_bytes());
    }
    out
}

// ---------------------------------------------------------------- the microphone's samples

/// How the microphone's samples are stored (Windows' mix format, usually 32-bit float).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
    /// 24-bit in 3 bytes.
    I24,
    I32,
}

impl SampleFormat {
    pub fn bytes(self) -> usize {
        match self {
            SampleFormat::I16 => 2,
            SampleFormat::I24 => 3,
            SampleFormat::F32 | SampleFormat::I32 => 4,
        }
    }

    fn read(self, b: &[u8]) -> f32 {
        match self {
            SampleFormat::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            SampleFormat::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            SampleFormat::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            SampleFormat::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        }
    }
}

/// A second-order low-pass (RBJ's cookbook), transposed direct form II.
#[derive(Clone, Copy, Debug)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn low_pass(rate: f32, cutoff: f32, q: f32) -> Biquad {
        let w = std::f32::consts::TAU * cutoff / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - cos) / a0;
        Biquad { b0: b1 / 2.0, b1, b2: b1 / 2.0, a1: -2.0 * cos / a0, a2: (1.0 - alpha) / a0, z1: 0.0, z2: 0.0 }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Turns the microphone's frames (any rate, channels and sample format)
/// into 16 kHz mono 16-bit: the channels averaged, a 4th-order low-pass
/// under 8 kHz (so nothing above it folds back into the speech), then
/// linear interpolation to 16 kHz. It keeps its place across packets, so
/// packets of any size give the same samples.
#[derive(Clone, Debug)]
pub struct Downmix {
    format: SampleFormat,
    channels: usize,
    /// Source samples per output sample.
    step: f64,
    /// Where the next output sample sits, in source samples after `prev`.
    pos: f64,
    prev: f32,
    filters: Option<[Biquad; 2]>,
}

impl Downmix {
    pub fn new(rate: u32, channels: u16, format: SampleFormat) -> Downmix {
        let rate = rate.max(1);
        // Butterworth, 4th order: two stages with these Qs, cut at 0.45 × 16 kHz.
        let filters = (rate > RATE + 1000).then(|| [0.541_196, 1.306_563].map(|q| Biquad::low_pass(rate as f32, 0.45 * RATE as f32, q)));
        Downmix { format, channels: channels.max(1) as usize, step: rate as f64 / RATE as f64, pos: 1.0, prev: 0.0, filters }
    }

    /// Interleaved frames in (`bytes`), 16 kHz samples onto `out`. Returns
    /// the loudest of them, 0..1 (the level meter).
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<i16>) -> f32 {
        let (size, channels) = (self.format.bytes(), self.channels);
        let start = out.len();
        for frame in bytes.chunks_exact(size * channels) {
            let sum: f32 = frame.chunks_exact(size).map(|s| self.format.read(s)).sum();
            self.feed(sum / channels as f32, out);
        }
        peak(&out[start..])
    }

    /// `frames` frames of silence (a packet the system flagged silent).
    pub fn silence(&mut self, frames: usize, out: &mut Vec<i16>) {
        for _ in 0..frames {
            self.feed(0.0, out);
        }
    }

    fn feed(&mut self, x: f32, out: &mut Vec<i16>) {
        let x = match self.filters.as_mut() {
            Some([a, b]) => b.run(a.run(x)),
            None => x,
        };
        while self.pos <= 1.0 {
            let y = self.prev + (x - self.prev) * self.pos as f32;
            out.push((y.clamp(-1.0, 1.0) * 32767.0).round() as i16);
            self.pos += self.step;
        }
        self.pos -= 1.0;
        self.prev = x;
    }
}

/// The loudest sample, 0..1.
pub fn peak(pcm: &[i16]) -> f32 {
    pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0
}

// ---------------------------------------------------------------- the session

/// What the page shows; the coach moves between them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Phase {
    /// The level ladder, the spice and Mulai.
    #[default]
    Home,
    /// Asking the server for a session.
    Starting,
    /// Oyen says the turn's lines.
    Talking,
    /// Part 2's preparation, until mono time `until`; then the recording starts by itself.
    Prep { until: f64 },
    /// The person's turn: the mic waits for a tap.
    Ready,
    /// Recording since `since`; it stops by itself at `until`.
    Recording { since: f64, until: f64 },
    /// The answer (or a skip) is on its way: Oyen thinks.
    Thinking,
    /// The band, the criteria, the roast and the tips (Oyen says the verdict and the roast).
    Result,
    /// Something stopped the session ([`Coach::problem`]), with a way on.
    Stuck,
}

/// What the server answered, or why it didn't (`services/ielts.rs`). The
/// clips are already files ([`Turn::save_clips`]). One at a time, boxed on
/// its way to the UI thread: the verdict's variant is the big one.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Reply {
    Me(Me),
    Started { id: String, turn: Turn, fillers: Vec<Say> },
    Answered { heard: String, turn: Turn, done: bool, result: Option<Verdict> },
    /// Skipping goes on to the next question, or ends the test (`done`).
    Skipped { turn: Turn, done: bool, result: Option<Verdict> },
    Ended,
    /// `say`: Oyen's own line about it, when the server sent one.
    Failed { call: CallKind, problem: Problem, say: Vec<Say> },
}

/// A call for the app's IELTS worker.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Me,
    Start { level: u8, spice: Spice },
    /// Upload the answer at `wav` (`secs` long).
    Answer { id: String, wav: PathBuf, secs: f32 },
    Skip { id: String },
    End { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Me,
    Start,
    Answer,
    Skip,
    End,
}

impl Call {
    pub fn kind(&self) -> CallKind {
        match self {
            Call::Me => CallKind::Me,
            Call::Start { .. } => CallKind::Start,
            Call::Answer { .. } => CallKind::Answer,
            Call::Skip { .. } => CallKind::Skip,
            Call::End { .. } => CallKind::End,
        }
    }
}

/// What the app should do now.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Call(Call),
    /// Play this clip; tell [`Coach::voice_ended`] when it ends (or
    /// [`Coach::voice_failed`] when it can't play).
    Play(PathBuf),
    StopVoice,
    /// Start the microphone (the phase says until when).
    Record,
    /// Stop it and hand the file to [`Coach::recorded`].
    StopRecord,
    /// Stop it and delete the file.
    DropRecord,
    Delete(PathBuf),
    /// Pause music that plays (once a session; the app keeps track).
    PauseMusic,
    /// Play the music paused for the session again.
    ResumeMusic,
}

/// A hint on the page that needs no screen of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    /// The answer was under [`ANSWER_MIN_S`]: nothing was sent.
    TooShort,
}

impl Hint {
    pub fn line(self) -> &'static str {
        match self {
            Hint::TooShort => "Kependekan. Ketuk mic, jawab, terus ketuk lagi kalau udah.",
        }
    }
}

/// The IELTS page's session (in `AppState`): what it shows, and what the
/// app should do next. The app calls it for every event; each call returns
/// the [`Effect`]s to carry out, in order.
#[derive(Clone, Debug, Default)]
pub struct Coach {
    pub phase: Phase,
    /// The ladder (`GET /me`), and why it couldn't be read.
    pub me: Option<Me>,
    pub me_problem: Option<Problem>,
    me_asked: Option<f64>,
    /// The level picked on the ladder (0 until the ladder came).
    pub pick: u8,
    /// The session, and its level.
    pub id: Option<String>,
    pub level: u8,
    /// The turn on screen (its lines move to the queue as they're said).
    pub turn: Option<Turn>,
    queue: VecDeque<Say>,
    /// The line Oyen says, or said last (the question, once he's done).
    pub say: Option<Say>,
    /// A clip of `say` plays now.
    pub voiced: bool,
    /// When Oyen began saying `say` (mono seconds; his talking bob runs from it).
    pub said_at: f64,
    /// `say` has no voice: it shows until this mono time.
    text_until: Option<f64>,
    /// What the server heard of the last answer.
    pub heard: String,
    pub result: Option<Verdict>,
    pub problem: Option<Problem>,
    pub hint: Option<Hint>,
    /// The answer, kept on disk until the server has it (Try again sends it again).
    answer: Option<(PathBuf, f32)>,
    /// What Try again sends.
    pending: Option<Call>,
    shown: bool,
    /// The microphone's level, 0..1 (the app sets it while recording).
    pub meter: f32,
    /// Answers the server took this session.
    pub answers: u32,
    /// Oyen's thinking lines for this session, said in turn (kept, and
    /// deleted, with the session).
    fillers: VecDeque<Say>,
    /// While an answer is heard: when one of them is due (one a wait).
    filler_at: Option<f64>,
    /// A thinking line plays now.
    filler: bool,
    /// The server's reply that came while a thinking line played: it
    /// follows the line, never cuts it.
    held: Option<Held>,
}

/// A reply waiting for a thinking line to end.
#[derive(Clone, Debug, PartialEq)]
struct Held {
    heard: Option<String>,
    turn: Turn,
    result: Option<Verdict>,
}

impl Coach {
    /// A session runs (or its result is still being said): the island stays
    /// open. A stuck session doesn't hold it: the page shows why when it opens.
    pub fn holds(&self) -> bool {
        match self.phase {
            Phase::Home | Phase::Stuck => false,
            Phase::Result => self.talking(),
            _ => true,
        }
    }

    /// Oyen is saying something now (his talking bob).
    pub fn talking(&self) -> bool {
        self.voiced || self.text_until.is_some()
    }

    /// The next moment the coach needs [`Coach::tick`] for, if any.
    pub fn deadline(&self) -> Option<f64> {
        let phase = match self.phase {
            Phase::Prep { until } | Phase::Recording { until, .. } => Some(until),
            _ => None,
        };
        match (phase, self.text_until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Lines still to say after `say`.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Whether level `n` can be started (until the ladder comes, none can).
    pub fn startable(&self, n: u8) -> bool {
        self.me.as_ref().is_some_and(|m| m.unlocked(n) && !m.capped())
    }

    /// The page came on screen (`true`) or went: a tab switch, the island
    /// closing, the PC locked. Away, Oyen stops mid-line (the line waits to
    /// be said again; a thinking line just stops), a recording is dropped and
    /// Part 2's minute ends: the person taps the mic when back. Back, the
    /// ladder is read again when it's old, and Oyen goes on talking.
    pub fn show(&mut self, shown: bool, now: f64) -> Vec<Effect> {
        if shown == self.shown {
            return Vec::new();
        }
        self.shown = shown;
        if !shown {
            return self.hush(now);
        }
        let mut fx = Vec::new();
        if self.phase == Phase::Home && self.me_asked.is_none_or(|t| now - t > ME_FRESH_S) {
            self.me_asked = Some(now);
            fx.push(Effect::Call(Call::Me));
        }
        if matches!(self.phase, Phase::Talking | Phase::Result) && !self.talking() && !self.queue.is_empty() {
            fx.extend(self.next_line(now));
        }
        fx
    }

    fn hush(&mut self, now: f64) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.talking() {
            if self.voiced {
                fx.push(Effect::StopVoice);
            }
            self.voiced = false;
            self.text_until = None;
            if !std::mem::take(&mut self.filler) {
                if let Some(s) = self.say.take() {
                    self.queue.push_front(s);
                }
            }
        }
        // A reply held for the thinking line: its turn waits for the page.
        if let Some(h) = self.held.take() {
            fx.extend(self.apply(h, now));
        }
        match self.phase {
            Phase::Recording { .. } => {
                fx.push(Effect::DropRecord);
                self.phase = Phase::Ready;
            }
            Phase::Prep { .. } => self.phase = Phase::Ready,
            _ => {}
        }
        fx
    }

    /// Mulai (or Lagi on the result): a session at the picked level.
    pub fn start(&mut self, spice: Spice) -> Vec<Effect> {
        if !matches!(self.phase, Phase::Home | Phase::Result) || !self.startable(self.pick) {
            return Vec::new();
        }
        let mut fx = self.stop_talk();
        fx.extend(self.drop_fillers());
        self.forget_session();
        self.phase = Phase::Starting;
        let call = Call::Start { level: self.pick, spice };
        self.pending = Some(call.clone());
        fx.push(Effect::Call(call));
        fx
    }

    /// The worker's answer.
    pub fn reply(&mut self, r: Reply, now: f64) -> Vec<Effect> {
        match r {
            Reply::Me(me) => {
                if !me.unlocked(self.pick) {
                    self.pick = me.level.clamp(1, LEVELS);
                }
                self.me = Some(me);
                self.me_problem = None;
                Vec::new()
            }
            Reply::Started { id, turn, fillers } => {
                if self.phase != Phase::Starting {
                    // Quit while it was on its way: end it at once.
                    let mut fx: Vec<Effect> = turn.clips().into_iter().chain(fillers.into_iter().filter_map(|f| f.clip)).map(Effect::Delete).collect();
                    fx.push(Effect::Call(Call::End { id }));
                    return fx;
                }
                self.id = Some(id);
                self.level = self.pick;
                self.pending = None;
                self.fillers = fillers.into();
                self.begin(turn, now)
            }
            Reply::Answered { heard, turn, done, result } => self.answered(Some(heard), turn, result.filter(|_| done), now),
            Reply::Skipped { turn, done, result } => self.answered(None, turn, result.filter(|_| done), now),
            Reply::Ended => Vec::new(),
            Reply::Failed { call, problem, say } => self.failed(call, problem, say, now),
        }
    }

    /// The next turn (or the verdict) for an answer (`heard`) or a skip.
    fn answered(&mut self, heard: Option<String>, turn: Turn, result: Option<Verdict>, now: f64) -> Vec<Effect> {
        if self.phase != Phase::Thinking {
            return clips_of(&turn, result.as_ref()).into_iter().map(Effect::Delete).collect();
        }
        let mut fx: Vec<Effect> = self.answer.take().map(|(p, _)| Effect::Delete(p)).into_iter().collect();
        self.pending = None;
        self.filler_at = None;
        let h = Held { heard, turn, result };
        if self.filler {
            // Oyen's thinking line goes to its end first.
            self.held = Some(h);
        } else {
            fx.extend(self.apply(h, now));
        }
        fx
    }

    fn apply(&mut self, h: Held, now: f64) -> Vec<Effect> {
        if let Some(heard) = h.heard {
            self.heard = heard;
            self.answers += 1;
        }
        match h.result {
            Some(v) => self.finish(h.turn, v, now),
            None => self.begin(h.turn, now),
        }
    }

    fn failed(&mut self, call: CallKind, problem: Problem, say: Vec<Say>, now: f64) -> Vec<Effect> {
        let mut fx = Vec::new();
        match call {
            CallKind::Me => self.me_problem = Some(problem),
            CallKind::Start if self.phase == Phase::Starting => {
                // Back to the ladder (read again), and Oyen says why: a capped
                // day and a locked level come with his own line.
                self.phase = Phase::Home;
                self.pending = None;
                self.problem = Some(problem);
                if problem == Problem::DailyCap {
                    if let Some(m) = self.me.as_mut() {
                        m.today.used = m.today.used.max(m.today.cap);
                    }
                }
                self.me_asked = Some(now);
                fx.push(Effect::Call(Call::Me));
                self.queue = say.into();
                fx.extend(self.next_line(now));
                return fx;
            }
            CallKind::Answer | CallKind::Skip if self.phase == Phase::Thinking => {
                self.filler_at = None;
                self.phase = Phase::Stuck;
                self.problem = Some(problem);
                if problem == Problem::NoSession {
                    self.id = None;
                }
            }
            _ => {}
        }
        // A line nobody will hear: its clip goes.
        fx.extend(say.into_iter().filter_map(|s| s.clip).map(Effect::Delete));
        fx
    }

    /// Show a new turn and start saying it.
    fn begin(&mut self, mut turn: Turn, now: f64) -> Vec<Effect> {
        self.queue = turn.say.drain(..).collect();
        self.turn = Some(turn);
        self.say = None;
        self.voiced = false;
        self.text_until = None;
        self.hint = None;
        self.phase = Phase::Talking;
        self.next_line(now)
    }

    /// The last answer's verdict: the result shows while Oyen says the
    /// verdict turn's lines (closing, roast, then a level opened or goodbye),
    /// and the result's own roast only when the turn doesn't say it.
    fn finish(&mut self, mut turn: Turn, mut v: Verdict, now: f64) -> Vec<Effect> {
        if let Some(m) = self.me.as_mut() {
            m.note(self.level, &v);
        }
        if v.level.unlocked {
            self.pick = v.level.after.clamp(1, LEVELS);
        }
        let said = roast_in_turn(&turn, &v);
        self.queue = turn.say.drain(..).collect();
        let roast = std::mem::take(&mut v.roast);
        let mut fx = Vec::new();
        if said {
            fx.extend(roast.clip.clone().map(Effect::Delete));
        } else if !roast.text.is_empty() || roast.clip.is_some() {
            self.queue.push_back(roast.clone());
        }
        v.roast = Say { clip: None, ..roast };
        self.turn = Some(turn);
        self.result = Some(v);
        self.say = None;
        self.voiced = false;
        self.text_until = None;
        self.phase = Phase::Result;
        fx.extend(self.next_line(now));
        fx
    }

    fn next_line(&mut self, now: f64) -> Vec<Effect> {
        if !self.shown {
            // Waits for the page.
            return Vec::new();
        }
        let Some(s) = self.queue.pop_front() else { return self.talked(now) };
        let mut fx = vec![Effect::PauseMusic];
        self.said_at = now;
        match s.clip.clone() {
            Some(p) => {
                self.voiced = true;
                fx.push(Effect::Play(p));
            }
            None => self.text_until = Some(now + s.read_secs()),
        }
        self.say = Some(s);
        fx
    }

    /// Every line of the turn is said.
    fn talked(&mut self, now: f64) -> Vec<Effect> {
        self.voiced = false;
        self.text_until = None;
        match self.phase {
            Phase::Result => {
                // The session is over: its thinking lines go too.
                self.id = None;
                let mut fx = self.drop_fillers();
                fx.push(Effect::ResumeMusic);
                fx
            }
            Phase::Talking => {
                // Part 2's minute starts once Oyen has said the card.
                let prep = self.turn.as_ref().filter(|t| t.wants_answer()).map_or(0.0, |t| t.prep_secs) as f64;
                self.phase = if prep > 0.0 { Phase::Prep { until: now + prep } } else { Phase::Ready };
                Vec::new()
            }
            // A line on the ladder (why Mulai was refused) is said.
            Phase::Home => vec![Effect::ResumeMusic],
            _ => Vec::new(),
        }
    }

    /// The clip playing ended.
    pub fn voice_ended(&mut self, now: f64) -> Vec<Effect> {
        if !self.voiced {
            return Vec::new();
        }
        self.voiced = false;
        if std::mem::take(&mut self.filler) {
            // A thinking line (kept for the next wait): the reply it held, if it came.
            return self.held.take().map_or_else(Vec::new, |h| self.apply(h, now));
        }
        let mut fx: Vec<Effect> = self.say.as_mut().and_then(|s| s.clip.take()).map(Effect::Delete).into_iter().collect();
        fx.extend(self.next_line(now));
        fx
    }

    /// The clip couldn't play: its words show for their reading time instead
    /// (a thinking line just stops).
    pub fn voice_failed(&mut self, now: f64) -> Vec<Effect> {
        if !self.voiced {
            return Vec::new();
        }
        if self.filler {
            return self.voice_ended(now);
        }
        self.voiced = false;
        let fx = self.say.as_mut().and_then(|s| s.clip.take()).map(Effect::Delete).into_iter().collect();
        self.text_until = Some(now + self.say.as_ref().map_or(READ_MIN_S, Say::read_secs));
        fx
    }

    /// The clock: a line's reading time, a thinking line due, Part 2's
    /// minute and the recording's limit.
    pub fn tick(&mut self, now: f64) -> Vec<Effect> {
        if !self.shown {
            return Vec::new();
        }
        if self.text_until.is_some_and(|t| now >= t) {
            self.text_until = None;
            return self.next_line(now);
        }
        if self.voiced && now - self.said_at > CLIP_MAX_S {
            let mut fx = vec![Effect::StopVoice];
            fx.extend(self.voice_ended(now));
            return fx;
        }
        if self.phase == Phase::Thinking && self.filler_at.is_some_and(|t| now >= t) && !self.talking() {
            self.filler_at = None;
            return self.say_filler(now);
        }
        match self.phase {
            Phase::Prep { until } if now >= until => self.record(now),
            Phase::Recording { until, .. } if now >= until => vec![Effect::StopRecord],
            _ => Vec::new(),
        }
    }

    /// One of Oyen's thinking lines while the answer is heard (the next in
    /// turn; only voiced ones: "Oyen mikir…" says the rest).
    fn say_filler(&mut self, now: f64) -> Vec<Effect> {
        let Some(f) = self.fillers.pop_front() else { return Vec::new() };
        self.fillers.push_back(f.clone());
        let Some(clip) = f.clip.clone() else { return Vec::new() };
        self.filler = true;
        self.voiced = true;
        self.said_at = now;
        self.say = Some(Say { clip: None, ..f });
        vec![Effect::PauseMusic, Effect::Play(clip)]
    }

    /// The mic button: answer (Oyen may still be talking), or stop the answer.
    pub fn tap(&mut self, now: f64) -> Vec<Effect> {
        match self.phase {
            Phase::Ready | Phase::Prep { .. } => self.record(now),
            Phase::Talking if self.turn.as_ref().is_some_and(Turn::wants_answer) => {
                let mut fx = self.stop_talk();
                fx.extend(self.record(now));
                fx
            }
            Phase::Recording { .. } => vec![Effect::StopRecord],
            _ => Vec::new(),
        }
    }

    fn record(&mut self, now: f64) -> Vec<Effect> {
        let max = self.turn.as_ref().map_or(45.0, |t| t.max_secs).clamp(RECORD_FLOOR_S, RECORD_MAX_S) as f64;
        self.phase = Phase::Recording { since: now, until: now + max };
        self.meter = 0.0;
        self.hint = None;
        vec![Effect::PauseMusic, Effect::Record]
    }

    /// The recording stopped: `wav` holds `secs` of the answer, sent now.
    pub fn recorded(&mut self, wav: PathBuf, secs: f32, now: f64) -> Vec<Effect> {
        let id = self.id.clone().filter(|_| matches!(self.phase, Phase::Recording { .. }));
        let Some(id) = id else { return vec![Effect::Delete(wav)] };
        self.meter = 0.0;
        if secs < ANSWER_MIN_S {
            self.phase = Phase::Ready;
            self.hint = Some(Hint::TooShort);
            return vec![Effect::Delete(wav)];
        }
        self.answer = Some((wav.clone(), secs));
        self.phase = Phase::Thinking;
        self.filler_at = Some(now + FILLER_AFTER_S);
        let call = Call::Answer { id, wav, secs };
        self.pending = Some(call.clone());
        vec![Effect::Call(call)]
    }

    /// The microphone wouldn't start, or stopped mid-answer.
    pub fn mic_failed(&mut self, issue: MicIssue) -> Vec<Effect> {
        if matches!(self.phase, Phase::Recording { .. } | Phase::Ready | Phase::Prep { .. }) {
            self.phase = Phase::Stuck;
            self.problem = Some(Problem::Mic(issue));
            self.meter = 0.0;
        }
        Vec::new()
    }

    /// Lewati: the next question, this one unscored.
    pub fn skip(&mut self) -> Vec<Effect> {
        let live = matches!(self.phase, Phase::Talking | Phase::Prep { .. } | Phase::Ready | Phase::Recording { .. });
        let Some(id) = self.id.clone().filter(|_| live) else { return Vec::new() };
        let mut fx = self.stop_talk();
        if matches!(self.phase, Phase::Recording { .. }) {
            fx.push(Effect::DropRecord);
        }
        self.phase = Phase::Thinking;
        self.hint = None;
        let call = Call::Skip { id };
        self.pending = Some(call.clone());
        fx.push(Effect::Call(call));
        fx
    }

    /// Udahan or Selesai: the session ends (the server forgets it), the
    /// music comes back, and the page shows the ladder, read again.
    pub fn quit(&mut self, now: f64) -> Vec<Effect> {
        if self.phase == Phase::Home {
            return Vec::new();
        }
        let mut fx = self.stop_talk();
        if matches!(self.phase, Phase::Recording { .. }) {
            fx.push(Effect::DropRecord);
        }
        if let Some(h) = self.held.take() {
            fx.extend(clips_of(&h.turn, h.result.as_ref()).into_iter().map(Effect::Delete));
        }
        fx.extend(self.drop_fillers());
        if let Some(id) = self.id.take() {
            fx.push(Effect::Call(Call::End { id }));
        }
        fx.push(Effect::ResumeMusic);
        self.forget_session();
        self.phase = Phase::Home;
        self.me_asked = Some(now);
        fx.push(Effect::Call(Call::Me));
        fx.extend(self.answer.take().map(|(p, _)| Effect::Delete(p)));
        fx
    }

    /// Coba lagi on a stuck session (or on the ladder's error).
    pub fn retry(&mut self, now: f64) -> Vec<Effect> {
        match (self.phase, self.problem) {
            (Phase::Home, _) => {
                self.problem = None;
                self.me_problem = None;
                self.me_asked = Some(now);
                vec![Effect::Call(Call::Me)]
            }
            (Phase::Stuck, Some(Problem::Mic(_) | Problem::BadAudio)) => {
                // The person taps the mic again (a recording that couldn't be read goes).
                self.problem = None;
                self.phase = Phase::Ready;
                self.answer.take().map(|(p, _)| Effect::Delete(p)).into_iter().collect()
            }
            (Phase::Stuck, Some(p)) if p.retryable() => match self.pending.clone() {
                Some(call @ (Call::Answer { .. } | Call::Skip { .. })) => {
                    self.problem = None;
                    self.phase = Phase::Thinking;
                    if matches!(call, Call::Answer { .. }) {
                        self.filler_at = Some(now + FILLER_AFTER_S);
                    }
                    vec![Effect::Call(call)]
                }
                _ => self.quit(now),
            },
            (Phase::Stuck, _) => self.quit(now),
            _ => Vec::new(),
        }
    }

    /// Stop saying things: the clip playing and the ones waiting go. The
    /// turn's last line (the question) stays on screen to read.
    fn stop_talk(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.voiced {
            fx.push(Effect::StopVoice);
        }
        self.voiced = false;
        self.filler = false;
        self.text_until = None;
        let last = self.queue.back().map(|s| Say { clip: None, audio: None, ..s.clone() });
        let clips = self.say.as_mut().and_then(|s| s.clip.take()).into_iter().chain(self.queue.drain(..).filter_map(|s| s.clip));
        fx.extend(clips.map(Effect::Delete));
        if last.is_some() {
            self.say = last;
        }
        fx
    }

    /// The session's thinking lines go with it.
    fn drop_fillers(&mut self) -> Vec<Effect> {
        self.filler_at = None;
        self.fillers.drain(..).filter_map(|f| f.clip).map(Effect::Delete).collect()
    }

    fn forget_session(&mut self) {
        self.id = None;
        self.turn = None;
        self.say = None;
        self.queue.clear();
        self.heard.clear();
        self.result = None;
        self.problem = None;
        self.hint = None;
        self.pending = None;
        self.meter = 0.0;
        self.answers = 0;
        self.held = None;
        self.filler = false;
    }
}

/// Every clip a reply brought (a turn's, and a verdict's roast).
fn clips_of(turn: &Turn, result: Option<&Verdict>) -> Vec<PathBuf> {
    turn.clips().into_iter().chain(result.and_then(|v| v.roast.clip.clone())).collect()
}

// ---------------------------------------------------------------- samples

/// The page's states with made-up content (all of it written for the app,
/// never taken from IELTS materials), for the self-test's and the Mac
/// render's pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sample {
    Home,
    /// Oyen reacting to an answer, in gaul.
    Talking,
    /// The person answering a Part 1 question.
    Recording,
    /// Part 2's card, during the preparation minute.
    CueCard,
    Result,
    /// The microphone blocked by Windows' privacy switch.
    MicDenied,
}

const SAMPLE_ME: &str = r#"{"levels":[
    {"n":1,"name":"Pemanasan","blurb":"Part 1, empat pertanyaan santai.","target":5.0,"parts":[1],"unlocked":true,"best":5.5},
    {"n":2,"name":"Lumayan","blurb":"Part 1 sama Part 2: ngomong dua menit.","target":5.5,"parts":[1,2],"unlocked":true,"best":null},
    {"n":3,"name":"Ujian Beneran","blurb":"Tes lengkap, Part 1 sampai 3.","target":6.5,"parts":[1,2,3],"unlocked":false,"best":null},
    {"n":4,"name":"Ngegas","blurb":"Tes lengkap, follow-up lebih galak.","target":7.0,"parts":[1,2,3],"unlocked":false,"best":null},
    {"n":5,"name":"Dewa IELTS","blurb":"Tes lengkap, Part 3 yang abstrak.","target":8.0,"parts":[1,2,3],"unlocked":false,"best":null}],
    "level":2,"voice":true,"today":{"used":2,"cap":5}}"#;

const SAMPLE_QUESTION: &str = "Let's talk about your hometown. What do you like most about living there?";

impl Coach {
    /// The page in state `s`, `now` being the moment it's drawn.
    pub fn sample(s: Sample, now: f64) -> Coach {
        let line = |text: &str, lang: &str| Say { text: text.into(), lang: lang.into(), audio: None, clip: None };
        let part1 = Turn { part: 1, kind: Kind::Question, say: Vec::new(), expect: Expect::Answer, max_secs: 40.0, prep_secs: 0.0, cue_card: None };
        let mut c = Coach { me: parse_me(SAMPLE_ME), pick: 2, ..Coach::default() };
        if s != Sample::Home {
            c.id = Some("sample".into());
            c.level = 2;
            c.turn = Some(part1.clone());
            c.say = Some(line(SAMPLE_QUESTION, "en"));
            c.answers = 1;
        }
        // Mid-syllable, the same in every picture.
        c.said_at = now - 0.37;
        match s {
            Sample::Home => {}
            Sample::Talking => {
                c.phase = Phase::Talking;
                c.heard = "My hometown is Bandung. It's nice.".into();
                c.say = Some(line("Anjir, \u{201c}it's nice\u{201d} doang? Kasih alasan sama contoh dong, examiner beneran pasti nanya itu.", "id"));
                c.queue.push_back(line("So, what do you like most about living in Bandung?", "en"));
                c.voiced = true;
            }
            Sample::Recording => {
                c.phase = Phase::Recording { since: now - 12.4, until: now - 12.4 + 40.0 };
                c.meter = 0.62;
            }
            Sample::CueCard => {
                c.phase = Phase::Prep { until: now + 42.5 };
                c.answers = 4;
                c.turn = Some(Turn {
                    part: 2,
                    kind: Kind::CueCard,
                    max_secs: 120.0,
                    prep_secs: 60.0,
                    cue_card: Some(CueCard {
                        topic: "Describe a time you helped someone you didn't know.".into(),
                        bullets: vec!["who the person was".into(), "where it happened".into(), "what you did to help".into()],
                        closing: "and explain how you felt about it afterwards.".into(),
                    }),
                    ..part1
                });
                c.say = Some(line("Now I'm going to give you a topic. You have one minute to prepare.", "en"));
            }
            Sample::Result => {
                let note = |band: f32, note: &str| Band { band, note: note.into() };
                let v = Verdict {
                    band: 6.5,
                    criteria: Criteria {
                        fc: note(6.5, "Lancar, cuma kebanyakan \u{201c}emm\u{201d}."),
                        lr: note(6.0, "Kosakata aman, tapi itu-itu aja."),
                        gra: note(6.0, "Tenses masih loncat-loncat, anjir."),
                        p: note(7.0, "Pelafalan jelas. Yang ini gue akuin."),
                    },
                    roast: line("Grammar lo kayak sinyal di lift: putus-putus. Tapi vocab lo lumayan, gue akuin.", "id"),
                    tips: vec![
                        "Jawab Part 1 pakai alasan sama contoh, jangan satu kalimat doang.".into(),
                        "Sambung ide pakai however, on top of that, that's why.".into(),
                        "Latih akhiran -ed: worked, helped, wanted.".into(),
                    ],
                    level: LevelChange { before: 2, after: 3, unlocked: true },
                    best: Some(6.5),
                };
                if let Some(m) = c.me.as_mut() {
                    m.note(2, &v);
                }
                c.pick = 3;
                c.phase = Phase::Result;
                c.id = None;
                c.answers = 9;
                c.say = Some(v.roast.clone());
                c.voiced = true;
                c.result = Some(v);
            }
            Sample::MicDenied => {
                c.phase = Phase::Stuck;
                c.problem = Some(Problem::Mic(MicIssue::Denied));
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = r#"{"levels":[
        {"n":1,"name":"Pemanasan","blurb":"Part 1 aja","target":5.0,"parts":[1],"unlocked":true,"best":5.5},
        {"n":2,"name":"Lumayan","blurb":"Part 1 + 2","target":5.5,"parts":[1,2],"unlocked":true,"best":null},
        {"n":3,"name":"Ujian Beneran","blurb":"Full test","target":6.5,"parts":[1,2,3],"unlocked":false,"best":null},
        {"n":4,"name":"Ngegas","target":7.0,"parts":[1,2,3],"unlocked":false},
        {"n":5,"name":"Dewa IELTS","target":8.0,"parts":[1,2,3],"unlocked":false}],
        "level":2,"voice":true,"today":{"used":2,"cap":5}}"#;

    fn say(text: &str, clip: Option<&str>) -> Say {
        Say { text: text.into(), lang: "en".into(), audio: None, clip: clip.map(PathBuf::from) }
    }

    fn turn(part: u8, kind: Kind, lines: Vec<Say>) -> Turn {
        Turn { part, kind, say: lines, expect: Expect::Answer, max_secs: 40.0, prep_secs: 0.0, cue_card: None }
    }

    fn verdict() -> Verdict {
        Verdict {
            band: 6.5,
            criteria: Criteria::default(),
            roast: say("Anjir, lumayan juga lo.", Some("roast.mp3")),
            tips: vec!["a".into(), "b".into(), "c".into()],
            level: LevelChange { before: 2, after: 3, unlocked: true },
            best: Some(6.5),
        }
    }

    /// A coach on screen with the ladder in.
    fn coach() -> Coach {
        let mut c = Coach::default();
        let fx = c.show(true, 0.0);
        assert_eq!(fx, [Effect::Call(Call::Me)], "the ladder is asked for when the page opens");
        c.reply(Reply::Me(parse_me(ME).unwrap()), 0.0);
        assert_eq!(c.pick, 2, "the person's level is picked");
        c
    }

    fn started(c: &mut Coach) -> Vec<Effect> {
        let fx = c.start(Spice::Ngegas);
        assert_eq!(fx, [Effect::Call(Call::Start { level: 2, spice: Spice::Ngegas })]);
        assert_eq!(c.phase, Phase::Starting);
        let t = turn(1, Kind::Intro, vec![say("Hi, I'm Oyen.", Some("a.mp3")), say("Where are you from?", Some("b.mp3"))]);
        c.reply(Reply::Started { id: "s1".into(), turn: t, fillers: Vec::new() }, 1.0)
    }

    #[test]
    fn parses_the_contract_shapes() {
        let me = parse_me(ME).unwrap();
        assert_eq!((me.level, me.voice, me.today), (2, true, Today { used: 2, cap: 5 }));
        assert_eq!(me.rung(1).unwrap().best, Some(5.5));
        assert!(me.unlocked(2) && !me.unlocked(3) && !me.capped());
        let s = parse_started(
            r#"{"id":"abc","turn":{"part":1,"kind":"intro","say":[{"text":"Hello","lang":"en","audio":{"mime":"audio/mpeg","b64":"SUQz"}},
            {"text":"Halo","lang":"id","audio":null}],"expect":"answer","max_secs":40,"prep_secs":0,"cue_card":null}}"#,
        )
        .unwrap();
        assert_eq!((s.id.as_str(), s.turn.kind, s.turn.say.len(), s.turn.max_secs), ("abc", Kind::Intro, 2, 40.0));
        assert_eq!(s.turn.say[0].audio.as_ref().map(|a| a.ext()), Some("mp3"));
        assert!(s.turn.say[1].audio.is_none());
        let a = parse_answered(
            r#"{"heard":"I'm from Jakarta","turn":{"part":2,"kind":"cue_card","say":[{"text":"Now, a topic.","lang":"en","audio":null}],
            "expect":"answer","max_secs":120,"prep_secs":60,"cue_card":{"topic":"Describe a book","bullets":["what","when","why"],"closing":"and explain why"}},"done":false}"#,
        )
        .unwrap();
        assert_eq!((a.turn.prep_secs, a.turn.cue_card.as_ref().unwrap().bullets.len(), a.done), (60.0, 3, false));
        let done = parse_answered(
            r#"{"heard":"","turn":{"part":0,"kind":"verdict","say":[],"expect":"none","max_secs":0,"prep_secs":0,"cue_card":null},"done":true,
            "result":{"band":6.5,"criteria":{"fc":{"band":6.5,"note":"a"},"lr":{"band":6,"note":"b"},"gra":{"band":6.5,"note":"c"},"p":{"band":7,"note":"d"}},
            "roast":{"text":"Anjir","lang":"id","audio":null},"tips":["x","y","z"],"level":{"before":2,"after":3,"unlocked":true},"best":6.5}}"#,
        )
        .unwrap();
        let r = done.result.unwrap();
        assert_eq!((r.band, r.criteria.p.band, r.level.after, r.best), (6.5, 7.0, 3, Some(6.5)));
        assert_eq!(r.criteria.rows()[2].0, "Grammar");
        assert!(!done.turn.wants_answer());
        // Done without its result can't be shown: unreadable.
        assert!(parse_answered(r#"{"heard":"","turn":{},"done":true}"#).is_none());
        assert_eq!(parse_skipped(r#"{"turn":{"part":3,"kind":"followup","say":[]}}"#).unwrap().turn.part, 3);
        // A kind from a newer server is a question like any other.
        assert_eq!(parse_skipped(r#"{"turn":{"kind":"warm_up"}}"#).unwrap().turn.kind, Kind::Other);
        // A skip that ends the test brings the verdict, like a last answer.
        let end = parse_skipped(r#"{"done":true,"turn":{"kind":"verdict","expect":"none"},"result":{"band":5.5}}"#).unwrap();
        assert!(end.done && end.result.is_some());
        // The fillers come with the session; a refusal can carry Oyen's own line.
        let s = parse_started(r#"{"id":"x","turn":{},"fillers":[{"text":"Hmm, bentar.","lang":"id","audio":null}]}"#).unwrap();
        assert_eq!(s.fillers[0].text, "Hmm, bentar.");
        assert_eq!(problem_say(r#"{"error":"daily_cap","say":[{"text":"Udah lima kali, besok lagi.","lang":"id"}]}"#)[0].lang, "id");
        assert!(problem_say("<html>").is_empty());
    }

    #[test]
    fn urls_and_bodies() {
        assert_eq!(me_url(BASE, "+07:00"), "https://api.oyenza.app/island/ielts/me?tz=%2B07%3A00");
        assert_eq!(session_url("http://127.0.0.1:9"), "http://127.0.0.1:9/island/ielts/session");
        assert_eq!(answer_url(BASE, "a b"), "https://api.oyenza.app/island/ielts/session/a%20b/answer");
        assert_eq!(skip_url(BASE, "x"), "https://api.oyenza.app/island/ielts/session/x/skip");
        assert_eq!(end_url(BASE, "x"), "https://api.oyenza.app/island/ielts/session/x");
        let v: serde_json::Value = serde_json::from_str(&session_body(9, Spice::Ngegas, "+07:00")).unwrap();
        assert_eq!(v, serde_json::json!({ "level": 5, "spice": 2, "tz": "+07:00" }));
        let wav = tone_wav(0.1, 440.0, RATE);
        let v: serde_json::Value = serde_json::from_str(&answer_body(&wav, 23.44)).unwrap();
        assert_eq!(v["audio"]["mime"], "audio/wav");
        assert_eq!(v["secs"], 23.4);
        assert_eq!(b64_decode(v["audio"]["b64"].as_str().unwrap()).unwrap(), wav);
    }

    #[test]
    fn base64_round_trips_with_padding() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(&[0xfb, 0xff]), "+/8=");
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(b64_decode(&b64_encode(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn problems_read_plainly() {
        assert_eq!(problem(503, r#"{"error":"paused"}"#, None), Problem::Paused);
        assert_eq!(problem(429, r#"{"error":"daily_cap"}"#, None), Problem::DailyCap);
        // The server's hourly guard is not the day's cap.
        assert_eq!(problem(429, r#"{"error":"rate_limited"}"#, None), Problem::TooFast);
        assert_eq!(problem(429, "", None), Problem::TooFast);
        assert_eq!(problem(403, r#"{"error":"level_locked"}"#, None), Problem::LevelLocked);
        assert_eq!(problem(404, r#"{"error":"no_session"}"#, None), Problem::NoSession);
        // A server without the routes yet reads as paused, not as an error.
        assert_eq!(problem(404, r#"{"error":"not_found"}"#, None), Problem::Paused);
        assert_eq!(problem(404, "Cannot GET /island/ielts/me", None), Problem::Paused);
        assert_eq!(problem(413, "", None), Problem::TooBig);
        assert_eq!(problem(503, r#"{"error":"ai_busy","retry_after":7}"#, None), Problem::Busy(Some(7)));
        assert_eq!(problem(503, r#"{"error":"ai_busy"}"#, Some(20)), Problem::Busy(Some(20)));
        assert_eq!(problem(502, r#"{"error":"ai_failed"}"#, None), Problem::AiFailed);
        assert_eq!(problem(401, r#"{"error":"unauthorized"}"#, None), Problem::SignedOut);
        assert_eq!(problem(500, "<html>", None), Problem::Server);
        assert_eq!(problem(409, r#"{"error":"busy"}"#, None), Problem::StillThinking);
        assert_eq!(problem(400, r#"{"error":"bad_level"}"#, None), Problem::BadLevel);
        assert_eq!(problem(400, r#"{"error":"bad_audio"}"#, None), Problem::BadAudio);
        assert_eq!(problem(400, r#"{"error":"something_new"}"#, None), Problem::Server);
        let every = [
            Problem::Paused,
            Problem::DailyCap,
            Problem::TooFast,
            Problem::LevelLocked,
            Problem::NoSession,
            Problem::TooBig,
            Problem::Busy(Some(30)),
            Problem::Busy(None),
            Problem::AiFailed,
            Problem::Offline,
            Problem::SignedOut,
            Problem::StillThinking,
            Problem::BadLevel,
            Problem::BadAudio,
            Problem::Server,
            Problem::Mic(MicIssue::Denied),
            Problem::Mic(MicIssue::NoDevice),
            Problem::Mic(MicIssue::Failed),
        ];
        for p in every {
            let words = format!("{} {} {}", p.title(), p.line(false), p.line(true));
            for raw in ["paused", "daily_cap", "level_locked", "no_session", "too_big", "ai_", "HTTP", "40", "50", "41"] {
                assert!(!words.contains(raw), "{p:?} says {raw:?}: {words}");
            }
        }
        assert!(Problem::Mic(MicIssue::Denied).line(true).contains("Privasi"));
        assert!(Problem::Busy(Some(30)).line(false).contains("30 detik"));
    }

    #[test]
    fn wav_writer_fills_in_the_header() {
        let dir = std::env::temp_dir().join(format!("poni-ielts-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.wav");
        let mut w = WavWriter::create(&path).unwrap();
        w.write(&vec![1000i16; 8000]).unwrap();
        w.write(&vec![-1000i16; 8000]).unwrap();
        assert_eq!(w.secs(), 1.0);
        let (p, secs) = w.finish().unwrap();
        assert_eq!((p.as_path(), secs), (path.as_path(), 1.0));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 32_000);
        assert_eq!(wav_info(&bytes), Some(WavInfo { rate: 16_000, channels: 1, bits: 16, secs: 1.0 }));
        assert_eq!(wav_info(&tone_wav(0.5, 440.0, 22_050)).map(|i| (i.rate, i.secs)), Some((22_050, 0.5)));
        assert_eq!(wav_info(b"RIFF\0\0\0\0WAVE"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mac_recording_s_padding_chunk_is_walked_past_and_left_out() {
        // Apple's recorder writes `fmt `, then a 4 KB `FLLR` chunk, then the audio.
        let audio: Vec<u8> = (0..32_000u32).map(|i| (i % 251) as u8).collect();
        let canonical = wav_header(16_000, 1, 16, audio.len() as u32);
        let mut mac = b"RIFF\0\0\0\0WAVE".to_vec();
        mac.extend_from_slice(&canonical[12..36]);
        mac.extend(b"FLLR");
        mac.extend(4044u32.to_le_bytes());
        mac.extend(vec![0u8; 4044]);
        mac.extend(b"data");
        mac.extend((audio.len() as u32).to_le_bytes());
        mac.extend(&audio);
        assert_eq!(mac.len() - audio.len(), 4096, "a 4096-byte header");
        assert_eq!(wav_info(&mac), Some(WavInfo { rate: 16_000, channels: 1, bits: 16, secs: 1.0 }));
        let clean = canonical_wav(&mac).unwrap();
        assert_eq!(&clean[..44], &canonical);
        assert_eq!(&clean[44..], &audio[..]);
        // A data size never filled in reads to the end of the file.
        let mut open = mac.clone();
        let at = open.len() - audio.len() - 4;
        open[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(wav_info(&open).map(|i| i.secs), Some(1.0));
        // Not PCM WAV: nothing to clean.
        assert_eq!(canonical_wav(b"ID3\x04 not a wav"), None);
    }

    fn sine(rate: u32, hz: f32, secs: f32, channels: usize) -> Vec<u8> {
        let n = (rate as f32 * secs) as usize;
        let mut out = Vec::new();
        for i in 0..n {
            let v = (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() * 0.5;
            for _ in 0..channels {
                out.extend(v.to_le_bytes());
            }
        }
        out
    }

    /// Crossings of zero going up, per second.
    fn freq(pcm: &[i16]) -> f32 {
        let ups = pcm.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        ups as f32 / (pcm.len() as f32 / RATE as f32)
    }

    #[test]
    fn downmix_resamples_speech_to_16k_mono() {
        // 48 kHz stereo float (the usual Windows mix format), a 1 kHz tone.
        let mut d = Downmix::new(48_000, 2, SampleFormat::F32);
        let mut out = Vec::new();
        let p = d.push(&sine(48_000, 1000.0, 1.0, 2), &mut out);
        assert!((out.len() as i64 - 16_000).abs() <= 1, "{}", out.len());
        assert!((freq(&out) - 1000.0).abs() < 5.0, "{}", freq(&out));
        assert!((0.45..0.55).contains(&p), "peak {p}");
        // 44.1 kHz 16-bit mono: the length follows the rate.
        let pcm: Vec<u8> = sine(44_100, 500.0, 1.0, 1).chunks(4).flat_map(|b| (((f32::from_le_bytes(b.try_into().unwrap())) * 32767.0) as i16).to_le_bytes()).collect();
        let mut d = Downmix::new(44_100, 1, SampleFormat::I16);
        let mut out = Vec::new();
        d.push(&pcm, &mut out);
        assert!((out.len() as i64 - 16_000).abs() <= 1, "{}", out.len());
        assert!((freq(&out) - 500.0).abs() < 5.0);
    }

    #[test]
    fn downmix_keeps_tones_above_8k_from_folding_back() {
        let mut d = Downmix::new(48_000, 1, SampleFormat::F32);
        let mut out = Vec::new();
        d.push(&sine(48_000, 12_000.0, 1.0, 1), &mut out);
        // Unfiltered, 12 kHz would land at 4 kHz at full strength (0.5).
        assert!(peak(&out[800..]) < 0.05, "{}", peak(&out[800..]));
    }

    #[test]
    fn downmix_gives_the_same_samples_whatever_the_packets() {
        let input = sine(44_100, 700.0, 0.5, 2);
        let mut whole = Vec::new();
        Downmix::new(44_100, 2, SampleFormat::F32).push(&input, &mut whole);
        let mut parts = Vec::new();
        let mut d = Downmix::new(44_100, 2, SampleFormat::F32);
        for chunk in input.chunks(8 * 37) {
            d.push(chunk, &mut parts);
        }
        assert_eq!(whole, parts);
        // 16 kHz mono 16-bit comes through as it is.
        let pcm: Vec<i16> = (0..1000).map(|i| (i * 13 % 2000 - 1000) as i16).collect();
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        Downmix::new(16_000, 1, SampleFormat::I16).push(&bytes, &mut out);
        let back: Vec<i16> = out.iter().map(|s| ((*s as f32 / 32767.0) * 32768.0).round() as i16).collect();
        assert!(back.iter().zip(&pcm).all(|(a, b)| (a - b).abs() <= 1));
        // 24-bit samples read right.
        let mut out = Vec::new();
        Downmix::new(16_000, 1, SampleFormat::I24).push(&[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0], &mut out);
        assert_eq!(out, [16384, -16384]);
        let mut out = Vec::new();
        Downmix::new(48_000, 2, SampleFormat::F32).silence(4800, &mut out);
        assert_eq!((out.len(), peak(&out)), (1600, 0.0));
    }

    #[test]
    fn a_session_from_mulai_to_the_verdict() {
        let mut c = coach();
        let fx = started(&mut c);
        assert_eq!(fx, [Effect::PauseMusic, Effect::Play("a.mp3".into())]);
        assert!(c.holds() && c.talking() && c.phase == Phase::Talking);
        assert_eq!(c.say.as_ref().unwrap().text, "Hi, I'm Oyen.");
        // A clip ends: deleted, the next one plays; then it's the person's turn.
        assert_eq!(c.voice_ended(2.0), [Effect::Delete("a.mp3".into()), Effect::PauseMusic, Effect::Play("b.mp3".into())]);
        assert_eq!(c.voice_ended(3.0), [Effect::Delete("b.mp3".into())]);
        assert_eq!(c.phase, Phase::Ready);
        assert_eq!(c.say.as_ref().unwrap().text, "Where are you from?", "the question stays on screen");
        // A late VoiceEnded from a stopped clip changes nothing.
        assert!(c.voice_ended(3.1).is_empty());
        // Tap: record (music paused again if it came back), tap again: stop.
        assert_eq!(c.tap(4.0), [Effect::PauseMusic, Effect::Record]);
        assert_eq!(c.phase, Phase::Recording { since: 4.0, until: 44.0 });
        assert_eq!(c.tap(20.0), [Effect::StopRecord]);
        let fx = c.recorded("ans1.wav".into(), 16.0, 20.0);
        assert_eq!(fx, [Effect::Call(Call::Answer { id: "s1".into(), wav: "ans1.wav".into(), secs: 16.0 })]);
        assert_eq!(c.phase, Phase::Thinking);
        // Part 2: the card is said, the minute runs, then the recording starts by itself.
        let mut card = turn(2, Kind::CueCard, vec![say("Now, a topic.", Some("c.mp3"))]);
        card.prep_secs = 60.0;
        card.max_secs = 120.0;
        let fx = c.reply(Reply::Answered { heard: "Jakarta".into(), turn: card, done: false, result: None }, 25.0);
        assert_eq!(fx, [Effect::Delete("ans1.wav".into()), Effect::PauseMusic, Effect::Play("c.mp3".into())]);
        assert_eq!((c.heard.as_str(), c.answers), ("Jakarta", 1));
        c.voice_ended(28.0);
        assert_eq!(c.phase, Phase::Prep { until: 88.0 }, "the minute starts once the card is said");
        assert_eq!(c.deadline(), Some(88.0));
        assert!(c.tick(28.1).is_empty() && c.tick(60.0).is_empty());
        assert_eq!(c.tick(88.2), [Effect::PauseMusic, Effect::Record]);
        assert_eq!(c.phase, Phase::Recording { since: 88.2, until: 208.2 });
        // It stops by itself at max_secs.
        assert_eq!(c.tick(208.3), [Effect::StopRecord]);
        c.recorded("ans2.wav".into(), 120.0, 208.3);
        // The verdict: the result shows while Oyen says its three lines (closing,
        // roast, the level that opened); the result's own copy of the roast isn't
        // said twice. Then the music comes back.
        let end = turn(
            0,
            Kind::Verdict,
            vec![say("That's the end of the test.", Some("d.mp3")), say("Anjir, lumayan juga lo.", Some("r.mp3")), say("Level 3 kebuka!", Some("u.mp3"))],
        );
        let fx = c.reply(Reply::Answered { heard: "a book".into(), turn: end, done: true, result: Some(verdict()) }, 215.0);
        assert_eq!(fx, [Effect::Delete("ans2.wav".into()), Effect::Delete("roast.mp3".into()), Effect::PauseMusic, Effect::Play("d.mp3".into())]);
        assert_eq!(c.phase, Phase::Result);
        assert_eq!(c.result.as_ref().unwrap().band, 6.5);
        assert!(c.holds());
        assert_eq!(c.voice_ended(218.0), [Effect::Delete("d.mp3".into()), Effect::PauseMusic, Effect::Play("r.mp3".into())]);
        assert_eq!(c.say.as_ref().unwrap().text, "Anjir, lumayan juga lo.");
        assert_eq!(c.voice_ended(221.0), [Effect::Delete("r.mp3".into()), Effect::PauseMusic, Effect::Play("u.mp3".into())]);
        assert_eq!(c.voice_ended(222.0), [Effect::Delete("u.mp3".into()), Effect::ResumeMusic]);
        assert!(!c.holds() && c.id.is_none(), "over: the island may close");
        // Level 3 opened and is picked; level 2's best went up.
        let me = c.me.as_ref().unwrap();
        assert!(me.unlocked(3) && me.level == 3 && c.pick == 3);
        assert_eq!(me.rung(2).unwrap().best, Some(6.5));
        // Selesai: back to the ladder, read again.
        assert_eq!(c.quit(230.0), [Effect::ResumeMusic, Effect::Call(Call::Me)]);
        assert_eq!(c.phase, Phase::Home);
    }

    #[test]
    fn lines_without_a_voice_show_for_their_reading_time() {
        let mut c = coach();
        c.start(Spice::Sopan);
        let t = turn(1, Kind::Intro, vec![say("Halo, aku Oyen. Kita mulai ya.", None), say("Do you work or study?", None)]);
        assert_eq!(c.reply(Reply::Started { id: "s".into(), turn: t, fillers: Vec::new() }, 10.0), [Effect::PauseMusic]);
        assert!(c.talking() && !c.voiced);
        let until = c.deadline().unwrap();
        assert!(until >= 12.0 && until <= 19.0, "{until}");
        assert!(c.tick(until - 0.1).is_empty());
        assert_eq!(c.tick(until), [Effect::PauseMusic]);
        assert_eq!(c.say.as_ref().unwrap().text, "Do you work or study?");
        c.tick(until + 10.0);
        assert_eq!(c.phase, Phase::Ready);
        // A clip that can't play shows its words instead.
        let mut c = coach();
        started(&mut c);
        assert_eq!(c.voice_failed(2.0), [Effect::Delete("a.mp3".into())]);
        assert!(c.talking() && !c.voiced);
        assert_eq!(c.tick(2.0 + READ_MIN_S), [Effect::PauseMusic, Effect::Play("b.mp3".into())]);
    }

    /// A session with two thinking lines, at the person's turn.
    fn with_fillers() -> Coach {
        let mut c = coach();
        c.start(Spice::Ngegas);
        let t = turn(1, Kind::Intro, vec![say("Where are you from?", None)]);
        let fillers = vec![say("Hmm, bentar.", Some("f1.mp3")), say("Oke, gue dengerin dulu.", Some("f2.mp3"))];
        c.reply(Reply::Started { id: "s1".into(), turn: t, fillers }, 1.0);
        c.tick(5.0);
        assert_eq!(c.phase, Phase::Ready);
        c
    }

    #[test]
    fn a_thinking_line_fills_the_wait_and_never_cuts_the_reply() {
        let mut c = with_fillers();
        c.tap(10.0);
        c.tap(20.0);
        assert_eq!(c.recorded("a1.wav".into(), 10.0, 20.0), [Effect::Call(Call::Answer { id: "s1".into(), wav: "a1.wav".into(), secs: 10.0 })]);
        // A second and a half into the wait, one thinking line.
        assert!(c.tick(21.4).is_empty());
        assert_eq!(c.tick(21.6), [Effect::PauseMusic, Effect::Play("f1.mp3".into())]);
        assert!(c.talking() && c.phase == Phase::Thinking);
        assert!(c.tick(23.0).is_empty(), "one a wait");
        // The reply comes mid-line: it waits for the line's end.
        let next = turn(1, Kind::Question, vec![say("Why?", Some("q.mp3"))]);
        assert_eq!(c.reply(Reply::Answered { heard: "Bandung".into(), turn: next, done: false, result: None }, 22.0), [Effect::Delete("a1.wav".into())]);
        assert_eq!(c.phase, Phase::Thinking);
        // The line ends (its clip is kept for later waits): the reply goes on.
        assert_eq!(c.voice_ended(23.5), [Effect::PauseMusic, Effect::Play("q.mp3".into())]);
        assert_eq!((c.phase, c.heard.as_str()), (Phase::Talking, "Bandung"));
        // A reply that comes before the line is due: no line.
        c.voice_ended(25.0);
        c.tap(26.0);
        c.tap(30.0);
        c.recorded("a2.wav".into(), 4.0, 30.0);
        let next = turn(1, Kind::Question, vec![say("And?", None)]);
        c.reply(Reply::Answered { heard: "x".into(), turn: next, done: false, result: None }, 31.0);
        assert!(c.tick(31.6).is_empty() && c.phase == Phase::Talking);
        // The next wait says the next line; Udahan deletes the session's lines.
        c.tick(40.0);
        c.tap(41.0);
        c.tap(45.0);
        c.recorded("a3.wav".into(), 4.0, 45.0);
        assert_eq!(c.tick(46.6), [Effect::PauseMusic, Effect::Play("f2.mp3".into())]);
        assert_eq!(
            c.quit(47.0),
            [
                Effect::StopVoice,
                Effect::Delete("f1.mp3".into()),
                Effect::Delete("f2.mp3".into()),
                Effect::Call(Call::End { id: "s1".into() }),
                Effect::ResumeMusic,
                Effect::Call(Call::Me),
                Effect::Delete("a3.wav".into())
            ]
        );
    }

    #[test]
    fn a_skip_that_ends_the_test_shows_the_verdict() {
        let mut c = with_fillers();
        assert_eq!(c.skip(), [Effect::Call(Call::Skip { id: "s1".into() })]);
        let end = turn(0, Kind::Verdict, vec![say("That's all.", None)]);
        c.reply(Reply::Skipped { turn: end, done: true, result: Some(verdict()) }, 6.0);
        assert_eq!(c.phase, Phase::Result);
        assert_eq!(c.heard, "", "a skip heard nothing");
        assert_eq!(c.answers, 0);
    }

    #[test]
    fn an_unreadable_recording_is_made_again_and_a_busy_session_waits() {
        let mut c = with_fillers();
        c.tap(10.0);
        c.tap(20.0);
        c.recorded("a1.wav".into(), 10.0, 20.0);
        c.reply(Reply::Failed { call: CallKind::Answer, problem: Problem::BadAudio, say: Vec::new() }, 21.0);
        assert_eq!(c.phase, Phase::Stuck);
        assert_eq!(c.retry(22.0), [Effect::Delete("a1.wav".into())]);
        assert_eq!(c.phase, Phase::Ready);
        // Still hearing an earlier answer: Coba lagi sends this one again, later.
        c.tap(23.0);
        c.tap(30.0);
        c.recorded("a2.wav".into(), 7.0, 30.0);
        c.reply(Reply::Failed { call: CallKind::Answer, problem: Problem::StillThinking, say: Vec::new() }, 31.0);
        assert_eq!(c.retry(35.0), [Effect::Call(Call::Answer { id: "s1".into(), wav: "a2.wav".into(), secs: 7.0 })]);
        assert_eq!(c.phase, Phase::Thinking);
        assert!(c.tap(35.5).is_empty(), "no second answer while one is heard");
    }

    #[test]
    fn a_clip_that_never_ends_doesnt_hold_the_session() {
        let mut c = coach();
        started(&mut c);
        assert!(c.tick(1.0 + CLIP_MAX_S - 0.1).is_empty());
        assert_eq!(c.tick(1.0 + CLIP_MAX_S + 0.1), [Effect::StopVoice, Effect::Delete("a.mp3".into()), Effect::PauseMusic, Effect::Play("b.mp3".into())]);
    }

    #[test]
    fn answering_while_oyen_talks_cuts_him_short() {
        let mut c = coach();
        started(&mut c);
        assert_eq!(c.tap(1.5), [Effect::StopVoice, Effect::Delete("a.mp3".into()), Effect::Delete("b.mp3".into()), Effect::PauseMusic, Effect::Record]);
        assert!(matches!(c.phase, Phase::Recording { .. }));
        assert_eq!(c.say.as_ref().unwrap().text, "Where are you from?", "the question he didn't get to stays on screen");
        // A slip of the finger sends nothing.
        c.tap(1.8);
        assert_eq!(c.recorded("short.wav".into(), 0.3, 1.9), [Effect::Delete("short.wav".into())]);
        assert_eq!((c.phase, c.hint), (Phase::Ready, Some(Hint::TooShort)));
    }

    #[test]
    fn leaving_the_page_pauses_and_coming_back_goes_on() {
        let mut c = coach();
        started(&mut c);
        // Away mid-line: the voice stops, the line waits.
        assert_eq!(c.show(false, 1.5), [Effect::StopVoice]);
        assert!(!c.talking() && c.queued() == 2);
        assert!(c.tick(100.0).is_empty(), "no clock while away");
        // Back: the line again, from its start.
        assert_eq!(c.show(true, 5.0), [Effect::PauseMusic, Effect::Play("a.mp3".into())]);
        // Away while recording: the recording goes, the person answers when back.
        c.voice_ended(6.0);
        c.voice_ended(7.0);
        c.tap(8.0);
        assert_eq!(c.show(false, 9.0), [Effect::DropRecord]);
        assert_eq!(c.phase, Phase::Ready);
        // A turn that comes while away waits to be said.
        c.show(true, 10.0);
        c.tap(10.0);
        c.recorded("a.wav".into(), 5.0, 10.5);
        c.show(false, 11.0);
        let next = turn(1, Kind::Question, vec![say("Why?", Some("e.mp3"))]);
        assert_eq!(c.reply(Reply::Answered { heard: "x".into(), turn: next, done: false, result: None }, 12.0), [Effect::Delete("a.wav".into())]);
        assert_eq!(c.show(true, 13.0), [Effect::PauseMusic, Effect::Play("e.mp3".into())]);
    }

    #[test]
    fn skip_and_quit() {
        let mut c = coach();
        started(&mut c);
        c.voice_ended(2.0);
        c.voice_ended(3.0);
        c.tap(4.0);
        assert_eq!(c.skip(), [Effect::DropRecord, Effect::Call(Call::Skip { id: "s1".into() })]);
        assert_eq!(c.phase, Phase::Thinking);
        assert!(c.skip().is_empty(), "nothing to skip while thinking");
        let next = turn(1, Kind::Question, vec![say("Next one.", Some("f.mp3"))]);
        c.reply(Reply::Skipped { turn: next, done: false, result: None }, 5.0);
        // Udahan mid-line: the voice and its clips go, the server forgets the session, music back.
        assert_eq!(
            c.quit(6.0),
            [Effect::StopVoice, Effect::Delete("f.mp3".into()), Effect::Call(Call::End { id: "s1".into() }), Effect::ResumeMusic, Effect::Call(Call::Me)]
        );
        assert!(c.phase == Phase::Home && c.id.is_none() && !c.holds());
        // A session that arrives after Udahan is ended at once.
        c.reply(Reply::Me(parse_me(ME).unwrap()), 6.5);
        c.start(Spice::Gaul);
        c.quit(7.0);
        let late = turn(1, Kind::Intro, vec![say("Hi", Some("g.mp3"))]);
        let late = Reply::Started { id: "s2".into(), turn: late, fillers: vec![say("Hmm.", Some("f.mp3"))] };
        assert_eq!(c.reply(late, 8.0), [Effect::Delete("g.mp3".into()), Effect::Delete("f.mp3".into()), Effect::Call(Call::End { id: "s2".into() })]);
        assert_eq!(c.phase, Phase::Home);
    }

    #[test]
    fn a_busy_server_keeps_the_answer_for_try_again() {
        let mut c = coach();
        started(&mut c);
        c.voice_ended(2.0);
        c.voice_ended(3.0);
        c.tap(4.0);
        c.recorded("ans.wav".into(), 9.0, 9.5);
        assert!(c.reply(Reply::Failed { call: CallKind::Answer, problem: Problem::Busy(Some(5)), say: Vec::new() }, 10.0).is_empty());
        assert_eq!((c.phase, c.problem), (Phase::Stuck, Some(Problem::Busy(Some(5)))));
        assert!(!c.holds(), "the island may close over it");
        assert_eq!(c.retry(16.0), [Effect::Call(Call::Answer { id: "s1".into(), wav: "ans.wav".into(), secs: 9.0 })]);
        assert_eq!((c.phase, c.problem), (Phase::Thinking, None));
        // An expired session can't take it: Mulai lagi goes home and deletes it.
        c.reply(Reply::Failed { call: CallKind::Answer, problem: Problem::NoSession, say: Vec::new() }, 17.0);
        assert_eq!(c.phase, Phase::Stuck);
        assert_eq!(c.retry(18.0), [Effect::ResumeMusic, Effect::Call(Call::Me), Effect::Delete("ans.wav".into())]);
        assert_eq!(c.phase, Phase::Home);
    }

    #[test]
    fn a_day_used_up_and_the_microphone_blocked() {
        let mut c = coach();
        c.start(Spice::Ngegas);
        // Oyen says why himself (his line comes with the refusal), and the ladder is read again.
        let why = vec![say("Udah lima kali hari ini, besok lagi.", Some("cap.mp3"))];
        let fx = c.reply(Reply::Failed { call: CallKind::Start, problem: Problem::DailyCap, say: why }, 1.0);
        assert_eq!(fx, [Effect::Call(Call::Me), Effect::PauseMusic, Effect::Play("cap.mp3".into())]);
        assert_eq!((c.phase, c.problem), (Phase::Home, Some(Problem::DailyCap)));
        assert!(c.talking() && !c.holds());
        assert_eq!(c.voice_ended(4.0), [Effect::Delete("cap.mp3".into()), Effect::ResumeMusic]);
        assert_eq!(c.say.as_ref().unwrap().text, "Udah lima kali hari ini, besok lagi.", "his line stays on the ladder");
        assert!(c.me.as_ref().unwrap().capped() && !c.startable(2));
        assert!(c.start(Spice::Ngegas).is_empty(), "no Mulai on a capped day");
        // A locked level can't start either.
        let mut c = coach();
        c.pick = 3;
        assert!(c.start(Spice::Ngegas).is_empty());
        // The microphone blocked: Coba lagi goes back to the person's turn.
        let mut c = coach();
        started(&mut c);
        c.voice_ended(2.0);
        c.voice_ended(3.0);
        c.tap(4.0);
        c.mic_failed(MicIssue::Denied);
        assert_eq!((c.phase, c.problem), (Phase::Stuck, Some(Problem::Mic(MicIssue::Denied))));
        assert!(c.retry(5.0).is_empty());
        assert_eq!(c.phase, Phase::Ready);
        // The ladder that couldn't be read: Coba lagi reads it again.
        let mut c = Coach::default();
        c.show(true, 0.0);
        c.reply(Reply::Failed { call: CallKind::Me, problem: Problem::Offline, say: Vec::new() }, 1.0);
        assert_eq!(c.me_problem, Some(Problem::Offline));
        assert_eq!(c.retry(2.0), [Effect::Call(Call::Me)]);
    }

    #[test]
    fn samples_show_each_state() {
        let now = 100.0;
        let phase = |s| Coach::sample(s, now).phase;
        assert_eq!(phase(Sample::Home), Phase::Home);
        assert_eq!(phase(Sample::Talking), Phase::Talking);
        assert!(matches!(phase(Sample::Recording), Phase::Recording { .. }));
        assert!(matches!(phase(Sample::CueCard), Phase::Prep { .. }));
        assert_eq!(phase(Sample::Result), Phase::Result);
        assert_eq!(phase(Sample::MicDenied), Phase::Stuck);
        let r = Coach::sample(Sample::Result, now);
        assert!(r.talking() && r.me.as_ref().unwrap().unlocked(3));
        assert_eq!(fmt_band(r.result.as_ref().unwrap().band), "6.5");
        assert!(Coach::sample(Sample::Talking, now).talking());
        assert!(Coach::sample(Sample::CueCard, now).turn.unwrap().cue_card.is_some());
        assert!(Coach::sample(Sample::Home, now).startable(2));
    }

    #[test]
    fn spice_and_labels() {
        assert_eq!(Spice::default(), Spice::Ngegas);
        assert_eq!(Spice::from_n(0), Spice::Sopan);
        assert_eq!(Spice::from_n(7), Spice::Ngegas);
        assert_eq!(Spice::Gaul.n(), 1);
        assert_eq!(fmt_band(7.0), "7.0");
        assert_eq!(fmt_band(6.25), "6.5");
        assert_eq!(fmt_band(11.0), "9.0");
        assert_eq!(fmt_secs(59.2), "1:00");
        assert_eq!(fmt_secs(7.0), "0:07");
        let mut t = turn(2, Kind::CueCard, vec![]);
        assert_eq!(part_label(&t), "Part 2");
        t.kind = Kind::Verdict;
        assert_eq!(part_label(&t), "Hasil");
    }

    #[test]
    fn clips_are_saved_and_the_base64_dropped() {
        let dir = std::env::temp_dir().join(format!("poni-ielts-clips-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = parse_skipped(
            r#"{"turn":{"say":[{"text":"a","audio":{"mime":"audio/mpeg","b64":"SUQzBA=="}},{"text":"b","audio":{"mime":"audio/wav","b64":"!!"}},{"text":"c"}]}}"#,
        )
        .unwrap()
        .turn;
        t.save_clips(&dir, "t1");
        assert_eq!(t.say[0].clip.as_deref(), Some(dir.join("t1-0.mp3").as_path()));
        assert_eq!(std::fs::read(dir.join("t1-0.mp3")).unwrap(), b"ID3\x04");
        assert!(t.say.iter().all(|s| s.audio.is_none()));
        assert!(t.say[1].clip.is_none() && t.say[2].clip.is_none(), "bad base64 and no audio: words only");
        assert_eq!(t.clips().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
