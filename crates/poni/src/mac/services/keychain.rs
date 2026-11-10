//! A password from this Mac's keychain, for the AI tab (`Want::Keychain`):
//! Claude Code keeps its sign-in there, as "Claude Code-credentials", and
//! Cursor and the Claude app the key their own sign-ins are sealed with
//! ("Cursor Safe Storage", "Claude Safe Storage": those the apps made
//! themselves, so macOS asks the person before it hands one over).
//!
//! Which way an item is read follows who made it:
//!
//! - Items made with macOS's own `/usr/bin/security` (Claude Code's, and
//!   Antigravity's through the Go `keyring` library) trust that tool, so they
//!   are read with it (`find-generic-password -w`) and the keychain hands
//!   them over without asking. Asked from inside the app, macOS would ask
//!   every time Claude Code rewrites its item.
//! - "<App> Safe Storage" items, which Chromium and Electron apps make with
//!   macOS's keychain calls, trust only the app that made them, so macOS
//!   asks the person whoever reads them. They are read from inside Oyenza
//!   Studio (`SecItemCopyMatching`), so the prompt names Oyenza Studio, and
//!   Always Allow trusts Oyenza Studio alone (its signature, kept across
//!   updates). Read with `security`, the prompt named "security", and Always
//!   Allow let any program read the item with that tool without asking.
//!
//! macOS's prompt names the reader and the item, with a field for the Mac's
//! password and Always Allow, Deny and Allow (a locked keychain asks too).
//! That prompt must never come out of nowhere, so macOS is asked only
//! while the AI tab is on screen, at most once an item each time the tab
//! opens, and never again after a no until the person clicks Refresh. With
//! the tab closed (the heads-up notices reading in the background) a read
//! answers with what was read while it was open, kept in memory while the
//! tab is open or the notices are switched on, also while they rest (the
//! Mac locked, the display off, Low Power Mode), so they go on after.
//!
//! The password never goes into the log.

use super::cf::{self, CFTypeRef};
use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFString, CFType};
use studio_usage::{Got, FILE_MAX};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::{mpsc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const SECURITY: &str = "/usr/bin/security";
/// One read waits this long at most: time for the person to read macOS's
/// prompt and type their password. Then it gives up, as a no.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The end of the items read from inside the app ("Claude Safe Storage").
const SAFE_STORAGE: &str = " Safe Storage";

// The keychain's own error codes, for a read from inside the app.
/// errSecItemNotFound.
const SEC_NOT_FOUND: i32 = -25300;
/// errSecNoDefaultKeychain.
const SEC_NO_DEFAULT_KEYCHAIN: i32 = -25307;
/// errSecNoSuchKeychain.
const SEC_NO_KEYCHAIN: i32 = -25294;

#[link(name = "Security", kind = "framework")]
#[allow(non_upper_case_globals)]
extern "C" {
    static kSecClass: &'static CFString;
    static kSecClassGenericPassword: &'static CFString;
    static kSecAttrService: &'static CFString;
    static kSecAttrAccount: &'static CFString;
    static kSecReturnData: &'static CFString;
    static kSecMatchLimit: &'static CFString;
    static kSecMatchLimitOne: &'static CFString;
    static kSecValueData: &'static CFString;
    fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> i32;
    fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> i32;
    fn SecItemDelete(query: CFTypeRef) -> i32;
}

/// A query dictionary as the plain pointer Security.framework takes.
fn dict_ref(d: &CFDictionary<CFString, CFType>) -> CFTypeRef {
    d.as_opaque() as *const _ as CFTypeRef
}

/// The smoke run's own "Safe Storage" item (`password`, or `None` to remove
/// it), added from inside the app: it trusts the app, so reading it back
/// asks nobody. One `security` adds trusts that tool alone, and reading that
/// from the app brings up macOS's prompt, which a CI runner can't answer.
/// The keychain's status: 0 when done.
pub fn smoke_item(service: &str, account: &str, password: Option<&str>) -> i32 {
    let (service, account) = (CFString::from_str(service), CFString::from_str(account));
    // SAFETY: Security.framework's constants, there for the whole run.
    let (class, generic, attr_service, attr_account, value) = unsafe { (kSecClass, kSecClassGenericPassword, kSecAttrService, kSecAttrAccount, kSecValueData) };
    let keys = [class, attr_service, attr_account];
    let values: [&CFType; 3] = [generic.as_ref(), service.as_ref(), account.as_ref()];
    let query = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: a dictionary alive for the call.
    let deleted = unsafe { SecItemDelete(dict_ref(&query)) };
    let Some(password) = password else {
        return if deleted == SEC_NOT_FOUND { 0 } else { deleted };
    };
    let data = CFData::from_bytes(password.as_bytes());
    let keys = [class, attr_service, attr_account, value];
    let values: [&CFType; 4] = [generic.as_ref(), service.as_ref(), account.as_ref(), data.as_ref()];
    let item = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: a dictionary alive for the call; no result asked for.
    unsafe { SecItemAdd(dict_ref(&item), std::ptr::null_mut()) }
}
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
    /// The tab is open or the notices are switched on: what was read is kept.
    keep: bool,
    /// Counts the tab's openings.
    opening: u64,
    kept: Vec<Kept>,
}

