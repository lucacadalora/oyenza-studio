//! Small secrets on disk (the Apple Music token), sealed with DPAPI so only
//! the same Windows user on this PC can read them back. Kept under
//! `%LOCALAPPDATA%\Poni\`: never roams, and uninstalling removes it.
//!
//! Also opens what other Electron apps sealed the same way, for the AI tab
//! ([`open_electron`]).

use std::fs;
use std::io;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    BCryptCloseAlgorithmProvider, BCryptDecrypt, BCryptDestroyKey, BCryptGenerateSymmetricKey, BCryptOpenAlgorithmProvider, BCryptSetProperty, CryptProtectData, CryptUnprotectData, BCRYPT_AES_ALGORITHM, BCRYPT_ALG_HANDLE,
    BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO, BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION, BCRYPT_CHAINING_MODE, BCRYPT_CHAIN_MODE_GCM, BCRYPT_FLAGS, BCRYPT_KEY_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, CRYPTPROTECT_UI_FORBIDDEN,
    CRYPT_INTEGER_BLOB,
};

pub fn save(path: &Path, secret: &str) -> io::Result<()> {
    let sealed = protect(secret.as_bytes())?;
    poni_core::store::write_bytes_atomic(path, &sealed)
}

/// The secret at `path`; `None` when there is none or it can't be unsealed
/// (another user's or another PC's file).
pub fn load(path: &Path) -> Option<String> {
    let sealed = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            crate::pwarn!("secret: cannot read {} ({e})", path.display());
            return None;
        }
    };
    match unprotect(&sealed).map(String::from_utf8) {
        Ok(Ok(s)) => Some(s),
        Ok(Err(_)) => None,
        Err(e) => {
            crate::pwarn!("secret: cannot unseal {} ({e})", path.display());
            None
        }
    }
}

pub fn delete(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(io::Error::other)?;
        Ok(take_blob(out))
    }
}

fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).map_err(io::Error::other)?;
        Ok(take_blob(out))
    }
}

/// What an Electron app sealed with `safeStorage` (Cursor's and the Claude
/// app's sign-ins, `studio_usage::safe_storage`): `v10`, a 12-byte nonce,
/// then AES-256-GCM with the 16-byte tag last, under `key` (the DPAPI blob
/// from the app's `Local State`); anything else is a DPAPI blob alone. None
/// when it doesn't open (another user's, or changed). Windows' own AES, so
/// poni.exe carries none.
pub fn open_electron(sealed: &[u8], key: Option<&[u8]>) -> Option<Vec<u8>> {
    let Some(body) = sealed.strip_prefix(b"v10") else { return unprotect(sealed).ok() };
    let key = unprotect(key?).ok()?;
    if key.len() != 32 || body.len() < 12 + 16 {
        return None;
    }
    let (nonce, rest) = body.split_at(12);
    let (data, tag) = rest.split_at(rest.len() - 16);
    gcm_open(&key, nonce, data, tag)
}

/// AES-GCM through CNG: `data` decrypted, None if the tag doesn't match.
fn gcm_open(key: &[u8], nonce: &[u8], data: &[u8], tag: &[u8]) -> Option<Vec<u8>> {
    let mut alg = BCRYPT_ALG_HANDLE::default();
    unsafe {
        BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_AES_ALGORITHM, PCWSTR::null(), BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0)).ok().ok()?;
        let out = (|| {
            // The property's value is the mode's name, NUL included, as UTF-16 bytes.
            let mode: Vec<u8> = BCRYPT_CHAIN_MODE_GCM.as_wide().iter().chain([&0]).flat_map(|c| c.to_le_bytes()).collect();
            BCryptSetProperty(alg.into(), BCRYPT_CHAINING_MODE, &mode, 0).ok().ok()?;
            let mut hkey = BCRYPT_KEY_HANDLE::default();
            BCryptGenerateSymmetricKey(alg, &mut hkey, None, key, 0).ok().ok()?;
            let info = BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
                cbSize: std::mem::size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32,
                dwInfoVersion: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
                pbNonce: nonce.as_ptr() as *mut u8,
                cbNonce: nonce.len() as u32,
                pbTag: tag.as_ptr() as *mut u8,
                cbTag: tag.len() as u32,
                ..Default::default()
            };
            let mut out = vec![0u8; data.len()];
            let mut written = 0u32;
            let status = BCryptDecrypt(hkey, Some(data), Some(&info as *const _ as *const core::ffi::c_void), None, Some(&mut out), &mut written, BCRYPT_FLAGS(0));
            let _ = BCryptDestroyKey(hkey);
            status.ok().ok()?;
            out.truncate(written as usize);
            Some(out)
        })();
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        out
    }
}

/// Copy a DPAPI output blob and free it.
unsafe fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }
    let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
    let _ = LocalFree(HLOCAL(blob.pbData as _));
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An AES-256 key and what it sealed the way Electron does on Windows
    /// (made with Python's `cryptography` by make.py beside it).
    const SEALED: &str = include_str!("../../../studio-usage/src/fixtures/safe-storage/windows-v10.txt");

    fn sample() -> (Vec<u8>, Vec<u8>) {
        let mut lines = SEALED.lines();
        let key = lines.next().unwrap();
        let key = (0..key.len()).step_by(2).map(|i| u8::from_str_radix(&key[i..i + 2], 16).unwrap()).collect();
        (key, studio_usage::b64url(lines.next().unwrap()).unwrap())
    }

    #[test]
    fn electron_seals_open_with_the_local_state_key() {
        let (key, sealed) = sample();
        // Local State keeps the key sealed with DPAPI.
        let wrapped = protect(&key).unwrap();
        assert_eq!(open_electron(&sealed, Some(&wrapped)).as_deref(), Some(&b"a test sign-in, not a real one"[..]));
        // A changed byte fails the tag; no key, or another one, opens nothing.
        let mut changed = sealed.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert_eq!(open_electron(&changed, Some(&wrapped)), None);
        assert_eq!(open_electron(&sealed, None), None);
        assert_eq!(open_electron(&sealed, Some(&protect(&[7; 32]).unwrap())), None);
        assert_eq!(open_electron(&sealed[..20], Some(&wrapped)), None);
        // Without v10: DPAPI alone.
        assert_eq!(open_electron(&protect(b"older Electron").unwrap(), None).as_deref(), Some(&b"older Electron"[..]));
        assert_eq!(open_electron(b"not sealed", None), None);
    }
}
