//! AI usage: how much of each AI coding plan a person has left (Codex,
//! Claude Code, OpenCode Go and the rest), for the AI tab, plus their
//! inference.oyenza.com balance once they connect it.
//!
//! Each service is one file here (`codex.rs`, `claude.rs`, ...) behind the
//! same [`Provider`] contract, and all of it is pure, so every service is
//! tested on any OS against answers kept in `fixtures/<service>/`:
//!
//! 1. [`Provider::wants`]: which files on this computer to read (the
//!    service's own sign-in or logs), a Mac keychain item, or a key the
//!    person typed into Oyenza Studio. The app reads them ([`Got`]).
//! 2. [`Provider::next`], called with what was read and the answers so far:
//!    either a request to make ([`Step::Fetch`], at most [`MAX_FETCHES`] a
//!    refresh) or the result ([`Step::Done`]): a [`Snapshot`] or why there
//!    is none ([`Trouble`]).
//!
//! The apps' `services::usage` (one file, the Windows app's, which the Mac
//! app runs too) does the reading and the HTTP on one worker thread, only
//! while the AI tab is on screen, or every few minutes for the person who
//! switched on the heads-up notices, resting while nobody is at the
//! computer.
//!
//! Rules every service keeps:
//! - Read only. Never write another app's files, and never refresh another
//!   app's sign-in: a refresh can sign the person out of that app. An
//!   expired sign-in is [`Trouble::Expired`] ("open Claude Code once").
//! - A sign-in read here goes only to its own service's server (the token
//!   from Codex's file goes to chatgpt.com, never anywhere else), and never
//!   into the log, an error message or a snapshot.
//! - Ask as Oyenza Studio: never set a `User-Agent` that pretends to be
//!   another app.
//! - Unknown is not zero: a number the answer doesn't carry stays `None`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub mod alerts;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod kimi;
pub mod minimax;
pub mod opencode;
pub mod openrouter;
pub mod oyenza;
pub mod pace;
pub mod zai;

/// The most requests one refresh of one service makes.
pub const MAX_FETCHES: usize = 4;
/// The most wants one refresh of one service reads: [`Provider::wants`] and
/// every [`Step::Read`] together.
pub const MAX_READS: usize = 64;
/// The largest file [`Want::File`] asks for unless a service says otherwise.
/// A bigger file reads as [`Got::Missing`], so a service whose file can grow
/// (a config that keeps history) sets its own `max`.
pub const FILE_MAX: u64 = 256 * 1024;

/// The services, in the order the AI tab lists them by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
    OpenCode,
    Gemini,
    Cursor,
    Copilot,
    Kimi,
    Zai,
    MiniMax,
    OpenRouter,
    /// inference.oyenza.com: the person's balance and spend, once connected.
    Oyenza,
}

impl ProviderId {
    pub const ALL: [ProviderId; 11] = [
        ProviderId::Codex,
        ProviderId::Claude,
        ProviderId::OpenCode,
        ProviderId::Gemini,
        ProviderId::Cursor,
        ProviderId::Copilot,
        ProviderId::Kimi,
        ProviderId::Zai,
        ProviderId::MiniMax,
        ProviderId::OpenRouter,
        ProviderId::Oyenza,
    ];

    /// Stable key: settings, the log, fixture folders.
    pub fn key(self) -> &'static str {
        match self {
            ProviderId::Codex => "codex",
            ProviderId::Claude => "claude",
            ProviderId::OpenCode => "opencode",
            ProviderId::Gemini => "gemini",
            ProviderId::Cursor => "cursor",
            ProviderId::Copilot => "copilot",
            ProviderId::Kimi => "kimi",
            ProviderId::Zai => "zai",
            ProviderId::MiniMax => "minimax",
            ProviderId::OpenRouter => "openrouter",
            ProviderId::Oyenza => "oyenza",
        }
    }

    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            ProviderId::Codex => "Codex",
            ProviderId::Claude => "Claude Code",
            ProviderId::OpenCode => "OpenCode Go",
            ProviderId::Gemini => "Gemini CLI",
            ProviderId::Cursor => "Cursor",
            ProviderId::Copilot => "GitHub Copilot",
            ProviderId::Kimi => "Kimi Code",
            ProviderId::Zai => "Z.ai GLM",
            ProviderId::MiniMax => "MiniMax",
            ProviderId::OpenRouter => "OpenRouter",
            ProviderId::Oyenza => "Oyenza Inference",
        }
    }

    pub fn from_key(key: &str) -> Option<ProviderId> {
        ProviderId::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// Which app is asking: paths and keychains differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
}

