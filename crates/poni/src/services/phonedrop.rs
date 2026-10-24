//! Phone drop on Windows: picks the PC's Wi-Fi or wired address, makes the
//! session secret and the inbox folder, and runs `poni_core::phonedrop`'s
//! web server while the Shelf's phone panel is open. Unlike the other
//! services it starts on demand and stops when the panel closes.

use crate::events::{AppEvent, EventSink};
use poni_core::phonedrop::{token, Config, Server};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    IP_ADAPTER_UNICAST_ADDRESS_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG};

/// Tried first so the address stays the same from one session to the next;
/// any free port when it's taken.
const PORT: u16 = 53530;
/// IfType values (ipifcons.h).
const IF_TYPE_ETHERNET: u32 = 6;
const IF_TYPE_WIFI: u32 = 71;

pub struct PhoneDrop {
    server: Server,
}

impl PhoneDrop {
    /// Start serving and return the link for the QR code. `Err` holds a
    /// sentence for the island.
    pub fn start(sink: EventSink, offer: Vec<PathBuf>) -> Result<(PhoneDrop, String), String> {
        let ip = lan_ipv4().ok_or_else(|| "This PC isn't on a Wi-Fi or wired network.".to_string())?;
        let inbox = crate::paths::phone_inbox_dir();
        std::fs::create_dir_all(&inbox).map_err(|e| format!("Couldn't make the folder {} ({e}).", inbox.display()))?;
        let secret = random_token().ok_or_else(|| "Windows couldn't make a secret link.".to_string())?;
        let sink = Mutex::new(sink);
        let send: Arc<dyn Fn(poni_core::phonedrop::Event) + Send + Sync> = Arc::new(move |e| {
            if let Ok(s) = sink.lock() {
                s.send(AppEvent::Phone(e));
            }
        });
        let cfg = Config::new(SocketAddr::new(IpAddr::V4(ip), PORT), secret, pc_name(), inbox.clone());
        let first = send.clone();
        let server = match Server::start(cfg.clone(), move |e| first(e)) {
            Ok(s) => s,
            Err(e) => {
                crate::pinfo!("phone drop: port {PORT} unavailable ({e}), using another");
                let mut cfg = cfg;
                cfg.bind.set_port(0);
                let again = send.clone();
                Server::start(cfg, move |e| again(e)).map_err(|e| format!("Couldn't open a network port ({e})."))?
            }
        };
        server.set_offer(offer);
        // Never log the link: it carries the secret.
        crate::pinfo!("phone drop: listening on {}, saving to {}", server.addr(), inbox.display());
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
        crate::pinfo!("phone drop: stopped");
    }
}