static GATE: Mutex<Gate> = Mutex::new(Gate { open: false, keep: false, opening: 0, kept: Vec::new() });
/// What the island last told [`set_watch`], packed, so telling it every
/// frame costs nothing while it stays the same.
static TOLD: AtomicU8 = AtomicU8::new(0);
/// How many times macOS was asked (the smoke run checks it).
static ASKED: AtomicU32 = AtomicU32::new(0);

/// Plain data inside, so a panic elsewhere can't leave it half changed.
fn gate() -> MutexGuard<'static, Gate> {
    GATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// Whether the AI tab is on screen (`open`) and whether the heads-up notices
/// are switched on (`notices_on`: the setting, resting or not). The island
/// tells it every frame (`App::set_usage_watch`); only a change does anything.
pub fn set_watch(open: bool, notices_on: bool) {
    let told = 0x80 | open as u8 | (notices_on as u8) << 1;
    if TOLD.swap(told, Ordering::Relaxed) == told {
        return;
    }
    let mut g = gate();
    if open && !g.open {
        g.opening += 1;
    }
    g.open = open;
    g.keep = open || notices_on;
    if !g.keep {
        // Nobody needs what was read: forget it, but not a no (so it isn't asked again).
        g.kept.retain(|k| k.got == Got::Denied);
    }
}

/// What the island last told: whether a read may ask macOS, and whether
/// what was read is kept (the smoke run checks it).
pub fn watching() -> (bool, bool) {
    let g = gate();
    (g.open, g.keep)
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

/// Ask macOS once, the way the item wants (see the top of this file).
fn ask(service: &str, account: Option<&str>) -> Got {
    if service.ends_with(SAFE_STORAGE) {
        ask_in_app(service, account)
    } else {
        ask_tool(service, account)
    }
}

/// In-app reads that gave up waiting while macOS's prompt was still up:
/// not asked again until it is answered, so the person never gets two.
static WAITING: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());

fn waiting() -> MutexGuard<'static, Vec<(String, Option<String>)>> {
    WAITING.lock().unwrap_or_else(|p| p.into_inner())
}