/// Where things live on this computer, filled in by the app. Its `Debug`
/// prints variable names only: some variables are keys.
#[derive(Clone, PartialEq, Eq)]
pub struct Dirs {
    pub os: Os,
    /// `%USERPROFILE%` / `$HOME`.
    pub home: PathBuf,
    /// `%APPDATA%` / `~/Library/Application Support`.
    pub config: PathBuf,
    /// `%LOCALAPPDATA%` / `~/Library/Caches`.
    pub local: PathBuf,
    /// The variables services asked for ([`Provider::env`]) that are set.
    /// Some are keys (`OPENROUTER_API_KEY`): never print the values.
    pub env: Vec<(String, String)>,
}

impl std::fmt::Debug for Dirs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("Dirs").field("os", &self.os).field("home", &self.home).field("config", &self.config).field("local", &self.local).field("env", &names).finish()
    }
}

impl Dirs {
    pub fn var(&self, name: &str) -> Option<&str> {
        self.env.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).filter(|v| !v.trim().is_empty())
    }

    /// A path from a variable, else the default under home.
    pub fn var_or_home(&self, name: &str, rel: &str) -> PathBuf {
        match self.var(name) {
            Some(v) => PathBuf::from(v),
            None => self.join(&self.home, &[rel]),
        }
    }

    /// `base` with `parts` below it, with this OS's separator, so a Windows
    /// path built in a test on Linux reads `C:\Users\u\.codex\auth.json`
    /// like the real one. Parts may hold `/` (split on it).
    pub fn join(&self, base: &Path, parts: &[&str]) -> PathBuf {
        match self.os {
            Os::Windows => {
                let mut s = base.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
                for part in parts.iter().flat_map(|p| p.split(['/', '\\'])).filter(|p| !p.is_empty()) {
                    s.push('\\');
                    s.push_str(part);
                }
                PathBuf::from(s)
            }
            Os::Mac => {
                let mut p = base.to_path_buf();
                for part in parts.iter().flat_map(|p| p.split('/')).filter(|p| !p.is_empty()) {
                    p.push(part);
                }
                p
            }
        }
    }

    /// Under home: `dirs.home_join(&[".codex", "auth.json"])`.
    pub fn home_join(&self, parts: &[&str]) -> PathBuf {
        self.join(&self.home, parts)
    }

    /// A sample for tests: `/home/u` style paths for the given OS.
    pub fn sample(os: Os) -> Dirs {
        match os {
            Os::Windows => Dirs {
                os,
                home: PathBuf::from(r"C:\Users\u"),
                config: PathBuf::from(r"C:\Users\u\AppData\Roaming"),
                local: PathBuf::from(r"C:\Users\u\AppData\Local"),
                env: Vec::new(),
            },
            Os::Mac => Dirs {
                os,
                home: PathBuf::from("/Users/u"),
                config: PathBuf::from("/Users/u/Library/Application Support"),
                local: PathBuf::from("/Users/u/Library/Caches"),
                env: Vec::new(),
            },
        }
    }
}

/// Something to read on this computer before asking anyone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Want {
    /// One file, whole, refused past `max` bytes.
    File { path: PathBuf, max: u64 },
    /// The newest files under `dir` (down `depth` folders) whose names end in
    /// `suffix`, newest first, at most `count`, each read from its end up to
    /// `tail` bytes (a log's last lines).
    Newest { dir: PathBuf, suffix: &'static str, depth: u8, count: usize, tail: u64 },
    /// A macOS keychain password (generic password, by service and account).
    /// Windows answers [`Got::Missing`].
    Keychain { service: String, account: Option<String> },
    /// A key the person typed into Oyenza Studio for this service, kept
    /// sealed by the app (DPAPI / keychain), by name.
    Secret(&'static str),
    /// `len` bytes of a file from `offset` (fewer at its end), for files too
    /// big to read whole that a service reads a piece at a time (Cursor's
    /// SQLite store can pass 10 GB). [`Got::One`] with what was read,
    /// [`Got::Missing`] if the file isn't there or can't be read. The app
    /// opens the file sharing read, write and delete (the owner keeps
    /// writing it), caps a piece at 4 MiB and never keeps pieces between
    /// refreshes.
    Range { path: PathBuf, offset: u64, len: u32 },
}

impl Want {
    pub fn file(path: impl Into<PathBuf>) -> Want {
        Want::File { path: path.into(), max: FILE_MAX }
    }
}

/// One file the app read. Its `Debug` shows the size, not the bytes (they
/// can be a sign-in).
#[derive(Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    /// Last change, unix seconds.
    pub modified: Option<i64>,
}

