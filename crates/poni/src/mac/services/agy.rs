//! Antigravity's `agy`, run for the AI tab (`studio_usage::Cli`): the limits
//! are answered to Antigravity itself only, so its own `/usage` command is
//! how we ask. One run is a 190 MB program starting and asking Google: about
//! 2 seconds of CPU, which is why the worker runs it only while the AI tab is
//! on screen, at most every 15 minutes (`studio_usage::antigravity`), and
//! after every other service, so their cards don't wait for it.
//!
//! Only the two fixed command lines of [`Cli`] are ever run, from where
//! Antigravity's installer puts `agy`, and a run is held to what it needs:
//! - no input, below normal priority, in a process group of its own, working
//!   in a private folder of its own that goes when the run ends;
//! - it never opens a browser: when its saved sign-in has run out it would
//!   open one to sign in again, with the `open` (or `xdg-open`) it finds on
//!   its PATH or the program `BROWSER` names. That private folder comes first
//!   on its PATH with an `open` and an `xdg-open` of ours, and `BROWSER`
//!   names ours too: they only note that it asked, and fail. A run that asks
//!   (or says it needs a sign-in) is stopped at once and the answer is 401,
//!   so the worker holds it until Refresh;
//! - at most 30 seconds (5 for `--version`), then it is stopped;
//! - at most 512 KB of what it prints is kept, and none of it is logged;
//! - its updater is switched off, so a probe never turns into an update;
//! - whatever it leaves running in its process group when it ends is ended
//!   too: asked first, then made to.

use studio_usage::{Cli, NetFail, Reply};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Where `agy install` and the homebrew and manual installs put it.
const PLACES: [&str; 3] = [".local/bin/agy", "/opt/homebrew/bin/agy", "/usr/local/bin/agy"];
/// The most of its output that is kept.
const OUTPUT_MAX: u64 = 512 * 1024;
/// What it prints when it wants the person to sign in in a browser.
const SIGN_IN: &str = "Authentication required. Please visit the URL";
/// The browser openers `agy` looks for on its PATH (in print mode too);
/// ours stand in front of the real ones.
const OPENERS: [&str; 2] = ["open", "xdg-open"];
/// What our openers leave in the private folder when `agy` asked for a
/// browser.
const ASKED: &str = "asked-for-a-browser";
/// The PATH of an app started from the Dock, for when none is set.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// How often a run is looked at while it works.
const POLL: Duration = Duration::from_millis(25);
/// After it ended, how long the rest of its output may take to arrive.
const DRAIN: Duration = Duration::from_millis(300);
/// How long what it left running gets to end when asked, before it is made to.
const GRACE: Duration = Duration::from_millis(300);

const PRIO_PGRP: i32 = 1;
/// A nice value of 10: below normal, as the downloader runs yt-dlp.
const BELOW_NORMAL: i32 = 10;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;

extern "C" {
    fn killpg(pgrp: i32, sig: i32) -> i32;
    fn setpriority(which: i32, who: u32, prio: i32) -> i32;
}

/// The command line and how long it may take.
fn command(cli: Cli) -> (&'static [&'static str], Duration) {
    match cli {
        Cli::AgyVersion => (&["--version"], Duration::from_secs(5)),
        Cli::AgyUsage => (&["-p", "/usage", "--output-format", "json", "--print-timeout", "20s"], Duration::from_secs(30)),
    }
}

/// `agy`, where it is installed (`home` for the first place).
fn program(home: &Path) -> Option<PathBuf> {
    PLACES.iter().map(|p| if p.starts_with('/') { PathBuf::from(p) } else { home.join(p) }).find(|p| p.is_file())
}

/// One run's private folder (only this user can open it): `agy` works in
/// it, and our browser openers sit in it. It goes when the run ends.
struct Room {
    dir: PathBuf,
    /// The note our openers leave there.
    asked: PathBuf,
}

