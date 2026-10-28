//! When the AI tab's worker reads, and the rules it keeps while it does
//! (`services/usage.rs` in the apps runs the thread, keeps the typed keys and
//! makes the requests). All of it is plain Rust, so it is tested on any OS.
//!
//! - A look at every shown service once a minute while the tab is open, and
//!   every 5 minutes for the person who switched on the heads-up notices;
//!   one as the tab opens or someone comes back, at most once in 15 seconds
//!   ([`Looks`]).
//! - A look reads each service's files, and asks its server only once the
//!   service's own gap has passed since its last ask (10 minutes at least
//!   for the notices alone) and no failure's wait is running: "too many
//!   asks" waits as long as the server says (else 5 minutes), no answer 1,
//!   2, 5, then 10 minutes. A service with only online numbers skips the
//!   looks that may not ask ([`Pace`]).
//! - The worker keeps the contract's rules even when a service doesn't: no
//!   request on a look that reads files only, at most [`MAX_FETCHES`]
//!   requests and [`MAX_READS`] reads, and no request with a line break in a
//!   header ([`run`]).
//! - Files are read the way the services ask ([`read_file`], [`read_range`],
//!   [`read_newest`]): never past a size, never a whole folder tree.
//! - A typed key a server refused isn't sent there again this run
//!   ([`Refusals`]): a key every region refused goes nowhere.

use super::{all, sort_small_by, Ctx, Dirs, FileRead, Got, NetFail, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Step, Trouble, Want, MAX_FETCHES, MAX_READS};
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// While the tab is open, a look every minute: files are cheap, and each
/// server is asked only at its own pace ([`Provider::min_gap_s`]).
pub const OPEN_EVERY_S: i64 = 60;
/// The notices alone: a look every 5 minutes, which reads the files...
pub const NOTICES_EVERY_S: i64 = 300;
/// ...and asks a server at most every 10 minutes.
pub const NOTICES_GAP_S: u64 = 600;
/// The tab opening again (or someone coming back) this soon after the last
/// look reads nothing new: flicking between tabs costs nothing.
pub const REOPEN_GAP_S: i64 = 15;
/// Refresh, a new key or a new connection asks a server again after this.
pub const ASKED_GAP_S: u64 = 15;
/// "Too many asks" without a Retry-After waits this long...
pub const BUSY_WAIT_S: u64 = 300;
/// ...and a Retry-After past an hour is cut to one: a card shouldn't stay
/// stale for days on one answer.
pub const BUSY_MAX_S: u64 = 3600;
/// No answer: the next ask after 1, 2, 5, then every 10 minutes.
pub const OFFLINE_WAITS_S: [u64; 4] = [60, 120, 300, 600];
/// A [`Want::Newest`] looks at this many folder entries at most.
pub const SCAN_MAX: usize = 2000;
/// The most one want reads, whatever it asks for (a file, or a
/// [`Want::Newest`]'s tails together).
pub const READ_MAX: u64 = 16 * 1024 * 1024;
/// A refusal's answer kept for [`Refusals`] at most this long (they are a
/// few dozen bytes); a longer one is kept without its body.
const REFUSAL_MAX: usize = 64 * 1024;

/// What a look is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// The AI tab is on screen.
    Open,
    /// Only the heads-up notices want it (the tab is closed).
    Notices,
    /// The person pressed Refresh, typed a key or connected.
    Asked,
}

/// When the worker looks, in seconds on any steady clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Looks {
    /// When the last look began.
    last: Option<i64>,
    /// An earlier look someone asked for.
    soon: Option<i64>,
    /// The next look is the person's (Refresh).
    asked: bool,
    was_open: bool,
    was_notices: bool,
}

impl Looks {
    /// What the island wants now: the tab opening or someone coming back
    /// (the notices coming on) brings a look, at most once in
    /// [`REOPEN_GAP_S`]; nobody there drops a look still to come.
    pub fn saw(&mut self, open: bool, notices: bool, now: i64) {
        if (open && !self.was_open) || (notices && !self.was_notices) {
            self.at(self.last.map_or(now, |t| (t + REOPEN_GAP_S).max(now)));
        }
        if !open && !notices {
            self.soon = None;
            self.asked = false;
        }
        self.was_open = open;
        self.was_notices = notices;
    }

    /// Refresh: a look now, the person's.
    pub fn ask(&mut self, now: i64) {
        self.at(now);
        self.asked = true;
    }

    /// A look now, not the person's (a service just came on the tab).
    pub fn now(&mut self, now: i64) {
        self.at(now);
    }

    fn at(&mut self, t: i64) {
        self.soon = Some(self.soon.map_or(t, |s| s.min(t)));
    }

    /// When the next look is due (at or before now: at once), or None:
    /// nobody wants one.
    pub fn due(&self, open: bool, notices: bool) -> Option<i64> {
        let every = if open {
            OPEN_EVERY_S
        } else if notices {
            NOTICES_EVERY_S
        } else {
            return None;
        };
        let regular = self.last.map_or(i64::MIN, |t| t + every);
        Some(self.soon.map_or(regular, |s| s.min(regular)))
    }

    /// A look begins: what it is for.
    pub fn begin(&mut self, open: bool, now: i64) -> Look {
        self.last = Some(now);
        self.soon = None;
        if self.asked {
            Look::Asked
        } else if open {
            Look::Open
        } else {
            Look::Notices
        }
    }

    /// The look stopped early for something the tab asked: it carries on
    /// right after, still the person's if it was.
    pub fn cut(&mut self, now: i64) {
        self.at(now);
    }

    /// The look went through every service.
    pub fn done(&mut self) {
        self.asked = false;
    }
}

/// One service's pace: when its server may be asked again. Seconds on any
/// steady clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pace {
    /// The last ask of its server.
    asked_at: Option<i64>,
    /// No ask before this (a failure's wait).
    wait_until: Option<i64>,
    /// The wait is the server's own ("too many asks"): Refresh keeps it.
    busy: bool,
    /// Asks without an answer in a row.
    offline: u32,
    /// Its last look at this computer's files said only its server has
    /// numbers ([`Trouble::NeedsOnline`]).
    online_only: bool,
}

impl Pace {
    /// What this look may do for the service: ask its server, read this
    /// computer only, or nothing (None: it has only online numbers, and may
    /// not ask now).
    pub fn reach(&self, now: i64, min_gap_s: u64, look: Look) -> Option<Reach> {
        let gap = match look {
            Look::Open => min_gap_s,
            Look::Notices => min_gap_s.max(NOTICES_GAP_S),
            Look::Asked => ASKED_GAP_S,
        } as i64;
        // A clock set back counts as time passed, so nothing waits forever.
        let waited = match self.wait_until {
            Some(t) => now >= t || t - now > BUSY_MAX_S as i64 || (look == Look::Asked && !self.busy),
            None => true,
        };
        let gapped = self.asked_at.is_none_or(|t| now - t >= gap || now < t);
        if waited && gapped {
            Some(Reach::Online)
        } else if self.online_only {
            None
        } else {
            Some(Reach::Local)
        }
    }