impl std::fmt::Debug for FileRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileRead").field("path", &self.path).field("bytes", &self.bytes.len()).field("modified", &self.modified).finish()
    }
}

/// What one [`Want`] gave, in the same order as the wants. Its `Debug`
/// never shows a keychain password or a typed key.
#[derive(Clone, PartialEq, Eq)]
pub enum Got {
    /// Not there (or too big).
    Missing,
    One(FileRead),
    /// [`Want::Newest`]: newest first, maybe empty.
    Many(Vec<FileRead>),
    /// A keychain password or a typed key.
    Text(String),
    /// The keychain said no.
    Denied,
}

impl std::fmt::Debug for Got {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Got::Missing => f.write_str("Missing"),
            Got::One(r) => f.debug_tuple("One").field(r).finish(),
            Got::Many(v) => f.debug_tuple("Many").field(v).finish(),
            Got::Text(t) => write!(f, "Text(<{} chars>)", t.chars().count()),
            Got::Denied => f.write_str("Denied"),
        }
    }
}

impl Got {
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Got::One(f) => Some(&f.bytes),
            _ => None,
        }
    }

    /// The text, without a UTF-8 byte order mark (older Windows editors
    /// write one); None when blank.
    pub fn text(&self) -> Option<&str> {
        let s = match self {
            Got::Text(s) => s.as_str(),
            Got::One(f) => std::str::from_utf8(&f.bytes).ok()?,
            _ => return None,
        };
        Some(s.trim_start_matches('\u{feff}')).filter(|s| !s.trim().is_empty())
    }

    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(self.text()?).ok()
    }

    pub fn files(&self) -> &[FileRead] {
        match self {
            Got::Many(v) => v,
            Got::One(f) => std::slice::from_ref(f),
            _ => &[],
        }
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, Got::Missing)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// A request for the app to make. The app adds its own `User-Agent`. Its
/// `Debug` shows the URL without its query and the header names only.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// A JSON body (POST).
    pub json: Option<String>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let url = self.url.split(['?', '#']).next().unwrap_or("");
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("Request").field("method", &self.method).field("url", &url).field("headers", &names).field("json", &self.json.as_ref().map(|j| j.len())).finish()
    }
}

impl Request {
    pub fn get(url: impl Into<String>) -> Request {
        Request { method: Method::Get, url: url.into(), headers: Vec::new(), json: None }
    }

    pub fn post_json(url: impl Into<String>, json: impl Into<String>) -> Request {
        Request { method: Method::Post, url: url.into(), headers: vec![("Content-Type".into(), "application/json".into())], json: Some(json.into()) }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Request {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn bearer(self, token: &str) -> Request {
        self.header("Authorization", format!("Bearer {}", token.trim()))
    }
}

/// An answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub retry_after_s: Option<u64>,
}

/// Why a request got no answer at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetFail {
    Timeout,
    Network(String),
    Other(String),
}

/// What one refresh may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Read this computer's files only (no network).
    Local,
    /// Files, then the service's own server.
    Online,
}

/// What [`Provider::next`] sees.
#[derive(Clone, Copy, Debug)]
pub struct Ctx<'a> {
    pub dirs: &'a Dirs,
    /// One entry per [`Provider::wants`], in order.
    pub got: &'a [Got],
    /// Unix seconds.
    pub now: i64,
    pub reach: Reach,
}

impl Ctx<'_> {
    /// The nth want's result ([`Got::Missing`] past the end).
    pub fn got(&self, i: usize) -> &Got {
        self.got.get(i).unwrap_or(&Got::Missing)
    }
}

#[derive(Debug)]
pub enum Step {
    Fetch(Request),
    /// Read these too, then ask again (the next piece of a file depends on
    /// the last one): the app reads them like [`Provider::wants`], appends
    /// what it got to [`Ctx::got`] in order and calls [`Provider::next`]
    /// again with the same replies. Fine with [`Reach::Local`]. At most
    /// [`MAX_READS`] wants a refresh, the first ones included.
    Read(Vec<Want>),
    Done(Outcome),
}

