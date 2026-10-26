//! What plays on the Mac, for the island.
//!
//! * macOS's own now-playing service, which every player reports to: a
//!   browser's tab (YouTube, YouTube Music, Spotify on the web), the Spotify
//!   and Music apps, podcasts, video players. Since macOS 15.4 it answers
//!   Apple's own programs only, so the MediaRemote adapter asks it through
//!   `/usr/bin/perl` (mediaremote.rs). Its stream runs while there is
//!   something to hear or a track to show: Core Audio says when sound starts
//!   (audio.rs), and the stream stops [`LINGER`] after the sound has stopped
//!   and the service names no track any more. Its play/pause, next, previous
//!   and seek go through the adapter too.
//! * Until the service has named a track (an app without the adapter, or a
//!   macOS that refuses it), Music and Spotify themselves. Both announce every
//!   track change, play and pause as a distributed notification, which needs
//!   no permission: `com.apple.Music.playerInfo` and
//!   `com.spotify.client.PlaybackStateChanged`. macOS holds such
//!   notifications back from apps that aren't in front, so the observer asks
//!   for immediate delivery. The exact position, the album art and the
//!   buttons go through each app's own AppleScript, only while that app runs
//!   (a script never starts it); while the service may still answer, the
//!   scripts wait [`FIRST_WORD`] for it. The first time, macOS asks "Oyenza
//!   Studio wants to control Music".
//!
//! A worker thread owns the state, runs the scripts and the adapter's
//! commands (a process each, which can take a moment), and reports
//! `AppEvent::Media`, `MediaTimeline` and `MediaArt` like the Windows media
//! service.

use super::mediaremote::{Adapter, NEXT_TRACK, PREVIOUS_TRACK, TOGGLE_PLAY_PAUSE};
use crate::commands::MediaCmd;
use crate::events::{AppEvent, EventSink};
use crate::state::MediaSession;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSNotificationSuspensionBehavior, NSNumber, NSObject, NSObjectProtocol, NSString};
use poni_core::mediaremote;
use poni_core::timeline::Timeline;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
/// The now-playing stream keeps running this long after the sound stopped
/// with no track named, then rests until sound plays again.
const LINGER: Duration = Duration::from_secs(45);
/// While sound plays, macOS's now playing has this long to name the track
/// before Music's and Spotify's own scripts are asked (a person who never
/// needs the scripts is never asked to let them run).
const FIRST_WORD: Duration = Duration::from_secs(2);
/// A stream that stops by itself this soon didn't start; after `MAX_FAILS`
/// of those the app stops asking for the run.
const FAILED_START: Duration = Duration::from_secs(5);
const MAX_FAILS: u32 = 3;
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

/// What the island shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// macOS's own now playing, through the adapter.
    Service,
    Player(Player),
}

