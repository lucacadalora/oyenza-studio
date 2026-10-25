//! New versions: reads the update file (`poni_core::update::FEED_URL`, about
//! 1 KB) and believes it only when its signature checks out with a built-in
//! key. The check uses the system's own crypto: CNG on Windows, and on a Mac
//! the P-256 code the app already carries for phone drop. It runs as a job on
//! the add-on downloader's thread (`AddonService::run`), which sleeps the
//! rest of the time; the app decides when (`crate::updates`).
//!
//! The Mac app runs this file as is, on its own HTTP client.

use crate::events::{AppEvent, EventSink};
use crate::services::addons::Job;
use crate::services::http::Client;
use poni_core::update::{self, Release};
use std::time::Duration;

/// Budget for the file: it is small, but a slow line still gets it.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What the update file said.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateEvent {
    /// The latest release (signature checked), or why the file couldn't be
    /// read or believed.
    Checked(Result<Release, String>),
}

/// A job for the add-on thread: read the file, answer with `AppEvent::Update`.
pub fn check_job(sink: EventSink) -> Job {
    Box::new(move |client: Result<&Client, &str>| {
        let r = client.map_err(str::to_string).and_then(check);
        match &r {
            Ok(rel) => crate::pinfo!("updates: the latest release is {} ({})", rel.version, rel.date),
            Err(e) => crate::pinfo!("updates: couldn't read the update file ({e})"),
        }
        sink.send(AppEvent::Update(UpdateEvent::Checked(r)));
    })
}

fn check(client: &Client) -> Result<Release, String> {
    let (status, body) = client.get_bytes(update::FEED_URL, TIMEOUT, update::MAX_FILE_BYTES, |_| {}).map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    believe(std::str::from_utf8(&body).map_err(|_| "not text".to_string())?)
}

/// The file's text to the release it describes, once its signature checks out.
pub fn believe(text: &str) -> Result<Release, String> {
    let signed = update::parse_file(text)?;
    if !verify(signed.payload.as_bytes(), &signed.signature, &signed.key) {
        return Err("its signature doesn't match, so it was ignored".into());
    }
    update::parse_payload(&signed.payload)
}

/// ECDSA P-256 over SHA-256: `signature` is r and s, `key` an uncompressed point.
#[cfg(windows)]
fn verify(message: &[u8], signature: &[u8; 64], key: &[u8; 65]) -> bool {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptImportKeyPair, BCryptOpenAlgorithmProvider, BCryptVerifySignature, BCRYPT_ALG_HANDLE,
        BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_FLAGS, BCRYPT_KEY_HANDLE,
        BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS,
    };
    // BCRYPT_ECCKEY_BLOB: magic, key length, then X and Y.
    let mut blob = Vec::with_capacity(72);
    blob.extend_from_slice(&BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes());
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(&key[1..]);
    let hash = poni_core::sha256::sha256(message);
    unsafe {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        if BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_ECDSA_P256_ALGORITHM, None, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0)).is_err() {
            return false;
        }
        let mut handle = BCRYPT_KEY_HANDLE::default();
        let ok = BCryptImportKeyPair(alg, BCRYPT_KEY_HANDLE::default(), BCRYPT_ECCPUBLIC_BLOB, &mut handle, &blob, 0).is_ok()
            && BCryptVerifySignature(handle, None, &hash, signature, BCRYPT_FLAGS(0)).is_ok();
        if !handle.is_invalid() {
            let _ = BCryptDestroyKey(handle);
        }
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        ok
    }
}

/// ECDSA P-256 over SHA-256: `signature` is r and s, `key` an uncompressed point.
#[cfg(target_os = "macos")]
fn verify(message: &[u8], signature: &[u8; 64], key: &[u8; 65]) -> bool {
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    let (Ok(key), Ok(sig)) = (VerifyingKey::from_sec1_bytes(key), Signature::from_slice(signature)) else { return false };
    key.verify(message, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real file, signed with the signing key for v0.2.3.
    const SAMPLE: &str = include_str!("../../../poni-core/data/update-sample.json");

    #[test]
    fn a_signed_file_is_believed() {
        let r = believe(SAMPLE).expect("the sample's signature checks out");
        assert_eq!((r.version.as_str(), r.headline.as_str()), ("0.2.3", "Sing along from the closed island"));
        assert_eq!(r.windows.as_ref().map(|d| d.size), Some(2457657));
    }

    #[test]
    fn a_changed_file_is_not() {
        // One character of the payload, or of the signature.
        let changed = SAMPLE.replacen("2457657", "2457658", 1);
        assert!(believe(&changed).unwrap_err().contains("signature"), "payload changed");
        let signed = update::parse_file(SAMPLE).unwrap();
        let mut sig = signed.signature;
        sig[10] ^= 1;
        assert!(!verify(signed.payload.as_bytes(), &sig, &signed.key));
        assert!(verify(signed.payload.as_bytes(), &signed.signature, &signed.key));
        // The spare key didn't sign it.
        let spare = update::trusted_key("f835f77c60bd2b7f").unwrap();
        assert!(!verify(signed.payload.as_bytes(), &signed.signature, &spare));
    }
}