pub type Outcome = Result<Snapshot, Trouble>;

/// One service's reader.
pub trait Provider: Sync {
    fn id(&self) -> ProviderId;

    /// Environment variables this service's paths depend on (`CODEX_HOME`,
    /// `CLAUDE_CONFIG_DIR`, ...); the app puts the set ones in [`Dirs::env`].
    fn env(&self) -> &'static [&'static str] {
        &[]
    }

    /// What to read first. Cheap: called on every refresh.
    fn wants(&self, dirs: &Dirs) -> Vec<Want>;

    /// The next step, from what was read and the answers so far, in order
    /// (empty the first time). With [`Reach::Local`] it must not fetch.
    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step;

    /// The shortest gap between two online asks, in seconds: what the
    /// service's numbers are worth refreshing at.
    fn min_gap_s(&self) -> u64 {
        120
    }

    /// The service needs a key typed into Oyenza Studio ([`Want::Secret`]);
    /// the tab then offers a field for it.
    fn takes_key(&self) -> Option<KeyAsk> {
        None
    }
}

/// How the tab asks for a typed key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyAsk {
    /// The [`Want::Secret`] name it is kept under.
    pub secret: &'static str,
    /// Where the person makes one (opened by the tab's link).
    pub url: &'static str,
    /// What the field says while empty, e.g. "sk-or-…".
    pub hint: &'static str,
}

/// Every service, in [`ProviderId::ALL`] order.
pub fn all() -> [&'static dyn Provider; 11] {
    [
        &codex::Codex,
        &claude::Claude,
        &opencode::OpenCode,
        &gemini::Gemini,
        &cursor::Cursor,
        &copilot::Copilot,
        &kimi::Kimi,
        &zai::Zai,
        &minimax::MiniMax,
        &openrouter::OpenRouter,
        &oyenza::Oyenza,
    ]
}

pub fn provider(id: ProviderId) -> &'static dyn Provider {
    all()[ProviderId::ALL.iter().position(|p| *p == id).unwrap_or(0)]
}

/// What a service's plan looks like right now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub provider: ProviderId,
    /// "Plus", "Pro", "Max 20x", "Go", ...
    pub plan: Option<String>,
    /// Who is signed in, already masked ([`mask_email`]).
    pub account: Option<String>,
    /// Limits that reset: the 5-hour window, the week, the month.
    pub windows: Vec<Window>,
    /// Prepaid money left (credits, a balance).
    pub balance: Option<Money>,
    /// Money spent over a period ("Today", "This month"), with its cap.
    pub spend: Vec<Spend>,
    /// Tokens used today, from this computer's own logs.
    pub tokens_today: Option<u64>,
    /// Use per day, oldest first, when the service gives it (a short line
    /// on the card).
    pub days: Vec<DayUse>,
    /// When these numbers were true, unix seconds (the answer, or the log
    /// line they came from).
    pub as_of: i64,
    pub source: Source,
}

impl Snapshot {
    pub fn new(provider: ProviderId, as_of: i64, source: Source) -> Snapshot {
        Snapshot { provider, plan: None, account: None, windows: Vec::new(), balance: None, spend: Vec::new(), tokens_today: None, days: Vec::new(), as_of, source }
    }

    /// The window closest to running out, as the numbers were read.
    pub fn tightest(&self) -> Option<&Window> {
        self.windows.iter().max_by(|a, b| a.used.total_cmp(&b.used))
    }

    /// The window closest to running out now (resets that passed count).
    pub fn tightest_at(&self, now: i64) -> Option<Window> {
        self.windows.iter().map(|w| w.at(now)).max_by(|a, b| a.used.total_cmp(&b.used))
    }
}

/// One day's use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayUse {
    /// "2026-10-27" (the service's day, usually UTC).
    pub day: String,
    pub money: Option<Money>,
    pub requests: Option<u64>,
    pub tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// The service's own server said so.
    Online,
    /// Read from this computer (a log the service's app writes).
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    /// A rolling few hours (Claude's and Codex's 5 hours).
    Session,
    Daily,
    Weekly,
    Monthly,
    Other,
}

/// One limit that fills up and resets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub kind: WindowKind,
    /// What the tab calls it: "5-hour", "Weekly", "Weekly · Opus",
    /// "Premium requests".
    pub label: String,
    /// Used, 0.0 = none, 1.0 = all (can pass 1.0 when over).
    pub used: f32,
    /// When it empties again, unix seconds.
    pub resets_at: Option<i64>,
    /// How long the window is, in seconds.
    pub length_s: Option<u32>,
    /// Counted units when the service gives them: (used, of).
    pub count: Option<(u64, u64)>,
}

