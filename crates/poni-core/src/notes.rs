//! Meeting notes, the Meeting notes add-on: one click and Oyen listens to the
//! microphone and to the PC's own sound together, so a call in Meet, Zoom,
//! Teams or WhatsApp is heard with nobody joining it; after Stop the Oyenza
//! server hears the meeting and writes the notes, which are saved as a
//! Markdown file on this PC.
//!
//! This is the part without Windows: the two sources mixed onto one track
//! ([`Mixer`]), the calls' addresses and answers (the contract,
//! `~/projects/studio-notes/CONTRACT.md`), the page's moments ([`Taker`]),
//! what went wrong in plain words ([`Problem`]), and the notes as Markdown.
//! The capture, the uploads and the file are `services/notes.rs`.

use crate::datetime::{LocalDateTime, MONTH_SHORT, WEEKDAY_SHORT};
use serde_json::Value;
use std::collections::VecDeque;

pub use crate::ielts::RATE;

pub const BASE: &str = crate::oyenza::API;
pub const PATH: &str = "/island/notes";
/// Each part of the audio: five minutes (9.6 MB), written and sent in turn.
pub const PART_SECS: u32 = 300;
/// The longest meeting the server takes; the recording stops itself there.
pub const MAX_SECS: f64 = 3.0 * 3600.0;
/// How long the PC waits between asks while the server writes the notes.
pub const POLL_S: f64 = 4.0;
/// How long it keeps asking: an hour of meeting takes the server ~1-3 minutes.
pub const WRITING_MAX_S: f64 = 20.0 * 60.0;

// ---------------------------------------------------------------- the mix

/// How far behind the clock the mix runs, so each source's late packets
/// still land where they were heard (a capture wakes every 40 ms).
const LAG_SAMPLES: u64 = RATE as u64 / 5;
/// A source this far ahead of the mix (its clock running fast against the
/// PC's) loses its oldest samples, so the two never drift apart.
const MAX_QUEUE: usize = RATE as usize * 3 / 5;

/// The microphone and the PC's sound (each already 16 kHz mono) mixed onto
/// one track, in step with the clock. The PC's sound stops coming while
/// nothing plays (Windows sends no loopback packets then), so the clock,
/// not either source, says how much track there is: a source with nothing
/// to give adds silence.
#[derive(Default)]
pub struct Mixer {
    mic: VecDeque<i16>,
    pc: VecDeque<i16>,
    /// Samples mixed out so far.
    out: u64,
}

impl Mixer {
    pub fn push_mic(&mut self, pcm: &[i16]) {
        push(&mut self.mic, pcm);
    }

    pub fn push_pc(&mut self, pcm: &[i16]) {
        push(&mut self.pc, pcm);
    }

    /// The track up to `clock` samples since the start (less the lag),
    /// appended to `out`.
    pub fn take(&mut self, clock: u64, out: &mut Vec<i16>) {
        let due = clock.saturating_sub(LAG_SAMPLES);
        while self.out < due {
            let m = self.mic.pop_front().unwrap_or(0) as i32;
            let p = self.pc.pop_front().unwrap_or(0) as i32;
            out.push((m + p).clamp(i16::MIN as i32, i16::MAX as i32) as i16);
            self.out += 1;
        }
    }

    /// Everything still waiting, at the end of the recording.
    pub fn drain(&mut self, out: &mut Vec<i16>) {
        let left = self.mic.len().max(self.pc.len()) as u64;
        self.take(self.out + LAG_SAMPLES + left, out);
    }
}

fn push(q: &mut VecDeque<i16>, pcm: &[i16]) {
    q.extend(pcm.iter().copied());
    if q.len() > MAX_QUEUE {
        let extra = q.len() - MAX_QUEUE;
        q.drain(..extra);
    }
}

// ---------------------------------------------------------------- the calls

pub fn start_url(base: &str) -> String {
    format!("{base}{PATH}")
}

pub fn part_url(base: &str, id: &str, n: u32) -> String {
    format!("{base}{PATH}/{}/part/{n}", crate::urlenc::encode(id))
}