    /// What a refresh did: whether it asked the server, and how it ended
    /// (None: the worker stopped it for breaking the rules).
    pub fn heard(&mut self, now: i64, reach: Reach, asked: bool, outcome: Option<&Outcome>) {
        if reach == Reach::Local {
            if let Some(o) = outcome {
                self.online_only = matches!(o, Err(Trouble::NeedsOnline));
            }
        }
        if !asked {
            return;
        }
        self.asked_at = Some(now);
        match outcome {
            Some(Err(Trouble::Busy { retry_after_s })) => {
                self.busy = true;
                self.offline = 0;
                self.wait_until = Some(now + retry_after_s.unwrap_or(BUSY_WAIT_S).min(BUSY_MAX_S) as i64);
            }
            Some(Err(Trouble::Offline)) => {
                self.busy = false;
                self.offline = self.offline.saturating_add(1);
                let i = (self.offline as usize - 1).min(OFFLINE_WAITS_S.len() - 1);
                self.wait_until = Some(now + OFFLINE_WAITS_S[i] as i64);
            }
            // An answer: whatever it said, the server is there.
            _ => {
                self.busy = false;
                self.offline = 0;
                self.wait_until = None;
            }
        }
    }
}

/// What one refresh of one service came to ([`run`]).
#[derive(Debug)]
pub struct Run {
    /// None when the service broke the rules and was stopped (or the worker
    /// set its answer aside): the tab keeps what it had.
    pub outcome: Option<Outcome>,
    /// Requests sent.
    pub asked: usize,
    /// The rule it broke, for the log.
    pub broke: Option<&'static str>,
    /// A request wasn't sent (a line break in a header, or not http(s)): the
    /// service heard "the request failed".
    pub unsent: bool,
}

/// One refresh of one service, as the worker runs it: its `wants` read with
/// `read`, then [`Provider::next`] until it is done, each [`Step::Read`]
/// read with `read` too and each request answered by `fetch`. It stops a
/// service that asks on a [`Reach::Local`] look, more than [`MAX_FETCHES`]
/// times or reads more than [`MAX_READS`] wants, and never sends a request
/// that isn't [`sendable`].
pub fn run(
    p: &dyn Provider,
    dirs: &Dirs,
    wants: &[Want],
    now: i64,
    reach: Reach,
    mut read: impl FnMut(&Want) -> Got,
    mut fetch: impl FnMut(&Request) -> Result<Reply, NetFail>,
) -> Run {
    let broke = |asked, unsent, rule| Run { outcome: None, asked, broke: Some(rule), unsent };
    if wants.len() > MAX_READS {
        return broke(0, false, "asked to read more than 64 things in one refresh");
    }
    let mut got: Vec<Got> = wants.iter().map(&mut read).collect();
    let mut replies = Vec::new();
    let (mut asked, mut unsent) = (0, false);
    loop {
        let cx = Ctx { dirs, got: &got, now, reach };
        let req = match p.next(&cx, &replies) {
            Step::Done(o) => return Run { outcome: Some(o), asked, broke: None, unsent },
            Step::Read(more) if got.len() + more.len() > MAX_READS => return broke(asked, unsent, "asked to read more than 64 things in one refresh"),
            Step::Read(more) => {
                got.extend(more.iter().map(&mut read));
                continue;
            }
            Step::Fetch(_) if reach == Reach::Local => return broke(asked, unsent, "asked its server on a look at this computer only"),
            Step::Fetch(_) if replies.len() >= MAX_FETCHES => return broke(asked, unsent, "asked its server more than 4 times in one refresh"),
            Step::Fetch(req) => req,
        };
        if sendable(&req) {
            asked += 1;
            replies.push(fetch(&req));
        } else {
            unsent = true;
            replies.push(Err(NetFail::Other("not sent".into())));
        }
    }
}

/// A request the worker sends: to an https address (plain http only to
/// this computer, a test copy of a server: a token never travels in the
/// clear) without user info in it, and with no control character anywhere.
/// A key pasted with a line break would otherwise end its header early (or
/// start another), and `https://localhost:1@example.com/` goes to
/// example.com.
pub fn sendable(req: &Request) -> bool {
    let clean = |s: &str| !s.chars().any(char::is_control);
    let to = match (req.url.strip_prefix("https://"), req.url.strip_prefix("http://")) {
        (Some(rest), _) => !authority(rest).is_empty() && !authority(rest).contains(['@', '\\']),
        (None, Some(rest)) => !authority(rest).contains(['@', '\\']) && loopback(rest),
        _ => false,
    };
    to && clean(&req.url) && !req.url.contains(' ') && req.headers.iter().all(|(k, v)| !k.is_empty() && k.bytes().all(|b| b.is_ascii_graphic() && b != b':') && clean(v))
}

/// An address's authority: what follows `scheme://` up to its path, query
/// or fragment.
fn authority(rest: &str) -> &str {
    rest.split(['/', '?', '#']).next().unwrap_or("")
}

/// An address on this computer: `localhost`, 127.x.x.x or `[::1]`, with or
/// without a port.
fn loopback(rest: &str) -> bool {
    let authority = authority(rest);
    match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().and_then(|h| h.parse::<std::net::Ipv6Addr>().ok()).is_some_and(|ip| ip.is_loopback()),
        None => {
            let host = authority.split(':').next().unwrap_or("");
            host == "localhost" || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
        }
    }
}