impl Window {
    pub fn new(kind: WindowKind, label: impl Into<String>, used: f32) -> Window {
        Window { kind, label: label.into(), used: if used.is_finite() { used.max(0.0) } else { 0.0 }, resets_at: None, length_s: None, count: None }
    }

    pub fn resets(mut self, at: Option<i64>) -> Window {
        self.resets_at = at;
        self
    }

    pub fn length(mut self, s: Option<u32>) -> Window {
        self.length_s = s;
        self
    }

    pub fn counted(mut self, used: u64, of: u64) -> Window {
        self.count = Some((used, of));
        self
    }

    /// What's left, 0.0..=1.0.
    pub fn left(&self) -> f32 {
        (1.0 - self.used).clamp(0.0, 1.0)
    }

    /// A reset that has already passed means the window is empty now, even
    /// if the last reading (say, a log line from yesterday) was full.
    pub fn at(&self, now: i64) -> Window {
        match self.resets_at {
            Some(t) if t <= now => {
                let mut w = self.clone();
                w.used = 0.0;
                w.count = w.count.map(|(_, of)| (0, of));
                // One window later, if that's still ahead. Past that it isn't
                // known: Codex's and Claude's 5 hours start again only with
                // the next use, so a later reset would be made up.
                w.resets_at = w.length_s.map(|len| t + len as i64).filter(|next| *next > now);
                w
            }
            _ => self.clone(),
        }
    }
}

/// An amount of money, in millionths (micro-units) of `currency`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub micros: i64,
    /// ISO code, "USD".
    pub currency: String,
}

impl Money {
    pub fn usd(dollars: f64) -> Money {
        Money { micros: micros(dollars), currency: "USD".into() }
    }

    /// "$4.20", "$0.003", "-$1.50"; other currencies "12.50 EUR".
    pub fn fmt(&self) -> String {
        let neg = self.micros < 0;
        let v = self.micros.unsigned_abs() as f64 / 1e6;
        let (lt, v) = if v != 0.0 && v < 0.001 { ("<", 0.001) } else { ("", v) };
        let digits = if v != 0.0 && v < 0.01 { 3 } else { 2 };
        let n = format!("{v:.digits$}");
        let s = if self.currency == "USD" { format!("{lt}${n}") } else { format!("{lt}{n} {}", self.currency) };
        if neg {
            format!("-{s}")
        } else {
            s
        }
    }
}

/// Dollars to micro-units, safely (NaN and absurd values are 0).
pub fn micros(dollars: f64) -> i64 {
    let m = (dollars * 1e6).round();
    if m.is_finite() && m.abs() < 9.0e15 {
        m as i64
    } else {
        0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spend {
    /// "Today", "This month", "Last 30 days".
    pub label: String,
    pub money: Money,
    pub cap: Option<Money>,
}

/// Why there is no snapshot. The tab says each in plain words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trouble {
    /// Nothing of this service on this computer.
    NotFound,
    /// Installed, but nobody is signed in.
    SignedOut,
    /// The sign-in here has expired; the service's own app renews it the
    /// next time it runs (we never do).
    Expired,
    /// It needs a key typed into Oyenza Studio.
    NeedsKey,
    /// The service refused the sign-in or key (401/403).
    Refused,
    /// The account itself is switched off on the service's side (signing in
    /// again won't help).
    Disabled,
    /// Too many asks: wait this long.
    Busy { retry_after_s: Option<u64> },
    /// No answer: offline, DNS, TLS, timeout.
    Offline,
    /// The service answered, but not in a shape we read (it changed).
    Unreadable(String),
    /// The keychain said no.
    Denied,
    /// Only online numbers exist and this refresh was [`Reach::Local`].
    NeedsOnline,
}

impl Trouble {
    /// The usual reading of a failed request: what to tell the person.
    /// A 403 isn't always a refused sign-in (GitHub says "rate limited" with
    /// 403, OpenRouter "management keys only"): read the body first where the
    /// service does that. The failure's own text never comes through, since
    /// it can carry a URL with a key in it.
    pub fn from_net(r: &Result<Reply, NetFail>) -> Option<Trouble> {
        match r {
            Err(NetFail::Timeout) | Err(NetFail::Network(_)) => Some(Trouble::Offline),
            Err(NetFail::Other(_)) => Some(Trouble::Unreadable("the request failed".into())),
            Ok(r) if r.status == 401 || r.status == 403 => Some(Trouble::Refused),
            Ok(r) if r.status == 429 => Some(Trouble::Busy { retry_after_s: r.retry_after_s }),
            Ok(r) if r.status >= 500 => Some(Trouble::Offline),
            Ok(r) if !(200..300).contains(&r.status) => Some(Trouble::Unreadable(format!("HTTP {}", r.status))),
            Ok(_) => None,
        }
    }
}

/// The last request's answer as JSON, or why not.
pub fn reply_json(r: &Result<Reply, NetFail>) -> Result<Value, Trouble> {
    if let Some(t) = Trouble::from_net(r) {
        return Err(t);
    }
    match r {
        Ok(r) => serde_json::from_str(r.body.trim_start_matches('\u{feff}')).map_err(|_| Trouble::Unreadable("not JSON".into())),
        Err(_) => Err(Trouble::Offline),
    }
}

/// "someone@example.com" -> "s•••@example.com"; anything else keeps its first
/// character only.
pub fn mask_email(s: &str) -> String {
    let s = s.trim();
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return String::new(),
    };
    match s.split_once('@') {
        Some((_, domain)) => format!("{first}•••@{domain}"),
        None => format!("{first}•••"),
    }
}