pub fn finish_url(base: &str, id: &str) -> String {
    format!("{base}{PATH}/{}/finish", crate::urlenc::encode(id))
}

pub fn view_url(base: &str, id: &str) -> String {
    format!("{base}{PATH}/{}", crate::urlenc::encode(id))
}

/// The notes' language: the meeting's own, or always one of these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Auto,
    Id,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Auto => "auto",
            Lang::Id => "id",
            Lang::En => "en",
        }
    }
}

pub fn start_body(lang: Lang) -> String {
    serde_json::json!({ "lang": lang.code() }).to_string()
}

fn json(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body).ok().filter(Value::is_object)
}

fn text(v: &Value) -> String {
    v.as_str().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(text).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

/// `POST /island/notes`: the meeting's id on the server.
pub fn parse_started(body: &str) -> Option<String> {
    let id = text(&json(body)?["id"]);
    (!id.is_empty()).then_some(id)
}

/// `POST …/part/n`: the part the server wants next.
pub fn parse_part(body: &str) -> Option<u32> {
    json(body)?["next"].as_u64().map(|n| n.min(u32::MAX as u64) as u32)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Action {
    pub who: String,
    pub what: String,
    pub when: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Topic {
    pub title: String,
    pub points: Vec<String>,
}

/// One turn of the transcript: `t` seconds from the start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub t: f64,
    pub who: String,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notes {
    pub title: String,
    pub lang: String,
    pub summary: Vec<String>,
    pub decisions: Vec<String>,
    pub actions: Vec<Action>,
    pub topics: Vec<Topic>,
    pub transcript: Vec<Line>,
    /// The meeting's length, in seconds.
    pub secs: u32,
}

/// `GET /island/notes/:id`.
#[derive(Clone, Debug, PartialEq)]
pub enum View {
    /// Still taking parts (the finish didn't arrive).
    Open,
    Working,
    Done(Box<Notes>),
    Failed(Problem),
}

pub fn parse_view(body: &str) -> Option<View> {
    let v = json(body)?;
    match v["status"].as_str()? {
        "open" => Some(View::Open),
        "working" => Some(View::Working),
        "failed" => Some(View::Failed(problem(200, body, None))),
        "done" => {
            let n = &v["notes"];
            let notes = Notes {
                title: Some(text(&n["title"])).filter(|t| !t.is_empty()).unwrap_or_else(|| "Meeting".into()),
                lang: text(&n["lang"]),
                summary: texts(&n["summary"]),
                decisions: texts(&n["decisions"]),
                actions: n["actions"]
                    .as_array()
                    .map(|a| a.iter().map(|a| Action { who: text(&a["who"]), what: text(&a["what"]), when: text(&a["when"]) }).filter(|a| !a.what.is_empty()).collect())
                    .unwrap_or_default(),
                topics: n["topics"]
                    .as_array()
                    .map(|a| a.iter().map(|t| Topic { title: text(&t["title"]), points: texts(&t["points"]) }).filter(|t| !t.title.is_empty()).collect())
                    .unwrap_or_default(),
                transcript: v["transcript"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|l| Line { t: l["t"].as_f64().filter(|t| t.is_finite()).unwrap_or(0.0).max(0.0), who: text(&l["who"]), text: text(&l["text"]) })
                            .filter(|l| !l.text.is_empty())
                            .collect()
                    })
                    .unwrap_or_default(),
                secs: v["secs"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
            };
            Some(View::Done(Box::new(notes)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- what went wrong

/// Why nothing could be recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureIssue {
    /// Windows' privacy switch keeps desktop apps from the microphone.
    MicDenied,
    /// Neither the microphone nor the PC's sound would open.
    NoSound,
    Failed,
}

/// What went wrong, as the page says it: never a code or a status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// 503 `paused`: switched off on the server.
    Paused,
    /// 429 `daily_cap`: today's minutes are used.
    DailyCap,
    /// 413 `too_long`: past the longest meeting the server takes.
    TooLong,
    /// `ai_busy` (and 429/503 otherwise), with the wait when the server said.
    Busy(Option<u64>),
    /// `ai_failed` (and 502).
    AiFailed,
    /// `nothing_heard`: nobody spoke, as far as the server could hear.
    NothingHeard,
    /// 404 `no_notes`: the server forgot the meeting (a restart): it is sent again.
    Gone,
    /// No answer: no internet, or the server didn't answer in time.
    Offline,
    /// 401, or no Oyenza key on this PC.
    SignedOut,
    /// The server took too long to write.
    TooSlow,
    /// The app closed on a meeting (a crash, a restart, the PC turned off)
    /// before its notes were written: its recording was kept.
    Unfinished,
    /// 403 `pro_required`: this Oyenza account doesn't have Studio Pro.
    ProRequired,
    /// Any other status, or an answer that couldn't be read.
    Server,
    Capture(CaptureIssue),
}

/// The problem a failed call's status and body (`{"error": "…"}`, with
/// `retry_after` seconds) stand for; a `failed` view's `error` too.
pub fn problem(status: u16, body: &str, retry_after: Option<u64>) -> Problem {
    let e = json(body).unwrap_or_default();
    let wait = e["retry_after"].as_f64().filter(|s| s.is_finite() && *s >= 0.0).map(|s| s.ceil() as u64).or(retry_after);
    match (status, e["error"].as_str().unwrap_or_default()) {
        (401, _) => Problem::SignedOut,
        (_, "paused") => Problem::Paused,
        (_, "daily_cap") => Problem::DailyCap,
        (_, "too_long") => Problem::TooLong,
        (_, "pro_required") => Problem::ProRequired,
        (_, "nothing_heard") => Problem::NothingHeard,
        (_, "no_notes") | (404, _) => Problem::Gone,
        (_, "ai_busy") | (429, _) | (503, _) => Problem::Busy(wait),
        (_, "ai_failed") | (502, _) => Problem::AiFailed,
        _ => Problem::Server,
    }
}

impl Problem {
    pub fn title(self) -> &'static str {
        match self {
            Problem::Paused => "Meeting notes are paused",
            Problem::DailyCap => "Today's minutes are used up",
            Problem::TooLong => "That meeting is too long",
            Problem::Busy(_) => "Oyen is swamped",
            Problem::AiFailed | Problem::Server | Problem::Gone | Problem::TooSlow => "Oyen couldn't write the notes",
            Problem::NothingHeard => "Oyen didn't hear anyone",
            Problem::Unfinished => "Your last meeting isn't written up",
            Problem::ProRequired => "Meeting notes are part of Studio Pro",
            Problem::Offline => "Can't reach Oyenza",
            Problem::SignedOut => "You're signed out of Oyenza",
            Problem::Capture(CaptureIssue::MicDenied) => "The microphone is blocked",
            Problem::Capture(CaptureIssue::NoSound) => "No sound to record",
            Problem::Capture(CaptureIssue::Failed) => "Recording stopped",
        }
    }

    /// What to do about it. The recording stays on this PC for Try again
    /// until the notes are saved or it is thrown away.
    pub fn line(self) -> String {
        match self {
            Problem::Paused => "Oyen is resting. Your recording is kept: try again later.".into(),
            Problem::DailyCap => "Your recording is kept. Try again tomorrow.".into(),
            Problem::TooLong => "Meetings up to 3 hours are written up.".into(),
            Problem::Busy(Some(s)) if s > 1 => format!("Your recording is kept. Try again in {s} seconds."),
            Problem::Busy(_) => "Your recording is kept. Try again in a minute.".into(),
            Problem::AiFailed | Problem::Server | Problem::Gone | Problem::TooSlow => "Your recording is kept. Try again.".into(),
            Problem::NothingHeard => "Was the call muted? Check that the bars move next time.".into(),
            Problem::Unfinished => "Its recording was kept when Oyenza Studio closed. Write it up now, or throw it away.".into(),
            Problem::ProRequired => "This Oyenza account doesn't have Pro. Your recording is kept.".into(),
            Problem::Offline => "Your recording is kept. Check the internet, then try again.".into(),
            Problem::SignedOut => "Sign in to Oyenza again: your recording is kept.".into(),
            Problem::Capture(CaptureIssue::MicDenied) if cfg!(target_os = "macos") => "Turn on Oyenza Studio in System Settings \u{203a} Privacy & Security \u{203a} Microphone.".into(),
            Problem::Capture(CaptureIssue::MicDenied) => "Turn on microphone access for desktop apps in Windows Settings.".into(),
            Problem::Capture(CaptureIssue::NoSound) => "Plug in a microphone or speakers, then try again.".into(),
            Problem::Capture(CaptureIssue::Failed) => "A sound device went away. What was heard is kept.".into(),
        }
    }

    /// Try again can help (and the recording is there for it).
    pub fn retryable(self) -> bool {
        !matches!(self, Problem::TooLong | Problem::NothingHeard | Problem::Capture(CaptureIssue::MicDenied) | Problem::Capture(CaptureIssue::NoSound))
    }
}

// ---------------------------------------------------------------- the page

/// Where the notes stand.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    /// Nothing under way: Take notes.
    #[default]
    Idle,
    /// Listening since `since` (mono seconds).
    Listening { since: f64 },
    /// The meeting is over and its audio goes up: `sent` of `parts`.
    Sending { sent: u32, parts: u32 },
    /// The server hears it and writes, since `since`.
    Writing { since: f64 },
    /// The notes are saved (`Taker::last`).
    Ready,
    /// Something went wrong; the recording is kept unless thrown away.
    Failed(Problem),
}

/// The notes saved last, for the page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    pub notes: Notes,
    /// The Markdown file.
    pub file: String,
    /// When the meeting started (local time).
    pub started: LocalDateTime,
}

/// What the worker reports while it hands a meeting over.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Sent { sent: u32, parts: u32 },
    Writing,
    Saved(Box<Saved>),
    Failed(Problem),
}

#[derive(Clone, Debug, Default)]
pub struct Taker {
    pub phase: Phase,
    /// The microphone's and the PC's sound's loudness, 0..1, while listening.
    pub mic: f32,
    pub pc: f32,
    /// Which sources record (one can be missing: no microphone, say).
    pub mic_on: bool,
    pub pc_on: bool,
    /// The recording's length, once stopped.
    pub secs: f64,
    pub last: Option<Saved>,
}

impl Taker {
    pub fn listening(&self) -> bool {
        matches!(self.phase, Phase::Listening { .. })
    }

    /// Something to show on the closed island: listening, or the notes on their way.
    pub fn live(&self) -> bool {
        matches!(self.phase, Phase::Listening { .. } | Phase::Sending { .. } | Phase::Writing { .. })
    }

    /// The recording's time now.
    pub fn elapsed(&self, now: f64) -> f64 {
        match self.phase {
            Phase::Listening { since } => (now - since).max(0.0),
            _ => self.secs,
        }
    }

    /// Seconds until the time on show moves to its next second, while listening.
    pub fn next_second(&self, now: f64) -> Option<f64> {
        self.listening().then(|| 1.0 - self.elapsed(now).fract())
    }

    pub fn started(&mut self, now: f64, mic: bool, pc: bool) {
        self.phase = Phase::Listening { since: now };
        (self.mic_on, self.pc_on, self.mic, self.pc, self.secs) = (mic, pc, 0.0, 0.0, 0.0);
    }

    /// Stopped with `parts` parts of `secs` seconds: on their way up.
    pub fn stopped(&mut self, parts: u32, secs: f64) {
        self.secs = secs;
        self.phase = Phase::Sending { sent: 0, parts };
        (self.mic, self.pc) = (0.0, 0.0);
    }

    pub fn reply(&mut self, r: Reply, now: f64) {
        self.phase = match r {
            Reply::Sent { sent, parts } => Phase::Sending { sent, parts },
            Reply::Writing => match self.phase {
                Phase::Writing { since } => Phase::Writing { since },
                _ => Phase::Writing { since: now },
            },
            Reply::Saved(s) => {
                self.last = Some(*s);
                Phase::Ready
            }
            Reply::Failed(p) => Phase::Failed(p),
        };
    }

    pub fn failed(&mut self, p: Problem) {
        self.phase = Phase::Failed(p);
        (self.mic, self.pc) = (0.0, 0.0);
    }

    /// Back to Take notes (after the notes, or a recording thrown away).
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        (self.mic, self.pc, self.secs) = (0.0, 0.0, 0.0);
    }
}

