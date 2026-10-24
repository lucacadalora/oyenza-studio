//! Paired phone drop on Windows: runs `poni_core::pairdrop` with HTTPS
//! through Windows' own HTTP client, the pair list sealed with DPAPI at
//! `paths::paired_phones_path()`, and the network addresses phone drop
//! already knows how to pick.
//!
//! With no paired phone it asks nothing at all. With one or more, a worker
//! thread keeps one request open at the meeting server (about one small
//! HTTPS request every 25 s, no CPU in between), so a phone on any network
//! finds this PC whenever Oyen Poni runs.

use crate::events::{AppEvent, EventSink};
use crate::services::http::{Client, FetchError, Method, MtaGuard};
use poni_core::pairdrop::keys::b64u;
use poni_core::pairdrop::{self, Book, Meet, MeetCfg, Pair, Post, Secret};
use serde_json::Value;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG};

const USER_AGENT: &str = concat!("OyenPoni/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/poni)");

pub struct PairDropService {
    meet: Meet,
}

impl PairDropService {
    pub fn start(sink: EventSink) -> windows::core::Result<Self> {
        pairdrop::set_logger(|line| crate::pinfo!("{line}"));
        let server = server();
        let client = random16().map(|b| b64u(&b)).ok_or_else(|| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "no random bytes"))?;
        let inbox = crate::paths::phone_inbox_dir();
        let cfg = MeetCfg {
            server: server.clone(),
            pc_name: super::phonedrop::pc_name(),
            inbox,
            stun: pairdrop::STUN_SERVERS.iter().map(|s| s.to_string()).collect(),
            client,
            local_ips: Arc::new(super::phonedrop::lan_addresses),
        };
        let book = load_book();
        if !book.pairs.is_empty() {
            crate::pinfo!("pair drop: {} paired phone(s), meeting server {server}", book.pairs.len());
        }
        let sink = Mutex::new(sink);
        let emit = Arc::new(move |e| {
            if let Ok(s) = sink.lock() {
                s.send(AppEvent::Paired(e));
            }
        });
        let save = Arc::new(|b: &Book| save_book(b));
        let meet = Meet::start(cfg, Arc::new(WinPost), book, emit, save)
            .map_err(|e| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, format!("could not start the pair-drop thread: {e}")))?;
        Ok(Self { meet })
    }

    /// Show a pairing QR code: returns the link it carries.
    pub fn begin_pairing(&self) -> Result<String, String> {
        let bytes = random16().ok_or_else(|| "Windows couldn't make a pairing code.".to_string())?;
        Ok(self.meet.begin_pairing(Secret::from_bytes(bytes)))
    }

    pub fn cancel_pairing(&self) {
        self.meet.cancel_pairing();
    }

    pub fn remove(&self, room: &str) {
        self.meet.remove(room);
    }

    pub fn set_shelf(&self, paths: Vec<PathBuf>) {
        self.meet.set_shelf(paths);
    }

    pub fn pairs(&self) -> Vec<Pair> {
        self.meet.pairs()
    }
}

/// The meeting server: `PONI_DROP_SERVER` (for self-hosting and tests), else Oyenza's.
fn server() -> String {
    std::env::var("PONI_DROP_SERVER")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| s.starts_with("https://") || s.starts_with("http://127.0.0.1") || s.starts_with("http://localhost"))
        .unwrap_or_else(|| pairdrop::DEFAULT_SERVER.to_string())
}

fn random16() -> Option<[u8; 16]> {
    let mut b = [0u8; 16];
    let status = unsafe { BCryptGenRandom(BCRYPT_ALG_HANDLE::default(), &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    status.is_ok().then_some(b)
}

fn load_book() -> Book {
    match super::secret::load(&crate::paths::paired_phones_path()) {
        Some(json) => Book::from_json(&json),
        None => Book::default(),
    }
}

fn save_book(b: &Book) {
    let path = crate::paths::paired_phones_path();
    let r = if b.pairs.is_empty() && b.unpair.is_empty() { super::secret::delete(&path) } else { super::secret::save(&path, &b.to_json()) };
    if let Err(e) = r {
        crate::pwarn!("pair drop: saving the paired phones failed: {e}");
    }
}

/// HTTPS POSTs through `Windows.Web.Http`, one client per calling thread
/// (the worker, each phone's session, the occasional kick).
struct WinPost;

thread_local! {
    // The client is released before COM is uninitialised (tuple fields drop in order).
    static CLIENT: RefCell<Option<(Client, MtaGuard)>> = const { RefCell::new(None) };
}

impl Post for WinPost {
    fn post(&self, path: &str, body: &Value, timeout: Duration) -> Result<(u16, Value), String> {
        let url = format!("{}{path}", server());
        CLIENT.with(|c| {
            let mut c = c.borrow_mut();
            if c.is_none() {
                let guard = MtaGuard::new();
                let client = Client::new(USER_AGENT).map_err(|e| e.to_string())?;
                *c = Some((client, guard));
            }
            let (client, _) = c.as_ref().ok_or("no HTTP client")?;
            match client.send(Method::Post, &url, &[], Some(&body.to_string()), timeout) {
                Ok(r) => Ok((r.status, serde_json::from_str(&r.body).unwrap_or(Value::Null))),
                Err(FetchError::Timeout) => Err("timed out".into()),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}