/// The headers to hand the app's HTTP client: a JSON body's `Content-Type`
/// is the client's to set (the Windows one refuses it as a request header).
pub fn send_headers(req: &Request) -> Vec<(&str, &str)> {
    req.headers.iter().filter(|(k, _)| !(req.json.is_some() && k.eq_ignore_ascii_case("content-type"))).map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

/// What servers said to the typed keys they refused, kept in memory for the
/// run (security review S7): the next refresh doesn't send a key to an
/// address that refused it but hands the service the same answer, so its
/// own fallback carries on (Kimi's, Z.ai's and MiniMax's other region), and
/// a key every address refused goes nowhere. Forgotten when the key changes,
/// at Refresh and when the app quits. Keeps a fingerprint of each key, never
/// the key.
#[derive(Default)]
pub struct Refusals {
    keys: HashMap<&'static str, Refused>,
}

/// One key name's refusals: which key it was, and each refusing address's
/// answer.
struct Refused {
    print: u64,
    said: Vec<(String, Reply)>,
}

impl Refusals {
    /// The key under `name` changed or went: what was refused goes too.
    pub fn forget(&mut self, name: &str) {
        self.keys.remove(name);
    }

    /// Refresh: every key is tried again.
    pub fn clear(&mut self) {
        self.keys.clear();
    }

    fn answer(&self, name: &str, key: &str, url: &str) -> Option<Reply> {
        let r = self.keys.get(name).filter(|r| r.print == fingerprint(key))?;
        r.said.iter().find(|(u, _)| u == url).map(|(_, reply)| reply.clone())
    }

    fn keep(&mut self, name: &'static str, key: &str, url: &str, reply: &Reply) {
        let print = fingerprint(key);
        let r = self.keys.entry(name).or_insert(Refused { print, said: Vec::new() });
        if r.print != print {
            *r = Refused { print, said: Vec::new() };
        }
        let body = if reply.body.len() <= REFUSAL_MAX { reply.body.clone() } else { String::new() };
        r.said.retain(|(u, _)| u != url);
        r.said.push((url.to_string(), Reply { status: reply.status, body, retry_after_s: None }));
    }
}

/// A key's fingerprint, to tell whether it changed without keeping it.
fn fingerprint(key: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.trim().hash(&mut h);
    h.finish()
}

/// Whether a request carries this key in a header, as the services send it
/// (trimmed, without quotes around it).
pub fn carries(req: &Request, key: &str) -> bool {
    let k = key.trim().trim_matches(['"', '\'']).trim();
    k.len() >= 8 && req.headers.iter().any(|(_, v)| v.contains(k))
}

/// One refresh's typed key at the door ([`Refusals`]): the worker shows it
/// each read ([`KeyGate::saw`]), sends through it ([`KeyGate::ask`]) and
/// tells it how the refresh ended ([`KeyGate::done`]). A request with the
/// key to an address that refused it gets that answer back unsent. An
/// answer is kept as a refusal when it says 401 or 403, or answered at all
/// (2xx, a refusal in its body: Z.ai, MiniMax) in a refresh that ended
/// [`Trouble::Refused`]; a server error or no answer never is.
pub struct KeyGate<'a> {
    refusals: &'a mut Refusals,
    /// The service's typed key's name ([`Provider::takes_key`]) and the key,
    /// once read.
    name: Option<&'static str>,
    key: Option<String>,
    /// The requests that carried it: where, and what came back.
    sent: Vec<(String, Result<Reply, NetFail>)>,
}

impl<'a> KeyGate<'a> {
    pub fn new(refusals: &'a mut Refusals, name: Option<&'static str>) -> KeyGate<'a> {
        KeyGate { refusals, name, key: None, sent: Vec::new() }
    }

    /// What a want gave: the typed key, when it is this service's.
    pub fn saw(&mut self, want: &Want, got: &Got) {
        if matches!(want, Want::Secret(s) if Some(*s) == self.name) {
            self.key = got.text().map(str::to_string);
        }
    }

    /// A request through the door: `send` makes it, unless it carries the
    /// key to an address that refused it already.
    pub fn ask(&mut self, req: &Request, send: impl FnOnce(&Request) -> Result<Reply, NetFail>) -> Result<Reply, NetFail> {
        let (Some(name), Some(key)) = (self.name, self.key.as_deref()) else { return send(req) };
        if !carries(req, key) {
            return send(req);
        }
        if let Some(said) = self.refusals.answer(name, key, &req.url) {
            return Ok(said);
        }
        let answer = send(req);
        self.sent.push((req.url.clone(), answer.clone()));
        answer
    }

    /// The refresh ended so (None: stopped for breaking the rules).
    pub fn done(self, outcome: Option<&Outcome>) {
        let (Some(name), Some(key), Some(outcome)) = (self.name, self.key, outcome) else { return };
        let refused = matches!(outcome, Err(Trouble::Refused));
        for (url, answer) in &self.sent {
            match answer {
                Ok(r) if matches!(r.status, 401 | 403) || (refused && (200..300).contains(&r.status)) => self.refusals.keep(name, &key, url, r),
                _ => {}
            }
        }
    }
}

/// Whether a good reading takes the place of the one the tab has: always,
/// except one from this computer's files older than the server's last
/// answer (a log line written before it).
pub fn replaces(kept: Option<&Snapshot>, new: &Snapshot) -> bool {
    match kept {
        Some(k) if k.source == Source::Online && new.source == Source::Local => new.as_of > k.as_of,
        _ => true,
    }
}

/// A service whose only way in is a key typed into Oyenza Studio: every
/// one of its wants is a [`Want::Secret`], and none of its variables is set
/// (`var_set`: OpenRouter also takes `OPENROUTER_API_KEY`). The worker reads
/// such a service only while one of its keys is kept or the person added it.
pub fn typed_key_only(wants: &[Want], var_set: bool) -> bool {
    !wants.is_empty() && !var_set && wants.iter().all(|w| matches!(w, Want::Secret(_)))
}

/// A service's bit in the app's sets of services (`settings.usage.hidden`
/// and `added` on the worker's watch).
pub fn bit(id: ProviderId) -> u32 {
    1 << ProviderId::ALL.iter().position(|p| *p == id).unwrap_or(31)
}

/// Service keys (`ProviderId::key`) as bits; unknown keys count for nothing.
pub fn bits(keys: &[String]) -> u32 {
    keys.iter().filter_map(|k| ProviderId::from_key(k)).fold(0, |m, id| m | bit(id))
}

/// The notices' line ([`super::alerts::AlertCfg::near`]) from
/// `settings.usage.near_pct`, kept to 50..=95 % as the app's own notice
/// settings keep it (a hand-edited 0 or 200 can't make every look speak).
pub fn near(pct: u8) -> f32 {
    pct.clamp(50, 95) as f32 / 100.0
}

