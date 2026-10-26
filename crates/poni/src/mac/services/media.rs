//! What plays on the Mac, from two sources.
//!
//! * **macOS's own Now Playing**, the player Control Center shows: Spotify,
//!   Music, YouTube or any other video in Safari, Chrome and other browsers,
//!   Podcasts, VLC, and every app that reports there. Since macOS 15.4 it
//!   answers only Apple's own programs, so it's read through MediaRemote
//!   Adapter (`third_party/mediaremote-adapter`, BSD-3-Clause): macOS's own
//!   `/usr/bin/perl` runs the adapter's script, which loads the adapter's
//!   small framework from this app's bundle and prints each change as a line
//!   of JSON (`poni_core::mediaremote`). That one process runs only while
//!   something plays through the Mac's output and for `ADAPTER_IDLE` after
//!   (Core Audio says when sound starts and stops: `audio::on_running`),
//!   asleep until macOS announces a change (it doesn't poll), and
//!   `now-playing.pl` stops it when the app quits or crashes. The island's
//!   buttons go the same way, one short run of the script per press. None of
//!   it asks for permission.
//! * **Music's and Spotify's own announcements**, distributed notifications
//!   that need no permission (`com.apple.Music.playerInfo` and
//!   `com.spotify.client.PlaybackStateChanged`), with each app's AppleScript
//!   for the exact position, the album art and the buttons, only while that
//!   app runs (a script never starts it). This was all the Mac had before; it
//!   stays for when Now Playing can't be read (a macOS that closes the
//!   adapter's way in, a build without the adapter). macOS holds such
//!   notifications back from apps that aren't in front, so the observer asks
//!   for immediate delivery. The scripts run only while Now Playing hasn't
//!   reported a track, so a working Now Playing never brings up macOS's
//!   "Oyenza Studio wants to control Spotify".
//!
//! When both know a track, Now Playing's wins. A worker thread owns the
//! state, reads the adapter's lines, runs the scripts (which can take a
//! moment), and reports `AppEvent::Media`, `MediaTimeline` and `MediaArt` like
//! the Windows media service.

use crate::commands::MediaCmd;
use crate::events::{AppEvent, EventSink};
use crate::state::MediaSession;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSNotificationSuspensionBehavior, NSNumber, NSObject, NSObjectProtocol, NSString};
use poni_core::mediaremote::{NowPlaying, NowPlayingStream};
use poni_core::timeline::Timeline;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// While a player that only its script can read plays, re-read the position
/// this often (seeks don't always announce themselves).
const RESYNC: Duration = Duration::from_secs(10);
/// Longest a script, or one of the adapter's quick commands, may run before
/// it is stopped.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(4);
/// After macOS refused the scripts ("not allowed to control Music"), try
/// again only this often.
const DENIED_RETRY: Duration = Duration::from_secs(60);
/// How long Now Playing gets to report a track before a script asks Music or
/// Spotify itself: the adapter's stream answers well within it, even when it
/// has just started with the sound.
const NOW_PLAYING_FIRST: Duration = Duration::from_secs(3);
/// A burst of changes (a new track's title, then its artist and art) comes
/// from the adapter as one line after this much quiet.
const ADAPTER_DEBOUNCE_MS: u32 = 100;
/// Once nothing plays through the Mac's output, the adapter's stream keeps
/// running this long (a paused song keeps its buttons), then rests until
/// sound starts again.
const ADAPTER_IDLE: Duration = Duration::from_secs(180);
/// The adapter's stream ended by itself (not with an error): start it again
/// after this, at most `ADAPTER_RESTARTS` times.
const ADAPTER_RETRY: Duration = Duration::from_secs(10);
const ADAPTER_RESTARTS: u32 = 3;
/// macOS's own Perl, which macOS lets read Now Playing.
const PERL: &str = "/usr/bin/perl";
/// Album art is decoded to this many pixels square.
const ART_PX: u32 = 240;
/// Separates the fields a script returns (ASCII "record separator").
const SEP: char = '\u{1e}';
const USER_AGENT: &str = "Poni/0.1 (+https://github.com/lucacadalora/oyenza-studio)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Player {
    Music,
    Spotify,
}

impl Player {
    const ALL: [Player; 2] = [Player::Music, Player::Spotify];

    fn bundle_id(self) -> &'static str {
        match self {
            Player::Music => "com.apple.Music",
            Player::Spotify => "com.spotify.client",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Player::Music => "Music",
            Player::Spotify => "Spotify",
        }
    }

    fn index(self) -> usize {
        match self {
            Player::Music => 0,
            Player::Spotify => 1,
        }
    }

    fn running(self) -> bool {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(self.bundle_id())).count() > 0
    }
}

