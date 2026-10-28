//! Now-playing via Windows GSMTC (`Windows.Media.Control`).
//!
//! One dedicated MTA worker thread owns the session manager and every session
//! object. WinRT event handlers fire on thread-pool threads and only post a
//! [`Msg`] to that worker; the worker coalesces bursts into at most one refresh
//! per ~100 ms, reads the chosen session and tells the UI thread what changed:
//!
//! * `AppEvent::Media(Some/None)` when the session, track or controls change,
//! * `AppEvent::MediaTimeline` when only position / play state changed,
//! * `AppEvent::MediaArt` once the album art of a track is decoded.
//!
//! UI commands (play/pause, next, seek, …) travel over the same channel, so
//! the UI thread never waits on GSMTC. GSMTC talks to other processes and can
//! hang when a player misbehaves, so every async operation is awaited with a
//! timeout (and cancelled when it expires), and errors from a dead session make
//! the worker pick another one.
//!
//! When nothing changes the worker sleeps in a blocking `recv` (zero CPU).

use crate::commands::MediaCmd;
use crate::events::{AppEvent, EventSink};
use crate::services::applemusic_ui::AppleMusicUi;
use crate::state::MediaSession;
use poni_core::nowplaying::{self, Controls as Caps};
use poni_core::timeline::{PendingSeek, Timeline};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use windows::core::{Interface, IUnknown, RuntimeType, HSTRING, PCWSTR};
use windows::Foundation::{AsyncOperationCompletedHandler, AsyncStatus, EventRegistrationToken, IAsyncOperation, TypedEventHandler};
use windows::Media::Control::{
    CurrentSessionChangedEventArgs, GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackInfo as PlaybackInfo,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus, MediaPropertiesChangedEventArgs,
    PlaybackInfoChangedEventArgs, SessionsChangedEventArgs, TimelinePropertiesChangedEventArgs,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference, InputStreamOptions};
use windows::Win32::Foundation::{BOOL, E_ABORT, E_FAIL, FALSE, HWND, LPARAM, REGDB_E_CLASSNOTREG, TRUE};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible};

/// Wait this long after the first event of a burst before refreshing.
const COALESCE: Duration = Duration::from_millis(50);
/// Never refresh more often than this.
const MIN_INTERVAL: Duration = Duration::from_millis(100);
/// Upper bound for one GSMTC async call.
const OP_TIMEOUT: Duration = Duration::from_secs(2);
/// A browser's window title can change a moment after the tab's media info:
/// look for the site once more after this long.
const SITE_RECHECK: Duration = Duration::from_millis(1500);
/// Upper bound for creating the session manager (cold start can be slow).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_RETRY_MIN: Duration = Duration::from_secs(2);
const CONNECT_RETRY_MAX: Duration = Duration::from_secs(300);
/// After a command, re-send the real playback state once this much later, so
/// the UI's optimistic play/pause flip is corrected if the player ignored us.
const RESYNC_AFTER_CMD: Duration = Duration::from_millis(1200);
/// While a seek waits for the player to report it, look again this often.
const SEEK_RECHECK: Duration = Duration::from_millis(300);
/// Retries for a media-properties read that timed out (500 ms, 1 s, 2 s).
const PROPS_RETRIES: u32 = 3;
/// Album-art reads per track: the first one plus re-reads when the player
/// announces new properties for the same track (some send a stale thumbnail first).
const ART_READS_PER_TRACK: u8 = 3;
const ART_MAX_PX: u32 = 160;
const ART_MAX_BYTES: u64 = 16 << 20;
const TICKS_PER_S: f64 = 10_000_000.0;
/// Timeline snapshots older than this (or from the future) are not extrapolated.
const MAX_SNAPSHOT_AGE_TICKS: i64 = 3600 * 10_000_000;
/// Position drift (seconds) tolerated before a timeline update is sent.
const POSITION_TOLERANCE_S: f64 = 0.35;
/// Position jumps logged per track (how steady a player's reports are; the
/// lyrics clock smooths them, the log tells how much it had to).
const TIMELINE_LOGS_PER_TRACK: u32 = 12;

// ------------------------------------------------------------------ service

pub struct MediaService {
    tx: Sender<Msg>,
}

impl MediaService {
    /// Spawn the GSMTC worker. Returns immediately; connecting to the session
    /// manager happens on the worker (and is retried there if it fails).
    pub fn start(sink: EventSink) -> windows::core::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let worker_tx = tx.clone();
        std::thread::Builder::new()
            .name("poni-media".into())
            .spawn(move || worker_main(sink, worker_tx, rx))
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("media thread: {e}")))?;
        Ok(Self { tx })
    }

    /// Queue a transport command; never blocks.
    pub fn send(&self, cmd: MediaCmd) {
        let _ = self.tx.send(Msg::Cmd(cmd));
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        // The worker unsubscribes and exits on its own; it is never joined so
        // the UI thread cannot get stuck behind a hung GSMTC call.
        let _ = self.tx.send(Msg::Quit);
    }
}

#[derive(Clone, Debug)]
enum Msg {
    Cmd(MediaCmd),
    SessionsChanged,
    CurrentChanged,
    /// Session events carry our id for the session they were registered on.
    /// Property / timeline events from a session we no longer follow are
    /// ignored; playback events matter for every session (a player that
    /// starts playing can take over).
    Props(u64),
    Timeline(u64),
    Playback(u64),
    Quit,
}

fn worker_main(sink: EventSink, tx: Sender<Msg>, rx: Receiver<Msg>) {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() {
        crate::pwarn!("media: COM init failed ({hr:?}); now playing disabled");
        return;
    }
    {
        let mut w = Worker::new(sink, tx);
        w.run(&rx);
        w.shutdown();
    }
    unsafe { CoUninitialize() };
}

// ------------------------------------------------------------------ async helper

#[derive(Debug)]
enum OpErr {
    Timeout,
    Os(windows::core::Error),
}

impl From<windows::core::Error> for OpErr {
    fn from(e: windows::core::Error) -> Self {
        OpErr::Os(e)
    }
}

impl std::fmt::Display for OpErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpErr::Timeout => f.write_str("timed out"),
            OpErr::Os(e) => write!(f, "{e}"),
        }
    }
}

