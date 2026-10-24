//! Paired phone drop: a phone paired once (by scanning a QR code on the
//! Shelf) finds this PC on any network and sends files straight onto the
//! Shelf, the way AirDrop works between your own devices.
//!
//! How the pieces fit (full protocol: `docs/paired-drop.md`):
//!
//! * [`keys`]: the pairing secret, shared only through the QR code, and the
//!   room, proof and signing key derived from it;
//! * [`meet`]: the PC waits for its phones through a small meeting server
//!   (one long request at a time, about every 25 s);
//! * [`signal`]: every message through that server is signed, so the server
//!   can pass messages along but can't forge or change them;
//! * [`peer`]: a direct WebRTC link per phone (str0m), straight over the
//!   Wi-Fi or across the internet, or through a TURN relay the phone picks
//!   when both networks block a direct link;
//! * [`transfer`]: the files on that link, staged and moved into the inbox
//!   only when every byte arrived.
//!
//! Like `phonedrop`, everything here is `std` plus pure-Rust crates, so the
//! tests run it end to end over loopback on any OS. The Windows crate makes
//! the HTTPS calls, picks the network addresses, seals the pair list with
//! DPAPI and draws the island.

pub mod keys;
pub mod meet;
pub mod peer;
pub mod plainhttp;
pub mod signal;
pub mod stun;
pub mod transfer;

pub use keys::Secret;
pub use meet::{Book, Meet, MeetCfg, MeetEvent, Pair, Post};
pub use peer::PeerEvent;

use std::sync::OnceLock;

/// The meeting server Oyen Poni uses unless `PONI_DROP_SERVER` says otherwise.
pub const DEFAULT_SERVER: &str = "https://drop.oyenza.com";

/// Public STUN servers for the PC's own internet address.
pub const STUN_SERVERS: &[&str] = &["stun.cloudflare.com:3478", "stun.l.google.com:19302"];

type Logger = Box<dyn Fn(&str) + Send + Sync>;

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Where this module's log lines go (the Windows crate's poni.log). Unset,
/// they're dropped.
pub fn set_logger(f: impl Fn(&str) + Send + Sync + 'static) {
    let _ = LOGGER.set(Box::new(f));
}

pub(crate) fn log(line: &str) {
    if let Some(f) = LOGGER.get() {
        f(line);
    }
}

#[cfg(test)]
mod tests;
