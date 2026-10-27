//! New versions: reads the update file (`poni_core::update::FEED_URL`, about
//! 1 KB) and believes it only when its signature checks out with a built-in
//! key. The check uses the system's own crypto: CNG on Windows, and on a Mac
//! the P-256 code the app already carries for phone drop. It runs as a job on
//! the add-on downloader's thread (`AddonService::run`), which sleeps the
//! rest of the time; the app decides when (`crate::updates`).
//!
//! On Windows the same thread then downloads the new version's Setup, once
//! per release, and keeps it only if its size and SHA-256 match the signed
//! file's; [`install`] runs it silently.
//!
//! The Mac app runs this file as is, on its own HTTP client. Where this copy
//! can replace itself ([`can_install`]), the same thread downloads the new
//! version's DMG, checked the same way, and takes the app out of it; Install
//! puts that app in this copy's place and opens it once this copy has quit.

use crate::events::{AppEvent, EventSink};
use crate::services::addons::Job;
use crate::services::http::Client;
use poni_core::update::{self, Release};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::Duration;

/// Budget for the file: it is small, but a slow line still gets it.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Budget for the Setup (about 2.5 MB) or the DMG (about 4.3 MB): a slow
/// line's few minutes.
const SETUP_TIMEOUT: Duration = Duration::from_secs(300);

/// What the update file said.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateEvent {
    /// The latest release (signature checked), or why the file couldn't be
    /// read or believed.
    Checked(Result<Release, String>),
    /// The new version is on disk and checked, or why not: its Setup on
    /// Windows, its app (out of the DMG) on a Mac.
    Downloaded(Result<std::path::PathBuf, String>),
}

/// A job for the add-on thread: read the file, answer with `AppEvent::Update`.
pub fn check_job(sink: EventSink) -> Job {
    Box::new(move |client: Result<&Client, &str>| {
        let r = client.map_err(str::to_string).and_then(check);
        match &r {
            Ok(rel) => crate::pinfo!("updates: the latest release is {} ({})", rel.version, rel.date),
            Err(e) => crate::pinfo!("updates: couldn't read the update file ({e})"),
        }
        sink.send(AppEvent::Update(UpdateEvent::Checked(r)));
    })
}

fn check(client: &Client) -> Result<Release, String> {
    let (status, body) = client.get_bytes(update::FEED_URL, TIMEOUT, update::MAX_FILE_BYTES, |_| {}).map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    believe(std::str::from_utf8(&body).map_err(|_| "not text".to_string())?)
}

/// The file's text to the release it describes, once its signature checks out.
pub fn believe(text: &str) -> Result<Release, String> {
    let signed = update::parse_file(text)?;
    if !verify(signed.payload.as_bytes(), &signed.signature, &signed.key) {
        return Err("its signature doesn't match, so it was ignored".into());
    }
    update::parse_payload(&signed.payload)
}

/// A job for the add-on thread: put the Setup `d` at `path`, unless it is
/// there already, and answer with `UpdateEvent::Downloaded`. On a Mac `d` is
/// the DMG, and the answer is its app, taken out next to it ([`staged`]).
pub fn download_job(sink: EventSink, d: update::Download, path: std::path::PathBuf) -> Job {
    Box::new(move |client: Result<&Client, &str>| {
        let r = download(client, &d, &path);
        #[cfg(target_os = "macos")]
        let r = r.and_then(|()| stage(&path, &d));
        #[cfg(windows)]
        let r = r.map(|()| path);
        match &r {
            Ok(_) => crate::pinfo!("updates: {} is ready", d.file_name()),
            Err(e) => crate::pinfo!("updates: couldn't download {} ({e})", d.file_name()),
        }
        sink.send(AppEvent::Update(UpdateEvent::Downloaded(r)));
    })
}

fn download(client: Result<&Client, &str>, d: &update::Download, path: &std::path::Path) -> Result<(), String> {
    use std::fs;
    // Downloaded on an earlier run: checked again, no request.
    if fs::read(path).is_ok_and(|b| d.matches(&b)) {
        return Ok(());
    }
    let (status, bytes) = client?.get_bytes(&d.url, SETUP_TIMEOUT, d.size + 64 * 1024, |_| {}).map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    if !d.matches(&bytes) {
        return Err(format!("it came back different ({} bytes, expected {})", bytes.len(), d.size));
    }
    let dir = path.parent().ok_or("no folder")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let part = path.with_extension("part");
    fs::write(&part, &bytes).and_then(|()| fs::rename(&part, path)).map_err(|e| {
        let _ = fs::remove_file(&part);
        e.to_string()
    })
}

