//! A password from this Mac's keychain, for the AI tab (`Want::Keychain`):
//! Claude Code keeps its sign-in there, as "Claude Code-credentials".
//!
//! It is read with macOS's own `/usr/bin/security` (`find-generic-password
//! -w`), the tool Claude Code keeps that item with, so the keychain usually
//! hands it over without asking. When it does ask (an item made another
//! way, a locked keychain), macOS shows its own prompt naming "security" and
//! the item, with a field for the Mac's password and Always Allow, Deny and
//! Allow. That prompt must never come out of nowhere, so macOS is asked only
//! while the AI tab is on screen, at most once an item each time the tab
//! opens, and never again after a no until the person clicks Refresh. With
//! the tab closed (the heads-up notices reading in the background) a read
//! answers with what was read while it was open, kept in memory only while
//! the tab or the notices want it.
//!
//! The password never goes into the log.

use poni_core::usage::{Got, FILE_MAX};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const SECURITY: &str = "/usr/bin/security";
/// One read waits this long at most: time for the person to read macOS's
/// prompt and type their password. Then it gives up, as a no.
const TIMEOUT: Duration = Duration::from_secs(60);
// `security` exits with the low byte of the keychain's error code (Claude
// Code reads these three the same way).
/// errSecItemNotFound (-25300).
const NOT_FOUND: i32 = 44;
/// errSecNoDefaultKeychain (-25307): this account has no login keychain.
const NO_DEFAULT_KEYCHAIN: i32 = 37;
/// errSecNoSuchKeychain (-25294).
const NO_KEYCHAIN: i32 = 50;

/// One item's last answer.
struct Kept {
    service: String,
    account: Option<String>,
    got: Got,
    /// The tab's opening it was read in.
    opening: u64,
}

struct Gate {
    /// The AI tab is on screen: a read may ask macOS (and its prompt show).
    open: bool,
    /// The tab or the notices want readings: what was read may be kept.
    keep: bool,
    /// Counts the tab's openings.
    opening: u64,
    kept: Vec<Kept>,
}

static GATE: Mutex<Gate> = Mutex::new(Gate { open: false, keep: false, opening: 0, kept: Vec::new() });
/// How many times macOS was asked (the smoke run checks it).
static ASKED: AtomicU32 = AtomicU32::new(0);

/// Plain data inside, so a panic elsewhere can't leave it half changed.
fn gate() -> MutexGuard<'static, Gate> {
    GATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The AI tab came on screen or left it (`open`), and whether the heads-up
/// notices read in the background (`notices`): the island calls this
/// whenever either changes (`App::set_usage_watch`).
pub fn set_watch(open: bool, notices: bool) {
    let mut g = gate();
    if open && !g.open {
        g.opening += 1;
    }
    g.open = open;
    g.keep = open || notices;
    if !g.keep {
        // Nobody needs what was read: forget it, but not a no (so it isn't asked again).
        g.kept.retain(|k| k.got == Got::Denied);
    }
}

/// The person clicked Refresh on the AI tab: the next read asks macOS
/// again, even after a no.
pub fn ask_again() {
    gate().kept.clear();
}

/// How many times macOS has been asked.
pub fn asked() -> u32 {
    ASKED.load(Ordering::Relaxed)
}

/// The password of the keychain's generic password item `service` (and
/// `account`, if given): `Got::Text` when there is one, `Got::Missing` when
/// there isn't (or the tab is closed and it wasn't read while it was open),
/// `Got::Denied` when the person said no, the prompt went unanswered, or the
/// keychain stayed locked. Blocks while macOS asks (up to a minute): call it
/// on the AI tab's worker thread, never the main one.
pub fn read(service: &str, account: Option<&str>) -> Got {
    let opening = {
        let g = gate();
        match g.kept.iter().find(|k| k.service == service && k.account.as_deref() == account) {
            // Tab closed, already asked this opening, or a no: no prompt.
            Some(k) if !g.open || k.opening == g.opening || k.got == Got::Denied => return k.got.clone(),
            // Tab closed and never read: nothing, rather than a prompt from nowhere.
            None if !g.open => return Got::Missing,
            _ => g.opening,
        }
    };
    let got = ask(service, account);
    let mut g = gate();
    g.kept.retain(|k| !(k.service == service && k.account.as_deref() == account));
    if g.keep || got == Got::Denied {
        g.kept.push(Kept { service: service.into(), account: account.map(Into::into), got: got.clone(), opening });
    }
    got
}