/// Every variable some service reads ([`Provider::env`]), once each: what
/// the app puts in [`Dirs::env`].
pub fn env_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = all().iter().flat_map(|p| p.env().iter().copied()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Every name a key is kept under: the typed keys ([`Provider::takes_key`])
/// and any other [`Want::Secret`] (inference.oyenza.com's).
pub fn secret_names(dirs: &Dirs) -> Vec<&'static str> {
    let mut v = Vec::new();
    for p in all() {
        v.extend(p.wants(dirs).into_iter().filter_map(|w| match w {
            Want::Secret(s) => Some(s),
            _ => None,
        }));
        v.extend(p.takes_key().map(|k| k.secret));
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// A name that is safe in a file name (`usage-<name>.bin`): letters,
/// digits, `-` and `_`, at most 40.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 40 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// ---------------------------------------------------------------- this computer

/// [`Want::File`]: the whole file, or [`Got::Missing`] when it isn't there,
/// isn't a plain file, is bigger than `max` (or [`READ_MAX`]), or the path
/// isn't a full one.
pub fn read_file(path: &Path, max: u64) -> Got {
    let max = max.min(READ_MAX);
    // Looked at before it is opened: opening a pipe would wait for a writer.
    if !path.is_absolute() || !fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Got::Missing;
    }
    let Ok(f) = File::open(path) else { return Got::Missing };
    let Ok(meta) = f.metadata() else { return Got::Missing };
    if !meta.is_file() || meta.len() > max {
        return Got::Missing;
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    // It may grow between the size and the read: still never past max.
    match f.take(max + 1).read_to_end(&mut bytes) {
        Ok(_) if bytes.len() as u64 <= max => Got::One(FileRead { path: path.to_path_buf(), bytes, modified: modified(&meta) }),
        _ => Got::Missing,
    }
}

/// Puts `file` into `found`, which holds the newest `count` so far, newest
/// first (the same second, the later name): no sorting of every file seen.
fn keep_newest(found: &mut Vec<(Option<SystemTime>, PathBuf)>, file: (Option<SystemTime>, PathBuf), count: usize) {
    let at = found.iter().position(|f| file > *f).unwrap_or(found.len());
    if at < count {
        found.insert(at, file);
        found.truncate(count);
    }
}

/// [`Want::Newest`]: the files under `dir` whose names end in `suffix`, down
/// `depth` folders (0: `dir` alone), newest first by last change, at most
/// `count`, each read from its end up to `tail` bytes (0 reads nothing: "is
/// it there"). Folders are walked newest first and the walk stops after
/// [`SCAN_MAX`] entries, so years of logs cost no more than the last weeks
/// of them. A link is followed to a file, never to a folder. Missing when
/// `dir` isn't a folder.
pub fn read_newest(dir: &Path, suffix: &str, depth: u8, count: usize, tail: u64) -> Got {
    if !dir.is_absolute() || !dir.is_dir() {
        return Got::Missing;
    }
    // By last change to the nanosecond where the system keeps it: folders
    // made the same second still walk newest first.
    let mut found: Vec<(Option<SystemTime>, PathBuf)> = Vec::new();
    let mut scanned = 0;
    // Folders still to walk, with how many levels each may go down; the
    // newest is on top.
    let mut todo = vec![(dir.to_path_buf(), depth)];
    'walk: while let Some((at, left)) = todo.pop() {
        let Ok(entries) = fs::read_dir(&at) else { continue };
        let mut subs = Vec::new();
        for e in entries.flatten() {
            scanned += 1;
            if scanned > SCAN_MAX {
                break 'walk;
            }
            let Ok(kind) = e.file_type() else { continue };
            let meta = if kind.is_symlink() { fs::metadata(e.path()).ok().filter(|m| m.is_file()) } else { e.metadata().ok() };
            let Some(meta) = meta else { continue };
            let t = meta.modified().ok();
            if meta.is_dir() {
                if left > 0 {
                    subs.push((t, e.path()));
                }
            } else if meta.is_file() && e.file_name().to_string_lossy().ends_with(suffix) {
                keep_newest(&mut found, (t, e.path()), count);
            }
        }
        // Oldest first, so the newest comes off the top next.
        sort_small_by(&mut subs, |a, b| a > b);
        todo.extend(subs.into_iter().map(|(_, p)| (p, left - 1)));
    }
    let mut budget = READ_MAX;
    let mut files = Vec::new();
    for (t, path) in found.into_iter().take(count) {
        if tail == 0 {
            files.push(FileRead { path, bytes: Vec::new(), modified: t.and_then(unix) });
        } else if let Some(f) = read_tail(&path, tail.min(budget)) {
            budget = budget.saturating_sub(f.bytes.len() as u64);
            files.push(f);
            if budget == 0 {
                break;
            }
        }
    }
    Got::Many(files)
}

/// [`Want::Range`]: `len` bytes of a file from `offset` (fewer at its end, at
/// most 4 MiB), a piece of a file too big to read whole (Cursor's SQLite
/// store). Opened sharing read, write and delete (Rust's default on
/// Windows), so the file's owner carries on writing it; never kept between
/// refreshes. Missing when it isn't a plain file or can't be read.
pub fn read_range(path: &Path, offset: u64, len: u32) -> Got {
    if !path.is_absolute() || !fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Got::Missing;
    }
    let read = || -> Option<FileRead> {
        let mut f = File::open(path).ok()?;
        let meta = f.metadata().ok()?;
        f.seek(SeekFrom::Start(offset)).ok()?;
        let len = u64::from(len).min(4 * 1024 * 1024);
        let mut bytes = Vec::with_capacity(len.min(meta.len().saturating_sub(offset)) as usize);
        f.take(len).read_to_end(&mut bytes).ok()?;
        Some(FileRead { path: path.to_path_buf(), bytes, modified: modified(&meta) })
    };
    read().map_or(Got::Missing, Got::One)
}

/// The last `tail` bytes of a file (all of a shorter one).
fn read_tail(path: &Path, tail: u64) -> Option<FileRead> {
    let mut f = File::open(path).ok()?;
    let meta = f.metadata().ok()?;
    let from = meta.len().saturating_sub(tail);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::with_capacity((meta.len() - from) as usize);
    f.take(tail).read_to_end(&mut bytes).ok()?;
    Some(FileRead { path: path.to_path_buf(), bytes, modified: modified(&meta) })
}

/// Last change, unix seconds.
fn modified(m: &fs::Metadata) -> Option<i64> {
    unix(m.modified().ok()?)
}