/// Wait for a WinRT async operation on the worker thread, giving up (and
/// cancelling it) after `timeout`. Completion is signalled by the operation's
/// Completed handler; if that cannot be attached, `Status()` is polled with
/// short, growing sleeps instead.
fn wait<T: RuntimeType + 'static>(op: &IAsyncOperation<T>, timeout: Duration) -> Result<T, OpErr> {
    if op.Status()? == AsyncStatus::Started {
        let (done_tx, done_rx) = mpsc::sync_channel::<()>(1);
        let on_done = AsyncOperationCompletedHandler::<T>::new(move |_, _| {
            let _ = done_tx.try_send(());
            Ok(())
        });
        if op.SetCompleted(&on_done).is_ok() {
            let _ = done_rx.recv_timeout(timeout);
        } else {
            let deadline = Instant::now() + timeout;
            let mut nap = Duration::from_millis(1);
            while matches!(op.Status(), Ok(AsyncStatus::Started)) && Instant::now() < deadline {
                std::thread::sleep(nap);
                nap = (nap * 2).min(Duration::from_millis(16));
            }
        }
    }
    match op.Status()? {
        AsyncStatus::Completed => Ok(op.GetResults()?),
        AsyncStatus::Error => Err(OpErr::Os(match op.ErrorCode() {
            Ok(hr) => windows::core::Error::from(hr),
            Err(e) => e,
        })),
        AsyncStatus::Canceled => Err(OpErr::Os(windows::core::Error::from(E_ABORT))),
        _ => {
            let _ = op.Cancel();
            Err(OpErr::Timeout)
        }
    }
}

fn run_bool(op: windows::core::Result<IAsyncOperation<bool>>) -> Result<bool, OpErr> {
    wait(&op?, OP_TIMEOUT)
}

/// windows-rs reports a null object as an "empty" error with code S_OK.
fn is_null(e: &windows::core::Error) -> bool {
    e.code().is_ok()
}

fn opt<T>(r: windows::core::Result<T>) -> windows::core::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if is_null(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

fn handler<S: RuntimeType + 'static, A: RuntimeType + 'static>(tx: &Sender<Msg>, msg: Msg) -> TypedEventHandler<S, A> {
    let tx = tx.clone();
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(msg.clone());
        Ok(())
    })
}

/// COM identity of a session (stable for as long as we hold a reference).
fn identity(s: &Session) -> usize {
    s.cast::<IUnknown>().map(|u| u.as_raw() as usize).unwrap_or_else(|_| s.as_raw() as usize)
}

fn playback_status(s: &Session) -> PlaybackStatus {
    s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).unwrap_or(PlaybackStatus::Closed)
}

// ------------------------------------------------------------------ subscriptions

/// The session manager plus its two event registrations.
struct ManagerSub {
    mgr: Manager,
    current: Option<EventRegistrationToken>,
    sessions: Option<EventRegistrationToken>,
}

impl ManagerSub {
    fn new(mgr: Manager, tx: &Sender<Msg>) -> windows::core::Result<Self> {
        let h: TypedEventHandler<Manager, CurrentSessionChangedEventArgs> = handler(tx, Msg::CurrentChanged);
        let current = mgr.CurrentSessionChanged(&h)?;
        let h: TypedEventHandler<Manager, SessionsChangedEventArgs> = handler(tx, Msg::SessionsChanged);
        let sessions = match mgr.SessionsChanged(&h) {
            Ok(t) => t,
            Err(e) => {
                let _ = mgr.RemoveCurrentSessionChanged(current);
                return Err(e);
            }
        };
        Ok(Self { mgr, current: Some(current), sessions: Some(sessions) })
    }
}

impl Drop for ManagerSub {
    fn drop(&mut self) {
        if let Some(t) = self.current.take() {
            let _ = self.mgr.RemoveCurrentSessionChanged(t);
        }
        if let Some(t) = self.sessions.take() {
            let _ = self.mgr.RemoveSessionsChanged(t);
        }
    }
}

/// A session we know about. Every session reports playback changes (so a
/// player that starts playing can take over); the chosen one also reports
/// media-property and timeline changes.
struct Tracked {
    sid: u64,
    session: Session,
    identity: usize,
    aumid: String,
    /// A call on it failed; skipped until it reports a playback change or
    /// the session list changes.
    dead: bool,
    playback: Option<EventRegistrationToken>,
    props: Option<EventRegistrationToken>,
    timeline: Option<EventRegistrationToken>,
}

impl Tracked {
    fn follow(&mut self, tx: &Sender<Msg>) {
        if self.props.is_none() {
            let h: TypedEventHandler<Session, MediaPropertiesChangedEventArgs> = handler(tx, Msg::Props(self.sid));
            match self.session.MediaPropertiesChanged(&h) {
                Ok(t) => self.props = Some(t),
                Err(e) => crate::pwarn!("media: MediaPropertiesChanged subscribe failed ({e})"),
            }
        }
        if self.timeline.is_none() {
            let h: TypedEventHandler<Session, TimelinePropertiesChangedEventArgs> = handler(tx, Msg::Timeline(self.sid));
            match self.session.TimelinePropertiesChanged(&h) {
                Ok(t) => self.timeline = Some(t),
                Err(e) => crate::pwarn!("media: TimelinePropertiesChanged subscribe failed ({e})"),
            }
        }
    }

    fn unfollow(&mut self) {
        // Errors are expected when the player is gone; nothing left to undo then.
        if let Some(t) = self.props.take() {
            let _ = self.session.RemoveMediaPropertiesChanged(t);
        }
        if let Some(t) = self.timeline.take() {
            let _ = self.session.RemoveTimelinePropertiesChanged(t);
        }
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.unfollow();
        if let Some(t) = self.playback.take() {
            let _ = self.session.RemovePlaybackInfoChanged(t);
        }
    }
}

// ------------------------------------------------------------------ data

#[derive(Clone, Debug, Default, PartialEq)]
struct Meta {
    app_id: String,
    app_name: String,
    title: String,
    artist: String,
    album: String,
}

/// What the UI currently shows (the last thing we sent).
struct Shown {
    meta: Meta,
    caps: Caps,
    timeline: Timeline,
}

#[derive(Default)]
struct ArtState {
    track_key: String,
    /// Hash of the last thumbnail bytes we decoded (or failed to decode).
    hash: Option<u64>,
    reads: u8,
}

#[derive(Default)]
struct Props {
    title: String,
    artist: String,
    album: String,
    thumb: Option<IRandomAccessStreamReference>,
}

#[derive(Clone, Copy, Default)]
struct Dirty {
    /// Re-read the session list.
    sessions: bool,
    /// Re-read the chosen session's media properties.
    props: bool,
    /// Something changed; refresh.
    any: bool,
}

// ------------------------------------------------------------------ worker

struct Worker {
    sink: EventSink,
    tx: Sender<Msg>,
    // Declared before `mgr` so sessions are released first.
    sessions: Vec<Tracked>,
    mgr: Option<ManagerSub>,
    next_sid: u64,
    chosen: Option<u64>,

