//! End to end on loopback: a second str0m instance plays the phone, a fake
//! meeting server passes the messages, and the real PC side pairs, answers
//! and receives files.

use super::keys::{b64u, Secret};
use super::meet::{Book, Meet, MeetCfg, MeetEvent, Post};
use super::peer::{self, PeerCfg, PeerEvent, Shelf};
use super::signal::{now_ms, Msg};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::channel::ChannelId;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("poni-pairdrop-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn secret(seed: u8) -> Secret {
    let mut b = [0u8; 16];
    for (i, x) in b.iter_mut().enumerate() {
        *x = seed.wrapping_mul(31).wrapping_add(i as u8);
    }
    Secret::from_bytes(b)
}

fn sid(seed: u8) -> String {
    b64u(&[seed; 16])
}

/// The phone's half of a WebRTC link, driven by hand.
struct Phone {
    rtc: Rtc,
    sock: UdpSocket,
    channel: Option<ChannelId>,
    pending: Option<SdpPendingOffer>,
    inbox: VecDeque<(bool, Vec<u8>)>,
    connected: bool,
}

impl Phone {
    fn new() -> (Phone, String) {
        let mut rtc = Rtc::builder().build(Instant::now());
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        rtc.add_local_candidate(Candidate::host(sock.local_addr().unwrap(), "udp").unwrap());
        let mut change = rtc.sdp_api();
        let _ = change.add_channel(peer::LABEL.to_string());
        let (offer, pending) = change.apply().unwrap();
        (Phone { rtc, sock, channel: None, pending: Some(pending), inbox: VecDeque::new(), connected: false }, offer.to_sdp_string())
    }

    fn accept(&mut self, answer: &str) {
        let a = SdpAnswer::from_sdp_string(answer).unwrap();
        self.rtc.sdp_api().accept_answer(self.pending.take().unwrap(), a).unwrap();
    }

    /// Drive the link until `done` says stop or `limit` passes.
    fn pump(&mut self, limit: Duration, mut done: impl FnMut(&mut Phone) -> bool) -> bool {
        let end = Instant::now() + limit;
        let mut buf = vec![0u8; 2048];
        while Instant::now() < end {
            let timeout = loop {
                match self.rtc.poll_output().unwrap() {
                    Output::Timeout(t) => break t,
                    Output::Transmit(t) => {
                        self.sock.send_to(&t.contents, t.destination).unwrap();
                    }
                    Output::Event(e) => match e {
                        Event::Connected => self.connected = true,
                        Event::ChannelOpen(id, _) => self.channel = Some(id),
                        Event::ChannelData(d) => self.inbox.push_back((d.binary, d.data)),
                        _ => {}
                    },
                }
            };
            if done(self) {
                return true;
            }
            let wait = timeout.saturating_duration_since(Instant::now()).clamp(Duration::from_millis(1), Duration::from_millis(50));
            self.sock.set_read_timeout(Some(wait)).unwrap();
            let input = match self.sock.recv_from(&mut buf) {
                Ok((n, source)) => Input::Receive(
                    Instant::now(),
                    Receive { proto: Protocol::Udp, source, destination: self.sock.local_addr().unwrap(), contents: buf[..n].try_into().unwrap() },
                ),
                Err(_) => Input::Timeout(Instant::now()),
            };
            self.rtc.handle_input(input).unwrap();
        }
        false
    }

    /// Write one frame, draining between tries until str0m takes it.
    fn write(&mut self, binary: bool, data: &[u8]) {
        let id = self.channel.unwrap();
        loop {
            if self.rtc.channel(id).unwrap().write(binary, data).unwrap() {
                break;
            }
            self.pump(Duration::from_millis(20), |_| false);
        }
        self.pump(Duration::from_millis(1), |_| true);
    }

    fn text(&mut self, v: Value) {
        self.write(false, v.to_string().as_bytes());
    }

