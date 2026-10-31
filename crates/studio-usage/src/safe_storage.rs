//! Electron's `safeStorage`, which seals Cursor's sign-in (Cursor 3.27 on,
//! `cursor-safe-storage:v1:` and the sealed bytes in base64) and the Claude
//! app's (`oauth:tokenCacheV2` in its `config.json`). The bytes are
//! Chromium's OSCrypt:
//!
//! - Mac: `v10`, then AES-128-CBC with 16 spaces as the IV. The key is
//!   PBKDF2-HMAC-SHA1 of the password in the keychain item "<App> Safe
//!   Storage" (salt `saltysalt`, 1003 rounds, 16 bytes). The keychain asks
//!   the person first (macOS names "security" and the item), so it is read
//!   like Claude Code's own item: only while the AI tab is on screen.
//! - Windows: `v10`, a 12-byte nonce, then AES-256-GCM with the tag last.
//!   The key sits in the app's `Local State` (`os_crypt.encrypted_key`:
//!   base64 of `DPAPI` and a DPAPI blob only this Windows user can open).
//!   Windows' own crypto opens both ([`Want::Unseal`]), so poni.exe carries
//!   no AES of its own. Bytes without `v10` are a DPAPI blob alone (older
//!   Electron).
//!
//! What comes out goes to the service's own server only, like every
//! sign-in the AI tab reads, and is never kept or renewed here.

use super::{b64url, Dirs, Got, Os, Trouble, Want};
use serde_json::Value;
use std::path::Path;

/// Chromium's mark on bytes it sealed with its own key.
const V10: &[u8] = b"v10";
/// A `Local State` is a few KB; anything far bigger isn't one.
const LOCAL_STATE_MAX: u64 = 1 << 20;

/// What opening sealed bytes needs next.
#[derive(Debug, PartialEq, Eq)]
pub enum Open {
    /// Read these too, then open again with what came back after the
    /// reader's own reads (`got` below).
    Need(Vec<Want>),
    Done(Result<Vec<u8>, Trouble>),
}

/// Bytes `app` sealed (`app` as macOS names its keychain item: "Cursor",
/// "Claude"; `folder` its data folder, where Windows keeps `Local State`),
/// opened step by step: `got` holds what the earlier [`Open::Need`]s read,
/// in order, and nothing else.
pub fn open(app: &str, folder: &Path, dirs: &Dirs, sealed: &[u8], got: &[Got]) -> Open {
    match dirs.os {
        Os::Mac => match got.first() {
            None => Open::Need(vec![Want::Keychain { service: format!("{app} Safe Storage"), account: None }]),
            Some(Got::Text(password)) => Open::Done(mac(sealed, password).ok_or_else(|| unopened(app))),
            Some(Got::Denied) => Open::Done(Err(Trouble::Denied)),
            // No item (the app couldn't open it either), or not asked yet:
            // the tab is closed and macOS is never asked from the background.
            Some(_) => Open::Done(Err(Trouble::SignedOut)),
        },
        Os::Windows => match (got.first(), got.get(1)) {
            (None, _) => Open::Need(vec![Want::File { path: dirs.join(folder, &["Local State"]), max: LOCAL_STATE_MAX }]),
            (Some(state), None) => {
                let key = state.json().and_then(|v| local_state_key(&v));
                if sealed.starts_with(V10) && key.is_none() {
                    return Open::Done(Err(unopened(app)));
                }
                Open::Need(vec![Want::Unseal { sealed: Sealed(sealed.to_vec()), key: key.map(Sealed) }])
            }
            (Some(_), Some(Got::Text(t))) => Open::Done(Ok(t.clone().into_bytes())),
            (Some(_), Some(_)) => Open::Done(Err(unopened(app))),
        },
    }
}

fn unopened(app: &str) -> Trouble {
    Trouble::Unreadable(format!("{app}'s sign-in didn't open"))
}

