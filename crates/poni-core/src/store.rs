//! Persisted user data and robust JSON load/save.
//!
//! Loading never loses data silently, and never sets a later save up to
//! destroy a file it could not read:
//!
//! * A damaged file (bad JSON or bad UTF-8) is moved aside under a unique
//!   `<stem>.corrupt-<unix>.json` name, and the last good `<stem>.bak.json`
//!   backup is used when it parses ([`LoadOutcome::Recovered`]).
//! * A file that exists but cannot be read right now (sharing violation,
//!   permissions, network drive) is left in place and reported as
//!   [`LoadOutcome::Unreadable`]. Callers must not save over it
//!   ([`LoadOutcome::writable`]).
//!
//! Saving writes `<name>.tmp`, flushes it to disk, keeps the previous file as
//! `<stem>.bak.json` and then renames the temp file over the target, so the
//! target is always either the old or the new complete file.

use crate::shelf::Shelf;
use crate::todo::TaskList;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const STORE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub version: u32,
    pub tasks: TaskList,
    /// Drinks logged by the old Water page, which Health replaced. Kept
    /// exactly as it was, so an update never throws a person's history away.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub water: serde_json::Value,
    pub shelf: Shelf,
    /// Day key → completed focus sessions.
    pub focus_sessions: BTreeMap<String, u32>,
    pub last_page: String,
    /// Oyenza bill reminders already shown (`oyenza::notice_key`), so a bill
    /// is reminded of once even across restarts.
    pub oyenza_noticed: Vec<String>,
    /// The Script page's script and its pace.
    pub script: crate::script::Script,
    /// The Stocks add-on's list: the stocks after the IHSG, the one shown,
    /// the change pills' choice and the chart's range.
    pub stocks: crate::stocks::Watchlist,
    /// Oyenza Studio Recorder has written to the island (`recorder::Message`)
    /// at least once, so it reads the Record page's links: Screen recorder
    /// shows from then on. 1.0.0 never writes, and its Record would do nothing.
    pub recorder_island: bool,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            tasks: TaskList::default(),
            water: serde_json::Value::Null,
            shelf: Shelf::default(),
            focus_sessions: BTreeMap::new(),
            last_page: String::new(),
            oyenza_noticed: Vec::new(),
            script: crate::script::Script::default(),
            stocks: crate::stocks::Watchlist::default(),
            recorder_island: false,
        }
    }
}

/// What [`load_json`] found on disk.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadOutcome {
    /// The file was read and parsed.
    Loaded,
    /// There is no file yet; the default was returned.
    Missing,
    /// The file was damaged. It was moved aside, and the backup (when it
    /// parsed) or the default was returned. Saving over the path is safe.
    Recovered(String),
    /// The file exists but could not be read, or was damaged and could not be
    /// moved aside. It was left untouched; do not save over it this session.
    Unreadable(String),
}

impl LoadOutcome {
    /// Whether saving over the loaded path is safe (false only for `Unreadable`).
    pub fn writable(&self) -> bool {
        !matches!(self, LoadOutcome::Unreadable(_))
    }

    /// A human-readable description of the problem, if there was one.
    pub fn message(&self) -> Option<&str> {
        match self {
            LoadOutcome::Recovered(m) | LoadOutcome::Unreadable(m) => Some(m),
            LoadOutcome::Loaded | LoadOutcome::Missing => None,
        }
    }
}

/// Parse JSON text, tolerating a leading UTF-8 byte-order mark (Notepad adds one).
pub fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

/// Load JSON from `path`; see the module docs and [`LoadOutcome`] for what
/// happens to missing, damaged and unreadable files.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> (T, LoadOutcome) {
    let problem = match fs::read(path) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => match parse_json::<T>(text) {
                Ok(v) => return (v, LoadOutcome::Loaded),
                Err(e) => e,
            },
            Err(e) => format!("not valid UTF-8: {e}"),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (T::default(), LoadOutcome::Missing),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => e.to_string(),
        Err(e) => {
            let msg = format!(
                "could not read {} ({e}); left it untouched and will not save over it",
                path.display()
            );
            return (T::default(), LoadOutcome::Unreadable(msg));
        }
    };
    recover(path, &problem)
}