    // Chosen-session cache.
    meta: Option<Meta>,
    thumb: Option<IRandomAccessStreamReference>,
    start_ticks: i64,
    props_retries: u32,

    shown: Option<Shown>,
    art: ArtState,
    names: HashMap<String, String>,
    /// The browser track whose site was already looked for a second time.
    site_recheck: String,

    dirty: Dirty,
    force_timeline: bool,
    /// A seek the player hasn't reported yet: its older reports are held back.
    pending_seek: Option<PendingSeek>,
    /// The controls a player last reported, as logged (session id, controls).
    logged_caps: Option<(String, Caps)>,
    /// Apple Music's own slider and back button (it declines both from the
    /// media session).
    apple_ui: AppleMusicUi,
    /// Position jumps logged for the shown track so far.
    timeline_logs: u32,
    refresh_at: Option<Instant>,
    last_refresh: Option<Instant>,
    resync_at: Option<Instant>,
    connect_at: Option<Instant>,
    connect_backoff: Duration,
}

impl Worker {
    fn new(sink: EventSink, tx: Sender<Msg>) -> Self {
        Self {
            sink,
            tx,
            sessions: Vec::new(),
            mgr: None,
            next_sid: 1,
            chosen: None,
            meta: None,
            thumb: None,
            start_ticks: 0,
            props_retries: 0,
            shown: None,
            art: ArtState::default(),
            names: HashMap::new(),
            site_recheck: String::new(),
            dirty: Dirty::default(),
            force_timeline: false,
            pending_seek: None,
            logged_caps: None,
            apple_ui: AppleMusicUi::default(),
            timeline_logs: 0,
            refresh_at: None,
            last_refresh: None,
            resync_at: None,
            connect_at: None,
            connect_backoff: CONNECT_RETRY_MIN,
        }
    }