/// The DPAPI blob in a `Local State`'s `os_crypt.encrypted_key`, without
/// its `DPAPI` mark.
fn local_state_key(v: &Value) -> Option<Vec<u8>> {
    let key = b64url(v.pointer("/os_crypt/encrypted_key")?.as_str()?.trim())?;
    key.strip_prefix(b"DPAPI").filter(|k| !k.is_empty()).map(<[u8]>::to_vec)
}

/// Bytes for Windows to open ([`Want::Unseal`]). Its `Debug` shows the
/// length only.
#[derive(Clone, PartialEq, Eq)]
pub struct Sealed(pub Vec<u8>);

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sealed(<{} bytes>)", self.0.len())
    }
}

/// `v10` bytes sealed on a Mac, opened with the keychain item's password.
/// None for anything else, or a wrong password (its padding won't check out).
#[cfg(not(windows))]
fn mac(sealed: &[u8], password: &str) -> Option<Vec<u8>> {
    use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
    let body = sealed.strip_prefix(V10).filter(|b| !b.is_empty() && b.len() % 16 == 0)?;
    let aes = aes::Aes128::new(&GenericArray::from(mac_key(password)));
    // CBC: each block decrypts, then takes the one before it (the IV first) away.
    let mut out = Vec::with_capacity(body.len());
    let mut before = [b' '; 16];
    for block in body.chunks_exact(16) {
        let mut b = GenericArray::clone_from_slice(block);
        aes.decrypt_block(&mut b);
        out.extend(b.iter().zip(before).map(|(x, y)| x ^ y));
        before.copy_from_slice(block);
    }
    // PKCS#7 padding: 1 to 16 bytes, each its own count.
    let pad = *out.last()? as usize;
    if !(1..=16).contains(&pad) || !out[out.len() - pad..].iter().all(|&b| b as usize == pad) {
        return None;
    }
    out.truncate(out.len() - pad);
    Some(out)
}

/// Windows never reads the Mac way.
#[cfg(windows)]
fn mac(_sealed: &[u8], _password: &str) -> Option<Vec<u8>> {
    None
}

