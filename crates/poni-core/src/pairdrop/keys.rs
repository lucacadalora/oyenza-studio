//! The pairing secret and everything derived from it (`docs/paired-drop.md`).
//!
//! One pairing = one PC and one phone, sharing a 16-byte random secret `S`
//! that travels only inside the QR code, after the `#` of the link (browsers
//! never send that part to a server). From it both sides derive:
//!
//! * `room`: where the pair meets on the meeting server;
//! * `proof`: shown with every request, so only the pair can use the room;
//! * `K`: the HMAC key that signs every message, which the server never sees.

use sha2::{Digest, Sha256};

/// Bytes in a pairing secret.
pub const SECRET_LEN: usize = 16;

const ROOM: &[u8] = b"oyen-drop/1 room";
const PROOF: &[u8] = b"oyen-drop/1 proof";
const MAC: &[u8] = b"oyen-drop/1 mac";

/// A pairing secret. The Windows crate fills it from the system RNG.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret([u8; SECRET_LEN]);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret itself (logs, panics).
        write!(f, "Secret({}…)", &self.room()[..6])
    }
}

impl Secret {
    pub fn from_bytes(b: [u8; SECRET_LEN]) -> Secret {
        Secret(b)
    }

    /// Read the base64url form (as kept in the pair list and the link).
    pub fn parse(s: &str) -> Option<Secret> {
        let v = b64u_decode(s.trim())?;
        Some(Secret(v.try_into().ok()?))
    }

    /// The base64url form (22 characters).
    pub fn encode(&self) -> String {
        b64u(&self.0)
    }

    pub fn room(&self) -> String {
        b64u(&hash(ROOM, &self.0))
    }

    pub fn proof(&self) -> String {
        b64u(&hash(PROOF, &self.0))
    }

    pub fn mac_key(&self) -> [u8; 32] {
        hash(MAC, &self.0)
    }

    /// The link the pairing QR code carries.
    pub fn link(&self, server: &str) -> String {
        format!("{}/p#k={}", server.trim_end_matches('/'), self.encode())
    }
}

fn hash(label: &[u8], secret: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(label);
    h.update(secret);
    h.finalize().into()
}

const B64U: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url without padding (RFC 4648 §5).
pub fn b64u(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.len();
        let v = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64U[(v >> 18) as usize & 63] as char);
        out.push(B64U[(v >> 12) as usize & 63] as char);
        if n > 1 {
            out.push(B64U[(v >> 6) as usize & 63] as char);
        }
        if n > 2 {
            out.push(B64U[v as usize & 63] as char);
        }
    }
    out
}

/// Decode base64url without padding; `None` for anything else.
pub fn b64u_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32)
    };
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut v = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            v |= val(c)? << (18 - 6 * i);
        }
        out.push((v >> 16) as u8);
        if chunk.len() > 2 {
            out.push((v >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(v as u8);
        }
    }
    // Reject non-canonical tails (bits left over that aren't zero).
    (b64u(&out) == s).then_some(out)
}

/// True for a 22-character base64url id (a phone session, a PC client).
pub fn is_id(s: &str) -> bool {
    s.len() == 22 && b64u_decode(s).is_some_and(|b| b.len() == 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector() -> Secret {
        let mut b = [0u8; 16];
        for (i, x) in b.iter_mut().enumerate() {
            *x = i as u8;
        }
        Secret::from_bytes(b)
    }

    #[test]
    fn contract_vector() {
        let s = vector();
        assert_eq!(s.encode(), "AAECAwQFBgcICQoLDA0ODw");
        assert_eq!(s.room(), "geDM0jEhk4eiOz_li-FtORW2YlPBbebr7NsHjLkHI_4");
        assert_eq!(s.proof(), "6JmvhsXX2_bzKYvHQ_iFgLcPnlG-m636-rWXAI5Vubo");
        assert_eq!(b64u(&s.mac_key()), "PbxVzGOl5YKxg6Fd-uSD6Yqr9716j9MddvFbISuFxL4");
        assert_eq!(s.link("https://drop.oyenza.com/"), "https://drop.oyenza.com/p#k=AAECAwQFBgcICQoLDA0ODw");
    }

    #[test]
    fn base64url_round_trips() {
        for n in 0..40usize {
            let v: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64u_decode(&b64u(&v)).unwrap(), v);
        }
        assert_eq!(b64u(b"\xfb\xff"), "-_8");
        assert!(b64u_decode("A").is_none());
        assert!(b64u_decode("AB=C").is_none());
        // "AB" has bits past the byte that must be zero; "AQ" is canonical for [0x01].
        assert!(b64u_decode("AB").is_none());
        assert_eq!(b64u_decode("AQ").unwrap(), vec![1]);
    }

    #[test]
    fn parses_secrets_and_ids() {
        let s = vector();
        assert_eq!(Secret::parse(&s.encode()), Some(s.clone()));
        assert_eq!(Secret::parse(" AAECAwQFBgcICQoLDA0ODw\n"), Some(s));
        assert!(Secret::parse("AAECAwQFBgcICQoLDA0O").is_none());
        assert!(is_id("AAECAwQFBgcICQoLDA0ODw"));
        assert!(!is_id("AAECAwQFBgcICQoLDA0OD"));
        assert!(!is_id("AAECAwQFBgcICQoLDA0OD!"));
        assert!(!format!("{:?}", vector()).contains("AAECAw"));
    }
}
