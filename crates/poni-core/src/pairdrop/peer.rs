//! One phone's direct link: answer its WebRTC offer, then speak the data
//! channel protocol until it says bye, goes quiet or the link drops.
//!
//! str0m does the WebRTC work (ICE, DTLS, SCTP) without touching the
//! network; this file owns the UDP sockets. One socket per local address
//! carries a "host" candidate, and a STUN answer for it adds the address the
//! internet sees ("server reflexive"), which is how a phone on another
//! network, or behind a TURN relay, reaches the PC.

use super::stun;
use super::transfer::{self, FromPhone, Incoming, Outgoing, ShelfFile};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event as RtcEvent, IceConnectionState, Input, Output, Rtc};

/// The data channel the phone page opens.
pub const LABEL: &str = "oyen-drop";
/// A link that hasn't connected by now has failed.
const CONNECT_WITHIN: Duration = Duration::from_secs(30);
/// A connected phone that sends nothing for this long has gone away.
const IDLE: Duration = Duration::from_secs(120);
/// Keep the outgoing buffer about this full while sending a Shelf file.
const OUT_BUFFER: usize = 1024 * 1024;
/// Progress to the app and to the phone at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How long the STUN question may take before the answer is sent without it.
const STUN_BUDGET: Duration = Duration::from_millis(900);

/// The Shelf as phones see it: paths plus a counter that changes with them.
#[derive(Default)]
pub struct Shelf {
    inner: Mutex<(u64, Vec<PathBuf>)>,
}

impl Shelf {
    pub fn set(&self, paths: Vec<PathBuf>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.1 != paths {
            g.0 += 1;
            g.1 = paths;
        }
    }

    fn get(&self) -> (u64, Vec<PathBuf>) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[derive(Clone)]
pub struct PeerCfg {
    pub pc_name: String,
    pub inbox: PathBuf,
    /// Local addresses to offer as host candidates (the Windows crate picks
    /// them: Wi-Fi and wired first, no VPN).
    pub local_ips: Vec<IpAddr>,
    /// STUN servers as `host:port`; empty in tests.
    pub stun: Vec<String>,
    pub shelf: Arc<Shelf>,
    /// This session's number (names its staging folder).
    pub n: u64,
}

/// What a session tells the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerEvent {
    /// The phone said who it is.
    Hello { phone: String },
    Progress { file: usize, files: usize, done: u64, total: u64 },
    /// Every file arrived and is in the inbox, in the order sent.
    Received { paths: Vec<PathBuf> },
    /// A send broke off; nothing from it was kept.
    Failed { reason: String, detail: String },
    /// The link is gone; `connected` = it ever worked.
    Ended { connected: bool },
}

struct Packet {
    data: Vec<u8>,
    source: SocketAddr,
    destination: SocketAddr,
}

pub struct Peer {
    rtc: Rtc,
    sockets: Vec<UdpSocket>,
    rx: Receiver<Packet>,
    stop: Arc<AtomicBool>,
    cfg: PeerCfg,
}