/// PBKDF2-HMAC-SHA1(password, "saltysalt", 1003 rounds): its first 16 bytes
/// (one SHA-1 block is enough for them).
#[cfg(not(windows))]
fn mac_key(password: &str) -> [u8; 16] {
    use hmac::{Hmac, Mac};
    let mac = Hmac::<sha1::Sha1>::new_from_slice(password.as_bytes()).expect("HMAC takes a key of any length");
    let round = |input: &[&[u8]]| {
        let mut m = mac.clone();
        input.iter().for_each(|i| m.update(i));
        m.finalize().into_bytes()
    };
    let mut u = round(&[b"saltysalt", &1u32.to_be_bytes()]);
    let mut key = u;
    for _ in 1..1003 {
        u = round(&[&u]);
        key.iter_mut().zip(u).for_each(|(k, x)| *k ^= x);
    }
    let mut out = [0; 16];
    out.copy_from_slice(&key[..16]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileRead;
    use std::path::PathBuf;

    /// The fixtures' made-up keychain password and what it sealed (sealed
    /// with Python's `cryptography` by `fixtures/safe-storage/make.py`).
    /// The Mac's opening isn't built on Windows: its tests run elsewhere.
    #[cfg(not(windows))]
    const PASSWORD: &str = "synthetic-safe-storage-password";
    const PLAIN: &str = "a test sign-in, not a real one";
    #[cfg(not(windows))]
    const MAC_SEALED: &str = include_str!("fixtures/safe-storage/mac-v10.b64");
    const LOCAL_STATE: &str = include_str!("fixtures/safe-storage/local-state.json");

    #[cfg(not(windows))]
    fn mac_sealed() -> Vec<u8> {
        b64url(MAC_SEALED.trim()).unwrap()
    }

    fn file(text: &str) -> Got {
        Got::One(FileRead { path: PathBuf::from("Local State"), bytes: text.as_bytes().to_vec(), modified: None })
    }

    #[test]
    #[cfg(not(windows))]
    fn the_mac_key_is_chromiums() {
        // Python: hashlib.pbkdf2_hmac("sha1", b"peanuts", b"saltysalt", 1003, 16).
        assert_eq!(mac_key("peanuts"), [0xd9, 0xa0, 0x9d, 0x49, 0x9b, 0x4e, 0x1b, 0x74, 0x61, 0xf2, 0x8e, 0x67, 0x97, 0x2c, 0x6d, 0xbd]);
    }

    #[test]
    #[cfg(not(windows))]
    fn a_mac_seal_opens_with_its_password_only() {
        let sealed = mac_sealed();
        assert_eq!(mac(&sealed, PASSWORD).as_deref(), Some(PLAIN.as_bytes()));
        assert_eq!(mac(&sealed, "another password"), None);
        // Not Chromium's, or cut short.
        assert_eq!(mac(&sealed[3..], PASSWORD), None);
        assert_eq!(mac(&sealed[..sealed.len() - 1], PASSWORD), None);
        assert_eq!(mac(b"v10", PASSWORD), None);
    }

    #[test]
    #[cfg(not(windows))]
    fn a_mac_asks_the_keychain_once() {
        let dirs = Dirs::sample(Os::Mac);
        let folder = dirs.config.join("Cursor");
        let sealed = mac_sealed();
        let step = |got: &[Got]| open("Cursor", &folder, &dirs, &sealed, got);
        assert_eq!(step(&[]), Open::Need(vec![Want::Keychain { service: "Cursor Safe Storage".into(), account: None }]));
        assert_eq!(step(&[Got::Text(PASSWORD.into())]), Open::Done(Ok(PLAIN.as_bytes().to_vec())));
        assert_eq!(step(&[Got::Text("wrong".into())]), Open::Done(Err(Trouble::Unreadable("Cursor's sign-in didn't open".into()))));
        assert_eq!(step(&[Got::Denied]), Open::Done(Err(Trouble::Denied)));
        assert_eq!(step(&[Got::Missing]), Open::Done(Err(Trouble::SignedOut)));
    }

    #[test]
    fn windows_reads_local_state_then_asks_windows() {
        let dirs = Dirs::sample(Os::Windows);
        let folder = PathBuf::from(r"C:\Users\u\AppData\Roaming\Claude");
        let sealed = b"v10 twelve-byte-nonce, then the rest".to_vec();
        let step = |got: &[Got]| open("Claude", &folder, &dirs, &sealed, got);
        assert_eq!(step(&[]), Open::Need(vec![Want::File { path: PathBuf::from(r"C:\Users\u\AppData\Roaming\Claude\Local State"), max: LOCAL_STATE_MAX }]));
        let key = Sealed(b"a DPAPI blob".to_vec());
        assert_eq!(step(&[file(LOCAL_STATE)]), Open::Need(vec![Want::Unseal { sealed: Sealed(sealed.clone()), key: Some(key.clone()) }]));
        assert_eq!(step(&[file(LOCAL_STATE), Got::Text(PLAIN.into())]), Open::Done(Ok(PLAIN.as_bytes().to_vec())));
        let unopened = Open::Done(Err(Trouble::Unreadable("Claude's sign-in didn't open".into())));
        assert_eq!(step(&[file(LOCAL_STATE), Got::Missing]), unopened);
        // A v10 seal needs the key: no Local State, or none in it.
        assert_eq!(step(&[Got::Missing]), unopened);
        assert_eq!(step(&[file(r#"{"os_crypt": {"encrypted_key": "bm90IGEgRFBBUEkga2V5"}}"#)]), unopened);
        // Without v10 it is DPAPI alone.
        let dpapi = b"\x01\x00\x00\x00 a DPAPI blob".to_vec();
        let alone = open("Claude", &folder, &dirs, &dpapi, &[Got::Missing]);
        assert_eq!(alone, Open::Need(vec![Want::Unseal { sealed: Sealed(dpapi.clone()), key: None }]));
        // Its Debug never shows the bytes.
        assert_eq!(format!("{key:?}"), "Sealed(<12 bytes>)");
    }
}
