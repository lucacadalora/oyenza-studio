//! Opening files, folders and URLs.
//!
//! `ShellExecuteW` never runs on the UI thread: shell handlers may run a
//! nested modal loop there that swallows the island's messages, and a UNC
//! path can block for many seconds while the network times out. Every call
//! gets a short-lived "poni-shell" STA thread instead.
//!
//! Targets arrive from drag-and-drop (shelf paths) and from pages (URLs), so
//! only two kinds are handed to the shell: existing absolute local or UNC
//! file-system paths, and `http`/`https` URLs. Anything else (`ms-settings:`,
//! `javascript:`, `shell:`, relative names that would be searched on `PATH`)
//! is logged and refused. Fixed exceptions: Settings > Apps > Startup
//! ([`open_startup_settings`]) and Settings > Privacy > Notifications
//! ([`open_notification_settings`]), constant URIs; Oyenza Studio Recorder's
//! own action links ([`open_recorder_link`]): its fixed scheme and one
//! percent-encoded value, as `poni_core::recorder` builds them; and an app
//! by its AppUserModelID in the Apps folder ([`open_app`]), the characters
//! of an id only.

use std::sync::atomic::{AtomicUsize, Ordering};
use windows::core::{w, HSTRING};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Shell calls allowed to be in progress at once. Calls stuck on an
/// unreachable share hold a slot each; past this, new ones are refused
/// instead of piling up threads.
const MAX_IN_FLIGHT: usize = 8;
/// Longest target accepted (the extended-length path limit).
const MAX_TARGET_CHARS: usize = 32_767;
/// Longest target quoted in a log line.
const LOG_CHARS: usize = 260;

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Open a file, folder or URL with its default handler (asynchronously).
pub fn open(target: &str) {
    let kind = match classify(target) {
        Ok(k) => k,
        Err(why) => {
            crate::pwarn!("open refused ({why}): {}", for_log(target));
            return;
        }
    };
    let target = target.to_string();
    run("open", move || {
        if kind == Target::Path && std::fs::metadata(&target).is_err() {
            crate::pwarn!("open refused (not found): {}", for_log(&target));
            return;
        }
        let t = HSTRING::from(target.as_str());
        let r = unsafe { ShellExecuteW(None, w!("open"), &t, None, None, SW_SHOWNORMAL) };
        if (r.0 as isize) <= 32 {
            crate::pwarn!("open failed ({}): {}", r.0 as isize, for_log(&target));
        }
    });
}

/// Open one of Oyenza Studio Recorder's action links (asynchronously). Only
/// the shape `poni_core::recorder` builds reaches the shell.
pub fn open_recorder_link(link: &str) {
    if !poni_core::recorder::is_action_link(link) {
        crate::pwarn!("recorder link refused: {}", for_log(link));
        return;
    }
    let link = link.to_string();
    run("recorder", move || {
        let t = HSTRING::from(link.as_str());
        let r = unsafe { ShellExecuteW(None, w!("open"), &t, None, None, SW_SHOWNORMAL) };
        if (r.0 as isize) <= 32 {
            crate::pwarn!("recorder link failed ({})", r.0 as isize);
        }
    });
}

/// Open Settings > Apps > Startup (asynchronously). The Store build sends
/// people there when they turned Poni's startup task off in Windows, the only
/// place it can be turned back on.
pub fn open_startup_settings() {
    run("open", || {
        let r = unsafe { ShellExecuteW(None, w!("open"), w!("ms-settings:startupapps"), None, None, SW_SHOWNORMAL) };
        if (r.0 as isize) <= 32 {
            crate::pwarn!("open failed ({}): ms-settings:startupapps", r.0 as isize);
        }
    });
}

/// Open Settings > Privacy & security > Notifications (asynchronously),
/// where Windows lists the apps that may read notifications: the only
/// place the Notifications add-on can be allowed again once turned off.
pub fn open_notification_settings() {
    run("open", || {
        let r = unsafe { ShellExecuteW(None, w!("open"), w!("ms-settings:privacy-notifications"), None, None, SW_SHOWNORMAL) };
        if (r.0 as isize) <= 32 {
            crate::pwarn!("open failed ({}): ms-settings:privacy-notifications", r.0 as isize);
        }
    });
}