    fn run(&mut self, rx: &Receiver<Msg>) {
        self.connect();
        loop {
            let msg = match self.next_deadline() {
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
                Some(at) => {
                    let now = Instant::now();
                    if at <= now {
                        None
                    } else {
                        match rx.recv_timeout(at - now) {
                            Ok(m) => Some(m),
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                }
            };
            match msg {
                Some(Msg::Quit) => return,
                Some(m) => self.on_msg(m),
                None => self.on_timers(),
            }
        }
    }

    fn shutdown(&mut self) {
        self.sessions.clear();
        self.mgr = None;
    }

    fn next_deadline(&self) -> Option<Instant> {
        [self.refresh_at, self.resync_at, self.connect_at].into_iter().flatten().min()
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Cmd(c) => self.command(c),
            Msg::SessionsChanged => {
                self.dirty.sessions = true;
                self.poke();
            }
            Msg::CurrentChanged => self.poke(),
            Msg::Playback(sid) => {
                // A session that reports changes is alive again.
                if let Some(i) = self.index_of(sid) {
                    self.sessions[i].dead = false;
                }
                self.poke();
            }
            Msg::Props(sid) => {
                if self.chosen == Some(sid) {
                    self.dirty.props = true;
                    self.poke();
                }
            }
            Msg::Timeline(sid) => {
                if self.chosen == Some(sid) {
                    self.poke();
                }
            }
            Msg::Quit => {}
        }
    }

    fn on_timers(&mut self) {
        let now = Instant::now();
        if self.connect_at.is_some_and(|t| t <= now) {
            self.connect_at = None;
            self.connect();
        }
        if self.resync_at.is_some_and(|t| t <= now) {
            self.resync_at = None;
            self.force_timeline = true;
            self.poke();
        }
        if self.refresh_at.is_some_and(|t| t <= Instant::now()) {
            self.refresh_at = None;
            self.refresh();
        }
    }

    fn poke(&mut self) {
        self.dirty.any = true;
        self.schedule(COALESCE);
    }

    /// Ask for a refresh after `delay`, but never sooner than MIN_INTERVAL
    /// after the previous one. An earlier pending refresh is kept.
    fn schedule(&mut self, delay: Duration) {
        let mut at = Instant::now() + delay;
        if let Some(last) = self.last_refresh {
            at = at.max(last + MIN_INTERVAL);
        }
        self.refresh_at = Some(self.refresh_at.map_or(at, |t| t.min(at)));
    }

    // -------------------------------------------------------------- manager

    fn connect(&mut self) {
        let result = Manager::RequestAsync()
            .map_err(OpErr::from)
            .and_then(|op| wait(&op, CONNECT_TIMEOUT))
            .and_then(|mgr| ManagerSub::new(mgr, &self.tx).map_err(OpErr::from));
        match result {
            Ok(m) => {
                crate::pinfo!("media: watching Windows media sessions");
                self.mgr = Some(m);
                self.connect_backoff = CONNECT_RETRY_MIN;
                self.dirty.sessions = true;
                self.poke();
            }
            Err(OpErr::Os(e)) if e.code() == REGDB_E_CLASSNOTREG => {
                crate::pwarn!("media: Windows media sessions are not available on this system ({e})");
                self.show_nothing();
            }
            Err(e) => {
                crate::pwarn!("media: could not reach Windows media sessions ({e}); retrying in {}s", self.connect_backoff.as_secs());
                self.show_nothing();
                self.connect_at = Some(Instant::now() + self.connect_backoff);
                self.connect_backoff = (self.connect_backoff * 2).min(CONNECT_RETRY_MAX);
            }
        }
    }

    /// The manager itself failed: drop everything and reconnect shortly.
    fn manager_failed(&mut self, e: windows::core::Error) {
        crate::pwarn!("media: session manager failed ({e}); reconnecting");
        self.sessions.clear();
        self.mgr = None;
        self.chosen = None;
        self.reset_chosen_cache();
        self.connect_at = Some(Instant::now() + CONNECT_RETRY_MIN);
    }

    /// Re-read the session list, keeping subscriptions of sessions we already
    /// track. Returns false when the manager failed.
    fn reload_sessions(&mut self) -> bool {
        let Some(mgr) = self.mgr.as_ref().map(|m| m.mgr.clone()) else { return false };
        let list = mgr.GetSessions().and_then(|v| (0..v.Size()?).map(|i| v.GetAt(i)).collect::<windows::core::Result<Vec<_>>>());
        let list = match list {
            Ok(l) => l,
            Err(e) => {
                self.manager_failed(e);
                return false;
            }
        };
        let mut old = std::mem::take(&mut self.sessions);
        let mut next = Vec::with_capacity(list.len());
        for session in list {
            let id = identity(&session);
            if let Some(pos) = old.iter().position(|t| t.identity == id) {
                // Sessions changed: give a session that failed earlier another chance.
                let mut t = old.swap_remove(pos);
                t.dead = false;
                next.push(t);
                continue;
            }
            let aumid = session.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
            let sid = self.next_sid;
            self.next_sid += 1;
            let mut t = Tracked { sid, session, identity: id, aumid, dead: false, playback: None, props: None, timeline: None };
            let h: TypedEventHandler<Session, PlaybackInfoChangedEventArgs> = handler(&self.tx, Msg::Playback(sid));
            match t.session.PlaybackInfoChanged(&h) {
                Ok(tok) => t.playback = Some(tok),
                Err(e) => crate::pwarn!("media: PlaybackInfoChanged subscribe failed ({e})"),
            }
            let name = self.app_name(&t.aumid);
            crate::pinfo!("media: session appeared: {name} ({})", t.aumid);
            next.push(t);
        }
        for t in &old {
            let name = self.app_name(&t.aumid);
            crate::pinfo!("media: session closed: {name}");
        }
        // Dropping the leftovers removes their event handlers.
        drop(old);
        self.sessions = next;
        true
    }

    fn index_of(&self, sid: u64) -> Option<usize> {
        self.sessions.iter().position(|t| t.sid == sid)
    }

    fn find(&self, s: &Session) -> Option<usize> {
        let id = identity(s);
        if let Some(i) = self.sessions.iter().position(|t| t.identity == id) {
            return Some(i);
        }
        let aumid = s.SourceAppUserModelId().ok()?.to_string();
        self.sessions.iter().position(|t| t.aumid == aumid && !t.dead).or_else(|| self.sessions.iter().position(|t| t.aumid == aumid))
    }

    /// Pick the session to show. `None` with `self.mgr == None` means the
    /// manager failed while choosing.
    fn choose(&mut self, allow_reload: bool) -> Option<usize> {
        let mgr = self.mgr.as_ref()?.mgr.clone();
        let current = match opt(mgr.GetCurrentSession()) {
            Ok(c) => c,
            Err(e) => {
                self.manager_failed(e);
                return None;
            }
        };
        let mut cur = current.as_ref().and_then(|c| self.find(c));
        if current.is_some() && cur.is_none() && allow_reload {
            // The list is stale (SessionsChanged not delivered yet).
            if !self.reload_sessions() {
                return None;
            }
            cur = current.as_ref().and_then(|c| self.find(c));
        }
        let alive: Vec<bool> = self.sessions.iter().map(|t| !t.dead).collect();
        let playing: Vec<bool> =
            self.sessions.iter().map(|t| !t.dead && playback_status(&t.session) == PlaybackStatus::Playing).collect();
        let prev = self.chosen.and_then(|sid| self.index_of(sid));
        pick_session(cur, prev, &playing, &alive)
    }

    /// Make `idx` the followed session. Returns true when it changed.
    fn set_chosen(&mut self, idx: Option<usize>) -> bool {
        let sid = idx.and_then(|i| self.sessions.get(i)).map(|t| t.sid);
        if sid == self.chosen {
            return false;
        }
        if let Some(old) = self.chosen.and_then(|s| self.index_of(s)) {
            self.sessions[old].unfollow();
        }
        self.chosen = sid;
        self.reset_chosen_cache();
        if let Some(i) = idx {
            let tx = self.tx.clone();
            self.sessions[i].follow(&tx);
            let aumid = self.sessions[i].aumid.clone();
            let name = self.app_name(&aumid);
            crate::pinfo!("media: following {name}");
        }
        true
    }

    fn reset_chosen_cache(&mut self) {
        self.meta = None;
        self.thumb = None;
        self.start_ticks = 0;
        self.props_retries = 0;
    }

    fn app_name(&mut self, aumid: &str) -> String {
        if let Some(n) = self.names.get(aumid) {
            return n.clone();
        }
        let name = known_app_name(aumid)
            .map(str::to_string)
            .or_else(|| shell_display_name(aumid))
            .unwrap_or_else(|| fallback_app_name(aumid));
        self.names.insert(aumid.to_string(), name.clone());
        name
    }

    // -------------------------------------------------------------- refresh

    fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        if self.mgr.is_none() {
            // connect() pokes again once the manager is back.
            return;
        }
        let d = std::mem::take(&mut self.dirty);
        let force = std::mem::take(&mut self.force_timeline);
        if d.sessions && !self.reload_sessions() {
            return;
        }
        let mut allow_reload = !d.sessions;
        for _ in 0..3 {
            let idx = self.choose(allow_reload);
            allow_reload = false;
            if self.mgr.is_none() {
                return;
            }
            let changed = self.set_chosen(idx);
            let Some(i) = idx else {
                self.show_nothing();
                return;
            };
            match self.read_and_publish(i, d.props || changed, d.props, force) {
                Ok(()) => return,
                Err(e) => {
                    // The player crashed or disconnected. Windows drops its
                    // session shortly (SessionsChanged); until then skip it.
                    let aumid = self.sessions[i].aumid.clone();
                    let name = self.app_name(&aumid);
                    crate::pinfo!("media: {name} session stopped responding ({e}); picking another");
                    self.sessions[i].dead = true;
                }
            }
        }
        self.set_chosen(None);
        self.show_nothing();
    }