/// A JWT's claims (the middle part), unverified: for reading a plan name
/// or an expiry out of a token this computer already holds.
pub fn jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_, claims) = (parts.next()?, parts.next()?);
    serde_json::from_slice(&b64url(claims)?).ok()
}

/// base64url without padding (padding tolerated).
pub fn b64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// RFC 3339 / ISO 8601 time to unix seconds: "2026-10-27T05:00:00Z",
/// "2026-10-27T05:00:00.123+07:00", "2026-10-27 05:00:00" (taken as UTC).
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ' || b[10] == b't') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r).filter(|d| d.bytes().all(|b| b.is_ascii_digit()))?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        rest = r.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        r if (r.starts_with('+') || r.starts_with('-')) && r.len() >= 5 => {
            let sign = if r.starts_with('-') { -1 } else { 1 };
            let digits = |d: &str| d.bytes().all(|b| b.is_ascii_digit());
            let hh: i64 = r.get(1..3).filter(|d| digits(d))?.parse().ok()?;
            let mm: i64 = r.get(r.len() - 2..).filter(|d| digits(d))?.parse().ok()?;
            sign * (hh * 3600 + mm * 60)
        }
        _ => return None,
    };
    let days = poni_core::datetime::days_from_civil(y as i32, mo as u8, d as u8);
    Some(days * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

/// A number that may arrive as a JSON number or a numeric string.
pub fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }.filter(|f| f.is_finite())
}

/// A time that may arrive as unix seconds, unix milliseconds or RFC 3339.
pub fn time(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => parse_time(s).or_else(|| s.trim().parse::<f64>().ok().map(unix_s)),
        _ => num(v).map(unix_s),
    }
}

/// Seconds or milliseconds since 1970 to seconds (anything past the year
/// 33658 in seconds is taken as milliseconds).
fn unix_s(x: f64) -> i64 {
    if x > 1.0e12 {
        (x / 1000.0) as i64
    } else {
        x as i64
    }
}