    /// Wait for the next text message of type `t`.
    fn expect(&mut self, t: &str, limit: Duration) -> Value {
        let mut found = None;
        let t = t.to_string();
        self.pump(limit, |p| {
            while let Some((binary, data)) = p.inbox.pop_front() {
                if !binary {
                    let v: Value = serde_json::from_slice(&data).unwrap();
                    if v["t"] == t.as_str() {
                        found = Some(v);
                        return true;
                    }
                }
            }
            false
        });
        found.unwrap_or_else(|| panic!("no {t} from the PC"))
    }
}

fn shelf_with(dir: &Path) -> (Arc<Shelf>, Vec<u8>) {
    let shelf_file = dir.join("on-the-shelf.bin");
    let data: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 251) as u8).collect();
    std::fs::write(&shelf_file, &data).unwrap();
    let shelf = Arc::new(Shelf::default());
    shelf.set(vec![shelf_file, dir.join("missing.txt")]);
    (shelf, data)
}

#[test]
fn a_phone_sends_files_and_saves_one_over_a_direct_link() {
    let dir = temp("direct");
    let inbox = dir.join("inbox");
    let (shelf, shelf_bytes) = shelf_with(&dir);
    let (mut phone, offer) = Phone::new();
    let cfg = PeerCfg { pc_name: "Test PC".into(), inbox: inbox.clone(), local_ips: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)], stun: vec![], shelf, n: 1 };
    let (pc, answer) = peer::answer(&offer, cfg, 1).unwrap();
    assert!(answer.contains("a=candidate"), "the answer carries the PC's address");
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let pc_thread = std::thread::spawn(move || pc.run(|e| tx.lock().unwrap().send(e).unwrap()));
    phone.accept(&answer);

    assert!(phone.pump(Duration::from_secs(10), |p| p.channel.is_some()), "the data channel opens");
    phone.text(json!({ "t": "hello", "v": 1, "name": "Test Phone" }));
    let hello = phone.expect("hello", Duration::from_secs(5));
    assert_eq!(hello["name"], "Test PC");
    assert_eq!(hello["auto"], true);
    let files = hello["shelf"].as_array().unwrap();
    assert_eq!(files.len(), 1, "missing files aren't offered");
    assert_eq!(files[0]["name"], "on-the-shelf.bin");

    // Two files: one bigger than many frames, one empty; then a third.
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
    phone.text(json!({ "t": "send", "files": [{ "name": "big photo.jpg", "size": big.len() }, { "name": "empty.txt", "size": 0 }, { "name": "note:1.txt", "size": 5 }] }));
    assert_eq!(phone.expect("ok", Duration::from_secs(5))["t"], "ok");
    let mut stream = big.clone();
    stream.extend_from_slice(b"hello");
    for frame in stream.chunks(16 * 1024) {
        phone.write(true, frame);
    }
    let saved = phone.expect("saved", Duration::from_secs(10));
    assert_eq!(saved["files"], 3);

    // Save the Shelf file on the phone.
    phone.text(json!({ "t": "get", "id": files[0]["id"] }));
    let head = phone.expect("file", Duration::from_secs(5));
    assert_eq!(head["size"], shelf_bytes.len());
    let mut got = Vec::new();
    let want = shelf_bytes.len();
    phone.pump(Duration::from_secs(10), |p| {
        while let Some((binary, data)) = p.inbox.pop_front() {
            if binary {
                got.extend(data);
            }
        }
        got.len() >= want
    });
    assert_eq!(got, shelf_bytes);
    phone.text(json!({ "t": "get", "id": 99 }));
    assert_eq!(phone.expect("nofile", Duration::from_secs(5))["id"], 99);
    phone.text(json!({ "t": "bye" }));
    phone.pump(Duration::from_millis(300), |_| false);

    pc_thread.join().unwrap();
    let evs: Vec<PeerEvent> = events.try_iter().collect();
    assert!(evs.contains(&PeerEvent::Hello { phone: "Test Phone".into() }));
    let received = evs.iter().find_map(|e| if let PeerEvent::Received { paths } = e { Some(paths.clone()) } else { None }).expect("files arrived");
    assert_eq!(received.len(), 3);
    assert_eq!(std::fs::read(&received[0]).unwrap(), big);
    assert_eq!(received[0].file_name().unwrap(), "big photo.jpg");
    assert_eq!(std::fs::read(&received[1]).unwrap(), b"");
    assert_eq!(received[2].file_name().unwrap(), "note_1.txt");
    assert_eq!(std::fs::read(&received[2]).unwrap(), b"hello");
    assert!(matches!(evs.last(), Some(PeerEvent::Ended { connected: true })));
    let leftovers: Vec<_> = std::fs::read_dir(&inbox).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with('.')).collect();
    assert!(leftovers.is_empty(), "no staging folder is left behind");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cut_send_keeps_nothing() {
    let dir = temp("cut");
    let inbox = dir.join("inbox");
    let (mut phone, offer) = Phone::new();
    let cfg = PeerCfg { pc_name: "Test PC".into(), inbox: inbox.clone(), local_ips: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)], stun: vec![], shelf: Arc::new(Shelf::default()), n: 2 };
    let (pc, answer) = peer::answer(&offer, cfg, 2).unwrap();
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let pc_thread = std::thread::spawn(move || pc.run(|e| tx.lock().unwrap().send(e).unwrap()));
    phone.accept(&answer);
    assert!(phone.pump(Duration::from_secs(10), |p| p.channel.is_some()));
    phone.text(json!({ "t": "hello", "v": 1, "name": "Test Phone" }));
    phone.expect("hello", Duration::from_secs(5));
    phone.text(json!({ "t": "send", "files": [{ "name": "half.bin", "size": 100_000 }] }));
    phone.expect("ok", Duration::from_secs(5));
    phone.write(true, &[1u8; 10_000]);
    // The phone vanishes (closes the channel).
    let id = phone.channel.unwrap();
    phone.rtc.direct_api().close_data_channel(id);
    phone.pump(Duration::from_millis(500), |_| false);
    drop(phone);
    pc_thread.join().unwrap();
    let evs: Vec<PeerEvent> = events.try_iter().collect();
    assert!(evs.iter().any(|e| matches!(e, PeerEvent::Failed { .. })), "{evs:?}");
    assert!(!evs.iter().any(|e| matches!(e, PeerEvent::Received { .. })));
    assert_eq!(std::fs::read_dir(&inbox).unwrap().count(), 0, "nothing kept");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A meeting server in memory.
