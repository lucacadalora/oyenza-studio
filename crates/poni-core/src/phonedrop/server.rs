//! The phone-drop web server: one thread accepts, one short-lived thread per
//! request. All routes live under the session secret:
//!
//! | Request | What it does |
//! |---|---|
//! | `GET /<secret>/` | the page (HTML, script and style in one file) |
//! | `GET /<secret>/info` | the PC's name and the Shelf files the phone can save |
//! | `POST /<secret>/ask` | the files the phone wants to send; waits for the answer |
//! | `PUT /<secret>/up/<transfer>/<n>` | file `n` of an accepted transfer, as the raw body |
//! | `GET /<secret>/get/<generation>/<n>` | Shelf file `n`, as a download |
//!
//! Anything else, including a wrong secret, is a bare 404.

use super::http::{self, Head, HeadError};
use super::names::{safe_name, unique_path};
use super::token;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The page the phone opens.
pub const PAGE_HTML: &str = include_str!("page.html");

/// Most files in one send (a whole camera roll is not what this is for).
pub const MAX_FILES: usize = 500;
/// Largest `/ask` body.
const MAX_ASK_BODY: usize = 256 * 1024;
/// Connections handled at once; more are closed straight away.
const MAX_CONNECTIONS: usize = 24;
/// A phone that stops sending mid-file for this long has gone away.
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// An accepted transfer whose phone uploads nothing for this long is dropped.
const TRANSFER_STALE: Duration = Duration::from_secs(90);
/// Progress events at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);
/// Staging folders in the inbox: `<prefix><transfer id>`.
const STAGING_PREFIX: &str = ".oyen-poni-incoming-";

#[derive(Clone, Debug)]
pub struct Config {
    /// Address to listen on: the PC's LAN address, port 0 for any free port.
    pub bind: SocketAddr,
    /// The session secret (base32, see [`token`]).
    pub token: String,
    /// What the page calls this PC.
    pub pc_name: String,
    /// Where received files go (created if missing).
    pub inbox: PathBuf,
    /// How long a phone waits for the person at the PC to answer.
    pub ask_timeout: Duration,
    /// Largest total size of one send.
    pub max_total: u64,
}

