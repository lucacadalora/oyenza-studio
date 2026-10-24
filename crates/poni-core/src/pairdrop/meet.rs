//! The PC's side of the meeting server: one long request at a time that
//! waits for messages from paired phones, answers them, and starts a direct
//! link ([`super::peer`]) for each phone that wants to connect.
//!
//! The worker thread sleeps inside that request (up to 25 s, then asks
//! again), so a PC with paired phones costs one small HTTPS request about
//! every 25 seconds and no CPU in between. With no pairs it asks nothing.

use super::keys::{is_id, Secret};
use super::peer::{self, PeerCfg, PeerEvent, Shelf};
use super::signal::{clean_name, now_ms, Msg, Replays};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

/// How long a request waits; the server holds it at most 25 s.
const WAIT_TIMEOUT: Duration = Duration::from_secs(35);
const SHORT_TIMEOUT: Duration = Duration::from_secs(10);
/// A pairing QR code works this long.
pub const PAIRING_FOR: Duration = Duration::from_secs(10 * 60);
/// Phones connected at once.
pub const MAX_SESSIONS: usize = 3;
/// Most pairs a PC keeps (the server takes 32 rooms per request).
pub const MAX_PAIRS: usize = 30;

/// One HTTP POST with a JSON body to the meeting server, answered with the
/// status and the JSON body. The Windows crate makes the HTTPS calls.
pub trait Post: Send + Sync {
    fn post(&self, path: &str, body: &Value, timeout: Duration) -> Result<(u16, Value), String>;
}

/// A paired phone, as kept on disk (sealed by the Windows crate).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    /// The pairing secret, base64url.
    pub k: String,
    pub name: String,
    /// Unix seconds.
    pub added: i64,
    /// Unix seconds this phone last connected (0 = not yet).
    #[serde(default)]
    pub seen: i64,
}

impl Pair {
    pub fn secret(&self) -> Option<Secret> {
        Secret::parse(&self.k)
    }

    pub fn room(&self) -> String {
        self.secret().map(|s| s.room()).unwrap_or_default()
    }
}

/// Everything kept on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub pairs: Vec<Pair>,
    /// Secrets of removed pairs the server hasn't heard about yet.
    #[serde(default)]
    pub unpair: Vec<String>,
}

impl Book {
    pub fn from_json(s: &str) -> Book {
        let mut b: Book = serde_json::from_str(s).unwrap_or_default();
        b.pairs.retain(|p| p.secret().is_some());
        b
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// What the meeting side tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeetEvent {
    /// The list changed (paired, renamed, removed, seen).
    Pairs(Vec<Pair>),
    /// A phone finished pairing.
    Paired { room: String, name: String },
    /// The meeting server answered again, or stopped answering.
    Server { reachable: bool },
    /// Something happened on a phone's direct link.
    Phone { room: String, phone: String, ev: PeerEvent },
}

pub struct MeetCfg {
    /// The meeting server, e.g. `https://drop.oyenza.com`.
    pub server: String,
    pub pc_name: String,
    pub inbox: PathBuf,
    pub stun: Vec<String>,
    /// This run's client id (22-char base64url, random).
    pub client: String,
    /// Local addresses for the direct link, asked again for every phone
    /// (the PC may have moved networks).
    pub local_ips: Arc<dyn Fn() -> Vec<IpAddr> + Send + Sync>,
}

type Emit = Arc<dyn Fn(MeetEvent) + Send + Sync>;
type Save = Arc<dyn Fn(&Book) + Send + Sync>;

struct Shared {
    cfg: MeetCfg,
    post: Arc<dyn Post>,
    emit: Emit,
    save: Save,
    book: Mutex<Book>,
    pending: Mutex<Option<(Secret, Instant)>>,
    replays: Mutex<Replays>,
    shelf: Arc<Shelf>,
    stop: AtomicBool,
    /// Bumped whenever the rooms change; the worker sleeps on `cv`.
    changed: Mutex<u64>,
    cv: Condvar,
    sessions: AtomicUsize,
    next_session: AtomicU64,
    reachable: Mutex<Option<bool>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The running meeting side. Dropping it stops it.
pub struct Meet {
    shared: Arc<Shared>,
}

impl Meet {
    pub fn start(cfg: MeetCfg, post: Arc<dyn Post>, book: Book, emit: Emit, save: Save) -> std::io::Result<Meet> {
        super::transfer::clear_stale_staging(&cfg.inbox);
        let shared = Arc::new(Shared {
            cfg,
            post,
            emit,
            save,
            book: Mutex::new(book),
            pending: Mutex::new(None),
            replays: Mutex::new(Replays::default()),
            shelf: Arc::new(Shelf::default()),
            stop: AtomicBool::new(false),
            changed: Mutex::new(0),
            cv: Condvar::new(),
            sessions: AtomicUsize::new(0),
            next_session: AtomicU64::new(1),
            reachable: Mutex::new(None),
        });
        let s = shared.clone();
        thread::Builder::new().name("pair-drop".into()).spawn(move || worker(s))?;
        Ok(Meet { shared })
    }