fn unix(t: SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{openrouter, opencode, oyenza, zai, Os, Window, WindowKind};
    use std::time::Duration;

    const T: i64 = 1_000;

    fn snap(source: Source, as_of: i64) -> Snapshot {
        Snapshot::new(ProviderId::Codex, as_of, source)
    }

    #[test]
    fn looks_rest_until_someone_wants_them() {
        let mut l = Looks::default();
        assert_eq!(l.due(false, false), None);
        l.saw(false, false, T);
        assert_eq!(l.due(false, false), None);
        // The tab opens: a look at once.
        l.saw(true, false, T);
        assert!(l.due(true, false).unwrap() <= T);
        assert_eq!(l.begin(true, T), Look::Open);
        // Then every minute while it stays open, every 5 for the notices.
        assert_eq!(l.due(true, false), Some(T + 60));
        assert_eq!(l.due(false, true), Some(T + 300));
        assert_eq!(l.due(true, true), Some(T + 60));
        // Nobody there: nothing.
        l.saw(false, false, T + 10);
        assert_eq!(l.due(false, false), None);
    }

    #[test]
    fn opening_the_tab_again_soon_reads_nothing_new() {
        let mut l = Looks::default();
        l.saw(true, false, T);
        l.begin(true, T);
        l.saw(false, false, T + 3);
        // Back 5 s later: the look waits for 15 s after the last one.
        l.saw(true, false, T + 5);
        assert_eq!(l.due(true, false), Some(T + 15));
        l.begin(true, T + 15);
        l.saw(false, false, T + 20);
        // Back after a while: at once.
        l.saw(true, false, T + 200);
        assert!(l.due(true, false).unwrap() <= T + 200);
        // Coming back with only the notices on looks too (a reset may have
        // passed while the screen was locked).
        let mut n = Looks::default();
        n.saw(false, true, T);
        n.begin(false, T);
        assert_eq!(n.due(false, true), Some(T + 300));
        n.saw(false, false, T + 100);
        n.saw(false, true, T + 1000);
        assert!(n.due(false, true).unwrap() <= T + 1000);
        assert_eq!(n.begin(false, T + 1000), Look::Notices);
    }

    #[test]
    fn refresh_is_the_persons_look_until_it_goes_through() {
        let mut l = Looks::default();
        l.saw(true, false, T);
        l.begin(true, T);
        l.done();
        l.ask(T + 5);
        assert_eq!(l.due(true, false), Some(T + 5));
        assert_eq!(l.begin(true, T + 5), Look::Asked);
        // Cut short by a command: it carries on at once, still the person's.
        l.cut(T + 6);
        assert_eq!(l.due(true, false), Some(T + 6));
        assert_eq!(l.begin(true, T + 6), Look::Asked);
        l.done();
        assert_eq!(l.due(true, false), Some(T + 66));
        // A look asked for while nobody is there is dropped.
        l.ask(T + 70);
        l.saw(false, false, T + 71);
        l.saw(true, false, T + 72);
        assert_eq!(l.begin(true, T + 72), Look::Open);
        // A service put on the tab is read at once, but it isn't a Refresh.
        l.now(T + 80);
        assert_eq!(l.due(true, false), Some(T + 80));
        assert_eq!(l.begin(true, T + 80), Look::Open);
    }

    #[test]
    fn a_server_is_asked_at_its_own_pace() {
        let mut p = Pace::default();
        // Never asked: ask, whatever the look.
        for look in [Look::Open, Look::Notices, Look::Asked] {
            assert_eq!(p.reach(T, 120, look), Some(Reach::Online));
        }
        p.heard(T, Reach::Online, true, Some(&Ok(snap(Source::Online, T))));
        assert_eq!(p.reach(T + 60, 120, Look::Open), Some(Reach::Local));
        assert_eq!(p.reach(T + 120, 120, Look::Open), Some(Reach::Online));
        // The notices alone ask every 10 minutes at most, slower services slower.
        assert_eq!(p.reach(T + 300, 120, Look::Notices), Some(Reach::Local));
        assert_eq!(p.reach(T + 600, 120, Look::Notices), Some(Reach::Online));
        assert_eq!(p.reach(T + 600, 900, Look::Notices), Some(Reach::Local));
        // Refresh asks again after 15 s.
        assert_eq!(p.reach(T + 10, 120, Look::Asked), Some(Reach::Local));
        assert_eq!(p.reach(T + 15, 120, Look::Asked), Some(Reach::Online));
        // A refresh that didn't ask (nothing to ask with) changes nothing.
        p.heard(T + 200, Reach::Online, false, Some(&Err(Trouble::NotFound)));
        assert_eq!(p.reach(T + 120, 120, Look::Open), Some(Reach::Online));
        // A clock set back doesn't stop the asking.
        assert_eq!(p.reach(T - 500, 120, Look::Open), Some(Reach::Online));
    }

    #[test]
    fn busy_waits_as_told_and_no_answer_backs_off() {
        let busy = |s| Err(Trouble::Busy { retry_after_s: s });
        let mut p = Pace::default();
        p.heard(T, Reach::Online, true, Some(&busy(Some(900))));
        assert_eq!(p.reach(T + 899, 60, Look::Open), Some(Reach::Local));
        assert_eq!(p.reach(T + 900, 60, Look::Open), Some(Reach::Online));
        // Refresh doesn't skip the server's own wait.
        assert_eq!(p.reach(T + 100, 60, Look::Asked), Some(Reach::Local));
        p.heard(T, Reach::Online, true, Some(&busy(None)));
        assert_eq!(p.reach(T + 299, 60, Look::Open), Some(Reach::Local));
        assert_eq!(p.reach(T + 300, 60, Look::Open), Some(Reach::Online));
        p.heard(T, Reach::Online, true, Some(&busy(Some(u64::MAX))));
        assert_eq!(p.reach(T + 3600, 60, Look::Open), Some(Reach::Online));

        let mut p = Pace::default();
        let mut t = T;
        for wait in [60, 120, 300, 600, 600] {
            p.heard(t, Reach::Online, true, Some(&Err(Trouble::Offline)));
            assert_eq!(p.reach(t + wait - 1, 0, Look::Open), Some(Reach::Local), "{wait}");
            assert_eq!(p.reach(t + wait, 0, Look::Open), Some(Reach::Online), "{wait}");
            t += wait;
        }
        // Refresh asks at once (the network may be back), and any answer
        // ends the waits.
        assert_eq!(p.reach(t + 15, 0, Look::Asked), Some(Reach::Online));
        p.heard(t, Reach::Online, true, Some(&Err(Trouble::Refused)));
        assert_eq!(p.reach(t + 1, 0, Look::Open), Some(Reach::Online));
        p.heard(t, Reach::Online, true, Some(&Err(Trouble::Offline)));
        assert_eq!(p.reach(t + 60, 0, Look::Open), Some(Reach::Online));
    }

    #[test]
    fn a_service_with_only_online_numbers_skips_the_looks_that_cant_ask() {
        let mut p = Pace::default();
        p.heard(T, Reach::Online, true, Some(&Ok(snap(Source::Online, T))));
        assert_eq!(p.reach(T + 60, 120, Look::Open), Some(Reach::Local));
        p.heard(T + 60, Reach::Local, false, Some(&Err(Trouble::NeedsOnline)));
        assert_eq!(p.reach(T + 61, 120, Look::Open), None);
        assert_eq!(p.reach(T + 120, 120, Look::Open), Some(Reach::Online));
        // An online answer doesn't make it local; numbers from its files do.
        p.heard(T + 120, Reach::Online, true, Some(&Ok(snap(Source::Online, T + 120))));
        assert_eq!(p.reach(T + 130, 120, Look::Open), None);
        p.heard(T + 130, Reach::Local, false, Some(&Ok(snap(Source::Local, T + 125))));
        assert_eq!(p.reach(T + 140, 120, Look::Open), Some(Reach::Local));
    }

    /// A service that asks `n` times, each with `header`, on any look.
    struct Asker {
        n: usize,
        header: &'static str,
    }

    impl Provider for Asker {
        fn id(&self) -> ProviderId {
            ProviderId::Zai
        }

        fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
            vec![Want::Secret("asker")]
        }

        fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
            if cx.got(0).text() != Some("key") {
                return Step::Done(Err(Trouble::NeedsKey));
            }
            if replies.len() < self.n {
                Step::Fetch(Request::get("https://x.test/usage").header("Authorization", self.header))
            } else {
                Step::Done(match replies.last() {
                    Some(Err(_)) => Err(Trouble::Unreadable("the request failed".into())),
                    _ => Ok(Snapshot::new(ProviderId::Zai, cx.now, Source::Online)),
                })
            }
        }
    }

    /// A service that reads `n` pieces of a file one step at a time (each
    /// depends on the last), then says how many it got.
    struct Stepper {
        n: usize,
    }

    impl Provider for Stepper {
        fn id(&self) -> ProviderId {
            ProviderId::Cursor
        }

        fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
            vec![Want::Range { path: PathBuf::from("/x/state.vscdb"), offset: 0, len: 100 }]
        }

        fn next(&self, cx: &Ctx, _replies: &[Result<Reply, NetFail>]) -> Step {
            if cx.got.len() < self.n {
                Step::Read(vec![Want::Range { path: PathBuf::from("/x/state.vscdb"), offset: 100 * cx.got.len() as u64, len: 100 }])
            } else {
                let mut s = Snapshot::new(ProviderId::Cursor, cx.now, Source::Local);
                s.tokens_today = Some(cx.got.iter().filter(|g| g.bytes().is_some()).count() as u64);
                Step::Done(Ok(s))
            }
        }
    }

    fn drive(p: &Asker, reach: Reach) -> (Run, Vec<String>) {
        let dirs = Dirs::sample(Os::Windows);
        let mut sent = Vec::new();
        let run = run(p, &dirs, &p.wants(&dirs), T, reach, |_| Got::Text("key".into()), |r| {
            sent.push(r.url.clone());
            Ok(Reply { status: 200, body: "{}".into(), retry_after_s: None })
        });
        (run, sent)
    }

    #[test]
    fn the_worker_keeps_the_rules_for_every_service() {
        let fine = drive(&Asker { n: 2, header: "Bearer k" }, Reach::Online);
        assert!(matches!(fine.0, Run { outcome: Some(Ok(_)), asked: 2, broke: None, unsent: false }), "{:?}", fine.0);
        assert_eq!(fine.1.len(), 2);
        // Four at most: the fifth stops it, and the tab keeps what it had.
        let greedy = drive(&Asker { n: 5, header: "Bearer k" }, Reach::Online);
        assert!(greedy.0.outcome.is_none() && greedy.0.broke.is_some() && greedy.0.asked == MAX_FETCHES);
        assert_eq!(greedy.1.len(), MAX_FETCHES);
        // Nothing on a look at this computer only.
        let sneaky = drive(&Asker { n: 1, header: "Bearer k" }, Reach::Local);
        assert!(sneaky.0.outcome.is_none() && sneaky.0.broke.is_some() && sneaky.0.asked == 0);
        assert!(sneaky.1.is_empty());
        // A key pasted with a line break never leaves; the service hears a failure.
        let broken = drive(&Asker { n: 1, header: "Bearer k\nX-Other: 1" }, Reach::Online);
        assert!(broken.0.unsent && broken.0.asked == 0 && broken.1.is_empty());
        assert_eq!(broken.0.outcome, Some(Err(Trouble::Unreadable("the request failed".into()))));
        // What is read goes to the service in order: no key, no asking.
        let dirs = Dirs::sample(Os::Mac);
        let p = Asker { n: 1, header: "Bearer k" };
        let run = run(&p, &dirs, &p.wants(&dirs), T, Reach::Online, |_| Got::Missing, |_| panic!("asked without a key"));
        assert_eq!(run.outcome, Some(Err(Trouble::NeedsKey)));
    }

    #[test]
    fn pieces_are_read_in_steps_up_to_the_limit() {
        let dirs = Dirs::sample(Os::Mac);
        let piece = |w: &Want| match w {
            Want::Range { path, len, .. } => Got::One(FileRead { path: path.clone(), bytes: vec![0; *len as usize], modified: None }),
            _ => Got::Missing,
        };
        // Reads come before the end, on a local look too, and ask nothing.
        let ten = Stepper { n: 10 };
        let ran = run(&ten, &dirs, &ten.wants(&dirs), T, Reach::Local, piece, |_| panic!("asked"));
        assert_eq!(ran.outcome.map(|o| o.unwrap().tokens_today), Some(Some(10)));
        let all = Stepper { n: MAX_READS };
        assert!(run(&all, &dirs, &all.wants(&dirs), T, Reach::Local, piece, |_| panic!("asked")).outcome.is_some());
        // One more than the limit stops it, and the tab keeps what it had.
        let greedy = Stepper { n: MAX_READS + 1 };
        let ran = run(&greedy, &dirs, &greedy.wants(&dirs), T, Reach::Local, piece, |_| panic!("asked"));
        assert!(ran.outcome.is_none() && ran.broke.is_some());
    }

    #[test]
    fn only_clean_requests_go_out() {
        let ok = Request::post_json("https://x.test/v1/a?b=c", "{}").bearer("tok").header("Cookie", "a=b; c=d");
        assert!(sendable(&ok));
        for local in ["http://127.0.0.1:8090/v1/studio/summary", "http://localhost/x", "http://[::1]:8090/", "http://127.1.2.3"] {
            assert!(sendable(&Request::get(local).bearer("oyis_test-token")), "{local}");
        }
        for bad in [
            Request::get("file:///etc/passwd"),
            Request::get("ftp://x.test/"),
            Request::get("https://"),
            // In the clear, only to this computer.
            Request::get("http://inference.oyenza.com/v1/studio/summary"),
            Request::get("http://127.0.0.1.example.com/"),
            Request::get("http://127.0.0.1@example.com/"),
            Request::get("http://localhost.example.com/"),
            Request::get("http://[::2]/"),
            // User info: these go to example.com.
            Request::get("http://localhost:1@example.com/"),
            Request::get("http://127.0.0.1:80@example.com/"),
            Request::get("https://user@inference.oyenza.com/v1/studio/summary"),
            Request::get("https://inference.oyenza.com\\@example.com/"),
            Request::get("https://x.test/a b"),
            Request::get("https://x.test/\r\nX: 1"),
            Request::get("https://x.test/").header("Authorization", "Bearer a\nb"),
            Request::get("https://x.test/").header("Authorization", "Bearer a\r"),
            Request::get("https://x.test/").header("X-Key", "a\tb"),
            Request::get("https://x.test/").header("Bad Name", "v"),
            Request::get("https://x.test/").header("Bad:Name", "v"),
            Request::get("https://x.test/").header("", "v"),
        ] {
            assert!(!sendable(&bad), "{bad:?}");
        }
        // A JSON body's Content-Type is the client's to set.
        assert_eq!(send_headers(&ok), [("Authorization", "Bearer tok"), ("Cookie", "a=b; c=d")]);
        let get = Request::get("https://x.test/").header("content-type", "text/plain");
        assert_eq!(send_headers(&get), [("content-type", "text/plain")]);
    }

    #[test]
    fn a_redirect_is_an_answer_the_service_doesnt_read() {
        // The worker's clients follow no redirect: the 3xx comes back, and the
        // service reads it as a trouble, without a second request.
        let dirs = Dirs::sample(Os::Windows);
        let p = oyenza::Oyenza;
        let mut asked = Vec::new();
        let ran = run(&p, &dirs, &p.wants(&dirs), T, Reach::Online, |_| Got::Text("oyis_test-token".into()), |r| {
            asked.push(r.url.clone());
            Ok(Reply { status: 302, body: String::new(), retry_after_s: None })
        });
        assert_eq!(ran.outcome, Some(Err(Trouble::Unreadable("HTTP 302".into()))));
        assert_eq!(asked, ["https://inference.oyenza.com/v1/studio/summary"]);
    }

    const QUOTA: &str = include_str!("fixtures/zai/quota.json");
    const KEY: &str = "zai-test-token.0123456789";

    /// One Z.ai refresh with `key`, through the door: each request is
    /// answered by `answer` (by host) and noted in `asked`.
    fn zai(refusals: &mut Refusals, key: &str, asked: &mut Vec<String>, answer: impl Fn(&str) -> Result<Reply, NetFail>) -> Outcome {
        let (dirs, p) = (Dirs::sample(Os::Windows), zai::Zai);
        let gate = std::cell::RefCell::new(KeyGate::new(refusals, p.takes_key().map(|k| k.secret)));
        let ran = run(
            &p,
            &dirs,
            &p.wants(&dirs),
            T,
            Reach::Online,
            |w| {
                let g = Got::Text(key.into());
                gate.borrow_mut().saw(w, &g);
                g
            },
            |req| {
                gate.borrow_mut().ask(req, |r| {
                    asked.push(r.url.split('/').nth(2).unwrap_or("").to_string());
                    answer(r.url.split('/').nth(2).unwrap_or(""))
                })
            },
        );
        gate.into_inner().done(ran.outcome.as_ref());
        ran.outcome.unwrap()
    }

    fn status(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    #[test]
    fn a_key_every_region_refused_goes_nowhere() {
        let mut r = Refusals::default();
        let mut asked = Vec::new();
        // Refused by both of Zhipu's hosts (one by status, one in its body).
        let refuse = |host: &str| if host == "api.z.ai" { status(401, "") } else { status(200, r#"{"code":1001,"msg":"bad key","success":false}"#) };
        assert_eq!(zai(&mut r, KEY, &mut asked, refuse), Err(Trouble::Refused));
        assert_eq!(asked, ["api.z.ai", "open.bigmodel.cn"]);
        // The next refresh sends it nowhere, and says the same.
        asked.clear();
        assert_eq!(zai(&mut r, KEY, &mut asked, |_| panic!("sent a refused key")), Err(Trouble::Refused));
        assert!(asked.is_empty());
        // Another key is asked about again; so is this one after Refresh.
        assert_eq!(zai(&mut r, "zai-another-token.987", &mut asked, refuse), Err(Trouble::Refused));
        assert_eq!(asked.len(), 2);
        r.clear();
        asked.clear();
        assert_eq!(zai(&mut r, KEY, &mut asked, refuse), Err(Trouble::Refused));
        assert_eq!(asked.len(), 2);
        r.forget("zai");
        asked.clear();
        assert_eq!(zai(&mut r, KEY, &mut asked, refuse), Err(Trouble::Refused));
        assert_eq!(asked.len(), 2);
    }

    #[test]
    fn a_key_goes_only_where_it_answered() {
        let mut r = Refusals::default();
        let mut asked = Vec::new();
        // A bigmodel.cn key: api.z.ai refuses it, bigmodel.cn answers.
        let china = |host: &str| if host == "api.z.ai" { status(401, "") } else { status(200, QUOTA) };
        assert!(zai(&mut r, KEY, &mut asked, china).is_ok());
        assert_eq!(asked, ["api.z.ai", "open.bigmodel.cn"]);
        asked.clear();
        assert!(zai(&mut r, KEY, &mut asked, china).is_ok());
        assert_eq!(asked, ["open.bigmodel.cn"]);
        // A server error or no answer is never a refusal: asked again.
        let mut r = Refusals::default();
        let down = |host: &str| if host == "api.z.ai" { status(401, "") } else { status(502, "") };
        assert_eq!(zai(&mut r, KEY, &mut asked, down), Err(Trouble::Refused));
        asked.clear();
        assert_eq!(zai(&mut r, KEY, &mut asked, down), Err(Trouble::Refused));
        assert_eq!(asked, ["open.bigmodel.cn"]);
        // A key from a variable isn't a typed key: never held back.
        let mut r = Refusals::default();
        let mut gate = KeyGate::new(&mut r, Some("zai"));
        gate.saw(&Want::Secret("zai"), &Got::Missing);
        let req = Request::get("https://api.z.ai/x").bearer(KEY);
        assert_eq!(gate.ask(&req, |_| status(401, "")), status(401, ""));
        gate.done(Some(&Err(Trouble::Refused)));
        assert!(r.keys.is_empty());
    }

    #[test]
    fn a_key_is_found_in_its_header_as_sent() {
        let req = Request::get("https://x.test/").bearer("sk-or-v1-abcdef123");
        assert!(carries(&req, "sk-or-v1-abcdef123"));
        assert!(carries(&req, " \"sk-or-v1-abcdef123\" "));
        assert!(!carries(&req, "sk-or-v1-other0000"));
        // Too short to tell apart from anything else in a header.
        assert!(!carries(&Request::get("https://x.test/").bearer("abc"), "abc"));
    }

    #[test]
    fn a_log_line_never_replaces_a_fresher_answer_from_the_server() {
        let online = snap(Source::Online, T);
        assert!(replaces(None, &snap(Source::Local, 1)));
        assert!(!replaces(Some(&online), &snap(Source::Local, T)));
        assert!(!replaces(Some(&online), &snap(Source::Local, T - 60)));
        assert!(replaces(Some(&online), &snap(Source::Local, T + 1)));
        assert!(replaces(Some(&online), &snap(Source::Online, T - 5)));
        assert!(replaces(Some(&snap(Source::Local, T)), &snap(Source::Local, T - 5)));
        let mut full = snap(Source::Online, T);
        full.windows.push(Window::new(WindowKind::Session, "5-hour", 0.5));
        assert!(!replaces(Some(&full), &snap(Source::Local, T - 1)));
    }

    #[test]
    fn typed_key_only_services() {
        let dirs = Dirs::sample(Os::Windows);
        assert!(typed_key_only(&[Want::Secret("a")], false));
        assert!(typed_key_only(&[Want::Secret("a"), Want::Secret("b")], false));
        assert!(!typed_key_only(&[Want::Secret("a")], true));
        assert!(!typed_key_only(&[Want::Secret("a"), Want::file("/x")], false));
        assert!(!typed_key_only(&[], false));
        // OpenRouter is one unless OPENROUTER_API_KEY is set; OpenCode Go
        // also reads its own sign-in file, so it never is.
        assert!(typed_key_only(&openrouter::OpenRouter.wants(&dirs), false));
        assert!(!typed_key_only(&opencode::OpenCode.wants(&dirs), false));
        assert!(typed_key_only(&oyenza::Oyenza.wants(&dirs), false));
    }

    #[test]
    fn services_as_bits_and_the_notices_line() {
        assert!(ProviderId::ALL.len() <= 32);
        assert_eq!(bit(ProviderId::Codex), 1);
        assert_eq!(bit(ProviderId::Oyenza), 1 << 10);
        assert_eq!(bits(&["claude".into(), "nope".into(), "openrouter".into(), "claude".into()]), bit(ProviderId::Claude) | bit(ProviderId::OpenRouter));
        assert_eq!(bits(&[]), 0);
        let all: u32 = ProviderId::ALL.iter().map(|id| bit(*id)).sum();
        assert_eq!(all, (1 << ProviderId::ALL.len()) - 1);
        assert_eq!(near(80), 0.8);
        assert_eq!(near(95), 0.95);
        assert_eq!(near(0), 0.5);
        assert_eq!(near(250), 0.95);
    }

    #[test]
    fn names_cover_every_service() {
        let env = env_names();
        for name in ["CODEX_HOME", "OPENROUTER_API_KEY", "PONI_INFERENCE_URL"] {
            assert!(env.contains(&name), "{name} missing from {env:?}");
        }
        assert!(env.windows(2).all(|w| w[0] < w[1]), "sorted, once each");
        for os in [Os::Windows, Os::Mac] {
            let names = secret_names(&Dirs::sample(os));
            assert!(names.contains(&oyenza::SECRET) && names.contains(&"openrouter"), "{names:?}");
            for p in all() {
                if let Some(k) = p.takes_key() {
                    assert!(names.contains(&k.secret), "{:?}", p.id());
                }
            }
            assert!(names.iter().all(|n| safe_name(n)), "{names:?}");
        }
        for bad in ["", "../x", "a/b", "a b", "a.bin", &"x".repeat(41)] {
            assert!(!safe_name(bad), "{bad}");
        }
    }

    /// A folder of its own for one test, gone at the end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let d = std::env::temp_dir().join(format!("poni-pace-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }

        /// A file with `body`, last changed `age` seconds before now.
        fn file(&self, rel: &str, body: &[u8], age: u64) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, body).unwrap();
            let f = File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(age)).unwrap();
            p
        }

        /// The folder `rel`, last changed `age` seconds before now. Windows
        /// opens a folder only with FILE_FLAG_BACKUP_SEMANTICS, and setting
        /// its times takes FILE_WRITE_ATTRIBUTES.
        fn age_folder(&self, rel: &str, age: u64) {
            #[cfg(windows)]
            let f = {
                use std::os::windows::fs::OpenOptionsExt;
                File::options().access_mode(0x100).custom_flags(0x0200_0000).open(self.0.join(rel))
            };
            #[cfg(not(windows))]
            let f = File::open(self.0.join(rel));
            f.unwrap().set_modified(SystemTime::now() - Duration::from_secs(age)).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(got: &Got) -> Vec<String> {
        got.files().iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_file_is_read_whole_or_not_at_all() {
        let s = Scratch::new("file");
        let p = s.file("auth.json", b"{\"a\":1}", 30);
        let Got::One(f) = read_file(&p, 1024) else { panic!("read") };
        assert_eq!(f.bytes, b"{\"a\":1}");
        assert!(f.modified.is_some_and(|t| (SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 - t - 30).abs() <= 2));
        assert_eq!(read_file(&p, 7), read_file(&p, 1024));
        assert_eq!(read_file(&p, 6), Got::Missing);
        assert_eq!(read_file(&s.0.join("none.json"), 1024), Got::Missing);
        assert_eq!(read_file(&s.0, 1 << 20), Got::Missing);
        assert_eq!(read_file(Path::new("auth.json"), 1024), Got::Missing);
    }

    #[test]
    fn a_piece_of_a_file() {
        let s = Scratch::new("range");
        let p = s.file("state.vscdb", b"SQLite format 3\0rest of the page", 30);
        let Got::One(f) = read_range(&p, 0, 16) else { panic!("read") };
        assert_eq!(f.bytes, b"SQLite format 3\0");
        assert!(f.modified.is_some());
        assert_eq!(read_range(&p, 16, 1000).bytes(), Some(&b"rest of the page"[..]));
        assert_eq!(read_range(&p, 999, 16).bytes(), Some(&b""[..]));
        assert_eq!(read_range(&s.0.join("none.vscdb"), 0, 16), Got::Missing);
        assert_eq!(read_range(&s.0, 0, 16), Got::Missing);
        assert_eq!(read_range(Path::new("state.vscdb"), 0, 16), Got::Missing);
    }

    #[test]
    fn the_newest_files_by_last_change_down_their_folders() {
        let s = Scratch::new("newest");
        let root = s.0.join("sessions");
        s.file("sessions/rollout-flat.jsonl", b"flat", 500);
        s.file("sessions/2026/10/26/rollout-a.jsonl", b"line 1\nline 2\nline 3\n", 300);
        // An old session resumed today: its name is older, its change newer.
        s.file("sessions/2026/10/20/rollout-old.jsonl", b"resumed", 10);
        s.file("sessions/2026/10/27/rollout-b.jsonl", b"newest day", 100);
        s.file("sessions/2026/10/27/notes.txt", b"not a log", 1);
        s.file("sessions/2026/10/27/deeper/rollout-c.jsonl", b"too deep", 1);
        let got = read_newest(&root, ".jsonl", 3, 10, 1024);
        assert_eq!(names(&got), ["rollout-old.jsonl", "rollout-b.jsonl", "rollout-a.jsonl", "rollout-flat.jsonl"]);
        assert_eq!(got.files()[1].bytes, b"newest day");
        // At most `count`; tails from the end.
        let got = read_newest(&root, ".jsonl", 3, 2, 7);
        assert_eq!(names(&got), ["rollout-old.jsonl", "rollout-b.jsonl"]);
        assert_eq!(got.files()[0].bytes, b"resumed");
        assert_eq!(got.files()[1].bytes, b"est day");
        // Depth 0: that folder alone; tail 0: "is it there", nothing read.
        let there = read_newest(&root, ".jsonl", 0, 1, 0);
        assert_eq!(names(&there), ["rollout-flat.jsonl"]);
        assert!(there.files()[0].bytes.is_empty() && there.files()[0].modified.is_some());
        assert_eq!(read_newest(&root, ".db", 0, 1, 0), Got::Many(Vec::new()));
        assert_eq!(read_newest(&s.0.join("none"), ".jsonl", 3, 4, 1024), Got::Missing);
        assert_eq!(read_newest(Path::new("sessions"), ".jsonl", 3, 4, 1024), Got::Missing);
    }

    #[test]
    fn a_long_history_costs_no_more_than_its_newest_folders() {
        let s = Scratch::new("history");
        for i in 0..SCAN_MAX + 50 {
            s.file(&format!("logs/old/{i:05}.jsonl"), b"", 10_000);
        }
        // Its last file came long ago (made here a moment before the new one).
        s.age_folder("logs/old", 10_000);
        s.file("logs/new/today.jsonl", b"today", 5);
        // The newest folder is walked first, so the cap cuts the old ones.
        assert_eq!(names(&read_newest(&s.0.join("logs"), ".jsonl", 1, 1, 64)), ["today.jsonl"]);
    }

    #[cfg(unix)]
    #[test]
    fn links_lead_to_files_never_round_in_circles() {
        let s = Scratch::new("links");
        let target = s.file("real/a.jsonl", b"a", 50);
        std::os::unix::fs::symlink(&target, s.0.join("logs-link.jsonl")).unwrap();
        std::os::unix::fs::symlink(&s.0, s.0.join("real/loop")).unwrap();
        let mut got = names(&read_newest(&s.0, ".jsonl", 5, 10, 16));
        got.sort();
        assert_eq!(got, ["a.jsonl", "logs-link.jsonl"]);
    }
}