/// Ask macOS once: `security find-generic-password -s <service> [-a
/// <account>] -w`, waiting for it (and for the person, if it prompts) at
/// most [`TIMEOUT`].
fn ask(service: &str, account: Option<&str>) -> Got {
    let started = Instant::now();
    let mut cmd = Command::new(SECURITY);
    cmd.args(["find-generic-password", "-s", service]);
    if let Some(a) = account.filter(|a| !a.is_empty()) {
        cmd.args(["-a", a]);
    }
    let mut child = match cmd.arg("-w").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(c) => c,
        Err(e) => {
            pwarn!("keychain: {service}: couldn't run security ({e})");
            return Got::Missing;
        }
    };
    ASKED.fetch_add(1, Ordering::Relaxed);
    // The answer is read on a thread of its own while this one waits, so a
    // big item can't fill the pipe and hold `security` up meanwhile.
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take();
    let reader = std::thread::Builder::new().name("poni-keychain".into()).spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = out {
            let _ = out.take(FILE_MAX + 1).read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    let printed = match reader {
        Ok(_) => rx.recv_timeout(TIMEOUT).ok(),
        Err(_) => None,
    };
    // Done, too big or too slow: either way the process ends here.
    let status = match &printed {
        Some(b) if b.len() as u64 <= FILE_MAX => child.wait().ok().and_then(|s| s.code()),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    };
    let (got, what) = match printed {
        None => (Got::Denied, "no answer in time".to_string()),
        Some(b) if b.len() as u64 > FILE_MAX => (Got::Missing, "too big".to_string()),
        Some(b) => match status {
            Some(0) => match password(&b) {
                s if s.is_empty() => (Got::Missing, "empty".to_string()),
                s => (Got::Text(s), "read".to_string()),
            },
            Some(NOT_FOUND | NO_DEFAULT_KEYCHAIN | NO_KEYCHAIN) => (Got::Missing, "not there".to_string()),
            // Deny, a closed prompt, a keychain left locked, or no way to ask.
            Some(code) => (Got::Denied, format!("no (security exited {code})")),
            None => (Got::Denied, "security stopped".to_string()),
        },
    };
    pinfo!("keychain: {service}: {what} after {} ms", started.elapsed().as_millis());
    got
}

/// What `security -w` printed, as the password. It prints the bytes as they
/// are, or as hex when one isn't printable (a line break, an accent), where
/// a byte from 0x80 up may come out as `ffffffXX` (a signed char). So hex is
/// read only when it decodes to such a byte: a password of plain hex digits
/// prints as is.
fn password(out: &[u8]) -> String {
    let out = out.strip_suffix(b"\n").unwrap_or(out);
    match unhex(out) {
        Some(bytes) if bytes.iter().any(|b| !(0x20..0x7f).contains(b)) => String::from_utf8_lossy(&bytes).into_owned(),
        _ => String::from_utf8_lossy(out).into_owned(),
    }
}

fn unhex(s: &[u8]) -> Option<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < s.len() {
        if i + 8 <= s.len() && s[i..i + 6].eq_ignore_ascii_case(b"ffffff") {
            if let (Some(hi), Some(lo)) = (digit(s[i + 6]), digit(s[i + 7])) {
                if hi >= 8 {
                    out.push(hi << 4 | lo);
                    i += 8;
                    continue;
                }
            }
        }
        out.push(digit(s[i])? << 4 | digit(s[i + 1])?);
        i += 2;
    }
    Some(out)
}