/// Run the Setup at `setup` silently, once it still matches `d`. Like any
/// install it closes this copy (it saves first); it keeps Start with
/// Windows as it is and opens the new version when done (`/reopen`,
/// installer/poni.nsi).
#[cfg(windows)]
pub fn install(setup: &std::path::Path, d: &update::Download) -> Result<(), String> {
    if !std::fs::read(setup).is_ok_and(|b| d.matches(&b)) {
        return Err("the downloaded Setup is gone or changed".into());
    }
    std::process::Command::new(setup).args(["/S", "/reopen"]).spawn().map(drop).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- a Mac's own install
//
// The app in the DMG is opened out of sight (hdiutil), copied out (ditto)
// and checked: this app's bundle id, the offered version, an intact
// signature, and this copy's developer team once it is signed with a
// Developer ID. Install copies it next to this copy, swaps the two in one
// step (renamex_np) and leaves a small shell to open it once this copy has
// quit. Where any of it fails, this copy stays as it is.

/// The bundle id every copy of the Mac app has (installer/mac/Info.plist).
#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "com.oyenza.poni";

/// The app bundle this copy runs from ("/Applications/Oyenza Studio.app"),
/// if it runs from one.
#[cfg(target_os = "macos")]
fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Contents/MacOS/poni
    let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
    (bundle.extension()? == "app").then_some(bundle)
}

/// The bundle Install would replace: this copy's, where this account can
/// change it (not the DMG itself, not a copy macOS opened from a temporary
/// place, not another account's).
#[cfg(target_os = "macos")]
fn installed_bundle() -> Option<PathBuf> {
    let bundle = running_bundle()?;
    (writable(bundle.parent()?) && writable(&bundle)).then_some(bundle)
}

/// Can this copy put a new version in its own place? Windows: always (the
/// Setup does it). A Mac: only from a bundle it can replace; otherwise the
/// Update sheet offers the DMG to download by hand, as before.
pub fn can_install() -> bool {
    #[cfg(target_os = "macos")]
    let can = installed_bundle().is_some();
    #[cfg(windows)]
    let can = true;
    can
}

#[cfg(target_os = "macos")]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn access(path: *const std::ffi::c_char, mode: i32) -> i32;
    }
    const W_OK: i32 = 2;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    unsafe { access(c.as_ptr(), W_OK) == 0 }
}

/// Where the app out of the DMG at `dmg` waits: next to it, as
/// "Oyenza-Studio-0.2.10-mac.app".
#[cfg(target_os = "macos")]
pub fn staged(dmg: &Path) -> PathBuf {
    dmg.with_extension("app")
}

/// The DMG at `dmg`, checked against `d`, to its app at [`staged`], checked
/// too. Its answer is that app's path.
#[cfg(target_os = "macos")]
pub fn stage(dmg: &Path, d: &update::Download) -> Result<PathBuf, String> {
    let version = d.version().ok_or("the DMG names no version")?;
    let app = staged(dmg);
    // Taken out on an earlier run.
    if check_app(&app, version).is_ok() {
        return Ok(app);
    }
    if !std::fs::read(dmg).is_ok_and(|b| d.matches(&b)) {
        return Err("the downloaded DMG is gone or changed".into());
    }
    let _ = std::fs::remove_dir_all(&app);
    let mnt = dmg.with_extension("mnt");
    let _ = std::fs::remove_dir_all(&mnt);
    std::fs::create_dir_all(&mnt).map_err(|e| e.to_string())?;
    run("hdiutil", Command::new("/usr/bin/hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-noverify", "-mountpoint"]).arg(&mnt).arg(dmg))?;
    let copied = std::fs::read_dir(&mnt)
        .map_err(|e| e.to_string())
        .and_then(|entries| entries.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "app")).ok_or_else(|| "no app in the DMG".to_string()))
        .and_then(|inside| run("ditto", Command::new("/usr/bin/ditto").arg(inside).arg(&app)));
    if run("hdiutil", Command::new("/usr/bin/hdiutil").arg("detach").arg(&mnt).arg("-quiet")).is_err() {
        let _ = run("hdiutil", Command::new("/usr/bin/hdiutil").arg("detach").arg(&mnt).arg("-force"));
    }
    let _ = std::fs::remove_dir(&mnt);
    let checked = copied.and_then(|()| check_app(&app, version));
    if checked.is_err() {
        let _ = std::fs::remove_dir_all(&app);
    }
    checked.map(|()| app)
}