    /// Show a pairing QR code for `secret`: returns the link it carries.
    pub fn begin_pairing(&self, secret: Secret) -> String {
        let link = secret.link(&self.shared.cfg.server);
        *lock(&self.shared.pending) = Some((secret, Instant::now()));
        self.rooms_changed();
        link
    }

    pub fn cancel_pairing(&self) {
        if lock(&self.shared.pending).take().is_some() {
            self.rooms_changed();
        }
    }

    /// Forget a paired phone, here and (as soon as it answers) on the server.
    pub fn remove(&self, room: &str) {
        let book = {
            let mut b = lock(&self.shared.book);
            let Some(i) = b.pairs.iter().position(|p| p.room() == room) else { return };
            let p = b.pairs.remove(i);
            b.unpair.push(p.k);
            b.clone()
        };
        (self.shared.save)(&book);
        (self.shared.emit)(MeetEvent::Pairs(book.pairs));
        self.rooms_changed();
    }

    /// The files on the Shelf, which connected phones may save.
    pub fn set_shelf(&self, paths: Vec<PathBuf>) {
        self.shared.shelf.set(paths);
    }

    pub fn pairs(&self) -> Vec<Pair> {
        lock(&self.shared.book).pairs.clone()
    }

    /// Wake the worker, and end the server's open wait so the next one
    /// carries the new list of rooms straight away.
    fn rooms_changed(&self) {
        *lock(&self.shared.changed) += 1;
        self.shared.cv.notify_all();
        let s = self.shared.clone();
        let _ = thread::Builder::new().name("pair-drop-kick".into()).spawn(move || {
            let _ = s.post.post("/api/pc/kick", &json!({ "client": s.cfg.client }), SHORT_TIMEOUT);
        });
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.cv.notify_all();
    }
}

impl Drop for Meet {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker(s: Arc<Shared>) {
    let mut backoff = Duration::ZERO;
    while !s.stop.load(Ordering::SeqCst) {
        send_unpairs(&s);
        let rooms = rooms(&s);
        let generation = *lock(&s.changed);
        if rooms.is_empty() {
            sleep_until_changed(&s, generation, Duration::from_secs(300));
            continue;
        }
        if backoff > Duration::ZERO {
            sleep_until_changed(&s, generation, backoff);
        }
        let list: Vec<Value> = rooms.iter().map(|(sec, _)| json!({ "room": sec.room(), "proof": sec.proof() })).collect();
        let body = json!({ "rooms": list, "client": s.cfg.client });
        match s.post.post("/api/pc/wait", &body, WAIT_TIMEOUT) {
            Ok((200, v)) => {
                reachable(&s, true);
                backoff = Duration::ZERO;
                handle_wait(&s, &v);
            }
            Ok((429, _)) => backoff = Duration::from_secs(30),
            Ok((status, _)) => {
                log(&format!("pair drop: the meeting server answered {status}"));
                reachable(&s, false);
                backoff = next_backoff(backoff);
            }
            Err(e) => {
                if lock(&s.reachable).unwrap_or(true) {
                    log(&format!("pair drop: can't reach the meeting server: {e}"));
                }
                reachable(&s, false);
                backoff = next_backoff(backoff);
            }
        }
    }
}

fn next_backoff(b: Duration) -> Duration {
    if b.is_zero() {
        Duration::from_secs(2)
    } else {
        (b * 2).min(Duration::from_secs(60))
    }
}

fn sleep_until_changed(s: &Shared, generation: u64, max: Duration) {
    let deadline = Instant::now() + max;
    let mut g = lock(&s.changed);
    while *g == generation && !s.stop.load(Ordering::SeqCst) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        g = s.cv.wait_timeout(g, deadline - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
    }
}

fn reachable(s: &Shared, ok: bool) {
    let mut r = lock(&s.reachable);
    if *r != Some(ok) {
        *r = Some(ok);
        drop(r);
        (s.emit)(MeetEvent::Server { reachable: ok });
    }
}

/// Every room this PC listens on: the pairs, plus the pairing QR's while it shows.
fn rooms(s: &Shared) -> Vec<(Secret, bool)> {
    let mut out: Vec<(Secret, bool)> = lock(&s.book).pairs.iter().filter_map(|p| p.secret()).map(|k| (k, false)).collect();
    let mut pending = lock(&s.pending);
    if pending.as_ref().is_some_and(|(_, at)| at.elapsed() > PAIRING_FOR) {
        *pending = None;
    }
    if let Some((k, _)) = pending.as_ref() {
        if !out.iter().any(|(x, _)| x == k) {
            out.push((k.clone(), true));
        }
    }
    out.truncate(32);
    out
}

fn send_unpairs(s: &Shared) {
    let todo = lock(&s.book).unpair.clone();
    for k in todo {
        let Some(sec) = Secret::parse(&k) else { continue };
        let r = s.post.post("/api/pc/unpair", &json!({ "room": sec.room(), "proof": sec.proof() }), SHORT_TIMEOUT);
        if matches!(r, Ok((200 | 403 | 410, _))) {
            let book = {
                let mut b = lock(&s.book);
                b.unpair.retain(|x| *x != k);
                b.clone()
            };
            (s.save)(&book);
        }
    }
}

#[derive(Deserialize)]
struct WaitAnswer {
    #[serde(default)]
    msgs: Vec<RoomMsg>,
    #[serde(default)]
    gone: Vec<String>,
}

#[derive(Deserialize)]
struct RoomMsg {
    room: String,
    msg: Msg,
}

fn handle_wait(s: &Arc<Shared>, v: &Value) {
    let Ok(a) = serde_json::from_value::<WaitAnswer>(v.clone()) else { return };
    if !a.gone.is_empty() {
        // The server says these pairs were removed (from another copy of
        // this PC's list, or a reinstall): drop them here too.
        let book = {
            let mut b = lock(&s.book);
            let before = b.pairs.len();
            b.pairs.retain(|p| !a.gone.contains(&p.room()));
            (b.pairs.len() != before).then(|| b.clone())
        };
        if let Some(book) = book {
            (s.save)(&book);
            (s.emit)(MeetEvent::Pairs(book.pairs));
        }
    }
    for m in a.msgs {
        handle_msg(s, &m.room, m.msg);
    }
}

fn find_secret(s: &Shared, room: &str) -> Option<(Secret, bool)> {
    if let Some(k) = lock(&s.book).pairs.iter().filter_map(|p| p.secret()).find(|k| k.room() == room) {
        return Some((k, false));
    }
    lock(&s.pending).as_ref().map(|(k, _)| k.clone()).filter(|k| k.room() == room).map(|k| (k, true))
}

fn handle_msg(s: &Arc<Shared>, room: &str, msg: Msg) {
    let Some((secret, pending)) = find_secret(s, room) else { return };
    if let Err(why) = msg.verify(&secret, now_ms()) {
        log(&format!("pair drop: dropped a message ({why:?})"));
        return;
    }
    if !lock(&s.replays).first(&msg) {
        return;
    }
    match msg.t.as_str() {
        "pair" => on_pair(s, &secret, pending, &msg),
        "offer" if !pending => on_offer(s, secret, msg),
        _ => {}
    }
}

fn reply(s: &Shared, secret: &Secret, t: &str, sid: &str, sdp: &str) -> bool {
    let msg = Msg::signed(secret, t, sid, &s.cfg.pc_name, sdp, now_ms());
    let body = json!({ "room": secret.room(), "proof": secret.proof(), "msg": msg });
    match s.post.post("/api/pc/send", &body, SHORT_TIMEOUT) {
        Ok((200, _)) => true,
        Ok((status, _)) => {
            log(&format!("pair drop: the server didn't take our {t} ({status})"));
            false
        }
        Err(e) => {
            log(&format!("pair drop: couldn't send our {t}: {e}"));
            false
        }
    }
}

fn on_pair(s: &Arc<Shared>, secret: &Secret, pending: bool, msg: &Msg) {
    let name = match clean_name(&msg.name) {
        n if n.is_empty() => "Phone".to_string(),
        n => n,
    };
    let now = now_ms() / 1000;
    let (book, fresh) = {
        let mut b = lock(&s.book);
        let k = secret.encode();
        let fresh = match b.pairs.iter_mut().find(|p| p.k == k) {
            Some(p) => {
                p.name = name.clone();
                p.seen = now;
                false
            }
            None => {
                if b.pairs.len() >= MAX_PAIRS {
                    // Oldest out, so pairing never just fails.
                    let old = b.pairs.remove(0);
                    b.unpair.push(old.k);
                }
                b.pairs.push(Pair { k, name: name.clone(), added: now, seen: now });
                true
            }
        };
        (b.clone(), fresh)
    };
    if pending {
        lock(&s.pending).take();
    }
    (s.save)(&book);
    (s.emit)(MeetEvent::Pairs(book.pairs));
    if fresh || pending {
        (s.emit)(MeetEvent::Paired { room: secret.room(), name });
    }
    reply(s, secret, "welcome", &msg.sid, "");
}

fn on_offer(s: &Arc<Shared>, secret: Secret, msg: Msg) {
    if s.sessions.load(Ordering::SeqCst) >= MAX_SESSIONS {
        reply(s, &secret, "busy", &msg.sid, "");
        return;
    }
    s.sessions.fetch_add(1, Ordering::SeqCst);
    let s2 = s.clone();
    let spawned = thread::Builder::new().name("pair-drop-peer".into()).spawn(move || {
        session(&s2, secret, msg);
        s2.sessions.fetch_sub(1, Ordering::SeqCst);
    });
    if spawned.is_err() {
        s.sessions.fetch_sub(1, Ordering::SeqCst);
    }
}

fn session(s: &Arc<Shared>, secret: Secret, offer: Msg) {
    let room = secret.room();
    let n = s.next_session.fetch_add(1, Ordering::SeqCst);
    let cfg = PeerCfg {
        pc_name: s.cfg.pc_name.clone(),
        inbox: s.cfg.inbox.clone(),
        local_ips: (s.cfg.local_ips)(),
        stun: s.cfg.stun.clone(),
        shelf: s.shelf.clone(),
        n,
    };
    let (peer, answer) = match peer::answer(&offer.sdp, cfg, n) {
        Ok(v) => v,
        Err(e) => {
            log(&format!("pair drop: couldn't answer a phone: {e}"));
            return;
        }
    };
    if !is_id(&offer.sid) || !reply(s, &secret, "answer", &offer.sid, &answer) {
        return;
    }
    let phone = Mutex::new(clean_name(&offer.name));
    let emit = s.emit.clone();
    let shared = s.clone();
    peer.run(|ev| {
        if let PeerEvent::Hello { phone: p } = &ev {
            *lock(&phone) = p.clone();
            mark_seen(&shared, &room);
        }
        let who = lock(&phone).clone();
        let who = if who.is_empty() { "Phone".to_string() } else { who };
        emit(MeetEvent::Phone { room: room.clone(), phone: who, ev });
    });
}

fn mark_seen(s: &Shared, room: &str) {
    let now = now_ms() / 1000;
    let book = {
        let mut b = lock(&s.book);
        let Some(p) = b.pairs.iter_mut().find(|p| p.room() == room) else { return };
        p.seen = now;
        b.clone()
    };
    (s.save)(&book);
    (s.emit)(MeetEvent::Pairs(book.pairs));
}

fn log(m: &str) {
    super::log(m);
}