/// The damaged-file path: use the backup if it parses, move the file aside.
fn recover<T: DeserializeOwned + Default>(path: &Path, problem: &str) -> (T, LoadOutcome) {
    let backup = backup_path(path);
    let restored = read_parsed::<T>(&backup);
    match set_aside(path, unix_now()) {
        Ok(aside) => match restored {
            Some(v) => {
                let msg = format!(
                    "{} was damaged ({problem}); moved it to {} and restored from backup {}",
                    path.display(),
                    aside.display(),
                    backup.display()
                );
                (v, LoadOutcome::Recovered(msg))
            }
            None => {
                let msg = format!(
                    "{} was damaged ({problem}) and there is no usable backup; moved it to {} and started fresh",
                    path.display(),
                    aside.display()
                );
                (T::default(), LoadOutcome::Recovered(msg))
            }
        },
        Err(e) => {
            // Without a preserved copy, a later save would destroy the only one.
            let what = if restored.is_some() { "using the backup" } else { "using defaults" };
            let msg = format!(
                "{} was damaged ({problem}) and could not be moved aside ({e}); {what}, left the file untouched and will not save over it",
                path.display()
            );
            (restored.unwrap_or_default(), LoadOutcome::Unreadable(msg))
        }
    }
}

fn read_parsed<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    parse_json(std::str::from_utf8(&bytes).ok()?).ok()
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `dir/<stem><suffix>` next to `path`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.file_stem().map(|s| s.to_os_string()).unwrap_or_else(|| "data".into());
    name.push(suffix);
    path.with_file_name(name)
}

/// `dir/<file name>.tmp`: unique per target, and has the `.tmp` extension
/// that cache pruning cleans up.
fn tmp_path(path: &Path) -> PathBuf {
    let mut name: OsString = path.file_name().map(|s| s.to_os_string()).unwrap_or_else(|| "data".into());
    name.push(".tmp");
    path.with_file_name(name)
}

/// Where [`save_json_atomic`] keeps the previous version: `<stem>.bak.json`.
pub fn backup_path(path: &Path) -> PathBuf {
    sibling(path, ".bak.json")
}

/// Move a damaged file to `<stem>.corrupt-<unix>[-n].json`, never replacing
/// an earlier copy. The name is reserved with an exclusive create first, so
/// the rename only ever replaces our own empty placeholder. If the file
/// cannot be renamed (open elsewhere), it is copied instead.
fn set_aside(path: &Path, unix: u64) -> io::Result<PathBuf> {
    let aside = reserve_aside_name(path, unix)?;
    match fs::rename(path, &aside) {
        Ok(()) => Ok(aside),
        Err(rename_err) => match fs::copy(path, &aside) {
            Ok(_) => Ok(aside),
            Err(_) => {
                let _ = fs::remove_file(&aside);
                Err(rename_err)
            }
        },
    }
}

fn reserve_aside_name(path: &Path, unix: u64) -> io::Result<PathBuf> {
    for n in 0..10_000u32 {
        let suffix = if n == 0 { format!(".corrupt-{unix}.json") } else { format!(".corrupt-{unix}-{n}.json") };
        let candidate = sibling(path, &suffix);
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => {
                drop(file);
                return Ok(candidate);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            // e.g. a folder with that name reports "access denied" on Windows.
            Err(_) if candidate.exists() => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name to move the damaged file to"))
}

/// Write JSON durably and atomically: write `<name>.tmp`, flush it to disk,
/// copy the current file to `<stem>.bak.json` (best effort), then rename the
/// temp file over the target.
pub fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_json(path, value, true)
}

/// Like [`save_json_atomic`] but keeps no `.bak.json` copy, for caches that
/// can be rebuilt (e.g. the lyrics cache).
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_json(path, value, false)
}

/// Write raw `bytes` to `path` atomically (temp file + rename), no backup.
pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(path, bytes, false)
}

fn write_json<T: Serialize>(path: &Path, value: &T, backup: bool) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    write_atomic(path, &json, backup)
}

