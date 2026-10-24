//! The PC side of paired phone drop, on any OS, for testing the phone page
//! and the meeting server end to end with the real Rust code (Oyen Poni
//! itself runs the same `poni_core::pairdrop` behind its island).
//!
//! ```text
//! cargo run -p poni-core --example pairdrop_pc -- \
//!     --server http://127.0.0.1:8797 --inbox /tmp/inbox [--book pairs.json] \
//!     [--pair] [--shelf FILE]... [--name "Test PC"] [--stun host:port]
//! ```
//!
//! Prints `LINK <url>` for a pairing code and one `EVENT <json>` line per
//! event on stdout; runs until killed.

use poni_core::pairdrop::plainhttp::PlainHttp;
use poni_core::pairdrop::{Book, Meet, MeetCfg, MeetEvent, PeerEvent, Secret};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    let mut server = String::new();
    let mut inbox = PathBuf::from("pairdrop-inbox");
    let mut book_path: Option<PathBuf> = None;
    let mut pair = false;
    let mut shelf: Vec<PathBuf> = Vec::new();
    let mut name = "Test PC".to_string();
    let mut stun: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().expect("--server URL"),
            "--inbox" => inbox = PathBuf::from(args.next().expect("--inbox DIR")),
            "--book" => book_path = Some(PathBuf::from(args.next().expect("--book FILE"))),
            "--pair" => pair = true,
            "--shelf" => shelf.push(PathBuf::from(args.next().expect("--shelf FILE"))),
            "--name" => name = args.next().expect("--name NAME"),
            "--stun" => stun.push(args.next().expect("--stun HOST:PORT")),
            other => panic!("unknown argument {other}"),
        }
    }
    let post = Arc::new(PlainHttp::new(&server).expect("--server must be http://host:port"));
    std::fs::create_dir_all(&inbox).expect("inbox");
    poni_core::pairdrop::set_logger(|l| eprintln!("{l}"));
    let book = book_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| Book::from_json(&s)).unwrap_or_default();
    let save_to = book_path.clone();
    let save = Arc::new(move |b: &Book| {
        if let Some(p) = &save_to {
            let _ = std::fs::write(p, b.to_json());
        }
    });
    let emit = Arc::new(|e: MeetEvent| {
        let v = match e {
            MeetEvent::Pairs(p) => json!({ "type": "pairs", "names": p.iter().map(|x| x.name.clone()).collect::<Vec<_>>() }),
            MeetEvent::Paired { name, .. } => json!({ "type": "paired", "name": name }),
            MeetEvent::Server { reachable } => json!({ "type": "server", "reachable": reachable }),
            MeetEvent::Phone { phone, ev, .. } => match ev {
                PeerEvent::Hello { .. } => json!({ "type": "hello", "phone": phone }),
                PeerEvent::Progress { done, total, .. } => json!({ "type": "progress", "phone": phone, "done": done, "total": total }),
                PeerEvent::Received { paths } => json!({ "type": "received", "phone": phone, "paths": paths }),
                PeerEvent::Failed { reason, .. } => json!({ "type": "failed", "phone": phone, "reason": reason }),
                PeerEvent::Ended { connected } => json!({ "type": "ended", "phone": phone, "connected": connected }),
            },
        };
        println!("EVENT {v}");
        let _ = std::io::stdout().flush();
    });
    let cfg = MeetCfg { server: server.clone(), pc_name: name, inbox, stun, client: random_id(), local_ips: Arc::new(local_ips) };
    let meet = Meet::start(cfg, post, book, emit, save).expect("start");
    meet.set_shelf(shelf);
    if pair {
        let mut b = [0u8; 16];
        b.copy_from_slice(&random_bytes(16));
        println!("LINK {}", meet.begin_pairing(Secret::from_bytes(b)));
        let _ = std::io::stdout().flush();
    }
    loop {
        std::thread::park();
    }
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut v)).expect("random bytes");
    v
}

fn random_id() -> String {
    poni_core::pairdrop::keys::b64u(&random_bytes(16))
}

/// Loopback plus the address this machine uses for the internet.
fn local_ips() -> Vec<IpAddr> {
    let mut v = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
        if s.connect("192.0.2.1:9").is_ok() {
            if let Ok(a) = s.local_addr() {
                if !a.ip().is_unspecified() {
                    v.push(a.ip());
                }
            }
        }
    }
    v
}