/// 16 bytes from the system RNG as an upper-case base32 secret.
fn random_token() -> Option<String> {
    let mut b = [0u8; 16];
    let status = unsafe { BCryptGenRandom(BCRYPT_ALG_HANDLE::default(), &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    status.is_ok().then(|| token::base32(&b))
}

/// What the page calls this PC: its device name (Settings > System > About).
pub(crate) fn pc_name() -> String {
    use windows::core::PWSTR;
    use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
    let mut len: u32 = 0;
    unsafe {
        let _ = GetComputerNameExW(ComputerNameDnsHostname, PWSTR::null(), &mut len);
    }
    if len > 0 {
        let mut buf = vec![0u16; len as usize];
        if unsafe { GetComputerNameExW(ComputerNameDnsHostname, PWSTR(buf.as_mut_ptr()), &mut len) }.is_ok() {
            let name = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
            if !name.trim().is_empty() {
                return name;
            }
        }
    }
    std::env::var("COMPUTERNAME").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "your PC".into())
}

/// The address a phone on the same Wi-Fi can reach: an IPv4 address of an
/// adapter that is up and has a gateway, Wi-Fi first, then Ethernet,
/// private ranges before anything else (VPN and Tailscale addresses sort
/// last). Falls back to the address Windows would use for the internet.
fn lan_ipv4() -> Option<Ipv4Addr> {
    let mut best: Option<(i32, Ipv4Addr)> = None;
    for (if_type, gateway, ip) in adapter_ipv4s() {
        let score = rank(if_type, gateway, ip);
        if score > 0 && best.is_none_or(|(s, _)| score > s) {
            best = Some((score, ip));
        }
    }
    best.map(|(_, ip)| ip).or_else(route_ipv4)
}

fn rank(if_type: u32, gateway: bool, ip: Ipv4Addr) -> i32 {
    if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() {
        return 0;
    }
    let mut s = 1;
    if ip.is_private() {
        s += 8;
    }
    if gateway {
        s += 4;
    }
    s += match if_type {
        IF_TYPE_WIFI => 2,
        IF_TYPE_ETHERNET => 1,
        _ => 0,
    };
    // 100.64.0.0/10 (carrier-grade NAT, Tailscale) isn't reachable from a phone on the Wi-Fi.
    let o = ip.octets();
    if o[0] == 100 && (64..128).contains(&o[1]) {
        s -= 6;
    }
    s.max(1)
}

/// (IfType, has a gateway, address) for every IPv4 address of every adapter that is up.
fn adapter_ipv4s() -> Vec<(u32, bool, Ipv4Addr)> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 16 * 1024;
    for _ in 0..4 {
        // u64 storage keeps the structures 8-byte aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let head = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let r = unsafe { GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(head), &mut size) };
        if r == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if r != NO_ERROR.0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut p = head as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            // SAFETY: the list lives in `buf`, which outlives this loop.
            let a = unsafe { &*p };
            if a.OperStatus == IfOperStatusUp {
                let gateway = !a.FirstGatewayAddress.is_null();
                let mut u = a.FirstUnicastAddress as *const IP_ADAPTER_UNICAST_ADDRESS_LH;
                while !u.is_null() {
                    let ua = unsafe { &*u };
                    let sa = ua.Address.lpSockaddr;
                    if !sa.is_null() && unsafe { (*sa).sa_family } == AF_INET {
                        let sin = unsafe { &*(sa as *const SOCKADDR_IN) };
                        let raw = unsafe { sin.sin_addr.S_un.S_addr };
                        out.push((a.IfType, gateway, Ipv4Addr::from(u32::from_be(raw))));
                    }
                    u = ua.Next as *const _;
                }
            }
            p = a.Next as *const _;
        }
        return out;
    }
    Vec::new()
}

/// Every address a phone might reach this PC on, best first (the same
/// ranking as [`lan_ipv4`]): the direct link of a paired phone offers each
/// one. Up to three, no loopback, link-local or CGNAT/Tailscale addresses.
pub(crate) fn lan_addresses() -> Vec<IpAddr> {
    let mut ranked: Vec<(i32, Ipv4Addr)> = adapter_ipv4s().into_iter().map(|(t, g, ip)| (rank(t, g, ip), ip)).filter(|(s, _)| *s > 0).collect();
    ranked.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut out: Vec<IpAddr> = Vec::new();
    for (_, ip) in ranked {
        let o = ip.octets();
        if (o[0] == 100 && (64..128).contains(&o[1])) || out.contains(&IpAddr::V4(ip)) {
            continue;
        }
        out.push(IpAddr::V4(ip));
        if out.len() == 3 {
            break;
        }
    }
    if out.is_empty() {
        if let Some(ip) = route_ipv4() {
            out.push(IpAddr::V4(ip));
        }
    }
    out
}

/// The local address of a route to the internet (no packet is sent).
pub(crate) fn route_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_home_wifi() {
        let wifi = rank(IF_TYPE_WIFI, true, Ipv4Addr::new(192, 168, 1, 20));
        let wired = rank(IF_TYPE_ETHERNET, true, Ipv4Addr::new(10, 0, 0, 5));
        let tailscale = rank(53, false, Ipv4Addr::new(100, 101, 102, 103));
        let hyperv = rank(IF_TYPE_ETHERNET, false, Ipv4Addr::new(172, 24, 160, 1));
        assert!(wifi > wired && wired > hyperv && hyperv > tailscale);
        assert_eq!(rank(IF_TYPE_WIFI, true, Ipv4Addr::new(169, 254, 3, 4)), 0);
        assert_eq!(rank(IF_TYPE_WIFI, true, Ipv4Addr::LOCALHOST), 0);
    }
}
