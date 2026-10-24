//! The smallest STUN client (RFC 8489): ask a public STUN server which
//! address and port our UDP socket shows up as on the internet. That address
//! becomes a "server reflexive" candidate, so a phone on another network can
//! reach the PC straight through its router, and so a phone's TURN relay
//! lets the PC's packets in (relays only forward from addresses the phone
//! has seen in the PC's candidates).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

const MAGIC: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// A Binding request with transaction id `tx`.
pub fn request(tx: &[u8; 12]) -> [u8; 20] {
    let mut b = [0u8; 20];
    b[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // Length 0: no attributes.
    b[4..8].copy_from_slice(&MAGIC.to_be_bytes());
    b[8..20].copy_from_slice(tx);
    b
}

/// The mapped address in a Binding success answer to `tx`, if `buf` is one.
pub fn parse_response(buf: &[u8], tx: &[u8; 12]) -> Option<SocketAddr> {
    if buf.len() < 20 || u16::from_be_bytes([buf[0], buf[1]]) != BINDING_SUCCESS {
        return None;
    }
    if u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) != MAGIC || &buf[8..20] != tx {
        return None;
    }
    let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let attrs = buf.get(20..20 + len)?;
    let mut plain = None;
    let mut i = 0;
    while i + 4 <= attrs.len() {
        let kind = u16::from_be_bytes([attrs[i], attrs[i + 1]]);
        let alen = u16::from_be_bytes([attrs[i + 2], attrs[i + 3]]) as usize;
        let val = attrs.get(i + 4..i + 4 + alen)?;
        match kind {
            XOR_MAPPED_ADDRESS => return address(val, Some(tx)),
            MAPPED_ADDRESS => plain = address(val, None),
            _ => {}
        }
        // Attributes are padded to 4 bytes.
        i += 4 + alen.div_ceil(4) * 4;
    }
    plain
}

fn address(v: &[u8], xor_tx: Option<&[u8; 12]>) -> Option<SocketAddr> {
    if v.len() < 8 {
        return None;
    }
    let mut port = u16::from_be_bytes([v[2], v[3]]);
    let cookie = MAGIC.to_be_bytes();
    if xor_tx.is_some() {
        port ^= (MAGIC >> 16) as u16;
    }
    match v[1] {
        0x01 => {
            let mut a = [v[4], v[5], v[6], v[7]];
            if xor_tx.is_some() {
                for (x, c) in a.iter_mut().zip(cookie) {
                    *x ^= c;
                }
            }
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(a)), port))
        }
        0x02 if v.len() >= 20 => {
            let mut a: [u8; 16] = v[4..20].try_into().ok()?;
            if let Some(tx) = xor_tx {
                let key: Vec<u8> = cookie.iter().chain(tx.iter()).copied().collect();
                for (x, k) in a.iter_mut().zip(key) {
                    *x ^= k;
                }
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(a)), port))
        }
        _ => None,
    }
}