/// Open the app with this AppUserModelID the way Start does, through the
/// Apps folder (asynchronously): a notification's Open. Only an id made of
/// the characters ids use reaches the shell ([`app_id_ok`]).
pub fn open_app(app_id: &str) {
    if !app_id_ok(app_id) {
        crate::pwarn!("open app refused (not an app id): {}", for_log(app_id));
        return;
    }
    let args = format!("shell:AppsFolder\\{app_id}");
    run("open", move || {
        let r = unsafe { ShellExecuteW(None, w!("open"), w!("explorer.exe"), &HSTRING::from(args.as_str()), None, SW_SHOWNORMAL) };
        if (r.0 as isize) <= 32 {
            crate::pwarn!("open app failed ({}): {}", r.0 as isize, for_log(&args));
        }
    });
}

/// An AppUserModelID as Windows hands one over: a packaged app's
/// "Family_hash!App", a desktop app's dotted name, or a path under a known
/// folder ("{GUID}\\Folder\\app.exe"). Letters, digits, spaces and
/// `. _ - ! { } \ ( )` only, no "..", no leading or trailing space, 255
/// characters at most: nothing that could end explorer's argument or leave
/// the Apps folder.
pub(crate) fn app_id_ok(id: &str) -> bool {
    let allowed = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '!' | '{' | '}' | '\\' | '(' | ')');
    !id.is_empty()
        && id.chars().count() <= 255
        && id.trim() == id
        && !id.contains("..")
        && !id.starts_with('\\')
        && id.chars().all(allowed)
}

/// Show a file selected in Explorer (asynchronously).
pub fn reveal(path: &str) {
    match classify(path) {
        Ok(Target::Path) => {}
        Ok(Target::Url) => {
            crate::pwarn!("reveal refused (not a file-system path): {}", for_log(path));
            return;
        }
        Err(why) => {
            crate::pwarn!("reveal refused ({why}): {}", for_log(path));
            return;
        }
    }
    let path = path.to_string();
    run("reveal", move || {
        if std::fs::metadata(&path).is_err() {
            crate::pwarn!("reveal refused (not found): {}", for_log(&path));
            return;
        }
        // `classify` rejected quotes, so the path cannot end the argument early.
        let args = HSTRING::from(format!("/select,\"{path}\""));
        let r = unsafe { ShellExecuteW(None, w!("open"), w!("explorer.exe"), &args, None, SW_SHOWNORMAL) };
        if (r.0 as isize) <= 32 {
            crate::pwarn!("reveal failed ({}): {}", r.0 as isize, for_log(&path));
        }
    });
}

/// Run `job` on a new "poni-shell" thread inside a single-threaded apartment.
fn run(what: &'static str, job: impl FnOnce() + Send + 'static) {
    if IN_FLIGHT.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        crate::pwarn!("{what} refused: {MAX_IN_FLIGHT} shell requests are still pending");
        return;
    }
    let spawned = std::thread::Builder::new().name("poni-shell".into()).spawn(move || {
        let _slot = Slot;
        // Shell extensions are apartment-threaded COM objects; DDE is off.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if hr.is_err() {
            crate::pwarn!("{what}: CoInitializeEx failed ({hr:?}); trying anyway");
        }
        job();
        if hr.is_ok() {
            unsafe { CoUninitialize() };
        }
    });
    if let Err(e) = spawned {
        // The closure (and its slot) never ran.
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        crate::pwarn!("{what} failed: shell thread did not start ({e})");
    }
}

/// One in-flight shell call; released when its thread ends (or unwinds).
struct Slot;

impl Drop for Slot {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------- validation

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// An `http`/`https` URL.
    Url,
    /// An absolute drive-letter or UNC path (existence is checked off-thread).
    Path,
}

/// What kind of target `s` is, or why it is refused. Pure syntax: no I/O.
fn classify(s: &str) -> Result<Target, &'static str> {
    if s.is_empty() {
        return Err("empty");
    }
    if s.chars().count() > MAX_TARGET_CHARS {
        return Err("too long");
    }
    if s.chars().any(char::is_control) {
        return Err("control characters");
    }
    if let Some(rest) = strip_prefix_ci(s, "https://").or_else(|| strip_prefix_ci(s, "http://")) {
        return web_url_rest(rest).map(|()| Target::Url);
    }
    if is_absolute_fs_path(s) {
        return Ok(Target::Path);
    }
    if s.starts_with(r"\\.\") || s.starts_with(r"\\?\") {
        return Err("device or unsupported namespace path");
    }
    if has_scheme(s) {
        return Err("unsupported scheme");
    }
    Err("not an absolute path or web URL")
}

/// The part of an http(s) URL after `scheme://`: needs a host, and no
/// characters a browser's command line could split on (valid URLs
/// percent-encode them).
fn web_url_rest(rest: &str) -> Result<(), &'static str> {
    if rest.chars().any(|c| c.is_whitespace() || matches!(c, '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}')) {
        return Err("invalid URL characters");
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Strip any user info; the host must be non-empty.
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    if host.is_empty() && !authority.starts_with('[') {
        return Err("URL without a host");
    }
    Ok(())
}

