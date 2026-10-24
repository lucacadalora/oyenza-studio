//! Signed messages between a phone and the PC, carried by the meeting server.
//!
//! Every message is signed with the pair's key `K`, which the server never
//! learns, so the server can pass messages along but can't make, change or
//! replay them. An `offer`/`answer` carries the SDP, and with it the DTLS
//! fingerprint of each side, so the signature also keeps the server out of
//! the middle of the WebRTC link. Layout: `docs/paired-drop.md`.

use super::keys::{b64u, b64u_decode, Secret};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How far a message's clock may be from ours (phones' clocks drift).
pub const MAX_SKEW_MS: i64 = 10 * 60 * 1000;
/// How long a seen message is remembered against replays.
const REPLAY_KEEP: Duration = Duration::from_secs(15 * 60);
/// Longest device name, in characters.
pub const MAX_NAME_CHARS: usize = 60;
/// Longest SDP we accept.
pub const MAX_SDP_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Msg {
    pub t: String,
    pub sid: String,
    pub ts: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sdp: String,
    #[serde(default)]
    pub mac: String,
}

/// Why a message was dropped (for the log only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Malformed,
    BadMac,
    Stale,
    Replay,
}

impl Msg {
    /// A new message from this side, signed for `room` with `secret`.
    pub fn signed(secret: &Secret, t: &str, sid: &str, name: &str, sdp: &str, now_ms: i64) -> Msg {
        let mut m = Msg { t: t.into(), sid: sid.into(), ts: now_ms, name: clean_name(name), sdp: sdp.into(), mac: String::new() };
        m.mac = b64u(&m.tag(secret));
        m
    }

    fn text(&self, room: &str) -> String {
        format!("oyen-drop/1\n{room}\n{}\n{}\n{}\n{}\n{}", self.t, self.sid, self.ts, self.name, self.sdp)
    }

    fn tag(&self, secret: &Secret) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret.mac_key()).expect("HMAC takes any key length");
        mac.update(self.text(&secret.room()).as_bytes());
        mac.finalize().into_bytes().into()
    }

    /// Check the signature and the clock. Replays are [`Replays`]' job.
    pub fn verify(&self, secret: &Secret, now_ms: i64) -> Result<(), Reject> {
        // A name is one line: the signed text separates fields with line
        // breaks, so a name holding one could shift text into the SDP's line.
        let name_ok = self.name.chars().count() <= MAX_NAME_CHARS && !self.name.chars().any(char::is_control);
        if !super::keys::is_id(&self.sid) || self.t.is_empty() || self.t.len() > 16 || !self.t.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') || self.sdp.len() > MAX_SDP_BYTES || !name_ok {
            return Err(Reject::Malformed);
        }
        let Some(got) = b64u_decode(&self.mac) else { return Err(Reject::BadMac) };
        let want = self.tag(secret);
        // Constant time: fold every byte before deciding.
        if got.len() != want.len() || got.iter().zip(want.iter()).fold(0u8, |d, (a, b)| d | (a ^ b)) != 0 {
            return Err(Reject::BadMac);
        }
        if (now_ms - self.ts).abs() > MAX_SKEW_MS {
            return Err(Reject::Stale);
        }
        Ok(())
    }
}

/// A device name as one plain line of at most [`MAX_NAME_CHARS`] characters.
pub fn clean_name(raw: &str) -> String {
    let one_line: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let words: Vec<&str> = one_line.split_whitespace().collect();
    words.join(" ").chars().take(MAX_NAME_CHARS).collect::<String>().trim_end().to_string()
}

/// Messages already handled, so a copy sent again is ignored.
#[derive(Default)]
pub struct Replays {
    seen: HashMap<(String, String, i64), Instant>,
}

impl Replays {
    /// True the first time a message is seen; false for a repeat.
    pub fn first(&mut self, m: &Msg) -> bool {
        let now = Instant::now();
        self.seen.retain(|_, at| now.duration_since(*at) < REPLAY_KEEP);
        self.seen.insert((m.sid.clone(), m.t.clone(), m.ts), now).is_none()
    }
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
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

    const SID: &str = "AAECAwQFBgcICQoLDA0ODw";
    const TS: i64 = 1_790_000_000_000;
    const SDP: &str = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\n";

    #[test]
    fn contract_vector() {
        let m = Msg::signed(&vector(), "offer", SID, "Pixel 10", SDP, TS);
        assert_eq!(m.mac, "3QYhMYuCNPzit2tYBw0ejDAQsE1vxoO4DQERkznV8o8");
        assert_eq!(m.verify(&vector(), TS + 1000), Ok(()));
    }

    #[test]
    fn rejects_changes_clock_and_shape() {
        let s = vector();
        let m = Msg::signed(&s, "offer", SID, "Pixel 10", SDP, TS);
        let mut changed = m.clone();
        changed.sdp.push_str("a=x\r\n");
        assert_eq!(changed.verify(&s, TS), Err(Reject::BadMac));
        let mut renamed = m.clone();
        renamed.name = "Pixel 11".into();
        assert_eq!(renamed.verify(&s, TS), Err(Reject::BadMac));
        let mut other = vector().encode().into_bytes();
        other[0] = b'B';
        let other = Secret::parse(std::str::from_utf8(&other).unwrap()).unwrap();
        assert_eq!(m.verify(&other, TS), Err(Reject::BadMac));
        assert_eq!(m.verify(&s, TS + MAX_SKEW_MS + 1), Err(Reject::Stale));
        assert_eq!(m.verify(&s, TS - MAX_SKEW_MS - 1), Err(Reject::Stale));
        let mut bad_sid = m.clone();
        bad_sid.sid = "short".into();
        assert_eq!(bad_sid.verify(&s, TS), Err(Reject::Malformed));
        // A line break in a name is refused even when the signature matches.
        let two_lines = Msg { name: "Pixel\n10".into(), ..Msg::default() };
        let mut forged = Msg::signed(&s, "offer", SID, "", SDP, TS);
        forged.name = two_lines.name.clone();
        forged.mac = b64u(&forged.tag(&s));
        assert_eq!(forged.verify(&s, TS), Err(Reject::Malformed));
        let json = serde_json::to_string(&m).unwrap();
        let back: Msg = serde_json::from_str(&json).unwrap();
        assert_eq!(back.verify(&s, TS), Ok(()));
    }

    #[test]
    fn replays_are_caught() {
        let m = Msg::signed(&vector(), "pair", SID, "iPhone", "", TS);
        let mut r = Replays::default();
        assert!(r.first(&m));
        assert!(!r.first(&m));
        let later = Msg::signed(&vector(), "pair", SID, "iPhone", "", TS + 1);
        assert!(r.first(&later));
    }

    #[test]
    fn names_are_one_short_line() {
        assert_eq!(clean_name("  Pixel\n10 \u{7}Pro "), "Pixel 10 Pro");
        assert_eq!(clean_name(&"x".repeat(80)).chars().count(), MAX_NAME_CHARS);
        assert_eq!(clean_name(""), "");
    }
}
