//! Antigravity's `agy`, run for the AI tab (`studio_usage::Cli`): the limits
//! are answered to Antigravity itself only, so its own `/usage` command is
//! how we ask. One run is a 190 MB program starting and asking Google: about
//! 2 seconds of CPU, which is why the worker runs it only while the AI tab is
//! on screen, at most every 15 minutes (`studio_usage::antigravity`).
//!
//! Only the two fixed command lines of [`Cli`] are ever run, from where
//! Antigravity's installer puts `agy`, and a run is held to what it needs:
//! - no input, below normal priority, in a process group of its own so a
//!   stop ends everything it started;
//! - at most 30 seconds (5 for `--version`), then it is stopped;
//! - at most 512 KB of what it prints is kept, and none of it is logged;
//! - its updater is switched off, so a probe never turns into an update;
//! - when it asks the person to sign in in the browser (it opens one and
//!   waits), it is stopped and the answer is 401, so the worker can hold it.

use studio_usage::{Cli, NetFail, Reply};
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Where `agy install` and the homebrew and manual installs put it.
const PLACES: [&str; 3] = [".local/bin/agy", "/opt/homebrew/bin/agy", "/usr/local/bin/agy"];
/// The most of its output that is kept.
const OUTPUT_MAX: u64 = 512 * 1024;
/// What it prints when it opens a browser to sign in.
const SIGN_IN: &str = "Authentication required. Please visit the URL";
/// How often a run is looked at while it works.
const POLL: Duration = Duration::from_millis(25);
/// After it ended, how long the rest of its output may take to arrive.
const DRAIN: Duration = Duration::from_millis(300);

const PRIO_PGRP: i32 = 1;
/// A nice value of 10: below normal, as the downloader runs yt-dlp.
const BELOW_NORMAL: i32 = 10;
const SIGKILL: i32 = 9;

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

/// Run `cli` and wait for it: 200 and what it printed when it exited 0, 401
/// when it asked for a browser sign-in, another status when it failed;
/// `NetFail::Timeout` when it did not finish in time, `NetFail::Other` when
/// it isn't installed or would not start. Blocks up to 30 seconds: call it on
/// the AI tab's worker thread, never the main one.
pub fn run(cli: Cli, home: &Path) -> Result<Reply, NetFail> {
    let Some(exe) = program(home) else { return Err(NetFail::Other("agy is not installed".into())) };
    let (args, limit) = command(cli);
    let started = Instant::now();
    let (out, into) = io::pipe().map_err(|_| NetFail::Other("no pipe".into()))?;
    let mut cmd = Command::new(exe);
    // stdout and stderr share one pipe: the answer is its JSON line, found by
    // the reader among any other lines.
    cmd.args(args)
        .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
        .current_dir(std::env::temp_dir())
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

    let deadline = started + limit;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => break None,
        }
    };
    if exited.is_none() {
        // Too slow (or waiting for a sign-in in the browser): end it and what it started.
        // SAFETY: as above.
        unsafe { killpg(group, SIGKILL) };
        let _ = child.wait();
    }
    let mut printed = Vec::new();
    while let Ok(chunk) = rx.recv_timeout(DRAIN) {
        printed.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&printed).into_owned();

    let (answer, what) = if text.contains(SIGN_IN) {
        (Ok(Reply { status: 401, body: String::new(), retry_after_s: None }), "asked to sign in".to_string())
    } else {
        match exited {
            None => (Err(NetFail::Timeout), "timed out".to_string()),
            Some(status) if status.success() => (Ok(Reply { status: 200, body: text, retry_after_s: None }), "ran".to_string()),
            Some(status) => (Ok(Reply { status: 500, body: String::new(), retry_after_s: None }), format!("exit {}", status.code().unwrap_or(-1))),
        }
    };
    pinfo!("agy: {cli:?}: {what} after {} ms", started.elapsed().as_millis());
    answer
}