/// Turn the phone's offer into our answer. Binding the sockets and asking
/// STUN takes up to about a second.
pub fn answer(offer_sdp: &str, cfg: PeerCfg, tx_seed: u64) -> Result<(Peer, String), String> {
    let offer_sdp = clean_offer(offer_sdp);
    let offer = SdpOffer::from_sdp_string(&offer_sdp).map_err(|e| format!("bad offer: {e}"))?;
    let mut rtc = Rtc::builder().build(Instant::now());

    let mut sockets = Vec::new();
    for ip in &cfg.local_ips {
        match UdpSocket::bind(SocketAddr::new(*ip, 0)) {
            Ok(s) => sockets.push(s),
            Err(e) => crate::pairdrop::log(&format!("pair drop: no socket on {ip}: {e}")),
        }
    }
    if sockets.is_empty() {
        return Err("no network address to listen on".into());
    }

    // Ask STUN from every IPv4 socket at once.
    let reflexive: Vec<Option<SocketAddr>> = if cfg.stun.is_empty() {
        vec![None; sockets.len()]
    } else {
        thread::scope(|sc| {
            let handles: Vec<_> = sockets
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let servers = &cfg.stun;
                    sc.spawn(move || {
                        let local = s.local_addr().ok()?;
                        if !local.is_ipv4() {
                            return None;
                        }
                        let tx = tx_id(tx_seed.wrapping_add(i as u64));
                        stun::public_address(s, servers, tx, STUN_BUDGET).filter(|a| *a != local)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().ok().flatten()).collect()
        })
    };

    for (s, refl) in sockets.iter().zip(&reflexive) {
        let local = s.local_addr().map_err(|e| e.to_string())?;
        if let Ok(c) = Candidate::host(local, "udp") {
            rtc.add_local_candidate(c);
        }
        if let Some(public) = refl {
            if let Ok(c) = Candidate::server_reflexive(*public, local, "udp") {
                rtc.add_local_candidate(c);
            }
        }
    }

    let answer = rtc.sdp_api().accept_offer(offer).map_err(|e| format!("couldn't answer: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    for s in &sockets {
        let s = s.try_clone().map_err(|e| e.to_string())?;
        let tx = tx.clone();
        let stop = stop.clone();
        thread::Builder::new().name("pair-drop-udp".into()).spawn(move || read_loop(s, tx, stop)).map_err(|e| e.to_string())?;
    }
    Ok((Peer { rtc, sockets, rx, stop, cfg }, answer.to_sdp_string()))
}

fn tx_id(seed: u64) -> [u8; 12] {
    // Not a secret: only has to differ between questions.
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut out = [0u8; 12];
    out[..8].copy_from_slice(&(t ^ seed.rotate_left(17)).to_le_bytes());
    out[8..].copy_from_slice(&(seed as u32 ^ 0x9e37_79b9).to_le_bytes());
    out
}

fn read_loop(s: UdpSocket, tx: mpsc::Sender<Packet>, stop: Arc<AtomicBool>) {
    let Ok(destination) = s.local_addr() else { return };
    let _ = s.set_read_timeout(Some(Duration::from_millis(250)));
    let mut buf = vec![0u8; 2048];
    while !stop.load(Ordering::SeqCst) {
        match s.recv_from(&mut buf) {
            Ok((n, source)) => {
                if tx.send(Packet { data: buf[..n].to_vec(), source, destination }).is_err() {
                    return;
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            // Windows reports an ICMP "port unreachable" from an earlier send
            // as a reset on the next read; the socket is still fine.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(_) => return,
        }
    }
}

/// Browsers hide their own host address behind a random `….local` name
/// (mDNS). Resolve those we can, briefly; drop the rest (the phone's
/// connectivity checks still reach us and reveal its real address).
pub fn clean_offer(sdp: &str) -> String {
    let mut out = String::with_capacity(sdp.len());
    for line in sdp.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if let Some(rest) = body.strip_prefix("a=candidate:") {
            let parts: Vec<&str> = rest.split(' ').collect();
            if parts.len() >= 6 && parts[4].parse::<IpAddr>().is_err() {
                if let Some(ip) = resolve_mdns(parts[4], parts[5]) {
                    let mut p = parts.clone();
                    let ip = ip.to_string();
                    p[4] = &ip;
                    out.push_str("a=candidate:");
                    out.push_str(&p.join(" "));
                    out.push_str("\r\n");
                }
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

fn resolve_mdns(host: &str, port: &str) -> Option<IpAddr> {
    if !host.ends_with(".local") || host.len() > 80 {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    let target = format!("{host}:{port}");
    let (tx, rx) = mpsc::channel();
    // getaddrinfo can hang on a name nobody answers; don't wait for it.
    thread::Builder::new()
        .name("pair-drop-mdns".into())
        .spawn(move || {
            let r = target.to_socket_addrs().ok().and_then(|mut a| a.next()).map(|a| a.ip());
            let _ = tx.send(r);
        })
        .ok()?;
    rx.recv_timeout(Duration::from_millis(600)).ok().flatten()
}

impl Peer {
    /// Run the link until it ends. Blocks; call on its own thread.
    pub fn run(mut self, emit: impl Fn(PeerEvent)) {
        let started = Instant::now();
        let mut s = Session::new(&self.cfg);
        let mut connected = false;
        // Drain everything str0m wants done (single-mutation rule); `None`
        // means the link broke.
        while let Some(timeout) = self.drain(&mut s, &mut connected, &emit) {
            if s.ended || !self.rtc.is_alive() {
                break;
            }
            if !connected && started.elapsed() > CONNECT_WITHIN {
                log("pair drop: the phone never connected");
                break;
            }
            if connected && s.last_heard.elapsed() > IDLE {
                log("pair drop: the phone went quiet");
                break;
            }
            // One mutation: a queued write, else a packet or the clock.
            if s.try_write(&mut self.rtc) {
                continue;
            }
            let wait = timeout.saturating_duration_since(Instant::now()).min(Duration::from_millis(250)).max(Duration::from_millis(1));
            let packet = match self.rx.recv_timeout(wait) {
                Ok(p) => Some(p),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let input = match &packet {
                Some(p) => {
                    let Ok(contents) = p.data.as_slice().try_into() else { continue };
                    Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: p.source, destination: p.destination, contents })
                }
                None => Input::Timeout(Instant::now()),
            };
            if let Err(e) = self.rtc.handle_input(input) {
                log(&format!("pair drop: link error: {e}"));
                break;
            }
        }
        s.finish(&emit);
        self.stop.store(true, Ordering::SeqCst);
        emit(PeerEvent::Ended { connected });
    }

    /// Poll until str0m only has a timeout left. `None` = the link broke.
    fn drain(&mut self, s: &mut Session, connected: &mut bool, emit: &impl Fn(PeerEvent)) -> Option<Instant> {
        loop {
            let out = match self.rtc.poll_output() {
                Ok(o) => o,
                Err(e) => {
                    log(&format!("pair drop: link error: {e}"));
                    return None;
                }
            };
            match out {
                Output::Timeout(t) => return Some(t),
                Output::Transmit(t) => {
                    let sock = self.sockets.iter().find(|x| x.local_addr().ok() == Some(t.source)).or_else(|| self.sockets.iter().find(|x| x.local_addr().is_ok_and(|a| a.is_ipv4() == t.destination.is_ipv4())));
                    if let Some(sock) = sock {
                        let _ = sock.send_to(&t.contents, t.destination);
                    }
                }
                Output::Event(e) => match e {
                    RtcEvent::Connected => *connected = true,
                    RtcEvent::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        s.ended = true;
                    }
                    RtcEvent::ChannelOpen(id, label) if label == LABEL => {
                        s.channel = Some(id);
                        s.last_heard = Instant::now();
                        if let Some(mut ch) = self.rtc.channel(id) {
                            ch.set_buffered_amount_low_threshold(OUT_BUFFER / 4);
                        }
                    }
                    RtcEvent::ChannelData(d) if Some(d.id) == s.channel => {
                        s.last_heard = Instant::now();
                        if d.binary {
                            s.on_bytes(&d.data, emit);
                        } else {
                            s.on_text(&d.data, emit);
                        }
                    }
                    RtcEvent::ChannelClose(id) if Some(id) == s.channel => s.ended = true,
                    _ => {}
                },
            }
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// The data channel protocol for one session.
struct Session {
    pc_name: String,
    inbox: PathBuf,
    shelf: Arc<Shelf>,
    n: u64,
    channel: Option<ChannelId>,
    ended: bool,
    last_heard: Instant,
    phone: Option<String>,
    /// Frames waiting to go out: (binary, bytes).
    queue: VecDeque<(bool, Vec<u8>)>,
    incoming: Option<Incoming>,
    sends: u64,
    outgoing: Option<Outgoing>,
    /// Shelf generation the phone last heard about.
    shelf_gen: Option<u64>,
    shelf_ids: HashMap<PathBuf, u64>,
    shelf_now: Vec<ShelfFile>,
    progress_at: Instant,
    closing: bool,
}

impl Session {
    fn new(cfg: &PeerCfg) -> Session {
        Session {
            pc_name: cfg.pc_name.clone(),
            inbox: cfg.inbox.clone(),
            shelf: cfg.shelf.clone(),
            n: cfg.n,
            channel: None,
            ended: false,
            last_heard: Instant::now(),
            phone: None,
            queue: VecDeque::new(),
            incoming: None,
            sends: 0,
            outgoing: None,
            shelf_gen: None,
            shelf_ids: HashMap::new(),
            shelf_now: Vec::new(),
            progress_at: Instant::now(),
            closing: false,
        }
    }

    fn text(&mut self, s: String) {
        self.queue.push_back((false, s.into_bytes()));
    }

    /// The Shelf files a phone may save now, with ids stable for this session.
    fn shelf_files(&mut self) -> Vec<ShelfFile> {
        let (generation, paths) = self.shelf.get();
        self.shelf_gen = Some(generation);
        let mut files = Vec::new();
        for p in paths {
            let Ok(md) = std::fs::metadata(&p) else { continue };
            if !md.is_file() {
                continue;
            }
            let next = self.shelf_ids.len() as u64;
            let id = *self.shelf_ids.entry(p.clone()).or_insert(next);
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            files.push(ShelfFile { id, path: p, name, size: md.len() });
        }
        self.shelf_now = files.clone();
        files
    }

    fn on_text(&mut self, data: &[u8], emit: &impl Fn(PeerEvent)) {
        let Some(msg) = transfer::parse(data) else { return };
        match msg {
            FromPhone::Hello { name } => {
                let phone = if name.is_empty() { "Phone".to_string() } else { name };
                self.phone = Some(phone.clone());
                emit(PeerEvent::Hello { phone });
                let files = self.shelf_files();
                let hello = transfer::hello(&self.pc_name, &files);
                self.text(hello);
            }
            FromPhone::Send(list) => {
                if self.incoming.is_some() {
                    self.text(transfer::no("busy"));
                    return;
                }
                let files = match transfer::check_send(&list) {
                    Ok(f) => f,
                    Err(why) => {
                        self.text(transfer::no(why));
                        return;
                    }
                };
                self.sends += 1;
                match Incoming::start(&self.inbox, self.n * 1000 + self.sends, files) {
                    Ok(mut inc) => {
                        self.text(transfer::ok());
                        emit(PeerEvent::Progress { file: 0, files: inc.files(), done: 0, total: inc.total });
                        match inc.finished_without_bytes() {
                            Ok(Some(paths)) => self.done(paths, 0, emit),
                            Ok(None) => self.incoming = Some(inc),
                            Err(b) => self.broke(b, emit),
                        }
                    }
                    Err(b) => self.broke(b, emit),
                }
            }
            FromPhone::Get { id } => {
                if self.outgoing.is_some() {
                    self.text(transfer::nofile(id));
                    return;
                }
                let hit = self.shelf_now.iter().find(|f| f.id == id).cloned();
                match hit.map(|f| Outgoing::open(&f).map(|o| (f, o))) {
                    Some(Ok((f, o))) => {
                        self.text(transfer::file_head(&f));
                        self.outgoing = Some(o);
                    }
                    _ => self.text(transfer::nofile(id)),
                }
            }
            FromPhone::Bye => self.closing = true,
            FromPhone::Other => {}
        }
    }

    fn on_bytes(&mut self, data: &[u8], emit: &impl Fn(PeerEvent)) {
        let Some(inc) = self.incoming.as_mut() else { return };
        match inc.take(data) {
            Ok(None) => {
                if self.progress_at.elapsed() >= PROGRESS_EVERY {
                    self.progress_at = Instant::now();
                    let (file, files, done, total) = (inc.current(), inc.files(), inc.done, inc.total);
                    emit(PeerEvent::Progress { file, files, done, total });
                    self.text(transfer::got(done));
                }
            }
            Ok(Some(paths)) => {
                let total = inc.total;
                self.incoming = None;
                self.done(paths, total, emit);
            }
            Err(b) => {
                self.incoming = None;
                self.broke(b, emit);
            }
        }
    }

    fn done(&mut self, paths: Vec<PathBuf>, total: u64, emit: &impl Fn(PeerEvent)) {
        let n = paths.len();
        self.text(transfer::got(total));
        self.text(transfer::saved(n));
        emit(PeerEvent::Received { paths });
    }

    fn broke(&mut self, b: transfer::Broken, emit: &impl Fn(PeerEvent)) {
        self.text(transfer::failed(b.why));
        let reason = match b.why {
            "size" => "A file didn't match the size the phone announced",
            "disk" => "Couldn't save the files",
            _ => "The send stopped",
        };
        emit(PeerEvent::Failed { reason: reason.into(), detail: b.detail });
    }

    /// Write one queued frame, or the next frame of a Shelf file. True when
    /// something was written (a mutation: the caller drains str0m next).
    fn try_write(&mut self, rtc: &mut Rtc) -> bool {
        let Some(id) = self.channel else { return false };
        // A Shelf change since the phone last heard: tell it.
        if self.phone.is_some() && self.queue.is_empty() && self.outgoing.is_none() {
            let (generation, _) = self.shelf.get();
            if self.shelf_gen != Some(generation) {
                let files = self.shelf_files();
                self.text(transfer::shelf(&files));
            }
        }
        let Some(mut ch) = rtc.channel(id) else {
            self.ended = true;
            return false;
        };
        if self.queue.is_empty() {
            if let Some(o) = self.outgoing.as_mut() {
                if ch.buffered_amount() < OUT_BUFFER {
                    match o.next_frame() {
                        Ok(frame) if !frame.is_empty() => self.queue.push_back((true, frame)),
                        Ok(_) => {}
                        Err(e) => {
                            log(&format!("pair drop: reading a Shelf file failed: {e}"));
                            self.ended = true;
                        }
                    }
                    if o.left == 0 {
                        self.outgoing = None;
                    }
                }
            } else if self.closing {
                self.ended = true;
                return false;
            }
        }
        let Some((binary, bytes)) = self.queue.front() else { return false };
        match ch.write(*binary, bytes) {
            Ok(true) => {
                self.queue.pop_front();
                true
            }
            // Buffer full: wait for packets to drain it.
            Ok(false) => false,
            Err(e) => {
                log(&format!("pair drop: write failed: {e}"));
                self.ended = true;
                false
            }
        }
    }

    fn finish(&mut self, emit: &impl Fn(PeerEvent)) {
        if let Some(inc) = self.incoming.take() {
            drop(inc);
            emit(PeerEvent::Failed { reason: "The connection dropped".into(), detail: String::new() });
        }
    }
}

fn log(s: &str) {
    crate::pairdrop::log(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_hidden_host_candidates() {
        let sdp = "v=0\r\na=candidate:1 1 udp 2113937151 4a3c1f2e-ab.local 54321 typ host generation 0\r\na=candidate:2 1 udp 1677729535 203.0.113.9 40000 typ srflx raddr 0.0.0.0 rport 0\r\na=end-of-candidates\r\n";
        let out = clean_offer(sdp);
        assert!(!out.contains(".local"));
        assert!(out.contains("203.0.113.9 40000 typ srflx"));
        assert!(out.starts_with("v=0\r\n"));
        assert!(out.ends_with("a=end-of-candidates\r\n"));
    }
}
