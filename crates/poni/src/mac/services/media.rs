//! What Music and Spotify are playing, without macOS's own now-playing
//! service (since macOS 15.4 that answers signed apps only).
//!
//! * Both apps announce every track change, play and pause as a distributed
//!   notification, which needs no permission: `com.apple.Music.playerInfo`
//!   and `com.spotify.client.PlaybackStateChanged`. macOS holds such
//!   notifications back from apps that aren't in front, so the observer
//!   asks for immediate delivery.
//! * The exact position, the album art and the buttons (play/pause, next,
//!   previous, seek) go through each app's own AppleScript, only while that
//!   app runs (a script never starts it). The first time, macOS asks
//!   "Oyen Poni wants to control Music".
//!
//! A worker thread owns the state and runs the scripts (`osascript`, which
//! can take a moment), and reports `AppEvent::Media`, `MediaTimeline` and
//! `MediaArt` like the Windows media service. Browsers and other apps need
//! the signed build.

use crate::commands::MediaCmd;
use crate::events::{AppEvent, EventSink};
use crate::state::MediaSession;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSNotificationSuspensionBehavior, NSNumber, NSObject, NSObjectProtocol, NSString};
use poni_core::timeline::Timeline;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// While something plays, re-read the position this often (seeks don't
/// always announce themselves).
const RESYNC: Duration = Duration::from_secs(10);
/// Longest a script may run before it is stopped.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(4);
/// After macOS refused the scripts ("not allowed to control Music"), try
/// again only this often.
const DENIED_RETRY: Duration = Duration::from_secs(60);
/// Album art is decoded to this many pixels square.
const ART_PX: u32 = 240;
/// Separates the fields a script returns (ASCII "record separator").
const SEP: char = '\u{1e}';
const USER_AGENT: &str = "Poni/0.1 (+https://github.com/lucacadalora/poni)";

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
        if let Err(e) = std::thread::Builder::new().name("poni-media".into()).spawn(move || Worker::new(sink).run(rx)) {
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

struct Worker {
    sink: EventSink,
    players: [PlayerNow; 2],
    /// The player the island shows.
    current: Option<Player>,
    /// What was last sent: the session's track key, and its timeline.
    sent_key: Option<String>,
    sent_timeline: Option<Timeline>,
    /// When macOS last refused a player's scripts.
    denied_at: [Option<Instant>; 2],
}

impl Worker {
    fn new(sink: EventSink) -> Worker {
        Worker { sink, players: [PlayerNow::default(), PlayerNow::default()], current: None, sent_key: None, sent_timeline: None, denied_at: [None, None] }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        // Whatever already plays when Poni starts.
        for p in Player::ALL {
            if p.running() {
                self.read(p);
            }
        }
        self.publish();
        loop {
            let wait = if self.players.iter().any(|p| p.state == State::Playing) { RESYNC } else { Duration::from_secs(3600) };
            match rx.recv_timeout(wait) {
                Ok(Msg::Notified(n)) => self.on_notified(n),
                Ok(Msg::Command(c)) => self.on_command(c),
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(p) = self.current.filter(|p| self.players[p.index()].state == State::Playing) {
                        self.read(p);
                    }
                }
            }
            self.publish();
        }
    }

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
        // Music's notification has no position: ask for the exact one.
        if n.player == Player::Music && n.state != State::Stopped && n.position_s.is_none() {
            self.read(Player::Music);
        }
    }

    fn on_command(&mut self, c: MediaCmd) {
        let Some(p) = self.current else { return };
        if !p.running() {
            return;
        }
        let verb = match &c {
            MediaCmd::PlayPause => "playpause".to_string(),
            MediaCmd::Next => "next track".to_string(),
            MediaCmd::Prev => "previous track".to_string(),
            MediaCmd::Seek(s) => format!("set player position to {}", s.max(0.0)),
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
                    pinfo!("media: macOS doesn't let Oyen Poni control {} (System Settings > Privacy & Security > Automation)", p.name());
                }
                Err(e)
            }
            r => r,
        }
    }

    /// The player to show: a playing one (the newest if both play), else the
    /// one shown so far while it's paused.
    fn pick(&self) -> Option<Player> {
        let playing = Player::ALL.into_iter().filter(|p| self.players[p.index()].state == State::Playing && self.players[p.index()].track.is_some());
        if let Some(p) = playing.max_by_key(|p| self.players[p.index()].changed) {
            return Some(p);
        }
        self.current.filter(|p| self.players[p.index()].track.is_some())
    }

    /// Tell the app what changed since the last report.
    fn publish(&mut self) {
        self.current = self.pick();
        let Some(p) = self.current else {
            if self.sent_key.take().is_some() {
                self.sent_timeline = None;
                self.sink.send(AppEvent::Media(None));
            }
            return;
        };
        let now = &self.players[p.index()];
        let Some(t) = now.track.clone() else { return };
        let key = MediaSession::make_track_key(&t.title, &t.artist, &t.album);
        let timeline = now.timeline;
        if self.sent_key.as_deref() != Some(key.as_str()) {
            let session = MediaSession {
                app_id: p.bundle_id().into(),
                app_name: p.name().into(),
                title: t.title.clone(),
                artist: t.artist.clone(),
                album: t.album.clone(),
                art: None,
                art_color: None,
                timeline,
                can_prev: true,
                can_next: true,
                can_play_pause: true,
                can_seek: true,
                track_key: key.clone(),
            };
            self.sink.send(AppEvent::Media(Some(session)));
            self.sent_key = Some(key.clone());
            self.sent_timeline = Some(timeline);
            let art = self.art(p);
            self.sink.send(AppEvent::MediaArt { track_key: key, art });
        } else if self.sent_timeline != Some(timeline) {
            self.sink.send(AppEvent::MediaTimeline { track_key: key, timeline });
            self.sent_timeline = Some(timeline);
        }
    }

    /// The current track's album art, decoded and squared.
    fn art(&mut self, p: Player) -> Option<crate::gfx::Image> {
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

/// Run one AppleScript in its own `osascript` process, stopping it after
/// [`SCRIPT_TIMEOUT`]. Its output is read while it runs (album art can be
/// bigger than a pipe holds).
fn osascript(script: &str) -> Result<String, String> {
    let mut child = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("osascript: {e}"))?;
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
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if started.elapsed() > SCRIPT_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break None,
        }
    };
    let stdout = reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    match status {
        Some(st) if st.success() => Ok(stdout),
        Some(_) => Err(stderr.trim().to_string()),
        None => Err("the script took too long".into()),
    }
}
