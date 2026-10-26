//! The MediaRemote adapter (third_party/mediaremote-adapter, BSD-3): how the
//! Mac reads macOS's own now-playing service, which every player reports to
//! (browsers too) and which since macOS 15.4 answers Apple's own programs
//! only. `/usr/bin/perl` is one of those: it loads the adapter's framework
//! and runs one of its commands, `stream` (a JSON line for every change
//! until it is stopped, `poni_core::mediaremote` reads them), `send` (play,
//! pause, next, previous) or `seek`.
//!
//! The app carries the framework in Contents/Frameworks and the script in
//! Contents/Resources (tools/make_mac_app.sh). Perl loads a copy from the
//! app's cache folder: a downloaded app's files are quarantined, and macOS
//! stops a quarantined library that isn't notarized when another program
//! (perl) loads it, while the files the app writes itself carry no
//! quarantine.

use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PERL: &str = "/usr/bin/perl";
const NAME: &str = "MediaRemoteAdapter";
const SCRIPT: &str = "mediaremote-adapter.pl";
/// A `send` or `seek` that takes longer than this is stopped.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
/// What a stream says on its error output goes to the log, this many lines
/// at most (it says something only when a read fails).
const ERR_LINES: usize = 20;

/// MediaRemote's commands, as `send` takes them.
pub const TOGGLE_PLAY_PAUSE: u8 = 2;
pub const NEXT_TRACK: u8 = 4;
pub const PREVIOUS_TRACK: u8 = 5;

pub struct Adapter {
    script: PathBuf,
    framework: PathBuf,
}

impl Adapter {
    /// The adapter the app carries, copied where perl loads it from. `None`
    /// in a build without it (`cargo run`), or when the copy failed.
    pub fn find() -> Option<Adapter> {
        let exe = std::env::current_exe().ok()?;
        let contents = exe.parent()?.parent()?;
        let lib = contents.join("Frameworks").join(format!("{NAME}.framework")).join(NAME);
        let script = contents.join("Resources").join(SCRIPT);
        if !lib.is_file() || !script.is_file() || !Path::new(PERL).is_file() {
            return None;
        }
        let dir = crate::paths::cache_dir().join("mediaremote");
        let framework = dir.join(format!("{NAME}.framework"));
        let copy = dir.join(SCRIPT);
        match write_if_changed(&lib, &framework.join(NAME)).and_then(|_| write_if_changed(&script, &copy)) {
            Ok(()) => Some(Adapter { script: copy, framework }),
            Err(e) => {
                pwarn!("media: could not ready macOS's now playing reader ({e})");
                None
            }
        }
    }

    fn command(&self) -> Command {
        let mut c = Command::new(PERL);
        c.arg(&self.script).arg(&self.framework).stdin(Stdio::null());
        c
    }

    /// Start `stream`: each line it prints goes to `line` (on a thread of
    /// its own), then `ended` once it stops, with what it said on its error
    /// output. The caller stops it by killing the child.
    pub fn stream(&self, line: impl Fn(String) + Send + 'static, ended: impl FnOnce(String) + Send + 'static) -> io::Result<Child> {
        // Microseconds with an epoch timestamp, and a tenth of a second to
        // gather a player's burst of changes (a new track's title, artist and
        // art) into one line.
        let mut child = self.command().args(["stream", "--micros", "--debounce=100"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        let out = child.stdout.take().ok_or_else(|| io::Error::other("no output"))?;
        let err = child.stderr.take().ok_or_else(|| io::Error::other("no error output"))?;
        let errors = std::thread::Builder::new().name("poni-nowplaying-err".into()).spawn(move || {
            let mut last = String::new();
            for (i, l) in BufReader::new(err).lines().map_while(Result::ok).enumerate() {
                if i < ERR_LINES {
                    pinfo!("media: now playing reader: {l}");
                }
                last = l;
            }
            last
        })?;
        std::thread::Builder::new().name("poni-nowplaying".into()).spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                line(l);
            }
            ended(errors.join().unwrap_or_default());
        })?;
        Ok(child)
    }

    /// `send <command>`: play/pause, next or previous in the player.
    pub fn send(&self, command: u8) -> Result<(), String> {
        self.run(&["send", &command.to_string()])
    }

    /// `seek <microseconds>`.
    pub fn seek(&self, s: f64) -> Result<(), String> {
        self.run(&["seek", &format!("{}", (s.max(0.0) * 1e6) as u64)])
    }

    fn run(&self, args: &[&str]) -> Result<(), String> {
        let mut child = self.command().args(args).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(st)) if st.success() => return Ok(()),
                Ok(Some(st)) => {
                    let mut why = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        let _ = e.read_to_string(&mut why);
                    }
                    return Err(format!("{st}: {}", why.trim()));
                }
                Ok(None) if started.elapsed() > COMMAND_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("it took too long".into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(15)),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// Put `from`'s bytes at `to` unless they are there already. Written anew
/// rather than copied with the file's attributes, so the copy has no
/// quarantine flag.
fn write_if_changed(from: &Path, to: &Path) -> io::Result<()> {
    let want = std::fs::read(from)?;
    if std::fs::read(to).is_ok_and(|have| have == want) {
        return Ok(());
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = to.with_extension("part");
    std::fs::write(&part, &want)?;
    std::fs::rename(&part, to)
}