#[derive(Default)]
struct Fake {
    st: Mutex<FakeState>,
    cv: Condvar,
}

#[derive(Default)]
struct FakeState {
    to_pc: Vec<(String, Msg)>,
    to_phone: Vec<Msg>,
    rooms: Vec<Vec<String>>,
    kicks: usize,
    unpaired: Vec<String>,
    gone: Vec<String>,
}

impl Post for Fake {
    fn post(&self, path: &str, body: &Value, _timeout: Duration) -> Result<(u16, Value), String> {
        let mut st = self.st.lock().unwrap();
        match path {
            "/api/pc/wait" => {
                let rooms: Vec<String> = body["rooms"].as_array().unwrap().iter().map(|r| r["room"].as_str().unwrap().to_string()).collect();
                assert_eq!(body["client"], "AAAAAAAAAAAAAAAAAAAAAA");
                st.rooms.push(rooms.clone());
                let end = Instant::now() + Duration::from_millis(200);
                while !st.to_pc.iter().any(|(r, _)| rooms.contains(r)) && Instant::now() < end {
                    st = self.cv.wait_timeout(st, Duration::from_millis(20)).unwrap().0;
                }
                let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut st.to_pc).into_iter().partition(|(r, _)| rooms.contains(r));
                st.to_pc = rest;
                let msgs: Vec<Value> = mine.into_iter().map(|(room, msg)| json!({ "room": room, "msg": msg })).collect();
                let gone: Vec<String> = st.gone.iter().filter(|g| rooms.contains(g)).cloned().collect();
                Ok((200, json!({ "msgs": msgs, "bad": [], "gone": gone })))
            }
            "/api/pc/send" => {
                st.to_phone.push(serde_json::from_value(body["msg"].clone()).unwrap());
                self.cv.notify_all();
                Ok((200, json!({ "ok": true })))
            }
            "/api/pc/kick" => {
                st.kicks += 1;
                Ok((200, json!({ "ok": true })))
            }
            "/api/pc/unpair" => {
                st.unpaired.push(body["room"].as_str().unwrap().to_string());
                Ok((200, json!({ "ok": true })))
            }
            _ => Ok((404, Value::Null)),
        }
    }
}