/// The app at `app` is this app at `version`: its bundle id and version,
/// an intact signature, and this copy's developer team if it has one (an
/// ad-hoc signed copy has none, and its update may be signed with one).
#[cfg(target_os = "macos")]
fn check_app(app: &Path, version: &str) -> Result<(), String> {
    let plist = app.join("Contents/Info.plist");
    let key = |k: &str| output(Command::new("/usr/bin/plutil").args(["-extract", k, "raw", "-o", "-"]).arg(&plist));
    if key("CFBundleIdentifier")?.trim() != BUNDLE_ID {
        return Err("it isn't Oyenza Studio".into());
    }
    let v = key("CFBundleShortVersionString")?;
    if v.trim() != version {
        return Err(format!("it is version {}, not {version}", v.trim()));
    }
    run("codesign", Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(app))?;
    if let Some(mine) = running_bundle().and_then(|b| team(&b)) {
        if team(app).as_deref() != Some(mine.as_str()) {
            return Err("it is signed by another developer".into());
        }
    }
    Ok(())
}

/// The developer team that signed `app`, if it is signed with one.
#[cfg(target_os = "macos")]
fn team(app: &Path) -> Option<String> {
    let out = Command::new("/usr/bin/codesign").args(["-dv", "--verbose=2"]).arg(app).output().ok()?;
    // codesign writes what it found to stderr.
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    let id = text.lines().find_map(|l| l.strip_prefix("TeamIdentifier="))?.trim().to_string();
    (!id.is_empty() && id != "not set").then_some(id)
}

/// Install the checked app at `app`, version `version`, in this copy's place
/// and have it opened once this copy has quit, which the caller does next.
/// Nothing changes where this fails: this copy stays where it was.
#[cfg(target_os = "macos")]
pub fn install(app: &Path, version: &str) -> Result<(), String> {
    let bundle = installed_bundle().ok_or("this copy can't be replaced where it is")?;
    check_app(app, version)?;
    let folder = bundle.parent().ok_or("no folder")?;
    // A hidden copy in the same folder (so on the same disk), swapped in.
    let next = folder.join(".Oyenza Studio update.app");
    let _ = std::fs::remove_dir_all(&next);
    run("ditto", Command::new("/usr/bin/ditto").arg(app).arg(&next))?;
    let old = match swap(&next, &bundle) {
        Ok(()) => next,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&next);
            return Err(e);
        }
    };
    crate::pinfo!("updates: {version} is in place at {}", bundle.display());
    reopen(&bundle, &old);
    Ok(())
}

/// Put the app at `next` where `bundle` is, in one step; the old app ends up
/// at `next`. Disks that can't swap (not APFS) take two renames.
#[cfg(target_os = "macos")]
fn swap(next: &Path, bundle: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn renamex_np(from: *const std::ffi::c_char, to: *const std::ffi::c_char, flags: u32) -> i32;
    }
    const RENAME_SWAP: u32 = 0x2;
    let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| "a path with a NUL".to_string());
    let (from, to) = (c(next)?, c(bundle)?);
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_SWAP) } == 0 {
        return Ok(());
    }
    let swap_err = std::io::Error::last_os_error();
    crate::pinfo!("updates: couldn't swap in one step ({swap_err}); moving the two apps");
    let aside = bundle.with_file_name(".Oyenza Studio old.app");
    let _ = std::fs::remove_dir_all(&aside);
    std::fs::rename(bundle, &aside).map_err(|e| format!("macOS didn't let this copy move ({e})"))?;
    if let Err(e) = std::fs::rename(next, bundle) {
        let _ = std::fs::rename(&aside, bundle);
        return Err(format!("the new app couldn't take its place ({e})"));
    }
    std::fs::rename(&aside, next).map_err(|e| e.to_string())
}