enum Msg {
    Notified(Notified),
    Command(MediaCmd),
    /// A line the now-playing stream printed, and which stream it was.
    Said(u32, String),
    /// That stream stopped, and the last thing it said on its error output.
    Ended(u32, String),
    /// Sound started or stopped on the default output (audio.rs).
    Sound(bool),
    /// Stop the stream now (the smoke run's fresh start).
    Rest,
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
        let own = tx.clone();
        if let Err(e) = std::thread::Builder::new().name("poni-media".into()).spawn(move || Worker::new(sink, own).run(rx)) {
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

/// Sound started (or stopped) on the default output: audio.rs's listener.
pub fn sound(on: bool) {
    if let Some(tx) = TX.lock().ok().and_then(|g| g.clone()) {
        let _ = tx.send(Msg::Sound(on));
    }
}

/// The smoke run's stand-in for sound: CI's Macs play none, and its test
/// player (the adapter's own) reports a track without any.
pub fn smoke_sound() {
    sound(true);
}

/// The smoke run stops the stream before its test player starts, so the
/// sound that follows starts a fresh one: the adapter follows a player
/// that has no app bundle (that test player, a command-line program) only
/// in what it reads as it starts, while an app's player comes in through
/// macOS's notifications at any time.
pub fn smoke_rest() {
    if let Some(tx) = TX.lock().ok().and_then(|g| g.clone()) {
        let _ = tx.send(Msg::Rest);
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
    /// The worker's own channel, for the now-playing stream's reader.
    tx: Sender<Msg>,
    players: [PlayerNow; 2],
    /// What the island shows.
    current: Option<Source>,
    /// What was last sent: the session's track key, its timeline, and for
    /// the service's track, which artwork (`mediaremote::Stream::artwork_sig`).
    sent_key: Option<String>,
    sent_timeline: Option<Timeline>,
    sent_art: u64,
    /// When macOS last refused a player's scripts.
    denied_at: [Option<Instant>; 2],
    /// When the players were last read (the one shown is read again after `RESYNC`).
    read_at: Instant,
    /// macOS's now playing: the adapter the app carries, and its stream
    /// while it runs (which one, the process, when it started).
    adapter: Option<Adapter>,
    stream: Option<(u32, Child, Instant)>,
    streams: u32,
    /// What the stream has said, and its track on the app's clock.
    said: mediaremote::Stream,
    said_timeline: Option<Timeline>,
    /// The service has named a track this run: it works on this Mac and
    /// speaks for Music and Spotify too (their scripts rest).
    service_works: bool,
    /// It named nothing while Music or Spotify played: their scripts no
    /// longer wait for it.
    service_silent: bool,
    /// Streams that stopped by themselves right after starting.
    fails: u32,
    /// Sound on the default output, and when it stopped.
    sound: bool,
    quiet_since: Option<Instant>,
    /// When Music and Spotify are read, if the service hasn't named the
    /// track by then (`FIRST_WORD`), and their art fetched with it.
    read_players_at: Option<Instant>,
    art_due: bool,
}

impl Worker {
    fn new(sink: EventSink, tx: Sender<Msg>) -> Worker {
        Worker {
            sink,
            tx,
            players: [PlayerNow::default(), PlayerNow::default()],
            current: None,
            sent_key: None,
            sent_timeline: None,
            sent_art: 0,
            denied_at: [None, None],
            read_at: Instant::now(),
            adapter: None,
            stream: None,
            streams: 0,
            said: mediaremote::Stream::default(),
            said_timeline: None,
            service_works: false,
            service_silent: false,
            fails: 0,
            sound: false,
            quiet_since: None,
            read_players_at: None,
            art_due: false,
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        self.adapter = Adapter::find();
        if self.adapter.is_none() {
            pinfo!("media: this build has no reader for macOS's now playing; Music and Spotify only");
        }
        // Whatever already plays when Poni starts: the service names it (a
        // paused track too); Music and Spotify are asked themselves where
        // there is no service to ask, or once it had its moment.
        self.sound = super::audio::sound_running();
        if !self.sound {
            self.quiet_since = Some(Instant::now());
        }
        if self.adapter.is_some() {
            self.listen();
            self.read_players_at = Some(Instant::now() + FIRST_WORD);
        } else {
            self.read_players();
        }
        self.publish();
        loop {
            match rx.recv_timeout(self.next_wake()) {
                Ok(Msg::Notified(n)) => self.on_notified(n),
                Ok(Msg::Command(c)) => self.on_command(c),
                Ok(Msg::Said(id, line)) => self.on_said(id, &line),
                Ok(Msg::Ended(id, why)) => self.on_ended(id, why),
                Ok(Msg::Sound(on)) => self.on_sound(on),
                Ok(Msg::Rest) => self.rest(),
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => self.on_wake(),
            }
            self.publish();
        }
        self.rest();
    }

    /// How long until something is due: the players' read once the service
    /// had its moment, the stream's rest, the shown player's resync.
    fn next_wake(&self) -> Duration {
        let resync = match self.current {
            Some(Source::Player(p)) if !self.service_works && self.players[p.index()].state == State::Playing => Some(self.read_at + RESYNC),
            _ => None,
        };
        [self.read_players_at, self.rest_at(), resync].into_iter().flatten().min().map_or(Duration::from_secs(3600), |t| t.saturating_duration_since(Instant::now()))
    }

    fn on_wake(&mut self) {
        let now = Instant::now();
        if self.read_players_at.is_some_and(|t| now >= t) {
            self.read_players_at = None;
            if !self.service_works {
                if self.stream.is_some() && !self.service_silent && self.players.iter().any(|p| p.state == State::Playing) {
                    self.service_silent = true;
                    pinfo!("media: macOS's now playing names nothing while Music or Spotify plays; asking them directly");
                }
                self.read_players();
                // Their art waited too, if one of them is to be shown.
                self.art_due = matches!(self.pick(), Some(Source::Player(_)));
            }
        }
        if self.rest_at().is_some_and(|t| now >= t) {
            self.rest();
        }
        if let Some(Source::Player(p)) = self.current.filter(|_| !self.service_works) {
            if self.players[p.index()].state == State::Playing && now >= self.read_at + RESYNC {
                self.read(p);
            }
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
        // A player starting says so before Core Audio hears it: macOS's now
        // playing gets its moment from here.
        if n.state == State::Playing {
            self.listen();
        }
        // Music's notification has no position: ask for the exact one.
        if n.player == Player::Music && n.state != State::Stopped && n.position_s.is_none() && !self.service_works {
            if self.may_hear_service() {
                self.read_players_at.get_or_insert_with(|| Instant::now() + FIRST_WORD);
            } else {
                self.read(Player::Music);
            }
        }
    }

    fn on_command(&mut self, c: MediaCmd) {
        match self.current {
            Some(Source::Service) => self.command_service(c),
            Some(Source::Player(p)) => self.command_player(p, c),
            None => {}
        }
    }

    /// Play/pause, next, previous or seek in the player macOS's now playing
    /// names. The stream says what changed.
    fn command_service(&mut self, c: MediaCmd) {
        let Some(a) = &self.adapter else { return };
        let done = match &c {
            MediaCmd::PlayPause => a.send(TOGGLE_PLAY_PAUSE),
            MediaCmd::Next => a.send(NEXT_TRACK),
            MediaCmd::Prev => a.send(PREVIOUS_TRACK),
            MediaCmd::Seek(s) => a.seek(*s),
            MediaCmd::Prepare => return,
        };
        if let Err(e) = done {
            pwarn!("media: macOS's now playing didn't take {c:?} ({e})");
        }
    }

    fn command_player(&mut self, p: Player, c: MediaCmd) {
        if !p.running() {
            return;
        }
        let verb = match &c {
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

    /// A line from the now-playing stream.
    fn on_said(&mut self, id: u32, line: &str) {
        if self.stream.as_ref().map(|s| s.0) != Some(id) || !self.said.apply(line) {
            return;
        }
        let playing = self.said.playing();
        self.said_timeline = playing.as_ref().map(service_timeline);
        if let (false, Some(p)) = (self.service_works, playing) {
            self.service_works = true;
            self.read_players_at = None;
            let who = if p.app_id.is_empty() { "a player" } else { p.app_id.as_str() };
            pinfo!("media: macOS's now playing answers ({who}); it speaks for Music and Spotify too");
        }
    }

    /// The stream stopped: by itself (a failed start counts against it), or
    /// because it was stopped (already let go of, so not this one).
    fn on_ended(&mut self, id: u32, why: String) {
        if self.stream.as_ref().map(|s| s.0) != Some(id) {
            return;
        }
        let Some((_, mut child, started)) = self.stream.take() else { return };
        let status = child.wait().map(|s| s.to_string()).unwrap_or_default();
        let ran = started.elapsed();
        self.said.clear();
        self.said_timeline = None;
        if ran < FAILED_START {
            self.fails += 1;
        }
        let why = if why.is_empty() { String::new() } else { format!(": {why}") };
        pwarn!("media: macOS's now playing reader stopped after {:.0} s ({status}{why})", ran.as_secs_f64());
        if self.fails >= MAX_FAILS {
            pwarn!("media: not asking macOS's now playing again this run; Music and Spotify only");
            self.service_works = false;
        } else if self.sound {
            self.listen();
        }
    }

    fn on_sound(&mut self, on: bool) {
        if on {
            self.quiet_since = None;
            self.listen();
        } else if self.sound || self.quiet_since.is_none() {
            self.quiet_since = Some(Instant::now());
        }
        self.sound = on;
    }

    /// Start the now-playing stream, unless it runs already or keeps failing.
    fn listen(&mut self) {
        if self.stream.is_some() || self.fails >= MAX_FAILS {
            return;
        }
        let Some(adapter) = &self.adapter else { return };
        self.streams = self.streams.wrapping_add(1);
        let id = self.streams;
        let (said, ended) = (self.tx.clone(), self.tx.clone());
        let line = move |l| {
            let _ = said.send(Msg::Said(id, l));
        };
        let end = move |why| {
            let _ = ended.send(Msg::Ended(id, why));
        };
        match adapter.stream(line, end) {
            Ok(child) => {
                pinfo!("media: listening to macOS's now playing");
                self.stream = Some((id, child, Instant::now()));
            }
            Err(e) => {
                pwarn!("media: macOS's now playing reader didn't start ({e}); Music and Spotify only");
                self.adapter = None;
            }
        }
    }

    /// Stop the stream: nothing to hear and no track named, or the app quits.
    fn rest(&mut self) {
        if let Some((_, mut child, _)) = self.stream.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.said.clear();
            self.said_timeline = None;
            pinfo!("media: macOS's now playing rests until sound plays");
        }
    }

    /// When the stream may rest: `LINGER` after the sound stopped (and after
    /// it started), while the service names no track (a paused one keeps it,
    /// so the island can still play it).
    fn rest_at(&self) -> Option<Instant> {
        let (_, _, started) = self.stream.as_ref()?;
        if self.sound || self.said.playing().is_some() {
            return None;
        }
        Some(self.quiet_since?.max(*started) + LINGER)
    }

    /// Whether macOS's now playing may yet name what plays: the app carries
    /// the adapter, it hasn't kept failing or stayed silent while Music or
    /// Spotify played, and its stream runs or sound is about to start it.
    fn may_hear_service(&self) -> bool {
        !self.service_works && !self.service_silent && self.adapter.is_some() && self.fails < MAX_FAILS && (self.stream.is_some() || self.sound)
    }

    fn read_players(&mut self) {
        for p in Player::ALL {
            if p.running() {
                self.read(p);
            }
        }
    }

    /// Ask a running player for its state, track and position.
    fn read(&mut self, p: Player) {
        self.read_at = Instant::now();
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

    /// What to show: the service's track once it works (it names what
    /// plays, whatever the player); before that a playing player (the
    /// newest if both play), else the one shown so far while it's paused.
    fn pick(&self) -> Option<Source> {
        if self.service_works {
            return self.said.playing().map(|_| Source::Service);
        }
        let playing = Player::ALL.into_iter().filter(|p| self.players[p.index()].state == State::Playing && self.players[p.index()].track.is_some());
        if let Some(p) = playing.max_by_key(|p| self.players[p.index()].changed) {
            return Some(Source::Player(p));
        }
        match self.current {
            Some(Source::Player(p)) if self.players[p.index()].track.is_some() => Some(Source::Player(p)),
            _ => None,
        }
    }

    /// The track `src` shows: its key and its timeline.
    fn track(&self, src: Source) -> Option<(String, Timeline)> {
        match src {
            Source::Service => {
                let p = self.said.playing()?;
                Some((MediaSession::make_track_key(&p.title, &p.artist, &p.album), self.said_timeline.unwrap_or_else(|| service_timeline(&p))))
            }
            Source::Player(pl) => {
                let now = &self.players[pl.index()];
                let t = now.track.as_ref()?;
                Some((MediaSession::make_track_key(&t.title, &t.artist, &t.album), now.timeline))
            }
        }
    }

    /// The session the app keeps for `src`'s track.
    fn session(&self, src: Source, track_key: String, timeline: Timeline) -> Option<MediaSession> {
        let (app_id, app_name, title, artist, album) = match src {
            Source::Service => {
                let p = self.said.playing()?;
                let name = app_name(&p.app_id, p.pid);
                (p.app_id, name, p.title, p.artist, p.album)
            }
            Source::Player(pl) => {
                let t = self.players[pl.index()].track.clone()?;
                (pl.bundle_id().to_string(), pl.name().to_string(), t.title, t.artist, t.album)
            }
        };
        Some(MediaSession {
            app_id,
            app_name,
            title,
            artist,
            album,
            art: None,
            art_color: None,
            timeline,
            can_prev: true,
            can_next: true,
            can_play_pause: true,
            can_seek: true,
            track_key,
        })
    }

    /// Tell the app what changed since the last report.
    fn publish(&mut self) {
        self.current = self.pick();
        let Some((src, (key, timeline))) = self.current.and_then(|s| Some((s, self.track(s)?))) else {
            if self.sent_key.take().is_some() {
                self.sent_timeline = None;
                self.sink.send(AppEvent::Media(None));
            }
            return;
        };
        if self.sent_key.as_deref() != Some(key.as_str()) {
            let Some(session) = self.session(src, key.clone(), timeline) else { return };
            self.sink.send(AppEvent::Media(Some(session)));
            self.sent_key = Some(key.clone());
            self.sent_timeline = Some(timeline);
            let art = match src {
                Source::Service => self.service_art(),
                // While the service may still name it, the art waits for it
                // too: it comes with the service's track, or at the read.
                Source::Player(_) if self.may_hear_service() && !self.art_due => {
                    self.read_players_at.get_or_insert_with(|| Instant::now() + FIRST_WORD);
                    None
                }
                Source::Player(p) => {
                    self.art_due = false;
                    self.art(p)
                }
            };
            self.sink.send(AppEvent::MediaArt { track_key: key, art });
            return;
        }
        if self.sent_timeline != Some(timeline) {
            self.sink.send(AppEvent::MediaTimeline { track_key: key.clone(), timeline });
            self.sent_timeline = Some(timeline);
        }
        // The service's artwork often comes a moment after its track; a
        // player's comes once the service had its moment.
        let art = match src {
            Source::Service if self.said.artwork_sig() != self.sent_art => Some(self.service_art()),
            Source::Player(p) if self.art_due => {
                self.art_due = false;
                Some(self.art(p))
            }
            _ => None,
        };
        if let Some(art) = art {
            self.sink.send(AppEvent::MediaArt { track_key: key, art });
        }
    }

    /// The service's artwork for its track, decoded and squared.
    fn service_art(&mut self) -> Option<crate::gfx::Image> {
        self.sent_art = self.said.artwork_sig();
        super::thumbs::decode(&self.said.artwork()?, ART_PX)
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

/// The service's track on the app's clock: where the player said it was,
/// when it said so.
fn service_timeline(p: &mediaremote::Playing) -> Timeline {
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let ago = p.at_unix_s.map_or(0.0, |at| (unix - at).max(0.0));
    let mut t = Timeline::new(p.elapsed_s, p.duration_s, crate::clock::mono() - ago, p.playing);
    t.rate = p.rate;
    t
}

/// What macOS calls the app with `bundle_id` (or the process `pid`):
/// "Google Chrome", "Safari", "Spotify".
fn app_name(bundle_id: &str, pid: Option<i64>) -> String {
    let by_id = (!bundle_id.is_empty()).then(|| NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id)).firstObject()).flatten();
    let app = by_id.or_else(|| pid.and_then(|p| NSRunningApplication::runningApplicationWithProcessIdentifier(p as i32)));
    app.and_then(|a| a.localizedName()).map(|n| n.to_string()).unwrap_or_default()
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