    /// Read the chosen session and send what changed. An `Err` means the
    /// session is unusable (player crashed / disconnected).
    fn read_and_publish(&mut self, i: usize, need_props: bool, explicit_props: bool, force: bool) -> windows::core::Result<()> {
        let session = self.sessions[i].session.clone();
        let mut fresh_props = false;
        if need_props || self.meta.is_none() {
            match read_props(&session) {
                Ok(p) => {
                    let app_id = self.sessions[i].aumid.clone();
                    let mut app_name = self.app_name(&app_id);
                    if nowplaying::is_browser(&app_id) {
                        if let Some(site) = browser_site(&p.title) {
                            app_name = site.to_string();
                        } else if self.site_recheck != p.title {
                            self.site_recheck = p.title.clone();
                            self.dirty.props = true;
                            self.dirty.any = true;
                            self.schedule(SITE_RECHECK);
                        }
                    }
                    let (artist, album) = nowplaying::artist_album(&app_id, p.artist, p.album);
                    self.meta = Some(Meta { app_id, app_name, title: p.title, artist, album });
                    self.thumb = p.thumb;
                    self.props_retries = 0;
                    fresh_props = true;
                }
                Err(OpErr::Timeout) => {
                    self.retry_props();
                    if self.meta.is_none() {
                        // Don't keep showing another app's track (its buttons
                        // would now control this one).
                        let aumid = &self.sessions[i].aumid;
                        if self.shown.as_ref().is_some_and(|s| &s.meta.app_id != aumid) {
                            self.show_nothing();
                        }
                        return Ok(());
                    }
                }
                Err(OpErr::Os(e)) => return Err(e),
            }
        }

        let info = session.GetPlaybackInfo()?;
        let playing = match info.PlaybackStatus()? {
            PlaybackStatus::Playing => true,
            // Briefly reported between tracks: keep the current state so the
            // UI doesn't flash paused.
            PlaybackStatus::Changing => self.shown.as_ref().is_some_and(|s| s.timeline.playing),
            _ => false,
        };
        let rate = opt(info.PlaybackRate()).ok().flatten().and_then(|r| r.Value().ok()).unwrap_or(1.0);
        let tl = session.GetTimelineProperties()?;
        let start = tl.StartTime()?.Duration;
        let end = tl.EndTime()?.Duration;
        let pos = tl.Position()?.Duration;
        let updated = tl.LastUpdatedTime().map(|t| t.UniversalTime).unwrap_or(0);
        let now = crate::clock::mono();
        let mut timeline = make_timeline(start, end, pos, updated, playing, rate, crate::clock::filetime_now(), now);
        // Before the controls are worked out: a blank length would also turn
        // seeking off and replace the whole card.
        let same_track = self.shown.as_ref().filter(|s| self.meta.as_ref() == Some(&s.meta));
        if let Some(filled) = same_track.map(|s| timeline.or_length_of(&s.timeline, now)).filter(|f| *f != timeline) {
            if self.timeline_logs < TIMELINE_LOGS_PER_TRACK {
                self.timeline_logs += 1;
                let app = self.meta.as_ref().map_or("", |m| m.app_name.as_str());
                crate::pinfo!("media: {app} reported no track length at {:.1} s; keeping {:.0} s", filled.position_at(now), filled.duration_s);
            }
            timeline = filled;
        }
        self.start_ticks = start;
        let reported = read_caps(&info);
        let aumid = &self.sessions[i].aumid;
        if self.logged_caps.as_ref().is_none_or(|(id, c)| id != aumid || *c != reported) {
            let on = |b: bool| if b { "on" } else { "off" };
            crate::pinfo!(
                "media: {} reports Previous {}, Next {}, play/pause {}, seeking {}",
                self.names.get(aumid).map_or(aumid.as_str(), String::as_str),
                on(reported.prev),
                on(reported.next),
                on(reported.play_pause),
                on(reported.seek)
            );
            self.logged_caps = Some((aumid.clone(), reported));
        }
        let caps = nowplaying::island_controls(aumid, reported, timeline.has_duration());

        self.publish(caps, timeline, fresh_props, explicit_props, force);
        Ok(())
    }

    fn retry_props(&mut self) {
        if self.props_retries < PROPS_RETRIES {
            let delay = Duration::from_millis(500) * (1 << self.props_retries);
            self.props_retries += 1;
            crate::pwarn!("media: player did not return track info in time; retrying in {} ms", delay.as_millis());
            self.dirty.props = true;
            self.dirty.any = true;
            self.schedule(delay);
        } else {
            crate::pwarn!("media: player keeps timing out; waiting for its next change");
        }
    }

    fn publish(&mut self, caps: Caps, timeline: Timeline, fresh_props: bool, explicit_props: bool, force: bool) {
        let Some(meta) = self.meta.clone() else { return };
        if meta.title.is_empty() {
            self.show_nothing();
            return;
        }
        let key = MediaSession::make_track_key(&meta.title, &meta.artist, &meta.album);
        if self.art.track_key != key {
            self.art = ArtState { track_key: key.clone(), ..Default::default() };
        }
        match self.shown.as_mut() {
            Some(s) if s.meta == meta && s.caps == caps => {
                let now = crate::clock::mono();
                if self.pending_seek.is_some_and(|p| p.holds(&timeline, now)) {
                    // After a seek the bar shows where it was dropped; a report
                    // from before the seek would pull it back for a moment.
                    self.schedule(SEEK_RECHECK);
                } else {
                    let settled = self.pending_seek.take();
                    if let Some(p) = settled.filter(|p| p.expired(now)) {
                        crate::pinfo!("media: {} did not move to {:.1} s (it reports {:.1} s)", meta.app_name, p.target_s(), timeline.position_at(now));
                    }
                    if s.timeline.playing && timeline.playing && self.timeline_logs < TIMELINE_LOGS_PER_TRACK {
                        let jump = timeline.position_at(now) - s.timeline.position_at(now);
                        if jump.abs() >= 0.05 {
                            self.timeline_logs += 1;
                            crate::pinfo!("media: {} position report moved {:+.0} ms at {:.1} s", meta.app_name, jump * 1000.0, timeline.position_at(now));
                        }
                    }
                    // Once a seek settles the UI gets the player's own timeline,
                    // even when it matches the one sent before the seek.
                    if force || settled.is_some() || timeline_differs(&s.timeline, &timeline, now) {
                        s.timeline = timeline;
                        self.sink.send(AppEvent::MediaTimeline { track_key: key.clone(), timeline });
                    }
                }
            }
            _ => {
                self.pending_seek = None;
                self.sink.send(AppEvent::Media(Some(MediaSession {
                    app_id: meta.app_id.clone(),
                    app_name: meta.app_name.clone(),
                    title: meta.title.clone(),
                    artist: meta.artist.clone(),
                    album: meta.album.clone(),
                    art: None,
                    art_color: None,
                    timeline,
                    can_prev: caps.prev,
                    can_next: caps.next,
                    can_play_pause: caps.play_pause,
                    can_seek: caps.seek,
                    track_key: key.clone(),
                })));
                self.shown = Some(Shown { meta, caps, timeline });
                self.timeline_logs = 0;
            }
        }
        if fresh_props {
            self.update_art(&key, explicit_props);
        }
    }

    fn show_nothing(&mut self) {
        if self.shown.take().is_some() {
            self.sink.send(AppEvent::Media(None));
        }
        self.art = ArtState::default();
        self.pending_seek = None;
    }

    /// Read + decode the thumbnail once per track; re-read (and only decode
    /// when the bytes differ) when the player announces new properties for the
    /// same track, since some players publish a stale thumbnail first.
    fn update_art(&mut self, key: &str, explicit_props: bool) {
        let Some(thumb) = self.thumb.clone() else { return };
        if self.art.reads >= ART_READS_PER_TRACK || (self.art.hash.is_some() && !explicit_props) {
            return;
        }
        self.art.reads += 1;
        let bytes = match read_stream_bytes(&thumb) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return,
            Err(e) => {
                crate::pwarn!("media: album art could not be read ({e})");
                return;
            }
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        let hash = h.finish();
        if self.art.hash == Some(hash) {
            return;
        }
        self.art.hash = Some(hash);
        match crate::services::imaging::decode_bytes(&bytes, ART_MAX_PX) {
            Ok(img) => self.sink.send(AppEvent::MediaArt { track_key: key.to_string(), art: Some(img) }),
            Err(e) => crate::pwarn!("media: album art could not be decoded ({e})"),
        }
    }

