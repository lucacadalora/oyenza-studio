//! Phone drop on the Mac: picks the Mac's Wi-Fi or wired address, makes the
//! session secret and the inbox folder (`~/Downloads/Oyenza Studio`), and runs
//! `poni_core::phonedrop`'s web server while the Shelf's phone panel is open
//! (the same server the Windows app runs; only this glue differs).

use crate::events::{AppEvent, EventSink};
use objc2_core_foundation::{CFRetained, CFString};
use poni_core::phonedrop::{token, Config, Server};
use std::ffi::{c_char, c_void, CStr};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Tried first so the address stays the same from one session to the next;
/// any free port when it's taken.
const PORT: u16 = 53530;

pub struct PhoneDrop {
    server: Server,
}

impl PhoneDrop {
    /// Start serving and return the link for the QR code. `Err` holds a
    /// sentence for the island.
    pub fn start(sink: EventSink, offer: Vec<PathBuf>) -> Result<(PhoneDrop, String), String> {
        let ip = lan_ipv4().ok_or_else(|| "This Mac isn't on a Wi-Fi or wired network.".to_string())?;
        let inbox = crate::paths::phone_inbox_dir();
        std::fs::create_dir_all(&inbox).map_err(|e| format!("Couldn't make the folder {} ({e}).", inbox.display()))?;
        let secret = random_token().ok_or_else(|| "macOS couldn't make a secret link.".to_string())?;
        let sink = Mutex::new(sink);
        let send: Arc<dyn Fn(poni_core::phonedrop::Event) + Send + Sync> = Arc::new(move |e| {
            if let Ok(s) = sink.lock() {
                s.send(AppEvent::Phone(e));
            }
        });
        let cfg = Config::new(SocketAddr::new(IpAddr::V4(ip), PORT), secret, mac_name(), inbox.clone());
        let first = send.clone();
        let server = match Server::start(cfg.clone(), move |e| first(e)) {
            Ok(s) => s,
            Err(e) => {
                pinfo!("phone drop: port {PORT} unavailable ({e}), using another");
                let mut cfg = cfg;
                cfg.bind.set_port(0);
                let again = send.clone();
                Server::start(cfg, move |e| again(e)).map_err(|e| format!("Couldn't open a network port ({e})."))?
            }
        };
        server.set_offer(offer);
        // Never log the link: it carries the secret.
        pinfo!("phone drop: listening on {}, saving to {}", server.addr(), inbox.display());
        let url = server.url();
        Ok((PhoneDrop { server }, url))
    }

    pub fn answer(&self, id: u64, accept: bool) {
        self.server.answer(id, accept);
    }

    pub fn set_offer(&self, paths: Vec<PathBuf>) {
        self.server.set_offer(paths);
    }

    /// Seconds since a phone last asked for anything.
    pub fn idle_secs(&self) -> f64 {
        self.server.idle_secs()
    }

    /// A question is waiting or files are coming in.
    pub fn busy(&self) -> bool {
        self.server.busy()
    }

    pub fn stop(self) {
        self.server.stop();
        pinfo!("phone drop: stopped");
    }
}

/// 16 bytes from the system's random source as an upper-case base32 secret.
fn random_token() -> Option<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").ok()?.read_exact(&mut b).ok()?;
    Some(token::base32(&b))
}

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCopyComputerName(store: *const c_void, encoding: *mut u32) -> *const CFString;
}

/// The Mac's name in System Settings > General > About (also what
/// oyenza.app lists it as, `services::oyenza`).
pub(crate) fn computer_name() -> Option<String> {
    let raw = unsafe { SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut()) };
    std::ptr::NonNull::new(raw as *mut CFString).map(|p| unsafe { CFRetained::from_raw(p) }.to_string()).filter(|n| !n.trim().is_empty())
}

/// What the page calls this Mac.
fn mac_name() -> String {
    computer_name().unwrap_or_else(|| "your Mac".into())
}

// ---------------------------------------------------------------- the address

#[repr(C)]
struct IfAddrs {
    ifa_next: *mut IfAddrs,
    ifa_name: *mut c_char,
    ifa_flags: u32,
    ifa_addr: *mut SockAddrIn,
    ifa_netmask: *mut c_void,
    ifa_dstaddr: *mut c_void,
    ifa_data: *mut c_void,
}

#[repr(C)]
struct SockAddrIn {
    sin_len: u8,
    sin_family: u8,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

extern "C" {
    fn getifaddrs(out: *mut *mut IfAddrs) -> i32;
    fn freeifaddrs(ifa: *mut IfAddrs);
}

const AF_INET: u8 = 2;
const IFF_UP: u32 = 0x1;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;

/// The address a phone on the same Wi-Fi can reach: the Mac's own network
/// (en0 is Wi-Fi on MacBooks), private ranges first; tunnels (VPN,
/// Tailscale) and virtual bridges last. Falls back to the address macOS
/// would use for the internet.
fn lan_ipv4() -> Option<Ipv4Addr> {
    let route = route_ipv4();
    let mut best: Option<(i32, Ipv4Addr)> = None;
    for (name, ip) in interface_ipv4s() {
        let score = rank(&name, Some(ip) == route, ip);
        if score > 0 && best.is_none_or(|(s, _)| score > s) {
            best = Some((score, ip));
        }
    }
    best.map(|(_, ip)| ip).or(route)
}

fn rank(name: &str, routed: bool, ip: Ipv4Addr) -> i32 {
    if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() {
        return 0;
    }
    // Apple's own peer-to-peer links never reach a phone's browser.
    if ["awdl", "llw", "anpi", "ap"].iter().any(|p| name.starts_with(p)) {
        return 0;
    }
    let mut s = 1;
    if ip.is_private() {
        s += 8;
    }
    if routed {
        s += 4;
    }
    s += match name {
        "en0" => 2,
        n if n.starts_with("en") => 1,
        n if ["utun", "ipsec", "ppp", "tun", "tap"].iter().any(|p| n.starts_with(p)) => -6,
        _ => 0,
    };
    // Carrier-grade NAT (Tailscale's range) is a tunnel, not the room's Wi-Fi.
    let o = ip.octets();
    if o[0] == 100 && (64..128).contains(&o[1]) {
        s -= 6;
    }
    s.max(1)
}

/// Each connected interface's IPv4 address, by name ("en0"); the System
/// page reads them too.
pub(super) fn interface_ipv4s() -> Vec<(String, Ipv4Addr)> {
    let mut out = Vec::new();
    let mut head: *mut IfAddrs = std::ptr::null_mut();
    if unsafe { getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut p = head;
    while !p.is_null() {
        let a = unsafe { &*p };
        let up = a.ifa_flags & (IFF_UP | IFF_RUNNING) == (IFF_UP | IFF_RUNNING) && a.ifa_flags & IFF_LOOPBACK == 0;
        if up && !a.ifa_addr.is_null() && !a.ifa_name.is_null() {
            let sa = unsafe { &*a.ifa_addr };
            if sa.sin_family == AF_INET {
                let name = unsafe { CStr::from_ptr(a.ifa_name) }.to_string_lossy().into_owned();
                out.push((name, Ipv4Addr::from(u32::from_be(sa.sin_addr))));
            }
        }
        p = a.ifa_next;
    }
    unsafe { freeifaddrs(head) };
    out
}

/// The address macOS would send internet traffic from (no packet is sent:
/// connecting a UDP socket only picks the route).
pub(super) fn route_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}