/// Ask each server in turn (`host:port`) from `sock` until one answers
/// within `budget` in total. The socket's read timeout is changed.
pub fn public_address(sock: &UdpSocket, servers: &[String], tx: [u8; 12], budget: Duration) -> Option<SocketAddr> {
    let local = sock.local_addr().ok()?;
    let deadline = Instant::now() + budget;
    let targets: Vec<SocketAddr> = servers
        .iter()
        .filter_map(|s| s.to_socket_addrs().ok())
        .flatten()
        .filter(|a| a.is_ipv4() == local.is_ipv4())
        .collect();
    if targets.is_empty() {
        return None;
    }
    let req = request(&tx);
    let mut buf = [0u8; 576];
    // Send to all of them at once, twice (UDP may drop one); the first answer wins.
    for round in 0..2 {
        for t in &targets {
            let _ = sock.send_to(&req, t);
        }
        let round_end = if round == 0 { Instant::now() + Duration::from_millis(400) } else { deadline };
        loop {
            let now = Instant::now();
            let until = round_end.min(deadline);
            if now >= until {
                break;
            }
            let _ = sock.set_read_timeout(Some((until - now).max(Duration::from_millis(1))));
            match sock.recv_from(&mut buf) {
                Ok((n, from)) if targets.contains(&from) => {
                    if let Some(a) = parse_response(&buf[..n], &tx) {
                        return Some(a);
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TX: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

    fn answer(addr: SocketAddr, xor: bool) -> Vec<u8> {
        let mut val = Vec::new();
        let (fam, ip): (u8, Vec<u8>) = match addr.ip() {
            IpAddr::V4(a) => (1, a.octets().to_vec()),
            IpAddr::V6(a) => (2, a.octets().to_vec()),
        };
        let mut port = addr.port();
        let mut ip = ip;
        if xor {
            port ^= 0x2112;
            let key: Vec<u8> = MAGIC.to_be_bytes().iter().chain(TX.iter()).copied().collect();
            for (x, k) in ip.iter_mut().zip(key) {
                *x ^= k;
            }
        }
        val.extend([0, fam]);
        val.extend(port.to_be_bytes());
        val.extend(ip);
        let mut b = Vec::new();
        b.extend(BINDING_SUCCESS.to_be_bytes());
        // A SOFTWARE attribute first (odd length, padded), then the address.
        let soft = b"x";
        let len = 4 + 4 + 4 + val.len();
        b.extend((len as u16).to_be_bytes());
        b.extend(MAGIC.to_be_bytes());
        b.extend(TX);
        b.extend(0x8022u16.to_be_bytes());
        b.extend(1u16.to_be_bytes());
        b.extend(soft);
        b.extend([0, 0, 0]);
        b.extend((if xor { XOR_MAPPED_ADDRESS } else { MAPPED_ADDRESS }).to_be_bytes());
        b.extend((val.len() as u16).to_be_bytes());
        b.extend(val);
        b
    }

    #[test]
    fn builds_requests() {
        let r = request(&TX);
        assert_eq!(&r[0..4], &[0, 1, 0, 0]);
        assert_eq!(&r[4..8], &[0x21, 0x12, 0xA4, 0x42]);
        assert_eq!(&r[8..], &TX);
    }

    #[test]
    fn reads_both_address_kinds() {
        let v4: SocketAddr = "203.0.113.7:54321".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::1:2]:40000".parse().unwrap();
        assert_eq!(parse_response(&answer(v4, true), &TX), Some(v4));
        assert_eq!(parse_response(&answer(v6, true), &TX), Some(v6));
        assert_eq!(parse_response(&answer(v4, false), &TX), Some(v4));
        let other_tx = [9u8; 12];
        assert_eq!(parse_response(&answer(v4, true), &other_tx), None);
        assert_eq!(parse_response(&answer(v4, true)[..24], &TX), None);
    }

    #[test]
    fn asks_a_local_server() {
        // A tiny STUN server on loopback answers with the address it saw.
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = server.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            let (n, from) = server.recv_from(&mut buf).unwrap();
            let tx: [u8; 12] = buf[8..20].try_into().unwrap();
            assert_eq!(n, 20);
            let mut b = Vec::new();
            b.extend(BINDING_SUCCESS.to_be_bytes());
            b.extend(12u16.to_be_bytes());
            b.extend(MAGIC.to_be_bytes());
            b.extend(tx);
            b.extend(XOR_MAPPED_ADDRESS.to_be_bytes());
            b.extend(8u16.to_be_bytes());
            b.extend([0, 1]);
            b.extend((from.port() ^ 0x2112).to_be_bytes());
            let IpAddr::V4(ip) = from.ip() else { unreachable!() };
            for (o, c) in ip.octets().iter().zip(MAGIC.to_be_bytes()) {
                b.push(o ^ c);
            }
            server.send_to(&b, from).unwrap();
        });
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let got = public_address(&sock, &[at.to_string()], TX, Duration::from_secs(2));
        assert_eq!(got, Some(sock.local_addr().unwrap()));
        t.join().unwrap();
    }
}