impl Fake {
    fn phone_says(&self, room: &str, m: Msg) {
        self.st.lock().unwrap().to_pc.push((room.to_string(), m));
        self.cv.notify_all();
    }

    fn reply_to(&self, sid: &str, limit: Duration) -> Msg {
        let end = Instant::now() + limit;
        let mut st = self.st.lock().unwrap();
        loop {
            if let Some(i) = st.to_phone.iter().position(|m| m.sid == sid) {
                return st.to_phone.remove(i);
            }
            assert!(Instant::now() < end, "no reply for {sid}");
            st = self.cv.wait_timeout(st, Duration::from_millis(20)).unwrap().0;
        }
    }
}

fn wait_event(rx: &Receiver<MeetEvent>, limit: Duration, mut want: impl FnMut(&MeetEvent) -> bool) -> MeetEvent {
    let end = Instant::now() + limit;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let e = rx.recv_timeout(left).expect("event in time");
        if want(&e) {
            return e;
        }
    }
}

#[test]
fn pairing_then_sending_through_the_meeting_server() {
    let dir = temp("meet");
    let inbox = dir.join("inbox");
    let fake = Arc::new(Fake::default());
    let saved: Arc<Mutex<Vec<Book>>> = Arc::default();
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let cfg = MeetCfg {
        server: "https://drop.example".into(),
        pc_name: "Test PC".into(),
        inbox: inbox.clone(),
        stun: vec![],
        client: "AAAAAAAAAAAAAAAAAAAAAA".into(),
        local_ips: Arc::new(|| vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]),
    };
    let saved2 = saved.clone();
    let meet = Meet::start(
        cfg,
        fake.clone(),
        Book::default(),
        Arc::new(move |e| tx.lock().unwrap().send(e).unwrap()),
        Arc::new(move |b: &Book| saved2.lock().unwrap().push(b.clone())),
    )
    .unwrap();

    // The QR code: the secret rides after the '#'.
    let k = secret(7);
    let link = meet.begin_pairing(k.clone());
    assert_eq!(link, format!("https://drop.example/p#k={}", k.encode()));
    let room = k.room();

    // A forged pair (wrong key) is ignored; the real one pairs.
    let forged = Msg::signed(&secret(8), "pair", &sid(1), "Evil", "", now_ms());
    fake.phone_says(&room, forged);
    fake.phone_says(&room, Msg::signed(&k, "pair", &sid(2), "Test Phone", "", now_ms()));
    let welcome = fake.reply_to(&sid(2), Duration::from_secs(5));
    assert_eq!(welcome.t, "welcome");
    assert_eq!(welcome.name, "Test PC");
    assert!(welcome.verify(&k, now_ms()).is_ok(), "the PC signs with the pair's key");
    match wait_event(&events, Duration::from_secs(2), |e| matches!(e, MeetEvent::Paired { .. })) {
        MeetEvent::Paired { name, .. } => assert_eq!(name, "Test Phone"),
        _ => unreachable!(),
    }
    let book = saved.lock().unwrap().last().cloned().unwrap();
    assert_eq!(book.pairs.len(), 1);
    assert_eq!(book.pairs[0].name, "Test Phone");
    assert_eq!(book.pairs[0].k, k.encode());
    let end = Instant::now() + Duration::from_secs(3);
    while fake.st.lock().unwrap().kicks == 0 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fake.st.lock().unwrap().kicks >= 1, "showing the QR ends the open wait");

    // Connect and send; the same offer sent again gets no second answer.
    let (mut phone, offer) = Phone::new();
    let offer_msg = Msg::signed(&k, "offer", &sid(3), "Test Phone", &offer, now_ms());
    fake.phone_says(&room, offer_msg.clone());
    let answer = fake.reply_to(&sid(3), Duration::from_secs(5));
    assert_eq!(answer.t, "answer");
    assert!(answer.verify(&k, now_ms()).is_ok());
    fake.phone_says(&room, offer_msg);
    phone.accept(&answer.sdp);
    assert!(phone.pump(Duration::from_secs(10), |p| p.channel.is_some()));
    phone.text(json!({ "t": "hello", "v": 1, "name": "Test Phone" }));
    phone.expect("hello", Duration::from_secs(5));
    phone.text(json!({ "t": "send", "files": [{ "name": "a.txt", "size": 3 }] }));
    phone.expect("ok", Duration::from_secs(5));
    phone.write(true, b"abc");
    phone.expect("saved", Duration::from_secs(5));
    match wait_event(&events, Duration::from_secs(5), |e| matches!(e, MeetEvent::Phone { ev: PeerEvent::Received { .. }, .. })) {
        MeetEvent::Phone { phone: who, ev: PeerEvent::Received { paths }, room: r } => {
            assert_eq!(who, "Test Phone");
            assert_eq!(r, room);
            assert_eq!(std::fs::read(&paths[0]).unwrap(), b"abc");
        }
        _ => unreachable!(),
    }
    phone.text(json!({ "t": "bye" }));
    phone.pump(Duration::from_millis(300), |_| false);
    assert!(fake.st.lock().unwrap().to_phone.is_empty(), "the replayed offer got no answer");

    // Removing the phone tells the server and stops listening on its room.
    meet.remove(&room);
    let end = Instant::now() + Duration::from_secs(5);
    while fake.st.lock().unwrap().unpaired.is_empty() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fake.st.lock().unwrap().unpaired, vec![room.clone()]);
    assert!(meet.pairs().is_empty());
    assert!(saved.lock().unwrap().last().unwrap().unpair.is_empty(), "the server heard, so nothing is left to retry");
    meet.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rooms_the_server_calls_gone_are_dropped() {
    let dir = temp("gone");
    let fake = Arc::new(Fake::default());
    let k = secret(9);
    fake.st.lock().unwrap().gone.push(k.room());
    let book = Book { pairs: vec![super::meet::Pair { k: k.encode(), name: "Old phone".into(), added: 1, seen: 0 }], unpair: vec![] };
    let (tx, events) = channel();
    let tx = Mutex::new(tx);
    let cfg = MeetCfg {
        server: "https://drop.example".into(),
        pc_name: "Test PC".into(),
        inbox: dir.join("inbox"),
        stun: vec![],
        client: "AAAAAAAAAAAAAAAAAAAAAA".into(),
        local_ips: Arc::new(Vec::new),
    };
    let meet = Meet::start(cfg, fake, book, Arc::new(move |e| tx.lock().unwrap().send(e).unwrap()), Arc::new(|_: &Book| {})).unwrap();
    wait_event(&events, Duration::from_secs(3), |e| matches!(e, MeetEvent::Pairs(p) if p.is_empty()));
    assert!(meet.pairs().is_empty());
    meet.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hidden_phone_addresses_still_connect() {
    // Browsers put `….local` names in place of their host address; the PC
    // drops those and the phone's checks still find it.
    let (mut phone, offer) = Phone::new();
    let port = phone.sock.local_addr().unwrap().port();
    let hidden = offer.replace(&format!("127.0.0.1 {port} typ host"), &format!("1f0e9d8c-7b6a-4c5d-9e8f-0a1b2c3d4e5f.local {port} typ host"));
    assert_ne!(hidden, offer);
    let dir = temp("mdns");
    let cfg = PeerCfg {
        pc_name: "Test PC".into(),
        inbox: dir.join("inbox"),
        local_ips: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        stun: vec![],
        shelf: Arc::new(Shelf::default()),
        n: 3,
    };
    let (pc, answer) = peer::answer(&hidden, cfg, 3).unwrap();
    let pc_thread = std::thread::spawn(move || pc.run(|_| {}));
    phone.accept(&answer);
    assert!(phone.pump(Duration::from_secs(10), |p| p.channel.is_some()), "connected through the phone's own checks");
    phone.text(json!({ "t": "bye" }));
    phone.pump(Duration::from_millis(300), |_| false);
    pc_thread.join().unwrap();
    let _: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