fn write_atomic(path: &Path, bytes: &[u8], backup: bool) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    let tmp = tmp_path(path);
    if let Err(e) = write_synced(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if backup {
        backup_existing(path);
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    sync_parent_dir(path);
    Ok(())
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Copy the current file to `<stem>.bak.json` via a temp name, so an
/// interrupted copy never leaves a half-written backup. Best effort.
fn backup_existing(path: &Path) {
    if !path.is_file() {
        return;
    }
    let bak = backup_path(path);
    let tmp = tmp_path(&bak);
    let ok = fs::copy(path, &tmp).is_ok() && fs::rename(&tmp, &bak).is_ok();
    if !ok {
        let _ = fs::remove_file(&tmp);
    }
}

/// Make the rename itself durable where the OS allows syncing a directory.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("poni-core-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Files in `d` whose names contain `.corrupt-`, sorted.
    fn corrupt_files(d: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().contains(".corrupt-"))
            .collect();
        v.sort();
        v
    }

    fn names(d: &Path) -> Vec<String> {
        let mut v: Vec<String> =
            fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    fn sample(n: u32) -> Store {
        let mut s = Store::default();
        s.tasks.add(&format!("task {n}"), 1);
        s.focus_sessions.insert("2026-10-24".into(), n);
        s
    }

    #[test]
    fn roundtrip_store() {
        let d = tmpdir("rt");
        let p = d.join("store.json");
        let mut s = Store::default();
        s.tasks.add("hello", 1);
        s.water = serde_json::json!({"days": {"2026-10-24": [{"at_unix": 5, "ml": 250}]}});
        save_json_atomic(&p, &s).unwrap();
        let (back, outcome): (Store, _) = load_json(&p);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, s);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn old_water_history_survives_a_save() {
        let d = tmpdir("water");
        let p = d.join("store.json");
        fs::write(&p, r#"{"version":1,"water":{"days":{"2026-10-20":[{"at_unix":7,"ml":300}]}},"last_page":"water"}"#).unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert_eq!(outcome, LoadOutcome::Loaded);
        save_json_atomic(&p, &s).unwrap();
        let back: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(back["water"]["days"]["2026-10-20"][0]["ml"], 300);
        // A store that never had water keeps no "water" key.
        save_json_atomic(&p, &Store::default()).unwrap();
        assert!(!fs::read_to_string(&p).unwrap().contains("water"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_is_default() {
        let d = tmpdir("missing");
        let p = d.join("store.json");
        let (s, outcome): (Store, _) = load_json(&p);
        assert_eq!(outcome, LoadOutcome::Missing);
        assert!(outcome.writable());
        assert!(outcome.message().is_none());
        assert_eq!(s, Store::default());
        // A backup alone does not resurrect a file the user deleted.
        save_json_atomic(&backup_path(&p), &sample(1)).unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert_eq!(outcome, LoadOutcome::Missing);
        assert_eq!(s, Store::default());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_json_without_backup_is_moved_aside() {
        let d = tmpdir("corrupt");
        let p = d.join("store.json");
        fs::write(&p, "{not json").unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)), "{outcome:?}");
        assert!(outcome.writable());
        assert!(outcome.message().unwrap().contains("started fresh"));
        assert_eq!(s, Store::default());
        assert!(!p.exists());
        let aside = corrupt_files(&d);
        assert_eq!(aside.len(), 1);
        assert_eq!(fs::read_to_string(&aside[0]).unwrap(), "{not json");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_file_counts_as_damaged() {
        let d = tmpdir("empty");
        let p = d.join("store.json");
        fs::write(&p, "").unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        assert_eq!(s, Store::default());
        assert_eq!(corrupt_files(&d).len(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn invalid_utf8_is_recovered_and_moved_aside() {
        let d = tmpdir("utf8");
        let p = d.join("store.json");
        let bad: &[u8] = b"{\"last_page\":\"\xff\xfe\"}";
        fs::write(&p, bad).unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)), "{outcome:?}");
        assert!(outcome.message().unwrap().contains("UTF-8"));
        assert_eq!(s, Store::default());
        assert!(!p.exists());
        let aside = corrupt_files(&d);
        assert_eq!(aside.len(), 1);
        assert_eq!(fs::read(&aside[0]).unwrap(), bad);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn backup_is_used_when_main_file_is_corrupt() {
        let d = tmpdir("usebak");
        let p = d.join("store.json");
        save_json_atomic(&p, &sample(1)).unwrap();
        save_json_atomic(&p, &sample(2)).unwrap();
        fs::write(&p, "{\"tasks\": [trunc").unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)), "{outcome:?}");
        assert!(outcome.writable());
        assert!(outcome.message().unwrap().contains("restored from backup"));
        assert_eq!(s, sample(1));
        assert!(!p.exists());
        assert_eq!(corrupt_files(&d).len(), 1);
        // The backup stays until a new main file exists to replace it.
        save_json_atomic(&p, &s).unwrap();
        let (bak, _): (Store, _) = load_json(&backup_path(&p));
        assert_eq!(bak, sample(1));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn damaged_backup_falls_back_to_default() {
        let d = tmpdir("badbak");
        let p = d.join("settings.json");
        fs::write(&p, "nope").unwrap();
        fs::write(backup_path(&p), "also nope").unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        assert_eq!(s, Store::default());
        assert!(backup_path(&p).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unique_aside_names_never_overwrite() {
        let d = tmpdir("unique");
        let p = d.join("store.json");
        fs::write(d.join("store.corrupt-123.json"), "earliest").unwrap();
        fs::write(&p, "second").unwrap();
        let a = set_aside(&p, 123).unwrap();
        assert_eq!(a, d.join("store.corrupt-123-1.json"));
        fs::write(&p, "third").unwrap();
        let b = set_aside(&p, 123).unwrap();
        assert_eq!(b, d.join("store.corrupt-123-2.json"));
        assert_eq!(fs::read_to_string(d.join("store.corrupt-123.json")).unwrap(), "earliest");
        assert_eq!(fs::read_to_string(&a).unwrap(), "second");
        assert_eq!(fs::read_to_string(&b).unwrap(), "third");
        assert!(!p.exists());

        // Two damaged loads in a row keep both copies.
        fs::write(&p, "fourth {").unwrap();
        let _: (Store, _) = load_json(&p);
        fs::write(&p, "fifth {").unwrap();
        let _: (Store, _) = load_json(&p);
        let contents: Vec<String> = corrupt_files(&d).iter().map(|f| fs::read_to_string(f).unwrap()).collect();
        assert_eq!(contents.len(), 5);
        assert!(contents.contains(&"fourth {".to_string()));
        assert!(contents.contains(&"fifth {".to_string()));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unreadable_is_left_in_place_and_not_writable() {
        let d = tmpdir("unreadable");
        // A folder where the file should be: reading fails, but not with
        // NotFound or InvalidData.
        let p = d.join("store.json");
        fs::create_dir_all(&p).unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert!(matches!(outcome, LoadOutcome::Unreadable(_)), "{outcome:?}");
        assert!(!outcome.writable());
        assert!(outcome.message().is_some());
        assert_eq!(s, Store::default());
        assert!(p.is_dir());
        assert!(corrupt_files(&d).is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn save_creates_backup_of_previous_version() {
        let d = tmpdir("mkbak");
        let p = d.join("store.json");
        save_json_atomic(&p, &sample(1)).unwrap();
        assert!(!backup_path(&p).exists(), "nothing to back up on the first save");
        save_json_atomic(&p, &sample(2)).unwrap();
        assert_eq!(backup_path(&p), d.join("store.bak.json"));
        let (bak, outcome): (Store, _) = load_json(&backup_path(&p));
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(bak, sample(1));
        let (cur, _): (Store, _) = load_json(&p);
        assert_eq!(cur, sample(2));
        // No temp files are left behind.
        assert_eq!(names(&d), vec!["store.bak.json".to_string(), "store.json".to_string()]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn write_without_backup() {
        let d = tmpdir("nobak");
        let p = d.join("cache").join("abc.json");
        write_json_atomic(&p, &sample(1)).unwrap();
        write_json_atomic(&p, &sample(2)).unwrap();
        assert_eq!(names(&d.join("cache")), vec!["abc.json".to_string()]);
        let (cur, _): (Store, _) = load_json(&p);
        assert_eq!(cur, sample(2));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn parse_json_tolerates_bom() {
        let s: Store = parse_json("\u{feff}{\"last_page\":\"water\"}").unwrap();
        assert_eq!(s.last_page, "water");
        assert!(parse_json::<Store>("{").is_err());
        let d = tmpdir("bom");
        let p = d.join("settings.json");
        fs::write(&p, "\u{feff}{\"last_page\":\"tasks\"}").unwrap();
        let (s, outcome): (Store, _) = load_json(&p);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(s.last_page, "tasks");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn outcome_helpers() {
        assert!(LoadOutcome::Loaded.writable());
        assert!(LoadOutcome::Missing.writable());
        assert!(LoadOutcome::Recovered("r".into()).writable());
        assert!(!LoadOutcome::Unreadable("u".into()).writable());
        assert_eq!(LoadOutcome::Recovered("r".into()).message(), Some("r"));
        assert_eq!(LoadOutcome::Unreadable("u".into()).message(), Some("u"));
        assert_eq!(LoadOutcome::Loaded.message(), None);
    }

    #[test]
    fn sidecar_names() {
        let p = Path::new("dir").join("settings.json");
        assert_eq!(backup_path(&p), Path::new("dir").join("settings.bak.json"));
        assert_eq!(tmp_path(&p), Path::new("dir").join("settings.json.tmp"));
        assert_eq!(sibling(&p, ".corrupt-5.json"), Path::new("dir").join("settings.corrupt-5.json"));
    }

    #[test]
    fn a_recorder_that_never_wrote_is_not_one_that_reads_record_links() {
        let old: Store = serde_json::from_str(r#"{"version":1,"last_page":"music"}"#).unwrap();
        assert!(!old.recorder_island, "files from before the Record page: not yet");
        let mut s = Store::default();
        s.recorder_island = true;
        let back: Store = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.recorder_island);
    }
}