    // -------------------------------------------------------------- commands

    fn command(&mut self, cmd: MediaCmd) {
        let Some(i) = self.chosen.and_then(|sid| self.index_of(sid)) else { return };
        let session = self.sessions[i].session.clone();
        let apple = nowplaying::is_apple_music(&self.sessions[i].aumid);
        if cmd == MediaCmd::Prepare {
            if apple {
                self.apple_ui.prepare_seek();
            }
            return;
        }
        let result = match &cmd {
            MediaCmd::PlayPause => toggle_play_pause(&session),
            MediaCmd::Next => run_bool(session.TrySkipNextAsync()),
            MediaCmd::Prev if apple => self.apple_back(&session),
            MediaCmd::Prev => run_bool(session.TrySkipPreviousAsync()),
            MediaCmd::Prepare => return,
            MediaCmd::Seek(s) => {
                if !s.is_finite() {
                    return;
                }
                let duration = self.shown.as_ref().map_or(0.0, |sh| sh.timeline.duration_s);
                let secs = if duration > 0.0 { s.clamp(0.0, duration) } else { s.max(0.0) };
                let result = if apple { self.apple_seek(&session, secs, duration) } else { self.session_seek(&session, secs) };
                self.pending_seek = matches!(result, Ok(true)).then(|| PendingSeek::new(secs, crate::clock::mono()));
                result
            }
        };
        match result {
            Ok(true) => {}
            Ok(false) => crate::pinfo!("media: player declined {cmd:?}"),
            Err(OpErr::Timeout) => crate::pwarn!("media: {cmd:?} timed out"),
            Err(OpErr::Os(e)) => {
                crate::pwarn!("media: {cmd:?} failed ({e})");
                self.dirty.sessions = true;
                self.poke();
            }
        }
        // Players often don't report a refused or partial change; re-send the
        // real state shortly so the UI's optimistic update can't stick.
        self.resync_at = Some(Instant::now() + RESYNC_AFTER_CMD);
    }

    fn session_seek(&self, session: &Session, secs: f64) -> Result<bool, OpErr> {
        let ticks = ((secs * TICKS_PER_S).round() as i64).saturating_add(self.start_ticks);
        run_bool(session.TryChangePlaybackPositionAsync(ticks))
    }

    /// Apple Music declines seeks from the media session: move its own
    /// progress slider, and ask the session only when the slider can't be
    /// reached.
    fn apple_seek(&mut self, session: &Session, secs: f64, duration: f64) -> Result<bool, OpErr> {
        if duration > 0.0 {
            match self.apple_ui.seek(secs / duration) {
                Ok(()) => {
                    crate::pinfo!("media: moved Apple Music's own progress slider to {secs:.1} s");
                    return Ok(true);
                }
                Err(e) => crate::pinfo!("media: Apple Music's progress slider: {e}; asking its media session"),
            }
        }
        self.session_seek(session, secs)
    }

    /// Apple Music usually reports Previous as off: press its own back button
    /// (restart the song, or go back one), and ask the media session only when
    /// the button can't be reached.
    fn apple_back(&mut self, session: &Session) -> Result<bool, OpErr> {
        match self.apple_ui.skip_back() {
            Ok(()) => {
                crate::pinfo!("media: pressed Apple Music's own back button");
                Ok(true)
            }
            Err(e) => {
                crate::pinfo!("media: Apple Music's back button: {e}; asking its media session");
                run_bool(session.TrySkipPreviousAsync())
            }
        }
    }
}

fn toggle_play_pause(session: &Session) -> Result<bool, OpErr> {
    if run_bool(session.TryTogglePlayPauseAsync())? {
        return Ok(true);
    }
    // Some players only implement separate play and pause.
    if playback_status(session) == PlaybackStatus::Playing {
        run_bool(session.TryPauseAsync())
    } else {
        run_bool(session.TryPlayAsync())
    }
}

// ------------------------------------------------------------------ reading

fn clean(s: windows::core::Result<HSTRING>) -> String {
    s.map(|h| h.to_string().chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()).unwrap_or_default()
}

fn read_props(session: &Session) -> Result<Props, OpErr> {
    let op = session.TryGetMediaPropertiesAsync()?;
    let p = match wait(&op, OP_TIMEOUT) {
        Ok(p) => p,
        // No properties published (yet): treat as an empty track.
        Err(OpErr::Os(e)) if is_null(&e) => return Ok(Props::default()),
        Err(e) => return Err(e),
    };
    let mut artist = clean(p.Artist());
    if artist.is_empty() {
        artist = clean(p.AlbumArtist());
    }
    Ok(Props { title: clean(p.Title()), artist, album: clean(p.AlbumTitle()), thumb: opt(p.Thumbnail()).ok().flatten() })
}

fn read_caps(info: &PlaybackInfo) -> Caps {
    let Ok(c) = info.Controls() else { return Caps::default() };
    let f = |r: windows::core::Result<bool>| r.unwrap_or(false);
    Caps {
        prev: f(c.IsPreviousEnabled()),
        next: f(c.IsNextEnabled()),
        play_pause: f(c.IsPlayPauseToggleEnabled()) || f(c.IsPlayEnabled()) || f(c.IsPauseEnabled()),
        seek: f(c.IsPlaybackPositionEnabled()),
    }
}

/// Read a thumbnail stream to bytes with a DataReader.
fn read_stream_bytes(r: &IRandomAccessStreamReference) -> Result<Vec<u8>, OpErr> {
    let stream = wait(&r.OpenReadAsync()?, OP_TIMEOUT)?;
    let size = stream.Size().unwrap_or(0);
    let result = (|| -> Result<Vec<u8>, OpErr> {
        if size > ART_MAX_BYTES {
            return Err(OpErr::Os(windows::core::Error::new(E_FAIL, format!("thumbnail too large ({size} bytes)"))));
        }
        let reader = DataReader::CreateDataReader(&stream)?;
        let mut out = Vec::new();
        if size > 0 {
            let got = wait(&reader.LoadAsync(size as u32)?.cast::<IAsyncOperation<u32>>()?, OP_TIMEOUT)?;
            out.resize(got as usize, 0);
            reader.ReadBytes(&mut out)?;
        } else {
            // Unknown length: read in chunks until the stream ends.
            reader.SetInputStreamOptions(InputStreamOptions::Partial)?;
            loop {
                let got = wait(&reader.LoadAsync(64 * 1024)?.cast::<IAsyncOperation<u32>>()?, OP_TIMEOUT)?;
                if got == 0 {
                    break;
                }
                let at = out.len();
                out.resize(at + got as usize, 0);
                reader.ReadBytes(&mut out[at..])?;
                if out.len() as u64 > ART_MAX_BYTES {
                    return Err(OpErr::Os(windows::core::Error::new(E_FAIL, "thumbnail too large")));
                }
            }
        }
        Ok(out)
    })();
    // The reader (dropped above) only held a reference; close the stream itself.
    let _ = stream.Close();
    result
}