impl Room {
    /// The folder, with an `open` and an `xdg-open` that note the ask and
    /// fail. None if it can't be made: then `agy` isn't run, as it could open
    /// a browser.
    fn new() -> io::Result<Room> {
        static RUNS: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("oyenza-studio-agy-{}-{}", std::process::id(), RUNS.fetch_add(1, Ordering::Relaxed)));
        // One a run of an earlier app with the same number left behind goes first.
        let _ = fs::remove_dir_all(&dir);
        DirBuilder::new().mode(0o700).create(&dir)?;
        let room = Room { asked: dir.join(ASKED), dir };
        let note = format!("'{}'", room.asked.to_string_lossy().replace('\'', "'\\''"));
        let opener = format!("#!/bin/sh\n# Oyenza Studio: agy wanted a browser (to sign in again). None is opened.\n: > {note}\nexit 1\n");
        for name in OPENERS {
            OpenOptions::new().write(true).create_new(true).mode(0o700).open(room.dir.join(name))?.write_all(opener.as_bytes())?;
        }
        Ok(room)
    }

    /// `agy`'s PATH: ours first, then the app's own.
    fn path(&self) -> std::ffi::OsString {
        let mut path = self.dir.clone().into_os_string();
        path.push(":");
        path.push(std::env::var_os("PATH").filter(|p| !p.is_empty()).unwrap_or_else(|| SYSTEM_PATH.into()));
        path
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Whether what `agy` printed so far says it needs a sign-in.
fn wants_sign_in(printed: &[u8]) -> bool {
    printed.windows(SIGN_IN.len()).any(|w| w == SIGN_IN.as_bytes())
}

/// After `agy` ended by itself: whatever it left running in its process
/// group (a helper it started) is asked to end, then made to. Its group
/// lives on only through such leftovers; with none, nothing is sent. Returns
/// whether there were any.
fn end_leftovers(group: i32) -> bool {
    // SAFETY: plain calls on the process group of a child of ours.
    if unsafe { killpg(group, SIGTERM) } != 0 {
        return false;
    }
    let until = Instant::now() + GRACE;
    while Instant::now() < until {
        std::thread::sleep(POLL);
        // SAFETY: as above (signal 0 only asks whether any are left).
        if unsafe { killpg(group, 0) } != 0 {
            return true;
        }
    }
    // SAFETY: as above.
    unsafe { killpg(group, SIGKILL) };
    true
}

/// Run `cli` and wait for it: 200 and what it printed when it exited 0, 401
/// when it wanted a sign-in in the browser (none opens: it is stopped), another
/// status when it failed; `NetFail::Timeout` when it did not finish in time,
/// `NetFail::Other` when it isn't installed or would not start. Blocks up to
/// 30 seconds: call it on the AI tab's worker thread, never the main one.
pub fn run(cli: Cli, home: &Path) -> Result<Reply, NetFail> {
    let Some(exe) = program(home) else { return Err(NetFail::Other("agy is not installed".into())) };
    let (args, limit) = command(cli);
    let started = Instant::now();
    let room = Room::new().map_err(|_| NetFail::Other("no private folder to run agy in".into()))?;
    let (out, into) = io::pipe().map_err(|_| NetFail::Other("no pipe".into()))?;
    let mut cmd = Command::new(exe);
    // stdout and stderr share one pipe: the answer is its JSON line, found by
    // the reader among any other lines.
    cmd.args(args)
        .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
        .env("PATH", room.path())
        .env("BROWSER", room.dir.join(OPENERS[0]))
        .current_dir(&room.dir)
        .stdin(Stdio::null())
        .stdout(into.try_clone().map_err(|_| NetFail::Other("no pipe".into()))?)
        .stderr(into)
        .process_group(0);
    let mut child = cmd.spawn().map_err(|_| NetFail::Other("agy would not start".into()))?;
    // `cmd` held the pipe's writing end: with it gone, the output ends once
    // agy and what it started have closed theirs.
    drop(cmd);
    let group = child.id() as i32;
    // SAFETY: plain calls on the process group of a child of ours.
    unsafe { setpriority(PRIO_PGRP, group as u32, BELOW_NORMAL) };

    // Read on a thread of its own, so a long answer can't fill the pipe and
    // hold agy up meanwhile. Chunks arrive as they come, so what agy printed
    // is there even if something it started keeps the pipe open.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let reader = std::thread::Builder::new().name("poni-agy".into()).spawn(move || {
        let mut out = out.take(OUTPUT_MAX);
        let mut chunk = [0u8; 16 * 1024];
        while let Ok(n) = out.read(&mut chunk) {
            if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    if reader.is_err() {
        // SAFETY: as above.
        unsafe { killpg(group, SIGKILL) };
        let _ = child.wait();
        return Err(NetFail::Other("no thread to read with".into()));
    }

    // Wait for it to end, watching for a sign-in it wants (our opener's note,
    // or its words): then there is nothing to wait for.
    let deadline = started + limit;
    let mut printed = Vec::new();
    let (exited, asked) = loop {
        let mut more = false;
        while let Ok(chunk) = rx.try_recv() {
            printed.extend_from_slice(&chunk);
            more = true;
        }
        if room.asked.exists() || (more && wants_sign_in(&printed)) {
            break (None, true);
        }
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => break (None, false),
        }
    };
    let left = match exited {
        // Too slow, or waiting for a sign-in: end it and what it started.
        None => {
            // SAFETY: as above.
            unsafe { killpg(group, SIGKILL) };
            let _ = child.wait();
            false
        }
        Some(_) => end_leftovers(group),
    };
    while let Ok(chunk) = rx.recv_timeout(DRAIN) {
        printed.extend_from_slice(&chunk);
    }
    let asked = asked || room.asked.exists() || wants_sign_in(&printed);
    drop(room);

    let (answer, what) = if asked {
        (Ok(Reply { status: 401, body: String::new(), retry_after_s: None }), "wanted a sign-in; stopped, no browser opened".to_string())
    } else {
        match exited {
            None => (Err(NetFail::Timeout), "timed out".to_string()),
            Some(status) if status.success() => {
                (Ok(Reply { status: 200, body: String::from_utf8_lossy(&printed).into_owned(), retry_after_s: None }), "ran".to_string())
            }
            Some(status) => (Ok(Reply { status: 500, body: String::new(), retry_after_s: None }), format!("exit {}", status.code().unwrap_or(-1))),
        }
    };
    pinfo!("agy: {cli:?}: {what} after {} ms{}", started.elapsed().as_millis(), if left { "; ended what it left running" } else { "" });
    answer
}