impl Config {
    pub fn new(bind: SocketAddr, token: String, pc_name: String, inbox: PathBuf) -> Self {
        Self { bind, token, pc_name, inbox, ask_timeout: Duration::from_secs(50), max_total: 64 * 1024 * 1024 * 1024 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskFile {
    /// The name as it will be saved (already made safe).
    pub name: String,
    pub size: u64,
}

/// A phone wants to send these files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: u64,
    /// "iPhone", "Android phone", … (from its User-Agent).
    pub from: String,
    pub files: Vec<AskFile>,
}

impl Ask {
    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// What the server tells the app. Sent from server threads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A phone opened the page.
    Visit { from: String },
    /// A phone asks to send files; answer with [`Server::answer`].
    Ask(Ask),
    /// The question went unanswered (the phone stopped waiting).
    AskEnded { id: u64 },
    Progress { id: u64, file: usize, files: usize, done: u64, total: u64 },
    /// Every file arrived and was moved into the inbox, in the order sent.
    Received { id: u64, from: String, paths: Vec<PathBuf> },
    /// The transfer broke off; nothing from it was kept. `reason` is a plain
    /// sentence for the island, `detail` the system's own words for the log.
    Failed { id: u64, from: String, reason: String, detail: String },
}

type Sink = Arc<dyn Fn(Event) + Send + Sync>;

struct Pending {
    id: u64,
    answer: Option<bool>,
}

struct Transfer {
    from: String,
    files: Vec<AskFile>,
    staging: PathBuf,
    arrived: Vec<bool>,
    /// A file is being written right now.
    writing: bool,
    done: u64,
    total: u64,
    last_io: Instant,
}

#[derive(Default)]
struct State {
    pending: Option<Pending>,
    transfers: HashMap<u64, Transfer>,
    offer: Vec<PathBuf>,
    offer_gen: u64,
}

struct Shared {
    cfg: Config,
    sink: Sink,
    stop: AtomicBool,
    next_id: AtomicU64,
    connections: AtomicUsize,
    state: Mutex<State>,
    answered: Condvar,
    last_request: Mutex<Instant>,
}

/// A running phone-drop server. Dropping it stops it.
pub struct Server {
    shared: Arc<Shared>,
    addr: SocketAddr,
    accept: Option<JoinHandle<()>>,
}

impl Server {
    /// Listen on `cfg.bind` and serve until [`Server::stop`].
    pub fn start(cfg: Config, sink: impl Fn(Event) + Send + Sync + 'static) -> io::Result<Server> {
        fs::create_dir_all(&cfg.inbox)?;
        clear_stale_staging(&cfg.inbox);
        let listener = TcpListener::bind(cfg.bind)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            cfg,
            sink: Arc::new(sink),
            stop: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            connections: AtomicUsize::new(0),
            state: Mutex::new(State::default()),
            answered: Condvar::new(),
            last_request: Mutex::new(Instant::now()),
        });
        let s = shared.clone();
        let accept = thread::Builder::new().name("phone-drop".into()).spawn(move || accept_loop(listener, s))?;
        Ok(Server { shared, addr, accept: Some(accept) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The link the QR code carries. Upper case on purpose (see [`token`]).
    pub fn url(&self) -> String {
        format!("HTTP://{}/{}/", self.addr, self.shared.cfg.token)
    }

    /// Answer the question with `id` (ignored if it already ended).
    pub fn answer(&self, id: u64, accept: bool) {
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = st.pending.as_mut() {
            if p.id == id && p.answer.is_none() {
                p.answer = Some(accept);
                self.shared.answered.notify_all();
            }
        }
    }

    /// The Shelf files the phone may save (folders and missing files are skipped).
    pub fn set_offer(&self, paths: Vec<PathBuf>) {
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.offer != paths {
            st.offer = paths;
            st.offer_gen += 1;
        }
    }

    /// Seconds since the last request from any phone.
    pub fn idle_secs(&self) -> f64 {
        self.shared.last_request.lock().map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    /// A question is waiting or files are coming in.
    pub fn busy(&self) -> bool {
        let st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        st.pending.is_some() || !st.transfers.is_empty()
    }

    /// Stop listening. Transfers in progress fail and leave nothing behind.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.shared.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shared.answered.notify_all();
        // Wake the blocking accept() with a connection of our own.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        // Transfers nobody is writing to right now are cleaned up here; a
        // writer that is still running notices `stop` and cleans up itself.
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let idle: Vec<u64> = st.transfers.iter().filter(|(_, t)| !t.writing).map(|(id, _)| *id).collect();
        for id in idle {
            if let Some(t) = st.transfers.remove(&id) {
                let _ = fs::remove_dir_all(&t.staging);
                (self.shared.sink)(Event::Failed { id, from: t.from, reason: "Phone drop was closed".into(), detail: String::new() });
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    for conn in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = conn else { continue };
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            let _ = stream.shutdown(Shutdown::Both);
            continue;
        }
        let s = shared.clone();
        let spawned = thread::Builder::new().name("phone-drop-conn".into()).spawn(move || {
            handle(&s, stream);
            s.connections.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn handle(shared: &Shared, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let (head, rest) = match http::read_head(&mut stream) {
        Ok(v) => v,
        Err(HeadError::TooLarge) => {
            let _ = http::respond_status(&mut stream, 431);
            return;
        }
        Err(HeadError::Malformed) => {
            let _ = http::respond_status(&mut stream, 400);
            return;
        }
        Err(HeadError::Closed) => return,
    };
    if shared.stop.load(Ordering::SeqCst) {
        let _ = http::respond_status(&mut stream, 503);
        return;
    }
    let segs = head.segments();
    if segs.is_empty() || !token::same(segs[0], &shared.cfg.token) {
        let _ = http::respond_status(&mut stream, 404);
        return;
    }
    if let Ok(mut t) = shared.last_request.lock() {
        *t = Instant::now();
    }
    let from = super::device_label(head.header("user-agent").unwrap_or("")).to_string();
    let r = match (head.method.as_str(), &segs[1..]) {
        ("GET", []) => serve_page(shared, &mut stream, &from),
        ("GET", ["info"]) => serve_info(shared, &mut stream),
        ("POST", ["ask"]) => serve_ask(shared, &mut stream, &head, rest, &from),
        ("PUT", ["up", id, n]) => serve_upload(shared, &mut stream, &head, rest, id, n),
        ("GET", ["get", generation, n]) => serve_download(shared, &mut stream, generation, n),
        (_, [] | ["info"] | ["ask"] | ["up", _, _] | ["get", _, _]) => http::respond_status(&mut stream, 405),
        _ => http::respond_status(&mut stream, 404),
    };
    if r.is_ok() {
        let _ = stream.flush();
    }
    linger(&mut stream);
}

/// Close politely: stop sending, then read what the phone still sends (a body
/// we answered without reading) for a moment, so its side sees our answer
/// instead of a reset connection.
fn linger(stream: &mut TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut sink = [0u8; 16 * 1024];
    let mut left: usize = 1024 * 1024;
    while left > 0 {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

const PAGE_CSP: &str = "Content-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'\r\n";

fn serve_page(shared: &Shared, w: &mut TcpStream, from: &str) -> io::Result<()> {
    (shared.sink)(Event::Visit { from: from.to_string() });
    http::respond(w, 200, "text/html; charset=utf-8", PAGE_HTML.as_bytes(), PAGE_CSP)
}

/// Files the phone can save right now: (index, path, name, size).
fn offered(st: &State) -> Vec<(usize, PathBuf, String, u64)> {
    st.offer
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let md = fs::metadata(p).ok()?;
            if !md.is_file() {
                return None;
            }
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some((i, p.clone(), name, md.len()))
        })
        .collect()
}

fn serve_info(shared: &Shared, w: &mut TcpStream) -> io::Result<()> {
    let (generation, files) = {
        let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        (st.offer_gen, offered(&st))
    };
    let files: Vec<_> = files.into_iter().map(|(i, _, name, size)| json!({ "n": i, "name": name, "size": size })).collect();
    http::respond_json(w, 200, &json!({ "name": shared.cfg.pc_name, "generation": generation, "files": files }))
}

#[derive(Deserialize)]
struct AskBody {
    files: Vec<AskBodyFile>,
}

#[derive(Deserialize)]
struct AskBodyFile {
    name: String,
    size: u64,
}

/// Validate the phone's list of files and make every name safe.
pub fn parse_ask(body: &[u8], max_total: u64) -> Result<Vec<AskFile>, &'static str> {
    let parsed: AskBody = serde_json::from_slice(body).map_err(|_| "bad-request")?;
    if parsed.files.is_empty() || parsed.files.len() > MAX_FILES {
        return Err("too-many");
    }
    let files: Vec<AskFile> = parsed.files.into_iter().map(|f| AskFile { name: safe_name(&f.name), size: f.size }).collect();
    let total = files.iter().try_fold(0u64, |acc, f| acc.checked_add(f.size)).ok_or("too-large")?;
    if total > max_total {
        return Err("too-large");
    }
    Ok(files)
}

fn reply(w: &mut TcpStream, ok: bool, extra: serde_json::Value) -> io::Result<()> {
    let mut v = json!({ "ok": ok });
    if let (Some(o), serde_json::Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    http::respond_json(w, 200, &v)
}

fn serve_ask(shared: &Shared, w: &mut TcpStream, head: &Head, rest: Vec<u8>, from: &str) -> io::Result<()> {
    let len = match head.content_length() {
        Ok(n) if n as usize <= MAX_ASK_BODY && !head.chunked() => n as usize,
        Ok(_) => return http::respond_status(w, 413),
        Err(()) => return http::respond_status(w, 400),
    };
    if head.expects_continue() {
        w.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let body = http::read_body(w, rest, len)?;
    let files = match parse_ask(&body, shared.cfg.max_total) {
        Ok(f) => f,
        Err(why) => return reply(w, false, json!({ "why": why })),
    };

    let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
    {
        let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        drop_stale_transfers(shared, &mut st);
        if st.pending.is_some() || !st.transfers.is_empty() {
            return reply(w, false, json!({ "why": "busy" }));
        }
        st.pending = Some(Pending { id, answer: None });
    }
    let ask = Ask { id, from: from.to_string(), files };
    (shared.sink)(Event::Ask(ask.clone()));

    // Wait for the person at the PC. Nothing else is read meanwhile.
    let deadline = Instant::now() + shared.cfg.ask_timeout;
    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    let answer = loop {
        let answer = st.pending.as_ref().and_then(|p| p.answer);
        let now = Instant::now();
        if answer.is_some() || shared.stop.load(Ordering::SeqCst) || now >= deadline {
            break answer;
        }
        st = shared.answered.wait_timeout(st, deadline - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
    };
    st.pending = None;
    match answer {
        Some(true) => {
            let staging = shared.cfg.inbox.join(format!("{STAGING_PREFIX}{id}"));
            let _ = fs::remove_dir_all(&staging);
            if let Err(e) = fs::create_dir_all(&staging) {
                drop(st);
                (shared.sink)(Event::Failed { id, from: ask.from, reason: "Couldn't make a folder for the files".into(), detail: e.to_string() });
                return reply(w, false, json!({ "why": "pc-error" }));
            }
            let total = ask.total();
            let n = ask.files.len();
            st.transfers.insert(
                id,
                Transfer { from: ask.from.clone(), files: ask.files, staging, arrived: vec![false; n], writing: false, done: 0, total, last_io: Instant::now() },
            );
            drop(st);
            (shared.sink)(Event::Progress { id, file: 0, files: n, done: 0, total });
            reply(w, true, json!({ "transfer": id }))
        }
        Some(false) => {
            drop(st);
            reply(w, false, json!({ "why": "declined" }))
        }
        None => {
            drop(st);
            (shared.sink)(Event::AskEnded { id });
            let why = if shared.stop.load(Ordering::SeqCst) { "closed" } else { "timeout" };
            reply(w, false, json!({ "why": why }))
        }
    }
}

/// Half-received sends left behind when the app quit mid-transfer.
fn clear_stale_staging(inbox: &Path) {
    let Ok(entries) = fs::read_dir(inbox) else { return };
    for e in entries.flatten() {
        let is_staging = e.file_name().to_str().is_some_and(|n| n.starts_with(STAGING_PREFIX));
        if is_staging && e.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Accepted transfers whose phone went quiet block new ones; let them go.
fn drop_stale_transfers(shared: &Shared, st: &mut State) {
    let stale: Vec<u64> = st.transfers.iter().filter(|(_, t)| !t.writing && t.last_io.elapsed() > TRANSFER_STALE).map(|(id, _)| *id).collect();
    for id in stale {
        if let Some(t) = st.transfers.remove(&id) {
            let _ = fs::remove_dir_all(&t.staging);
            (shared.sink)(Event::Failed { id, from: t.from, reason: "The phone stopped sending".into(), detail: String::new() });
        }
    }
}

fn fail_transfer(shared: &Shared, id: u64, reason: &str, detail: String) {
    let t = shared.state.lock().unwrap_or_else(|e| e.into_inner()).transfers.remove(&id);
    if let Some(t) = t {
        let _ = fs::remove_dir_all(&t.staging);
        (shared.sink)(Event::Failed { id, from: t.from, reason: reason.into(), detail });
    }
}

fn serve_upload(shared: &Shared, w: &mut TcpStream, head: &Head, rest: Vec<u8>, id: &str, n: &str) -> io::Result<()> {
    let (Ok(id), Ok(n)) = (id.parse::<u64>(), n.parse::<usize>()) else {
        return http::respond_status(w, 404);
    };
    let Ok(len) = head.content_length() else {
        return http::respond_status(w, 400);
    };
    if head.chunked() {
        return http::respond_status(w, 411);
    }
    // Claim file `n` of the transfer: it must exist, not have arrived, and
    // the body must be exactly the size the phone announced.
    let (path, expected, files) = {
        let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(t) = st.transfers.get_mut(&id) else {
            return http::respond_status(w, 404);
        };
        if n >= t.files.len() || t.arrived[n] || t.writing {
            return http::respond_status(w, 409);
        }
        t.writing = true;
        t.last_io = Instant::now();
        (t.staging.join(format!("{n}.part")), t.files[n].size, t.files.len())
    };
    if len != expected {
        fail_transfer(shared, id, "A file didn't match the size the phone announced", String::new());
        return reply(w, false, json!({ "why": "size" }));
    }
    if head.expects_continue() {
        w.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }

    match write_file(shared, w, rest, &path, len, id, n, files) {
        Ok(()) => {}
        Err(e) => {
            let reason = if shared.stop.load(Ordering::SeqCst) { "Phone drop was closed" } else { "The connection dropped" };
            fail_transfer(shared, id, reason, e.to_string());
            return reply(w, false, json!({ "why": "failed" }));
        }
    }

    // Mark it arrived; the last file moves the whole batch into the inbox.
    let finished = {
        let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(t) = st.transfers.get_mut(&id) else {
            return reply(w, false, json!({ "why": "failed" }));
        };
        t.writing = false;
        t.arrived[n] = true;
        t.last_io = Instant::now();
        if t.arrived.iter().all(|a| *a) {
            st.transfers.remove(&id)
        } else {
            None
        }
    };
    match finished {
        None => reply(w, true, json!({})),
        Some(t) => match commit(&shared.cfg.inbox, &t) {
            Ok(paths) => {
                let _ = fs::remove_dir_all(&t.staging);
                (shared.sink)(Event::Received { id, from: t.from, paths });
                reply(w, true, json!({ "done": true }))
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&t.staging);
                (shared.sink)(Event::Failed { id, from: t.from, reason: "Couldn't save the files".into(), detail: e.to_string() });
                reply(w, false, json!({ "why": "pc-error" }))
            }
        },
    }
}

/// Stream one upload body into `path`, reporting progress.
#[allow(clippy::too_many_arguments)]
fn write_file(shared: &Shared, r: &mut TcpStream, rest: Vec<u8>, path: &Path, len: u64, id: u64, n: usize, files: usize) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(256 * 1024, File::create(path)?);
    let mut left = len;
    let mut last = Instant::now();
    let mut chunk = vec![0u8; 128 * 1024];
    let mut pending: Vec<u8> = rest;
    pending.truncate(len as usize);
    let mut add = |shared: &Shared, k: u64, force: bool| {
        let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = st.transfers.get_mut(&id) {
            t.done += k;
            t.last_io = Instant::now();
            if force || last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                let (done, total) = (t.done, t.total);
                drop(st);
                (shared.sink)(Event::Progress { id, file: n, files, done, total });
            }
        }
    };
    if !pending.is_empty() {
        out.write_all(&pending)?;
        left -= pending.len() as u64;
        add(shared, pending.len() as u64, false);
    }
    while left > 0 {
        if shared.stop.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"));
        }
        let want = chunk.len().min(left as usize);
        let k = match r.read(&mut chunk[..want]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the phone closed the connection")),
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        out.write_all(&chunk[..k])?;
        left -= k as u64;
        add(shared, k as u64, false);
    }
    out.flush()?;
    out.get_ref().sync_all()?;
    add(shared, 0, true);
    Ok(())
}

/// Move every staged file into the inbox under its own name (numbered when
/// taken). If one move fails, the ones already moved are taken back out so
/// the transfer still lands whole or not at all.
fn commit(inbox: &Path, t: &Transfer) -> io::Result<Vec<PathBuf>> {
    let mut placed: Vec<PathBuf> = Vec::with_capacity(t.files.len());
    for (i, f) in t.files.iter().enumerate() {
        let dest = unique_path(inbox, &f.name, &placed);
        if let Err(e) = fs::rename(t.staging.join(format!("{i}.part")), &dest) {
            for p in &placed {
                let _ = fs::remove_file(p);
            }
            return Err(e);
        }
        placed.push(dest);
    }
    Ok(placed)
}

fn serve_download(shared: &Shared, w: &mut TcpStream, generation: &str, n: &str) -> io::Result<()> {
    let (Ok(generation), Ok(n)) = (generation.parse::<u64>(), n.parse::<usize>()) else {
        return http::respond_status(w, 404);
    };
    let hit = {
        let st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.offer_gen != generation {
            None
        } else {
            offered(&st).into_iter().find(|(i, ..)| *i == n)
        }
    };
    let Some((_, path, name, _)) = hit else {
        return http::respond_status(w, 404);
    };
    let mut f = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return http::respond_status(w, 404),
    };
    let len = f.metadata()?.len();
    http::write_head(w, 200, http::content_type_for(&name), len, &http::attachment(&name))?;
    let copied = io::copy(&mut (&mut f).take(len), w)?;
    if copied != len {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the file shrank while it was sent"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::sync::mpsc::{channel, Receiver};

    const TOKEN: &str = "K7Q2M9XWZ4R8T6YB3NPV5HJC2D";
    const PHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 27_0 like Mac OS X) Mobile/15E148 Safari/604.1";

    struct Fixture {
        server: Option<Server>,
        events: Receiver<Event>,
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            Self::with(name, Duration::from_secs(5))
        }

        fn with(name: &str, ask_timeout: Duration) -> Fixture {
            let dir = std::env::temp_dir().join(format!("poni-phonedrop-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            let (tx, rx) = channel();
            let tx = Mutex::new(tx);
            let mut cfg = Config::new("127.0.0.1:0".parse().unwrap(), TOKEN.into(), "Test PC".into(), dir.join("inbox"));
            cfg.ask_timeout = ask_timeout;
            let server = Server::start(cfg, move |e| {
                let _ = tx.lock().unwrap().send(e);
            })
            .unwrap();
            Fixture { server: Some(server), events: rx, dir }
        }

        fn server(&self) -> &Server {
            self.server.as_ref().unwrap()
        }

        fn inbox(&self) -> PathBuf {
            self.dir.join("inbox")
        }

        /// Send a raw request; returns (status, body).
        fn request(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
            request(self.server().addr(), method, path, body)
        }

        fn next_event(&self) -> Event {
            self.events.recv_timeout(Duration::from_secs(5)).expect("an event")
        }

        /// Next event that isn't a Progress or Visit.
        fn next_major(&self) -> Event {
            loop {
                match self.next_event() {
                    Event::Progress { .. } | Event::Visit { .. } => continue,
                    e => return e,
                }
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(s) = self.server.take() {
                s.stop();
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn request(addr: SocketAddr, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: {PHONE}\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
        s.write_all(body).unwrap();
        let mut r = io::BufReader::new(s);
        let mut status = String::new();
        r.read_line(&mut status).unwrap();
        let code: u16 = status.split(' ').nth(1).unwrap().parse().unwrap();
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
        }
        let mut out = vec![0u8; len];
        r.read_exact(&mut out).unwrap();
        (code, out)
    }

    fn json(body: &[u8]) -> serde_json::Value {
        serde_json::from_slice(body).unwrap()
    }

    fn p(rest: &str) -> String {
        format!("/{TOKEN}/{rest}")
    }

    /// Ask in a thread; returns the thread's (status, body).
    fn ask_async(addr: SocketAddr, files: serde_json::Value) -> JoinHandle<(u16, Vec<u8>)> {
        let body = json!({ "files": files }).to_string();
        thread::spawn(move || request(addr, "POST", &format!("/{TOKEN}/ask"), body.as_bytes()))
    }

    #[test]
    fn serves_the_page_only_under_the_secret() {
        let f = Fixture::new("page");
        let (code, body) = f.request("GET", &p(""), b"");
        assert_eq!(code, 200);
        assert!(String::from_utf8_lossy(&body).contains("<title>"));
        assert_eq!(f.next_event(), Event::Visit { from: "iPhone".into() });
        assert_eq!(f.request("GET", "/", b"").0, 404);
        assert_eq!(f.request("GET", "/WRONGTOKEN/", b"").0, 404);
        assert_eq!(f.request("GET", &format!("/{}/", &TOKEN[..25]), b"").0, 404);
        assert_eq!(f.request("GET", &p("nope"), b"").0, 404);
        assert_eq!(f.request("DELETE", &p("info"), b"").0, 405);
        assert!(f.server().url().starts_with("HTTP://127.0.0.1:"));
        assert!(f.server().url().ends_with(&format!("/{TOKEN}/")));
    }

    #[test]
    fn receives_files_after_the_yes() {
        let f = Fixture::new("yes");
        let addr = f.server().addr();
        let asker = ask_async(addr, json!([{ "name": "IMG_2041.HEIC", "size": 5 }, { "name": "../notes.txt", "size": 0 }, { "name": "IMG_2041.HEIC", "size": 3 }]));
        let Event::Ask(ask) = f.next_major() else { panic!("expected an ask") };
        assert_eq!(ask.from, "iPhone");
        assert_eq!(ask.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["IMG_2041.HEIC", "notes.txt", "IMG_2041.HEIC"]);
        assert_eq!(ask.total(), 8);
        assert!(f.server().busy());
        // A second phone asking meanwhile is told the PC is busy.
        let (_, busy) = f.request("POST", &p("ask"), json!({ "files": [{ "name": "a", "size": 1 }] }).to_string().as_bytes());
        assert_eq!(json(&busy)["why"], "busy");
        f.server().answer(ask.id, true);
        let (code, body) = asker.join().unwrap();
        assert_eq!(code, 200);
        let v = json(&body);
        assert_eq!(v["ok"], true);
        let t = v["transfer"].as_u64().unwrap();
        assert_eq!(t, ask.id);

        // Files may come in any order, but each one only once.
        assert_eq!(f.request("PUT", &p(&format!("up/{t}/0")), b"hello").0, 200);
        assert_eq!(f.request("PUT", &p(&format!("up/{t}/0")), b"hello").0, 409, "a file arrives once");
        assert_eq!(f.request("PUT", &p(&format!("up/{t}/2")), b"abc").0, 200);
        let (_, done) = f.request("PUT", &p(&format!("up/{t}/1")), b"");
        assert_eq!(json(&done)["done"], true);
        let Event::Received { id, from, paths } = f.next_major() else { panic!("expected received") };
        assert_eq!((id, from.as_str()), (t, "iPhone"));
        let names: Vec<String> = paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["IMG_2041.HEIC", "notes.txt", "IMG_2041 (1).HEIC"]);
        assert_eq!(fs::read(&paths[0]).unwrap(), b"hello");
        assert_eq!(fs::read(&paths[1]).unwrap(), b"");
        assert_eq!(fs::read(&paths[2]).unwrap(), b"abc");
        // The staging folder is gone and the PC is free again.
        let left: Vec<_> = fs::read_dir(f.inbox()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 3);
        assert!(!f.server().busy());
    }

    #[test]
    fn a_no_reads_nothing() {
        let f = Fixture::new("no");
        let asker = ask_async(f.server().addr(), json!([{ "name": "secret.pdf", "size": 10 }]));
        let Event::Ask(ask) = f.next_major() else { panic!() };
        f.server().answer(ask.id, false);
        let (_, body) = asker.join().unwrap();
        assert_eq!(json(&body), json!({ "ok": false, "why": "declined" }));
        // Without an accepted transfer there is nothing to upload into.
        assert_eq!(f.request("PUT", &p(&format!("up/{}/0", ask.id)), b"0123456789").0, 404);
        assert_eq!(fs::read_dir(f.inbox()).unwrap().count(), 0);
    }

    #[test]
    fn an_unanswered_ask_times_out() {
        let f = Fixture::with("timeout", Duration::from_millis(300));
        let asker = ask_async(f.server().addr(), json!([{ "name": "a.jpg", "size": 1 }]));
        let Event::Ask(ask) = f.next_major() else { panic!() };
        assert_eq!(f.next_major(), Event::AskEnded { id: ask.id });
        assert_eq!(json(&asker.join().unwrap().1)["why"], "timeout");
        // A late answer changes nothing.
        f.server().answer(ask.id, true);
        assert!(!f.server().busy());
    }

    #[test]
    fn a_wrong_size_keeps_nothing() {
        let f = Fixture::new("size");
        let asker = ask_async(f.server().addr(), json!([{ "name": "a.txt", "size": 2 }, { "name": "b.txt", "size": 4 }]));
        let Event::Ask(ask) = f.next_major() else { panic!() };
        f.server().answer(ask.id, true);
        asker.join().unwrap();
        assert_eq!(f.request("PUT", &p(&format!("up/{}/0", ask.id)), b"ok").0, 200);
        let (_, body) = f.request("PUT", &p(&format!("up/{}/1", ask.id)), b"too long");
        assert_eq!(json(&body)["why"], "size");
        assert!(matches!(f.next_major(), Event::Failed { .. }));
        assert_eq!(fs::read_dir(f.inbox()).unwrap().count(), 0, "staged files are removed");
    }

    #[test]
    fn rejects_bad_asks() {
        let f = Fixture::new("bad");
        let (_, b) = f.request("POST", &p("ask"), b"{not json");
        assert_eq!(json(&b)["why"], "bad-request");
        let (_, b) = f.request("POST", &p("ask"), json!({ "files": [] }).to_string().as_bytes());
        assert_eq!(json(&b)["why"], "too-many");
        let (_, b) = f.request("POST", &p("ask"), json!({ "files": [{ "name": "a", "size": u64::MAX }, { "name": "b", "size": 2 }] }).to_string().as_bytes());
        assert_eq!(json(&b)["why"], "too-large");
        assert!(!f.server().busy());
    }

    #[test]
    fn offers_shelf_files_for_download() {
        let f = Fixture::new("offer");
        let shelf = f.dir.join("shelf");
        fs::create_dir_all(shelf.join("a folder")).unwrap();
        fs::write(shelf.join("Résumé.pdf"), b"%PDF-1.7 tiny").unwrap();
        f.server().set_offer(vec![shelf.join("a folder"), shelf.join("Résumé.pdf"), shelf.join("gone.txt")]);
        let (code, body) = f.request("GET", &p("info"), b"");
        assert_eq!(code, 200);
        let v = json(&body);
        assert_eq!(v["name"], "Test PC");
        let generation = v["generation"].as_u64().unwrap();
        assert_eq!(v["files"], json!([{ "n": 1, "name": "Résumé.pdf", "size": 13 }]), "folders and missing files are left out");
        let (code, got) = f.request("GET", &p(&format!("get/{generation}/1")), b"");
        assert_eq!((code, got.as_slice()), (200, b"%PDF-1.7 tiny".as_slice()));
        assert_eq!(f.request("GET", &p(&format!("get/{generation}/0")), b"").0, 404, "a folder isn't served");
        assert_eq!(f.request("GET", &p(&format!("get/{}/1", generation + 1)), b"").0, 404, "a stale list isn't served");
        // A changed Shelf starts a new generation; the same list doesn't.
        f.server().set_offer(vec![shelf.join("a folder"), shelf.join("Résumé.pdf"), shelf.join("gone.txt")]);
        assert_eq!(json(&f.request("GET", &p("info"), b"").1)["generation"].as_u64(), Some(generation));
        f.server().set_offer(vec![shelf.join("Résumé.pdf")]);
        assert_eq!(json(&f.request("GET", &p("info"), b"").1)["generation"].as_u64(), Some(generation + 1));
    }

    #[test]
    fn stopping_ends_waits_and_transfers() {
        let mut f = Fixture::new("stop");
        let addr = f.server().addr();
        let asker = ask_async(addr, json!([{ "name": "a.jpg", "size": 1 }]));
        let Event::Ask(ask) = f.next_major() else { panic!() };
        let started = Instant::now();
        f.server.take().unwrap().stop();
        assert_eq!(json(&asker.join().unwrap().1)["why"], "closed");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(f.next_major(), Event::AskEnded { id: ask.id });
        assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err() || request_fails(addr));
    }

    /// After stop, a connection may still be accepted by the OS backlog but
    /// nobody answers it.
    fn request_fails(addr: SocketAddr) -> bool {
        let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return true };
        let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
        let _ = write!(s, "GET /{TOKEN}/ HTTP/1.1\r\n\r\n");
        let mut b = [0u8; 16];
        !matches!(s.read(&mut b), Ok(n) if n > 0)
    }

    #[test]
    fn clears_leftovers_of_an_interrupted_send() {
        let dir = std::env::temp_dir().join(format!("poni-phonedrop-leftover-{}", std::process::id()));
        let inbox = dir.join("inbox");
        fs::create_dir_all(inbox.join(".oyen-poni-incoming-3")).unwrap();
        fs::write(inbox.join(".oyen-poni-incoming-3").join("0.part"), b"half").unwrap();
        fs::write(inbox.join("kept.jpg"), b"a photo").unwrap();
        let s = Server::start(Config::new("127.0.0.1:0".parse().unwrap(), TOKEN.into(), "PC".into(), inbox.clone()), |_| {}).unwrap();
        let left: Vec<String> = fs::read_dir(&inbox).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["kept.jpg"]);
        s.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn handles_expect_continue() {
        let f = Fixture::new("continue");
        let mut s = TcpStream::connect(f.server().addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = json!({ "files": [{ "name": "a", "size": 1 }] }).to_string();
        write!(s, "POST /{TOKEN}/ask HTTP/1.1\r\nContent-Length: {}\r\nExpect: 100-continue\r\n\r\n", body.len()).unwrap();
        let mut first = [0u8; 25];
        s.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"HTTP/1.1 100 Continue\r\n\r\n");
        s.write_all(body.as_bytes()).unwrap();
        let Event::Ask(ask) = f.next_major() else { panic!() };
        f.server().answer(ask.id, false);
        let mut rest = String::new();
        s.read_to_string(&mut rest).unwrap();
        assert!(rest.contains("declined"));
    }
}