/// Display name of an installed app from the shell's Apps folder.
fn shell_display_name(aumid: &str) -> Option<String> {
    if aumid.is_empty() {
        return None;
    }
    let path = HSTRING::from(format!("shell:AppsFolder\\{aumid}"));
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None::<&IBindCtx>).ok()?;
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        if p.is_null() {
            return None;
        }
        let name = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(aumid))
    }
}

// ------------------------------------------------------------------ pure logic

/// The current session, unless it isn't playing while another one is (then
/// prefer the playing one, sticking with the previous pick when it plays).
/// Dead sessions are never picked.
fn pick_session(current: Option<usize>, previous: Option<usize>, playing: &[bool], alive: &[bool]) -> Option<usize> {
    let ok = |i: &usize| alive.get(*i).copied().unwrap_or(false);
    let is_playing = |i: &usize| playing.get(*i).copied().unwrap_or(false);
    let current = current.filter(ok);
    let previous = previous.filter(ok);
    if let Some(c) = current.filter(is_playing) {
        return Some(c);
    }
    if let Some(p) = previous.filter(is_playing) {
        return Some(p);
    }
    if let Some(i) = (0..playing.len()).find(|i| ok(i) && is_playing(i)) {
        return Some(i);
    }
    current.or(previous).or_else(|| (0..alive.len()).find(ok))
}

/// Convert a GSMTC timeline snapshot (100 ns ticks) into our monotonic clock.
#[allow(clippy::too_many_arguments)]
fn make_timeline(
    start: i64,
    end: i64,
    position: i64,
    last_updated_ft: i64,
    playing: bool,
    rate: f64,
    now_ft: i64,
    now_mono: f64,
) -> Timeline {
    let duration_s = if end > start { end.saturating_sub(start) as f64 / TICKS_PER_S } else { 0.0 };
    let mut position_s = position.saturating_sub(start).max(0) as f64 / TICKS_PER_S;
    if duration_s > 0.0 {
        position_s = position_s.min(duration_s);
    }
    let age_ticks = if last_updated_ft > 0 { now_ft.saturating_sub(last_updated_ft) } else { 0 };
    let age_s = if (0..=MAX_SNAPSHOT_AGE_TICKS).contains(&age_ticks) { age_ticks as f64 / TICKS_PER_S } else { 0.0 };
    let rate = if rate.is_finite() && rate > 0.0 { rate } else { 1.0 };
    Timeline { position_s, duration_s, sampled_at: now_mono - age_s, playing, rate }
}

/// Whether the UI needs a new timeline (drift-free extrapolations are skipped
/// so chatty players don't wake the UI).
fn timeline_differs(a: &Timeline, b: &Timeline, now: f64) -> bool {
    a.playing != b.playing
        || (a.rate - b.rate).abs() > 0.01
        || (a.duration_s - b.duration_s).abs() > 0.5
        || (a.position_at(now) - b.position_at(now)).abs() > POSITION_TOLERANCE_S
}

/// The site a browser tab is playing ("YouTube", "YouTube Music"), judged
/// from the titles of visible windows; `None` when no window shows the
/// track's title next to a known site (e.g. the tab isn't the front one).
fn browser_site(media_title: &str) -> Option<&'static str> {
    struct Search<'a> {
        media_title: &'a str,
        found: Option<&'static str>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lp: LPARAM) -> BOOL {
        let search = &mut *(lp.0 as *mut Search);
        if !IsWindowVisible(hwnd).as_bool() {
            return TRUE;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return TRUE;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        let title = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        match nowplaying::site_from_window_title(&title, search.media_title) {
            Some(site) => {
                search.found = Some(site);
                FALSE
            }
            None => TRUE,
        }
    }
    let mut search = Search { media_title, found: None };
    // Stopping early makes EnumWindows report an error; the answer is in `search`.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    search.found
}

/// YouTube Music installed as a browser web app.
const YT_MUSIC_CRX: &str = "cinhimbnkkaeohfgghhklpknlkffjgod";

/// Lower-case AUMID fragments → friendly names (first match wins).
const KNOWN_APPS: &[(&str, &str)] = &[
    (YT_MUSIC_CRX, "YouTube Music"),
    ("youtube-music", "YouTube Music"),
    ("youtubemusic", "YouTube Music"),
    ("ytmdesktop", "YouTube Music"),
    ("spotify", "Spotify"),
    ("applemusic", "Apple Music"),
    ("zunemusic", "Media Player"),
    ("vlc", "VLC"),
    ("msedge", "Edge"),
    ("microsoftedge", "Edge"),
    ("brave", "Brave"),
    ("opera", "Opera"),
    ("firefox", "Firefox"),
    ("chrome", "Chrome"),
];

fn known_app_name(aumid: &str) -> Option<&'static str> {
    let id = aumid.trim().to_ascii_lowercase();
    // Firefox registers a 16-hex-digit hash of its install path as its AUMID.
    if id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some("Firefox");
    }
    // Other browser web apps ("Chrome._crx_<id>"): let the shell name them.
    if id.contains("_crx_") && !id.contains(YT_MUSIC_CRX) {
        return None;
    }
    KNOWN_APPS.iter().find(|(k, _)| id.contains(k)).map(|(_, v)| *v)
}