/// Sorts a short list (a few windows, a month of days) so that `after(a, b)`
/// holds for no two neighbours, keeping equal ones in their order. The
/// standard stable sort adds about 6 KB to poni.exe for each type it sorts;
/// these lists hold a few dozen at most, where moving each one back into
/// place is just as quick.
pub fn sort_small_by<T>(v: &mut [T], after: impl Fn(&T, &T) -> bool) {
    for i in 1..v.len() {
        let mut j = i;
        while j > 0 && after(&v[j - 1], &v[j]) {
            v.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// `path` joined from parts, for building paths without caring about `\`.
pub fn join(base: &Path, parts: &[&str]) -> PathBuf {
    let mut p = base.to_path_buf();
    for part in parts {
        p.push(part);
    }
    p
}

/// Runs a provider's steps against canned answers, the way the worker does:
/// at most [`MAX_FETCHES`] requests, none under [`Reach::Local`]; any
/// [`Step::Read`] finds nothing. Returns the requests made and the outcome.
#[cfg(test)]
pub fn drive(p: &dyn Provider, cx: &Ctx, answer: impl FnMut(&Request) -> Result<Reply, NetFail>) -> (Vec<Request>, Outcome) {
    drive_reads(p, cx, |_| Got::Missing, answer)
}

/// [`drive`], with `read` answering each [`Step::Read`] want.
#[cfg(test)]
pub fn drive_reads(p: &dyn Provider, cx: &Ctx, mut read: impl FnMut(&Want) -> Got, mut answer: impl FnMut(&Request) -> Result<Reply, NetFail>) -> (Vec<Request>, Outcome) {
    let (mut asked, mut replies, mut got) = (Vec::new(), Vec::new(), cx.got.to_vec());
    loop {
        let now = Ctx { dirs: cx.dirs, got: &got, now: cx.now, reach: cx.reach };
        match p.next(&now, &replies) {
            Step::Done(out) => return (asked, out),
            Step::Read(wants) => {
                assert!(got.len() + wants.len() <= MAX_READS, "{:?} read more than {MAX_READS} wants", p.id());
                got.extend(wants.iter().map(&mut read));
            }
            Step::Fetch(req) => {
                assert!(cx.reach == Reach::Online, "{:?} fetched under Reach::Local: {}", p.id(), req.url);
                assert!(asked.len() < MAX_FETCHES, "{:?} asked more than {MAX_FETCHES} times", p.id());
                replies.push(answer(&req));
                asked.push(req);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_key_services_want_their_key() {
        for p in all() {
            if let Some(ask) = p.takes_key() {
                for os in [Os::Windows, Os::Mac] {
                    assert!(p.wants(&Dirs::sample(os)).contains(&Want::Secret(ask.secret)), "{:?} takes a key but doesn't want Secret({})", p.id(), ask.secret);
                }
            }
        }
    }

    #[test]
    fn nothing_found_fetches_nothing() {
        // With nothing on the computer and no keys, no service may ask a
        // server (they'd be asking with nothing), and none may panic.
        for p in all() {
            for os in [Os::Windows, Os::Mac] {
                let dirs = Dirs::sample(os);
                let got = vec![Got::Missing; p.wants(&dirs).len()];
                let cx = Ctx { dirs: &dirs, got: &got, now: 1_793_077_200, reach: Reach::Online };
                let (asked, out) = drive(p, &cx, |_| Err(NetFail::Timeout));
                assert!(asked.is_empty(), "{:?} asked {:?} with nothing found", p.id(), asked.first().map(|r| &r.url));
                assert!(out.is_err(), "{:?} made a snapshot from nothing", p.id());
            }
        }
    }

    #[test]
    fn dirs_join_with_the_os_separator_and_debug_hides_values() {
        let mut w = Dirs::sample(Os::Windows);
        assert_eq!(w.home_join(&[".codex", "auth.json"]), PathBuf::from(r"C:\Users\u\.codex\auth.json"));
        assert_eq!(w.join(&w.config, &["Cursor/User", "x.db"]), PathBuf::from(r"C:\Users\u\AppData\Roaming\Cursor\User\x.db"));
        assert_eq!(Dirs::sample(Os::Mac).home_join(&[".claude/.credentials.json"]), PathBuf::from("/Users/u/.claude/.credentials.json"));
        w.env.push(("OPENROUTER_API_KEY".into(), "sk-or-v1-secret".into()));
        let shown = format!("{w:?}");
        assert!(shown.contains("OPENROUTER_API_KEY") && !shown.contains("secret"), "{shown}");
    }

    #[test]
    fn debug_never_shows_secrets() {
        let req = Request::get("https://x.test/v1/usage?key=sk-123").bearer("tok-456").header("x-api-key", "k-789");
        let got = [Got::Text("sk-or-v1-abc".into()), Got::One(FileRead { path: PathBuf::from("auth.json"), bytes: b"{\"access_token\":\"tok-456\"}".to_vec(), modified: None })];
        let shown = format!("{req:?} {got:?}");
        for secret in ["sk-123", "tok-456", "k-789", "sk-or-v1-abc"] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        assert!(shown.contains("https://x.test/v1/usage") && shown.contains("Authorization"), "{shown}");
    }

    #[test]
    fn text_drops_a_byte_order_mark() {
        let g = Got::One(FileRead { path: PathBuf::from("a"), bytes: b"\xef\xbb\xbf{\"a\":1}".to_vec(), modified: None });
        assert_eq!(g.json().unwrap()["a"], 1);
        assert_eq!(Got::Text("  ".into()).text(), None);
    }

    #[test]
    fn ids_round_trip_and_cover_every_service() {
        for (p, id) in all().iter().zip(ProviderId::ALL) {
            assert_eq!(p.id(), id, "all() is out of order at {id:?}");
            assert_eq!(ProviderId::from_key(id.key()), Some(id));
            assert_eq!(provider(id).id(), id);
        }
    }

    #[test]
    fn short_lists_sort_and_keep_ties_in_order() {
        let mut v = vec![(3, 'a'), (1, 'b'), (2, 'c'), (1, 'd'), (3, 'e'), (0, 'f')];
        sort_small_by(&mut v, |a, b| a.0 > b.0);
        assert_eq!(v, [(0, 'f'), (1, 'b'), (1, 'd'), (2, 'c'), (3, 'a'), (3, 'e')]);
        let mut none: [u8; 0] = [];
        sort_small_by(&mut none, |a, b| a > b);
    }

    #[test]
    fn times_parse() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-10-27T05:00:00Z"), Some(1_793_077_200));
        assert_eq!(parse_time("2026-10-27T12:00:00.5+07:00"), Some(1_793_077_200));
        assert_eq!(parse_time("2026-10-27 05:00:00"), Some(1_793_077_200));
        assert_eq!(parse_time("27 Oct 2026"), None);
        assert_eq!(parse_time("2026-10-27T-1:00:00Z"), None);
        assert_eq!(time(&serde_json::json!(1_793_077_200_000u64)), Some(1_793_077_200));
        assert_eq!(time(&serde_json::json!("1793077200")), Some(1_793_077_200));
    }

    #[test]
    fn jwt_claims_read() {
        // {"alg":"none"} . {"plan":"pro","exp":2000000000}
        let t = "eyJhbGciOiJub25lIn0.eyJwbGFuIjoicHJvIiwiZXhwIjoyMDAwMDAwMDAwfQ.sig";
        let c = jwt_claims(t).unwrap();
        assert_eq!(c["plan"], "pro");
        assert_eq!(c["exp"], 2_000_000_000u64);
        assert!(jwt_claims("nope").is_none());
    }

    #[test]
    fn money_and_masks() {
        assert_eq!(Money::usd(4.2).fmt(), "$4.20");
        assert_eq!(Money::usd(0.0031).fmt(), "$0.003");
        assert_eq!(Money::usd(-1.5).fmt(), "-$1.50");
        assert_eq!(Money::usd(0.0002).fmt(), "<$0.001");
        assert_eq!(Money::usd(0.0).fmt(), "$0.00");
        assert_eq!(micros(f64::NAN), 0);
        assert_eq!(mask_email("someone@example.com"), "s•••@example.com");
        assert_eq!(mask_email("org-123"), "o•••");
    }

    #[test]
    fn a_passed_reset_empties_the_window() {
        let w = Window::new(WindowKind::Session, "5-hour", 0.97).resets(Some(1000)).length(Some(18_000)).counted(97, 100);
        assert_eq!(w.at(999).used, 0.97);
        let after = w.at(1000);
        assert_eq!(after.used, 0.0);
        assert_eq!(after.count, Some((0, 100)));
        assert_eq!(after.resets_at, Some(19_000));
        // Two days later: when it resets next isn't known (never a reset
        // long past, never a made-up one).
        assert_eq!(w.at(1000 + 2 * 86_400 + 5).resets_at, None);
    }

    #[test]
    fn failed_requests_read_as_trouble() {
        let r = |status| Ok(Reply { status, body: "{}".into(), retry_after_s: Some(9) });
        assert_eq!(Trouble::from_net(&r(200)), None);
        assert_eq!(Trouble::from_net(&r(401)), Some(Trouble::Refused));
        assert_eq!(Trouble::from_net(&r(429)), Some(Trouble::Busy { retry_after_s: Some(9) }));
        assert_eq!(Trouble::from_net(&Err(NetFail::Timeout)), Some(Trouble::Offline));
        let leaky = Trouble::from_net(&Err(NetFail::Other("GET https://x.test/?key=sk-123 failed".into())));
        assert!(!format!("{leaky:?}").contains("sk-123"));
        assert_eq!(Request::get("u").bearer("tok\n").headers[0].1, "Bearer tok");
        assert!(reply_json(&r(200)).is_ok());
    }
}