/// A small shell, out of this app's process group so it outlives it: once
/// this copy has quit (at most 30 s), it deletes `old` and opens `app`.
#[cfg(target_os = "macos")]
fn reopen(app: &Path, old: &Path) {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    const SCRIPT: &str = r#"i=0; while kill -0 "$1" 2>/dev/null && [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done; rm -rf "$2"; exec /usr/bin/open "$3""#;
    let spawned = Command::new("/bin/sh")
        .args(["-c", SCRIPT, "sh", &std::process::id().to_string()])
        .arg(old)
        .arg(app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    if let Err(e) = spawned {
        // The new version is in place: it opens when Oyenza Studio is next opened.
        crate::pwarn!("updates: couldn't arrange the reopen ({e})");
    }
}

/// Run a tool; its first line of complaint if it fails.
#[cfg(target_os = "macos")]
fn run(name: &str, cmd: &mut Command) -> Result<(), String> {
    output(cmd).map(drop).map_err(|e| format!("{name}: {e}"))
}

/// A tool's output, or its first line of complaint.
#[cfg(target_os = "macos")]
fn output(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string())
}

/// ECDSA P-256 over SHA-256: `signature` is r and s, `key` an uncompressed point.
#[cfg(windows)]
fn verify(message: &[u8], signature: &[u8; 64], key: &[u8; 65]) -> bool {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptImportKeyPair, BCryptOpenAlgorithmProvider, BCryptVerifySignature, BCRYPT_ALG_HANDLE,
        BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_FLAGS, BCRYPT_KEY_HANDLE,
        BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS,
    };
    // BCRYPT_ECCKEY_BLOB: magic, key length, then X and Y.
    let mut blob = Vec::with_capacity(72);
    blob.extend_from_slice(&BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes());
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(&key[1..]);
    let hash = poni_core::sha256::sha256(message);
    unsafe {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        if BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_ECDSA_P256_ALGORITHM, None, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0)).is_err() {
            return false;
        }
        let mut handle = BCRYPT_KEY_HANDLE::default();
        let ok = BCryptImportKeyPair(alg, BCRYPT_KEY_HANDLE::default(), BCRYPT_ECCPUBLIC_BLOB, &mut handle, &blob, 0).is_ok()
            && BCryptVerifySignature(handle, None, &hash, signature, BCRYPT_FLAGS(0)).is_ok();
        if !handle.is_invalid() {
            let _ = BCryptDestroyKey(handle);
        }
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        ok
    }
}

/// ECDSA P-256 over SHA-256: `signature` is r and s, `key` an uncompressed point.
#[cfg(target_os = "macos")]
fn verify(message: &[u8], signature: &[u8; 64], key: &[u8; 65]) -> bool {
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    let (Ok(key), Ok(sig)) = (VerifyingKey::from_sec1_bytes(key), Signature::from_slice(signature)) else { return false };
    key.verify(message, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real file, signed with the signing key for v0.2.3.
    const SAMPLE: &str = include_str!("../../../poni-core/data/update-sample.json");

    #[test]
    fn a_signed_file_is_believed() {
        let r = believe(SAMPLE).expect("the sample's signature checks out");
        assert_eq!((r.version.as_str(), r.headline.as_str()), ("0.2.3", "Sing along from the closed island"));
        assert_eq!(r.windows.as_ref().map(|d| d.size), Some(2457657));
    }

    #[test]
    fn a_changed_file_is_not() {
        // One character of the payload, or of the signature.
        let changed = SAMPLE.replacen("2457657", "2457658", 1);
        assert!(believe(&changed).unwrap_err().contains("signature"), "payload changed");
        let signed = update::parse_file(SAMPLE).unwrap();
        let mut sig = signed.signature;
        sig[10] ^= 1;
        assert!(!verify(signed.payload.as_bytes(), &sig, &signed.key));
        assert!(verify(signed.payload.as_bytes(), &signed.signature, &signed.key));
        // The spare key didn't sign it.
        let spare = update::trusted_key("f835f77c60bd2b7f").unwrap();
        assert!(!verify(signed.payload.as_bytes(), &signed.signature, &spare));
    }
}