/// Where the track the island shows comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    NowPlaying,
    Script(Player),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Playing,
    Paused,
    Stopped,
}

impl State {
    fn parse(s: &str) -> State {
        match s.trim().to_ascii_lowercase().as_str() {
            "playing" => State::Playing,
            "paused" => State::Paused,
            _ => State::Stopped,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Track {
    title: String,
    artist: String,
    album: String,
    duration_s: f64,
}

/// What a notification said.
#[derive(Clone, Debug)]
struct Notified {
    player: Player,
    state: State,
    track: Option<Track>,
    position_s: Option<f64>,
}

enum Msg {
    Notified(Notified),
    Command(MediaCmd),
    /// A line the adapter printed, from its `run`-th start.
    Line(u64, String),
    /// The adapter's output closed (it stopped).
    Ended(u64),
    /// Sound started (true) or stopped on the Mac's output.
    Output(bool),
    Stop,
}

static TX: Mutex<Option<Sender<Msg>>> = Mutex::new(None);

pub struct MediaService {
    tx: Sender<Msg>,
    _observer: Retained<NowPlayingObserver>,
}

impl MediaService {
    pub fn start(mtm: MainThreadMarker, sink: EventSink) -> MediaService {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut g) = TX.lock() {
            *g = Some(tx.clone());
        }
        let observer: Retained<NowPlayingObserver> = unsafe { msg_send![NowPlayingObserver::alloc(mtm), init] };
        let center = NSDistributedNotificationCenter::defaultCenter();
        for (name, sel) in [("com.apple.Music.playerInfo", sel!(musicChanged:)), ("com.spotify.client.PlaybackStateChanged", sel!(spotifyChanged:))] {
            let obs: &AnyObject = observer.as_ref();
            unsafe {
                center.addObserver_selector_name_object_suspensionBehavior(obs, sel, Some(&NSString::from_str(name)), None, NSNotificationSuspensionBehavior::DeliverImmediately)
            };
        }
        let worker_tx = tx.clone();
        if let Err(e) = std::thread::Builder::new().name("poni-media".into()).spawn(move || Worker::new(sink, worker_tx).run(rx)) {
            pwarn!("media: could not start its thread ({e})");
        }
        MediaService { tx, _observer: observer }
    }

    pub fn command(&self, c: MediaCmd) {
        let _ = self.tx.send(Msg::Command(c));
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

/// Sound started or stopped on the Mac's output (`audio::on_running`, on
/// Core Audio's thread).
pub fn output_running_changed(running: bool) {
    if let Some(tx) = TX.lock().ok().and_then(|g| g.clone()) {
        let _ = tx.send(Msg::Output(running));
    }
}

define_class!(
    /// Receives the players' distributed notifications (main thread).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PoniNowPlayingObserver"]
    struct NowPlayingObserver;

    unsafe impl NSObjectProtocol for NowPlayingObserver {}

    impl NowPlayingObserver {
        #[unsafe(method(musicChanged:))]
        fn music_changed(&self, n: &NSNotification) {
            forward(Player::Music, n);
        }

        #[unsafe(method(spotifyChanged:))]
        fn spotify_changed(&self, n: &NSNotification) {
            forward(Player::Spotify, n);
        }
    }
);

/// Read a notification's fields and hand them to the worker.
fn forward(player: Player, n: &NSNotification) {
    let Some(info) = n.userInfo() else { return };
    let text = |k: &str| {
        info.objectForKey(NSString::from_str(k).as_ref() as &AnyObject).and_then(|v| v.downcast::<NSString>().ok()).map(|s| s.to_string()).unwrap_or_default()
    };
    let num = |k: &str| info.objectForKey(NSString::from_str(k).as_ref() as &AnyObject).and_then(|v| v.downcast::<NSNumber>().ok()).map(|n| n.doubleValue());
    let state = State::parse(&text("Player State"));
    let title = text("Name");
    let track = (!title.is_empty()).then(|| {
        // Music says "Total Time", Spotify "Duration"; both in milliseconds.
        let ms = num("Total Time").or_else(|| num("Duration")).unwrap_or(0.0);
        Track { title, artist: text("Artist"), album: text("Album"), duration_s: (ms / 1000.0).max(0.0) }
    });
    // Spotify says where it is ("Playback Position", seconds); Music doesn't.
    let position_s = num("Playback Position");
    let msg = Msg::Notified(Notified { player, state, track, position_s });
    if let Some(tx) = TX.lock().ok().and_then(|g| g.clone()) {
        let _ = tx.send(msg);
    }
}

// ---------------------------------------------------------------- the adapter

/// The adapter's files in the app: `now-playing.pl` and the adapter's script
/// in `Contents/Resources`, its framework in `Contents/Frameworks`.
#[derive(Clone)]
struct AdapterFiles {
    keeper: PathBuf,
    script: PathBuf,
    framework: PathBuf,
}

impl AdapterFiles {
    /// Next to `Contents/MacOS/poni`; `None` for a binary outside the app (a
    /// build run from `target/`). The adapter takes absolute paths only.
    fn find() -> Option<AdapterFiles> {
        let exe = std::env::current_exe().ok()?;
        let contents = exe.parent()?.parent()?;
        let f = AdapterFiles {
            keeper: contents.join("Resources/now-playing.pl"),
            script: contents.join("Resources/mediaremote-adapter.pl"),
            framework: contents.join("Frameworks/MediaRemoteAdapter.framework"),
        };
        (contents.is_absolute() && f.keeper.is_file() && f.script.is_file() && f.framework.is_dir()).then_some(f)
    }

    /// One of the adapter's quick commands (`send 2`, `seek 61000000`).
    fn run(&self, args: &[String]) -> Result<String, String> {
        let mut cmd = Command::new(PERL);
        cmd.arg(&self.script).arg(&self.framework).args(args);
        run(cmd, SCRIPT_TIMEOUT)
    }
}

/// The adapter's stream while it runs.
struct Adapter {
    child: Child,
    /// Held open and never written: once it closes (this app quit or
    /// crashed), `now-playing.pl` ends the stream.
    stdin: Option<ChildStdin>,
    /// Which start this is, so lines from an earlier one are dropped.
    run: u64,
}

// ---------------------------------------------------------------- the worker

#[derive(Clone, Debug)]
struct PlayerNow {
    state: State,
    track: Option<Track>,
    timeline: Timeline,
    /// When this player last changed (the newest playing one wins).
    changed: Instant,
}

impl Default for PlayerNow {
    fn default() -> Self {
        PlayerNow { state: State::Stopped, track: None, timeline: Timeline::new(0.0, 0.0, 0.0, false), changed: Instant::now() }
    }
}

/// What the island shows, whichever source it came from.
struct Showing {
    source: Source,
    app_id: String,
    app_name: String,
    track: Track,
    timeline: Timeline,
}

struct Worker {
    sink: EventSink,
    /// For the adapter's reader thread.
    tx: Sender<Msg>,
    files: Option<AdapterFiles>,
    adapter: Option<Adapter>,
    adapter_runs: u64,
    adapter_restarts: u32,
    adapter_restart_at: Option<Instant>,
    /// The adapter can't work here (it said so, or kept stopping): the scripts
    /// carry on alone.
    adapter_gave_up: bool,
    /// Something plays through the Mac's output.
    output_running: bool,
    /// When the adapter's stream rests, nothing having played since.
    adapter_idle_at: Option<Instant>,
    now_playing: NowPlayingStream,
    /// Now Playing has reported a track since the adapter started, so the
    /// scripts rest.
    now_playing_works: bool,
    /// What Now Playing said when its timeline was last sent.
    sent_now_playing: Option<NowPlaying>,
    /// Apps' names by bundle id ("Google Chrome").
    names: HashMap<String, String>,
    started: Instant,
    /// The scripts have had their first look at what already plays.
    first_look_done: bool,
    /// Script reads waiting for Now Playing to answer first.
    read_due: [Option<Instant>; 2],
    /// When each player was last read by script.
    read_at: [Option<Instant>; 2],
    /// Album art to get by script for this track, once Now Playing has had
    /// its moment to answer.
    art_due: Option<(String, Player, Instant)>,
    players: [PlayerNow; 2],
    /// The player the scripts' side would show.
    current: Option<Player>,
    /// What was last sent: where it came from, the session's track key, and
    /// its timeline.
    shown: Option<Source>,
    sent_key: Option<String>,
    sent_timeline: Option<Timeline>,
    /// When macOS last refused a player's scripts.
    denied_at: [Option<Instant>; 2],
}

impl Worker {
    fn new(sink: EventSink, tx: Sender<Msg>) -> Worker {
        Worker {
            sink,
            tx,
            files: AdapterFiles::find(),
            adapter: None,
            adapter_runs: 0,
            adapter_restarts: 0,
            adapter_restart_at: None,
            adapter_gave_up: false,
            output_running: false,
            adapter_idle_at: None,
            now_playing: NowPlayingStream::default(),
            now_playing_works: false,
            sent_now_playing: None,
            names: HashMap::new(),
            started: Instant::now(),
            first_look_done: false,
            read_due: [None, None],
            read_at: [None, None],
            art_due: None,
            players: [PlayerNow::default(), PlayerNow::default()],
            current: None,
            shown: None,
            sent_key: None,
            sent_timeline: None,
            denied_at: [None, None],
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        if self.files.is_none() {
            pinfo!("media: this build has no Now Playing adapter: Music and Spotify only");
        }
        self.output_running = super::audio::output_running();
        if self.output_running {
            self.start_adapter();
        }
        if !self.now_playing_possible() {
            self.first_look();
        }
        self.publish();
        loop {
            let wait = self.next_due().map_or(Duration::from_secs(3600), |t| t.saturating_duration_since(Instant::now()));
            match rx.recv_timeout(wait) {
                Ok(Msg::Notified(n)) => self.on_notified(n),
                Ok(Msg::Command(c)) => self.on_command(c),
                Ok(Msg::Line(run, line)) => self.on_line(run, &line),
                Ok(Msg::Ended(run)) => self.on_adapter_ended(run),
                Ok(Msg::Output(running)) => self.on_output(running),
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.run_due();
            self.publish();
        }
        self.stop_adapter();
    }

    /// The next thing that is due: the scripts' first look, a waiting read,
    /// re-reading a playing player's position, starting the adapter again.
    fn next_due(&self) -> Option<Instant> {
        let mut due: Vec<Instant> = self.read_due.iter().flatten().copied().collect();
        if !self.first_look_done {
            due.push(self.started + NOW_PLAYING_FIRST);
        }
        due.extend(self.adapter_restart_at);
        due.extend(self.adapter_idle_at);
        due.extend(self.art_due.as_ref().map(|a| a.2));
        if let Some(p) = self.resync_player() {
            due.push(self.resync_at(p));
        }
        due.into_iter().min()
    }

    fn run_due(&mut self) {
        let now = Instant::now();
        if !self.first_look_done && now >= self.started + NOW_PLAYING_FIRST {
            self.first_look();
        }
        for p in Player::ALL {
            if self.read_due[p.index()].is_some_and(|t| now >= t) {
                self.read_due[p.index()] = None;
                if !self.now_playing_works {
                    self.read(p);
                }
            }
        }
        if self.adapter_restart_at.is_some_and(|t| now >= t) {
            self.adapter_restart_at = None;
            if self.output_running {
                self.start_adapter();
            }
        }
        if self.adapter_idle_at.is_some_and(|t| now >= t) {
            self.adapter_idle_at = None;
            if !self.output_running && self.adapter.is_some() {
                pinfo!("media: nothing has played for {} min: Now Playing rests until something does", ADAPTER_IDLE.as_secs() / 60);
                self.stop_adapter();
                self.now_playing.clear();
            }
        }
        if let Some((key, p, _)) = self.art_due.take_if(|a| now >= a.2) {
            if self.shown == Some(Source::Script(p)) && self.sent_key.as_deref() == Some(key.as_str()) {
                let art = self.art(p);
                self.sink.send(AppEvent::MediaArt { track_key: key, art });
            }
        }
        if let Some(p) = self.resync_player() {
            if now >= self.resync_at(p) {
                self.read(p);
            }
        }
    }

    /// When `p`'s position is next re-read: `RESYNC` after its last read, or
    /// after the announcement that made it show.
    fn resync_at(&self, p: Player) -> Instant {
        self.read_at[p.index()].unwrap_or(self.players[p.index()].changed) + RESYNC
    }

    /// The player whose position is re-read now and then: the one the island
    /// shows from the scripts' side while it plays.
    fn resync_player(&self) -> Option<Player> {
        match self.shown {
            Some(Source::Script(p)) if !self.now_playing_works && self.players[p.index()].state == State::Playing => Some(p),
            _ => None,
        }
    }

    /// The scripts' first look at whatever Music or Spotify already plays:
    /// not when Now Playing has answered, nor when nothing plays through the
    /// speakers (a paused player isn't shown, and a script would only bring up
    /// macOS's permission prompt).
    fn first_look(&mut self) {
        self.first_look_done = true;
        if self.now_playing_works || !super::audio::output_running() {
            return;
        }
        for p in Player::ALL {
            if p.running() {
                self.read(p);
            }
        }
    }

    // ------------------------------------------------------------ Now Playing

    /// Whether Now Playing can report what plays, now or as soon as sound
    /// starts (the scripts give it a moment first).
    fn now_playing_possible(&self) -> bool {
        self.files.is_some() && !self.adapter_gave_up
    }

    /// Sound started or stopped on the Mac's output: the adapter's stream
    /// starts with it, and rests `ADAPTER_IDLE` after it stops.
    fn on_output(&mut self, running: bool) {
        self.output_running = running;
        if running {
            self.adapter_idle_at = None;
            if self.adapter.is_none() && self.adapter_restart_at.is_none() && self.now_playing_possible() {
                self.start_adapter();
            }
        } else if self.adapter.is_some() && self.adapter_idle_at.is_none() {
            self.adapter_idle_at = Some(Instant::now() + ADAPTER_IDLE);
        }
    }

    fn start_adapter(&mut self) {
        let Some(f) = self.files.clone() else { return };
        // The adapter's error lines go to a log of their own, next to poni.log.
        let log = super::super::paths::log_path().with_file_name("now-playing.log");
        let stderr = std::fs::File::create(&log).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
        let mut cmd = Command::new(PERL);
        cmd.arg(&f.keeper).arg(&f.script).arg(&f.framework).args(["stream", "--micros"]).arg(format!("--debounce={ADAPTER_DEBOUNCE_MS}"));
        let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr).spawn() {
            Ok(c) => c,
            Err(e) => {
                pwarn!("media: could not start the Now Playing adapter ({e}): Music and Spotify only");
                return;
            }
        };
        self.adapter_runs += 1;
        let run = self.adapter_runs;
        if let Some(out) = child.stdout.take() {
            let tx = self.tx.clone();
            let reader = std::thread::Builder::new().name("poni-now-playing".into()).spawn(move || {
                let mut r = BufReader::new(out);
                let mut line = String::new();
                loop {
                    line.clear();
                    match r.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if tx.send(Msg::Line(run, std::mem::take(&mut line))).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send(Msg::Ended(run));
            });
            if let Err(e) = reader {
                pwarn!("media: could not read the Now Playing adapter ({e})");
            }
        }
        pinfo!("media: reading macOS's Now Playing (adapter pid {})", child.id());
        self.adapter = Some(Adapter { stdin: child.stdin.take(), child, run });
    }

    /// Stop the stream: closing its stdin has `now-playing.pl` end it.
    fn stop_adapter(&mut self) {
        let Some(mut a) = self.adapter.take() else { return };
        drop(a.stdin.take());
        if wait_exit(&mut a.child, Duration::from_secs(1)).is_none() {
            let _ = a.child.kill();
            let _ = a.child.wait();
        }
    }

    fn on_line(&mut self, run: u64, line: &str) {
        if self.adapter.as_ref().map(|a| a.run) != Some(run) || !self.now_playing.apply(line) {
            return;
        }
        if let (false, Some(n)) = (self.now_playing_works, self.now_playing.now()) {
            pinfo!("media: macOS's Now Playing works ({} plays)", n.app_id);
            self.now_playing_works = true;
            self.read_due = [None, None];
            self.art_due = None;
        }
    }

    fn on_adapter_ended(&mut self, run: u64) {
        let Some(mut a) = self.adapter.take_if(|a| a.run == run) else { return };
        drop(a.stdin.take());
        let status = wait_exit(&mut a.child, Duration::from_secs(2));
        if status.is_none() {
            let _ = a.child.kill();
            let _ = a.child.wait();
        }
        self.now_playing.clear();
        self.now_playing_works = false;
        // An exit code other than 0 is the adapter saying it can't work here
        // (its README: don't start it again). Anything else (a crash, a stray
        // signal) gets a few more tries.
        match status.and_then(|s| s.code()) {
            Some(code) if code != 0 => {
                pwarn!("media: the Now Playing adapter stopped (exit code {code}): Music and Spotify only");
                self.adapter_gave_up = true;
            }
            _ if self.adapter_restarts < ADAPTER_RESTARTS => {
                self.adapter_restarts += 1;
                pwarn!("media: the Now Playing adapter stopped ({status:?}); starting it again in {} s", ADAPTER_RETRY.as_secs());
                self.adapter_restart_at = Some(Instant::now() + ADAPTER_RETRY);
            }
            _ => {
                pwarn!("media: the Now Playing adapter keeps stopping ({status:?}): Music and Spotify only");
                self.adapter_gave_up = true;
            }
        }
    }

    /// An app's name for the island ("Google Chrome", "Spotify").
    fn app_name(&mut self, id: &str) -> String {
        if let Some(n) = self.names.get(id) {
            return n.clone();
        }
        let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(id)).firstObject().and_then(|a| a.localizedName()).map(|s| s.to_string());
        let name = match running {
            Some(n) if !n.trim().is_empty() => n,
            // WebKit's player process, when macOS doesn't name Safari as its parent.
            _ if id.to_ascii_lowercase().starts_with("com.apple.webkit") => "Safari".to_string(),
            _ => id.rsplit('.').next().unwrap_or(id).to_string(),
        };
        self.names.insert(id.to_string(), name.clone());
        name
    }

    // ------------------------------------------------------------ Music and Spotify

    fn on_notified(&mut self, n: Notified) {
        let now = crate::clock::mono();
        let i = n.player.index();
        let new_track = n.track.as_ref().is_some_and(|t| self.players[i].track.as_ref() != Some(t));
        let p = &mut self.players[i];
        if let Some(t) = n.track {
            p.track = Some(t);
        }
        if n.state == State::Stopped {
            p.track = None;
        }
        p.state = n.state;
        p.changed = Instant::now();
        let duration = p.track.as_ref().map_or(0.0, |t| t.duration_s);
        let position = match n.position_s {
            Some(s) => s,
            None if new_track => 0.0,
            None => p.timeline.position_at(now),
        };
        p.timeline = Timeline::new(position, duration, now, n.state == State::Playing);
        // Music's notification has no position: ask for the exact one, unless
        // Now Playing has it (or will in a moment).
        if n.player == Player::Music && n.state != State::Stopped && n.position_s.is_none() && !self.now_playing_works {
            if self.now_playing_possible() {
                self.read_due[i].get_or_insert(Instant::now() + NOW_PLAYING_FIRST);
            } else {
                self.read(Player::Music);
            }
        }
    }

    fn on_command(&mut self, c: MediaCmd) {
        match self.shown {
            Some(Source::NowPlaying) => self.now_playing_command(&c),
            Some(Source::Script(p)) => self.script_command(p, &c),
            None => {}
        }
    }

    /// The island's buttons for what Now Playing shows, through the adapter
    /// (macOS's media keys do the same): its stream reports the result.
    fn now_playing_command(&mut self, c: &MediaCmd) {
        let Some(f) = self.files.clone() else { return };
        let args: Vec<String> = match c {
            MediaCmd::PlayPause => vec!["send".into(), "2".into()],
            MediaCmd::Next => vec!["send".into(), "4".into()],
            MediaCmd::Prev => vec!["send".into(), "5".into()],
            // In microseconds.
            MediaCmd::Seek(s) => vec!["seek".into(), format!("{}", (s.max(0.0) * 1e6).round() as u64)],
            MediaCmd::Prepare => return,
        };
        if let Err(e) = f.run(&args) {
            pwarn!("media: Now Playing didn't take {c:?} ({e})");
        }
    }

    fn script_command(&mut self, p: Player, c: &MediaCmd) {
        if !p.running() {
            return;
        }
        let verb = match c {
            MediaCmd::PlayPause => "playpause".to_string(),
            MediaCmd::Next => "next track".to_string(),
            MediaCmd::Prev => "previous track".to_string(),
            MediaCmd::Seek(s) => format!("set player position to {}", s.max(0.0)),
            MediaCmd::Prepare => return,
        };
        if let Err(e) = self.script(p, &format!("tell application id \"{}\" to {verb}", p.bundle_id())) {
            pwarn!("media: {} didn't take {c:?} ({e})", p.name());
            return;
        }
        // Spotify announces play, pause and track changes; a seek, and
        // Music's position, are read back.
        self.read(p);
    }

    /// Ask a running player for its state, track and position.
    fn read(&mut self, p: Player) {
        self.read_at[p.index()] = Some(Instant::now());
        let script = format!(
            "tell application id \"{id}\"\n\
             set s to player state as text\n\
             if s is \"stopped\" then return s\n\
             set t to current track\n\
             set r to character id 30\n\
             return s & r & (name of t) & r & (artist of t) & r & (album of t) & r & ((duration of t) as text) & r & ((player position) as text)\n\
             end tell",
            id = p.bundle_id()
        );
        let Ok(out) = self.script(p, &script) else { return };
        let now = crate::clock::mono();
        let f: Vec<&str> = out.trim_end_matches('\n').split(SEP).collect();
        let pl = &mut self.players[p.index()];
        pl.changed = Instant::now();
        pl.state = State::parse(f[0]);
        if pl.state == State::Stopped || f.len() < 6 {
            pl.track = None;
            pl.timeline = Timeline::new(0.0, 0.0, now, false);
            return;
        }
        // Music gives seconds; Spotify gives milliseconds for the duration.
        let mut duration = number(f[4]);
        if p == Player::Spotify {
            duration /= 1000.0;
        }
        pl.track = Some(Track { title: f[1].to_string(), artist: f[2].to_string(), album: f[3].to_string(), duration_s: duration.max(0.0) });
        pl.timeline = Timeline::new(number(f[5]).max(0.0), duration.max(0.0), now, pl.state == State::Playing);
    }

    /// Run an AppleScript for `p` (only while it runs, never starting it).
    fn script(&mut self, p: Player, script: &str) -> Result<String, String> {
        if !p.running() {
            return Err(format!("{} isn't running", p.name()));
        }
        if self.denied_at[p.index()].is_some_and(|t| t.elapsed() < DENIED_RETRY) {
            return Err("not allowed to control it".into());
        }
        match osascript(script) {
            Err(e) if e.contains("-1743") => {
                if self.denied_at[p.index()].replace(Instant::now()).is_none() {
                    pinfo!("media: macOS doesn't let Oyenza Studio control {} (System Settings > Privacy & Security > Automation)", p.name());
                }
                Err(e)
            }
            r => r,
        }
    }

    /// The player to show from the scripts' side: a playing one (the newest
    /// if both play), else the one shown so far while it's paused.
    fn pick(&self) -> Option<Player> {
        let playing = Player::ALL.into_iter().filter(|p| self.players[p.index()].state == State::Playing && self.players[p.index()].track.is_some());
        if let Some(p) = playing.max_by_key(|p| self.players[p.index()].changed) {
            return Some(p);
        }
        self.current.filter(|p| self.players[p.index()].track.is_some())
    }

    // ------------------------------------------------------------ to the app

    /// What the island should show: Now Playing's track, else Music's or
    /// Spotify's.
    fn showing(&mut self) -> Option<Showing> {
        self.current = self.pick();
        if let Some(n) = self.now_playing.now() {
            let app_name = self.app_name(&n.app_id);
            // Now Playing times its reports by the wall clock.
            let unix_now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
            let mut timeline = Timeline::new(n.position_at(unix_now), n.duration_s, crate::clock::mono(), n.playing);
            if n.rate > 0.0 {
                timeline.rate = n.rate;
            }
            let track = Track { title: n.title.clone(), artist: n.artist.clone(), album: n.album.clone(), duration_s: n.duration_s };
            return Some(Showing { source: Source::NowPlaying, app_id: n.app_id, app_name, track, timeline });
        }
        let p = self.current?;
        let now = &self.players[p.index()];
        let track = now.track.clone()?;
        Some(Showing { source: Source::Script(p), app_id: p.bundle_id().into(), app_name: p.name().into(), track, timeline: now.timeline })
    }

    /// Tell the app what changed since the last report.
    fn publish(&mut self) {
        let art_changed = self.now_playing.take_art_changed();
        let Some(s) = self.showing() else {
            self.shown = None;
            self.sent_now_playing = None;
            if self.sent_key.take().is_some() {
                self.sent_timeline = None;
                self.sink.send(AppEvent::Media(None));
            }
            return;
        };
        let key = MediaSession::make_track_key(&s.track.title, &s.track.artist, &s.track.album);
        let new_session = self.sent_key.as_deref() != Some(key.as_str()) || self.shown != Some(s.source);
        self.shown = Some(s.source);
        if new_session {
            let t = &s.track;
            let session = MediaSession {
                app_id: s.app_id,
                app_name: s.app_name,
                title: t.title.clone(),
                artist: t.artist.clone(),
                album: t.album.clone(),
                art: None,
                art_color: None,
                timeline: s.timeline,
                can_prev: true,
                can_next: true,
                can_play_pause: true,
                can_seek: true,
                track_key: key.clone(),
            };
            self.sink.send(AppEvent::Media(Some(session)));
            let same_track = self.sent_key.as_deref() == Some(key.as_str());
            self.sent_key = Some(key.clone());
            self.sent_timeline = Some(s.timeline);
            self.sent_now_playing = self.now_playing.now();
            self.art_due = None;
            let art = match s.source {
                Source::NowPlaying => self.now_playing.artwork().and_then(|b| super::thumbs::decode(&b, ART_PX)),
                // Now Playing may answer in a moment, with the art: the script,
                // which macOS may ask permission for, waits until then.
                Source::Script(p) if self.now_playing_possible() && !self.now_playing_works => {
                    self.art_due = Some((key.clone(), p, Instant::now() + NOW_PLAYING_FIRST));
                    None
                }
                Source::Script(p) => self.art(p),
            };
            // The same track from the other source keeps the art it has until
            // this one's comes.
            if art.is_some() || !same_track {
                self.sink.send(AppEvent::MediaArt { track_key: key, art });
            }
            return;
        }
        match s.source {
            Source::NowPlaying => {
                // A new timeline only when Now Playing reported one: the
                // position worked out from the clock moves on by itself.
                let n = self.now_playing.now();
                let moved = |a: &NowPlaying, b: &NowPlaying| (a.elapsed_s, a.at_unix_s, a.playing, a.rate, a.duration_s) != (b.elapsed_s, b.at_unix_s, b.playing, b.rate, b.duration_s);
                if let (Some(n), Some(sent)) = (n.as_ref(), self.sent_now_playing.as_ref()) {
                    if moved(n, sent) {
                        self.sink.send(AppEvent::MediaTimeline { track_key: key.clone(), timeline: s.timeline });
                        self.sent_timeline = Some(s.timeline);
                    }
                }
                self.sent_now_playing = n;
                if art_changed {
                    let art = self.now_playing.artwork().and_then(|b| super::thumbs::decode(&b, ART_PX));
                    self.sink.send(AppEvent::MediaArt { track_key: key, art });
                }
            }
            Source::Script(_) => {
                if self.sent_timeline != Some(s.timeline) {
                    self.sink.send(AppEvent::MediaTimeline { track_key: key, timeline: s.timeline });
                    self.sent_timeline = Some(s.timeline);
                }
            }
        }
    }

    /// The current track's album art from Music or Spotify, decoded and
    /// squared (by script, so not once Now Playing works).
    fn art(&mut self, p: Player) -> Option<crate::gfx::Image> {
        if self.now_playing_works {
            return None;
        }
        let id = p.bundle_id();
        let bytes = match p {
            // Raw picture data, printed by osascript as «data JPEG…» (hex).
            Player::Music => {
                let out = self.script(p, &format!("tell application id \"{id}\" to get raw data of artwork 1 of current track")).ok()?;
                hex_data(&out)?
            }
            Player::Spotify => {
                let url = self.script(p, &format!("tell application id \"{id}\" to get artwork url of current track")).ok()?;
                super::http::get_bytes(url.trim(), USER_AGENT, Duration::from_secs(6)).ok()?
            }
        };
        super::thumbs::decode(&bytes, ART_PX)
    }
}

/// A number AppleScript printed, with either decimal mark.
fn number(s: &str) -> f64 {
    s.trim().replace(',', ".").parse::<f64>().unwrap_or(0.0)
}

/// The bytes inside osascript's «data XXXX0123…» (4-letter type, then hex).
fn hex_data(out: &str) -> Option<Vec<u8>> {
    let start = out.find("«data ")? + "«data ".len();
    let rest = &out[start..];
    let end = rest.find('»')?;
    let hex = rest[..end].get(4..)?;
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect()
}

/// Wait up to `limit` for `child` to exit.
fn wait_exit(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(15)),
            _ => return None,
        }
    }
}

/// Run one AppleScript in its own `osascript` process.
fn osascript(script: &str) -> Result<String, String> {
    let mut cmd = Command::new("/usr/bin/osascript");
    cmd.arg("-e").arg(script);
    run(cmd, SCRIPT_TIMEOUT)
}

/// Run a program, stopping it after `limit`. Its output is read while it runs
/// (album art can be bigger than a pipe holds).
fn run(mut cmd: Command, limit: Duration) -> Result<String, String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{}: {e}", cmd.get_program().to_string_lossy()))?;
    let mut out = child.stdout.take().ok_or("no output")?;
    let mut err = child.stderr.take().ok_or("no error output")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let status = wait_exit(&mut child, limit);
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    match status {
        Some(st) if st.success() => Ok(stdout),
        Some(_) => Err(stderr.trim().to_string()),
        None => Err("it took too long".into()),
    }
}