/// "0:07", "12:34", "1:02:03".
pub fn fmt_length(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// ---------------------------------------------------------------- the file

/// "2026-10-29 14.05 Launch plan.md": sorts by date, and keeps to what
/// Windows allows in a name.
pub fn file_name(started: &LocalDateTime, title: &str) -> String {
    let clean: String = title.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { ' ' } else { c }).collect();
    let mut clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > 60 {
        clean = clean.chars().take(60).collect::<String>().trim_end().to_string();
    }
    let clean = clean.trim_end_matches('.').trim().to_string();
    let title = if clean.is_empty() { "Meeting".to_string() } else { clean };
    format!("{:04}-{:02}-{:02} {:02}.{:02} {title}.md", started.year, started.month, started.day, started.hour, started.minute)
}

/// The notes as Markdown: what Copy puts on the clipboard and the file holds.
pub fn markdown(n: &Notes, started: &LocalDateTime, with_transcript: bool) -> String {
    let mut md = format!("# {}\n\n", n.title);
    let mins = ((n.secs as f64) / 60.0).round().max(1.0) as u32;
    md += &format!(
        "{} {} {} {} · {:02}:{:02} · {mins} min\n",
        WEEKDAY_SHORT[started.weekday() as usize % 7],
        started.day,
        MONTH_SHORT[(started.month.clamp(1, 12) - 1) as usize],
        started.year,
        started.hour,
        started.minute
    );
    let list = |md: &mut String, head: &str, items: &[String]| {
        if !items.is_empty() {
            *md += &format!("\n## {head}\n\n");
            for i in items {
                *md += &format!("- {i}\n");
            }
        }
    };
    list(&mut md, "Summary", &n.summary);
    list(&mut md, "Decisions", &n.decisions);
    if !n.actions.is_empty() {
        md += "\n## Action items\n\n";
        for a in &n.actions {
            let by = [a.who.as_str(), a.when.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
            md += &if by.is_empty() { format!("- [ ] {}\n", a.what) } else { format!("- [ ] {} ({by})\n", a.what) };
        }
    }
    if !n.topics.is_empty() {
        md += "\n## Notes\n";
        for t in &n.topics {
            md += &format!("\n### {}\n\n", t.title);
            for p in &t.points {
                md += &format!("- {p}\n");
            }
        }
    }
    if with_transcript && !n.transcript.is_empty() {
        md += "\n## Transcript\n\n";
        for l in &n.transcript {
            md += &format!("**[{}] {}:** {}\n\n", fmt_length(l.t), l.who, l.text);
        }
    }
    md += "\n---\nNotes by Oyen, in Oyenza Studio\n";
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u8, m: u8) -> LocalDateTime {
        LocalDateTime { year: 2026, month: 10, day: 29, hour: h, minute: m, second: 0 }
    }

    #[test]
    fn the_mix_follows_the_clock_and_fills_a_quiet_source_with_silence() {
        let mut m = Mixer::default();
        let mut out = Vec::new();
        m.push_mic(&[100; 4000]);
        // The PC plays nothing: no loopback packets at all.
        m.take(LAG_SAMPLES + 4000, &mut out);
        assert_eq!(out.len(), 4000);
        assert!(out.iter().all(|&s| s == 100));
        // Then both, added together, and a peak clipped rather than wrapped.
        m.push_mic(&[30_000; 10]);
        m.push_pc(&[10_000; 10]);
        out.clear();
        m.take(LAG_SAMPLES + 4010, &mut out);
        assert_eq!(out, vec![i16::MAX; 10]);
        // The clock moved on with nothing from either: silence, not a gap.
        out.clear();
        m.take(LAG_SAMPLES + 4110, &mut out);
        assert_eq!(out, vec![0; 100]);
    }

    #[test]
    fn a_source_running_ahead_loses_its_oldest_samples() {
        let mut m = Mixer::default();
        m.push_pc(&vec![1; MAX_QUEUE]);
        m.push_pc(&[2; 500]);
        let mut out = Vec::new();
        m.take(LAG_SAMPLES + MAX_QUEUE as u64, &mut out);
        assert_eq!(out.len(), MAX_QUEUE);
        assert_eq!(out[..MAX_QUEUE - 500], vec![1; MAX_QUEUE - 500][..]);
        assert_eq!(out[MAX_QUEUE - 500..], [2; 500][..], "the newest are kept");
    }

    #[test]
    fn the_end_of_a_recording_takes_what_still_waits() {
        let mut m = Mixer::default();
        m.push_mic(&[5; 300]);
        m.push_pc(&[7; 100]);
        let mut out = Vec::new();
        m.drain(&mut out);
        assert_eq!(out.len(), 300);
        assert_eq!(out[..100], [12; 100][..]);
        assert_eq!(out[100..], [5; 200][..]);
    }

    #[test]
    fn addresses_and_answers() {
        assert_eq!(start_url(BASE), "https://api.oyenza.app/island/notes");
        assert_eq!(part_url("http://127.0.0.1:9", "a b", 3), "http://127.0.0.1:9/island/notes/a%20b/part/3");
        assert_eq!(finish_url("x", "id1"), "x/island/notes/id1/finish");
        assert_eq!(view_url("x", "id1"), "x/island/notes/id1");
        assert_eq!(start_body(Lang::Auto), r#"{"lang":"auto"}"#);
        assert_eq!(parse_started(r#"{"id":"dxY","part_max":12582912}"#).as_deref(), Some("dxY"));
        assert_eq!(parse_started(r#"{"error":"paused"}"#), None);
        assert_eq!(parse_part(r#"{"next":2,"secs":137}"#), Some(2));
        assert_eq!(parse_view(r#"{"status":"working","secs":12}"#), Some(View::Working));
        assert_eq!(parse_view(r#"{"status":"failed","secs":2,"error":"nothing_heard"}"#), Some(View::Failed(Problem::NothingHeard)));
        assert_eq!(parse_view("<html>"), None);
    }

    #[test]
    fn done_notes_are_read_whole_and_tidy() {
        let body = r#"{"status":"done","secs":137,"notes":{"title":"Launch  plan","lang":"id","summary":["Rilis Jumat",""],
            "decisions":["Harga 49rb"],"actions":[{"who":"Budi","what":"Kirim deck","when":"Kamis"},{"who":"","what":""}],
            "topics":[{"title":"Harga","points":["49rb"]},{"title":"","points":["x"]}]},
            "transcript":[{"t":12.5,"who":"Speaker 1","text":"Halo  semua"},{"t":"x","who":"Budi","text":"oke"},{"t":20,"who":"A","text":""}]}"#;
        let Some(View::Done(n)) = parse_view(body) else { panic!("done") };
        assert_eq!(n.title, "Launch plan");
        assert_eq!(n.summary, vec!["Rilis Jumat"]);
        assert_eq!(n.actions, vec![Action { who: "Budi".into(), what: "Kirim deck".into(), when: "Kamis".into() }]);
        assert_eq!(n.topics.len(), 1);
        assert_eq!(n.transcript, vec![Line { t: 12.5, who: "Speaker 1".into(), text: "Halo semua".into() }, Line { t: 0.0, who: "Budi".into(), text: "oke".into() }]);
        assert_eq!(n.secs, 137);
    }

    #[test]
    fn problems_in_plain_words() {
        assert_eq!(problem(401, "", None), Problem::SignedOut);
        assert_eq!(problem(503, r#"{"error":"paused"}"#, None), Problem::Paused);
        assert_eq!(problem(429, r#"{"error":"daily_cap","used_min":240,"cap_min":240}"#, None), Problem::DailyCap);
        assert_eq!(problem(503, r#"{"error":"ai_busy","retry_after":17}"#, None), Problem::Busy(Some(17)));
        assert_eq!(problem(404, r#"{"error":"no_notes"}"#, None), Problem::Gone);
        assert_eq!(problem(403, r#"{"error":"pro_required"}"#, None), Problem::ProRequired);
        assert_eq!(problem(500, "", None), Problem::Server);
        assert!(Problem::Gone.retryable() && Problem::Offline.retryable());
        assert!(!Problem::NothingHeard.retryable() && !Problem::TooLong.retryable());
        assert_eq!(Problem::Busy(Some(30)).line(), "Your recording is kept. Try again in 30 seconds.");
    }

    #[test]
    fn the_page_follows_the_meeting() {
        let mut t = Taker::default();
        assert!(!t.live());
        t.started(100.0, true, true);
        assert!(t.listening() && t.live());
        assert_eq!(fmt_length(t.elapsed(165.4)), "1:05");
        assert!((t.next_second(165.4).unwrap() - 0.6).abs() < 1e-9);
        t.stopped(3, 742.0);
        assert_eq!(t.phase, Phase::Sending { sent: 0, parts: 3 });
        assert_eq!(t.next_second(800.0), None);
        t.reply(Reply::Sent { sent: 3, parts: 3 }, 900.0);
        t.reply(Reply::Writing, 901.0);
        t.reply(Reply::Writing, 905.0);
        assert_eq!(t.phase, Phase::Writing { since: 901.0 }, "a second ask keeps the first time");
        t.reply(Reply::Saved(Box::new(Saved { file: "x.md".into(), ..Default::default() })), 950.0);
        assert_eq!(t.phase, Phase::Ready);
        assert!(!t.live());
        assert_eq!(t.last.as_ref().unwrap().file, "x.md");
        assert_eq!(fmt_length(3723.0), "1:02:03");
    }

    #[test]
    fn file_names_sort_by_date_and_keep_to_what_windows_allows() {
        assert_eq!(file_name(&at(14, 5), "Launch plan: Q4 / pricing?"), "2026-10-29 14.05 Launch plan Q4 pricing.md");
        assert_eq!(file_name(&at(9, 0), "  "), "2026-10-29 09.00 Meeting.md");
        assert_eq!(file_name(&at(9, 0), "Ends with dots..."), "2026-10-29 09.00 Ends with dots.md");
        assert!(file_name(&at(9, 0), &"x".repeat(200)).chars().count() <= 80);
    }

    #[test]
    fn markdown_has_every_part_that_has_something() {
        let n = Notes {
            title: "Launch plan".into(),
            summary: vec!["Rilis Jumat".into()],
            actions: vec![Action { who: "Budi".into(), what: "Kirim deck".into(), when: "Kamis".into() }, Action { what: "Cek harga".into(), ..Default::default() }],
            topics: vec![Topic { title: "Harga".into(), points: vec!["49rb".into()] }],
            transcript: vec![Line { t: 65.0, who: "Budi".into(), text: "Oke.".into() }],
            secs: 1834,
            ..Default::default()
        };
        let md = markdown(&n, &at(14, 5), true);
        assert!(md.starts_with("# Launch plan\n\nThu 29 Oct 2026 · 14:05 · 31 min\n"), "{md}");
        assert!(md.contains("\n## Summary\n\n- Rilis Jumat\n"));
        assert!(!md.contains("## Decisions"), "nothing decided: no heading");
        assert!(md.contains("- [ ] Kirim deck (Budi, Kamis)\n- [ ] Cek harga\n"));
        assert!(md.contains("### Harga\n\n- 49rb\n"));
        assert!(md.contains("**[1:05] Budi:** Oke.\n"));
        assert!(!markdown(&n, &at(14, 5), false).contains("Transcript"));
    }
}