/// Best-effort name from the AUMID itself: `C:\…\foo.exe` → "Foo",
/// `Publisher.AppName_8wekyb3d8bbwe!App` → "App Name".
fn fallback_app_name(aumid: &str) -> String {
    let id = aumid.trim();
    if let Some(p) = id.to_ascii_lowercase().find("_crx_") {
        if let Some(browser) = id.get(..p).and_then(known_app_name) {
            return browser.to_string();
        }
    }
    let mut s = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let packaged = s.contains('!');
    if let Some(p) = s.find('!') {
        s = &s[..p];
    }
    // Package family name: Name_<13-char publisher id>.
    if let Some((name, publisher)) = s.rsplit_once('_') {
        if publisher.len() == 13 && publisher.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            s = name;
        }
    }
    if s.len() >= 4 && s.get(s.len() - 4..).is_some_and(|ext| ext.eq_ignore_ascii_case(".exe")) {
        s = &s[..s.len() - 4];
    }
    // "Publisher.AppName" / reverse-DNS ids → the last segment.
    if let Some(last) = s.rsplit('.').next().filter(|x| !x.is_empty()) {
        s = last;
    }
    let mut name = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if packaged && prev_lower && c.is_uppercase() {
            name.push(' ');
        }
        prev_lower = c.is_lowercase();
        name.push(c);
    }
    if !name.chars().any(char::is_uppercase) {
        let mut cs = name.chars();
        if let Some(first) = cs.next() {
            name = first.to_uppercase().chain(cs).collect();
        }
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        "Media".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_players() {
        assert_eq!(known_app_name("Spotify.exe"), Some("Spotify"));
        assert_eq!(known_app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), Some("Spotify"));
        assert_eq!(known_app_name("Chrome"), Some("Chrome"));
        assert_eq!(known_app_name("MSEdge"), Some("Edge"));
        assert_eq!(known_app_name("308046B0AF4A39CB"), Some("Firefox"));
        assert_eq!(known_app_name("firefox.exe"), Some("Firefox"));
        assert_eq!(known_app_name("Brave"), Some("Brave"));
        assert_eq!(known_app_name("Opera"), Some("Opera"));
        assert_eq!(known_app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), Some("Media Player"));
        assert_eq!(known_app_name("VideoLAN.VLC"), Some("VLC"));
        assert_eq!(known_app_name("AppleInc.AppleMusicWin_nzyj5cx40ttqa!App"), Some("Apple Music"));
        assert_eq!(known_app_name("com.github.th-ch.youtube-music"), Some("YouTube Music"));
        assert_eq!(known_app_name("Chrome._crx_cinhimbnkkaeohfgghhklpknlkffjgod"), Some("YouTube Music"));
        assert_eq!(known_app_name("Chrome._crx_abcdefabcdefabcdefabcdefabcdefab"), None);
        assert_eq!(known_app_name("SomePlayer.exe"), None);
    }

    #[test]
    fn fallback_names() {
        assert_eq!(fallback_app_name("foobar2000.exe"), "Foobar2000");
        assert_eq!(fallback_app_name("MusicBee.EXE"), "MusicBee");
        assert_eq!(fallback_app_name(r"C:\Program Files\Tool\player.exe"), "Player");
        assert_eq!(fallback_app_name("Contoso.GrooveBox_1a2b3c4d5e6f7!App"), "Groove Box");
        assert_eq!(fallback_app_name("Chrome._crx_abcdefabcdefabcdefabcdefabcdefab"), "Chrome");
        assert_eq!(fallback_app_name("MSEdge._crx_abcdefabcdefabcdefabcdefabcdefab"), "Edge");
        assert_eq!(fallback_app_name("com.example.tunes"), "Tunes");
        assert_eq!(fallback_app_name(""), "Media");
        assert_eq!(fallback_app_name("über.exe"), "Über");
    }

    #[test]
    fn picks_current_unless_another_plays() {
        let alive = [true, true, true];
        assert_eq!(pick_session(Some(0), None, &[true, true, false], &alive), Some(0));
        assert_eq!(pick_session(Some(0), None, &[false, false, true], &alive), Some(2));
        assert_eq!(pick_session(Some(0), Some(1), &[false, true, true], &alive), Some(1));
        assert_eq!(pick_session(Some(0), Some(1), &[false, false, false], &alive), Some(0));
        assert_eq!(pick_session(None, Some(1), &[false, false, false], &alive), Some(1));
        assert_eq!(pick_session(None, None, &[false, false, false], &alive), Some(0));
        assert_eq!(pick_session(None, None, &[], &[]), None);
    }

    #[test]
    fn skips_dead_sessions() {
        assert_eq!(pick_session(Some(0), None, &[false, false], &[false, true]), Some(1));
        assert_eq!(pick_session(Some(0), Some(0), &[false], &[false]), None);
    }

    #[test]
    fn timeline_from_ticks() {
        let now_ft = 133_000_000_000_000_000;
        // 30 s into a 200 s track, sampled 2 s ago, playing.
        let t = make_timeline(0, 200 * 10_000_000, 30 * 10_000_000, now_ft - 2 * 10_000_000, true, 1.0, now_ft, 50.0);
        assert!((t.duration_s - 200.0).abs() < 1e-9);
        assert!((t.position_s - 30.0).abs() < 1e-9);
        assert!((t.position_at(50.0) - 32.0).abs() < 1e-6);
        assert!(t.playing);
    }

    #[test]
    fn timeline_clamps_bad_ages_and_offsets() {
        let now_ft = 133_000_000_000_000_000;
        // Snapshot from the future: not extrapolated.
        let t = make_timeline(0, 0, 10 * 10_000_000, now_ft + 50 * 10_000_000, true, 1.0, now_ft, 10.0);
        assert_eq!(t.sampled_at, 10.0);
        assert_eq!(t.duration_s, 0.0);
        // Two hours old: not extrapolated.
        let t = make_timeline(0, 600 * 10_000_000, 0, now_ft - 7200 * 10_000_000, true, 1.0, now_ft, 10.0);
        assert_eq!(t.sampled_at, 10.0);
        // Unset update time, non-zero start, bogus rate.
        let t = make_timeline(5 * 10_000_000, 65 * 10_000_000, 15 * 10_000_000, 0, false, f64::NAN, now_ft, 1.0);
        assert!((t.position_s - 10.0).abs() < 1e-9);
        assert!((t.duration_s - 60.0).abs() < 1e-9);
        assert_eq!(t.rate, 1.0);
        // Position past the end is clamped.
        let t = make_timeline(0, 60 * 10_000_000, 90 * 10_000_000, 0, false, 1.0, now_ft, 1.0);
        assert_eq!(t.position_s, 60.0);
    }

    #[test]
    fn timeline_dedup() {
        let a = Timeline::new(10.0, 200.0, 100.0, true);
        // Same extrapolated position, sampled later.
        let b = Timeline::new(15.0, 200.0, 105.0, true);
        assert!(!timeline_differs(&a, &b, 106.0));
        let c = Timeline::new(15.0, 200.0, 100.0, true);
        assert!(timeline_differs(&a, &c, 106.0));
        let d = Timeline { playing: false, ..a };
        assert!(timeline_differs(&a, &d, 100.0));
    }
}