/// Ask macOS once from inside the app (`SecItemCopyMatching`), so its prompt
/// names Oyenza Studio, waiting at most [`TIMEOUT`]. The call can't be
/// called off, so it runs on a thread of its own: past the wait, the prompt
/// stays up until the person answers, and that late answer is dropped (the
/// read is a no until Refresh, as with `security`).
fn ask_in_app(service: &str, account: Option<&str>) -> Got {
    let started = Instant::now();
    let item = (service.to_string(), account.map(str::to_string));
    {
        let mut w = waiting();
        if w.contains(&item) {
            pinfo!("keychain: {service}: macOS's prompt for it is still up");
            return Got::Denied;
        }
        w.push(item.clone());
    }
    ASKED.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    let asked = item.clone();
    let thread = std::thread::Builder::new().name("poni-keychain".into()).spawn(move || {
        let answer = copy_password(&asked.0, asked.1.as_deref());
        waiting().retain(|w| *w != asked);
        let _ = tx.send(answer);
    });
    if let Err(e) = thread {
        waiting().retain(|w| *w != item);
        pwarn!("keychain: {service}: no thread to ask with ({e})");
        return Got::Missing;
    }
    let (got, what) = rx.recv_timeout(TIMEOUT).unwrap_or_else(|_| (Got::Denied, "no answer in time".to_string()));
    pinfo!("keychain: {service}: {what} after {} ms (asked by the app)", started.elapsed().as_millis());
    got
}

/// `SecItemCopyMatching` for the generic password `service` (and `account`,
/// if given): its bytes as text, or why not. Blocks while macOS asks.
fn copy_password(service: &str, account: Option<&str>) -> (Got, String) {
    let service = CFString::from_str(service);
    let account = account.filter(|a| !a.is_empty()).map(CFString::from_str);
    // SAFETY: Security.framework's constants, there for the whole run.
    let (class, generic, attr_service, attr_account, return_data, limit, one) =
        unsafe { (kSecClass, kSecClassGenericPassword, kSecAttrService, kSecAttrAccount, kSecReturnData, kSecMatchLimit, kSecMatchLimitOne) };
    let mut keys: Vec<&CFString> = vec![class, attr_service, return_data, limit];
    let mut values: Vec<&CFType> = vec![generic.as_ref(), service.as_ref(), CFBoolean::new(true).as_ref(), one.as_ref()];
    if let Some(a) = &account {
        keys.push(attr_account);
        values.push(a.as_ref());
    }
    let query = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    let mut out: CFTypeRef = std::ptr::null();
    // SAFETY: a query dictionary alive for the call; `out` is ours to release.
    let status = unsafe { SecItemCopyMatching(dict_ref(&query), &mut out) };
    let bytes = cf::data_bytes(out);
    if !out.is_null() {
        // SAFETY: SecItemCopyMatching handed it over with a retain.
        unsafe { cf::CFRelease(out) };
    }
    match (status, bytes) {
        (0, Some(b)) if b.len() as u64 > FILE_MAX => (Got::Missing, "too big".to_string()),
        (0, Some(b)) if b.is_empty() => (Got::Missing, "empty".to_string()),
        (0, Some(b)) => (Got::Text(String::from_utf8_lossy(&b).into_owned()), "read".to_string()),
        (0, None) => (Got::Missing, "no password in it".to_string()),
        (SEC_NOT_FOUND | SEC_NO_DEFAULT_KEYCHAIN | SEC_NO_KEYCHAIN, _) => (Got::Missing, "not there".to_string()),
        // Deny (-128), a closed prompt or a keychain left locked.
        (code, _) => (Got::Denied, format!("no (keychain said {code})")),
    }
}

/// Ask macOS once: `security find-generic-password -s <service> [-a
/// <account>] -w`, waiting for it (and for the person, if it prompts) at
/// most [`TIMEOUT`].
fn ask_tool(service: &str, account: Option<&str>) -> Got {
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
    if let Err(e) = reader {
        let _ = child.kill();
        let _ = child.wait();
        pwarn!("keychain: {service}: no thread to read with ({e})");
        return Got::Missing;
    }
    let printed = rx.recv_timeout(TIMEOUT).ok();
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
/// are, or all as hex when one isn't printable (a line break, an accent:
/// "café" comes out as 636166c3a9, macOS 15 and 26). So hex is read only when
/// it decodes to such a byte: a password of plain hex digits prints as is.
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
    s.chunks(2).map(|p| Some(digit(p[0])? << 4 | digit(p[1])?)).collect()
}