/// `C:\…` (or `C:/…`), `\\server\share…`, and their `\\?\` long forms.
fn is_absolute_fs_path(s: &str) -> bool {
    if let Some(rest) = strip_prefix_ci(s, r"\\?\UNC\") {
        return is_unc_tail(rest);
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return is_drive_path(rest);
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        return is_unc_tail(rest);
    }
    is_drive_path(s)
}

/// `X:\rest` or `X:/rest` (a bare `X:` is relative to that drive's current folder).
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/') && is_clean_tail(&s[3..])
}

/// `server\share[\rest]` after the leading `\\`.
fn is_unc_tail(s: &str) -> bool {
    let mut parts = s.splitn(3, ['\\', '/']);
    let server = parts.next().unwrap_or("");
    let share = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    let named = !server.is_empty() && server != "." && server != "?" && !share.is_empty();
    named && is_clean_tail(server) && is_clean_tail(share) && is_clean_tail(rest)
}

/// No characters that are invalid in Windows path components (`:` would be
/// an alternate data stream).
fn is_clean_tail(s: &str) -> bool {
    !s.chars().any(|c| matches!(c, '"' | '<' | '>' | '|' | '?' | '*' | ':'))
}

/// `name:` with a scheme of two or more characters (one letter is a drive).
fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else { return false };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// A target shortened and stripped of control characters for one log line.
fn for_log(s: &str) -> String {
    let mut out: String = s.chars().take(LOG_CHARS).map(|c| if c.is_control() { '?' } else { c }).collect();
    if s.chars().nth(LOG_CHARS).is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_from_windows_open_and_nothing_else_does() {
        for id in [
            "5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App",
            "Telegram.TelegramDesktop",
            "Microsoft.YourPhone_8wekyb3d8bbwe!App",
            r"{6D809377-6AF0-444B-8957-A3773F02200E}\Mozilla Firefox\firefox.exe",
            "Microsoft.Office.OUTLOOK.EXE.15",
        ] {
            assert!(app_id_ok(id), "{id}");
        }
        let long = "a".repeat(256);
        for id in ["", " Telegram", "a\"b", "a&b", "..\\..\\Windows", r"\\server\share", "x\ny", "app:id", "a/b", long.as_str()] {
            assert!(!app_id_ok(id), "{id}");
        }
    }

    #[test]
    fn accepts_absolute_paths() {
        for p in [
            r"C:\Users\me\file.txt",
            "c:/Users/me",
            r"D:\",
            r"\\server\share",
            r"\\server\share\dir\f.txt",
            r"\\?\C:\very\long\path",
            r"\\?\UNC\server\share\x",
            r"C:\a\..\b",
        ] {
            assert_eq!(classify(p), Ok(Target::Path), "{p}");
        }
    }

    #[test]
    fn accepts_web_urls() {
        for u in ["https://lrclib.net", "HTTP://example.com/a?b=1#c", "https://user@host:8080/x", "http://[::1]:80/"] {
            assert_eq!(classify(u), Ok(Target::Url), "{u}");
        }
    }

    #[test]
    fn refuses_everything_else() {
        for t in [
            "",
            "ms-settings:",
            "ms-settings:privacy",
            "javascript:alert(1)",
            "shell:startup",
            "file:///C:/Windows",
            "notepad",
            "calc.exe",
            r"..\x",
            r"dir\file.txt",
            r"\rooted\on\current\drive",
            "C:",
            "C:relative",
            r"\\.\PhysicalDrive0",
            r"\\?\Volume{1234}\x",
            r"\\server",
            r"\\",
            r"\\\share",
            "https://",
            "https:///path",
            "https://exa mple.com",
            "http://a\"b",
            "https://a\nb",
            r#"C:\a" /evil"#,
            "C:\\a\nb",
            r"C:\file.txt:stream",
            r"C:\a|b",
            r"C:\a*",
        ] {
            assert!(classify(t).is_err(), "{t:?} should be refused");
        }
    }

    #[test]
    fn scheme_detection() {
        assert!(has_scheme("ms-settings:x"));
        assert!(has_scheme("mailto:a@b"));
        assert!(!has_scheme(r"C:\x"));
        assert!(!has_scheme("plain"));
        assert!(!has_scheme("1x:y"));
    }

    #[test]
    fn log_text_is_one_short_line() {
        assert_eq!(for_log("a\nb"), "a?b");
        let long = "x".repeat(LOG_CHARS + 5);
        assert_eq!(for_log(&long).chars().count(), LOG_CHARS + 1);
    }
}
