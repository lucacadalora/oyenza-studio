//! Small secrets on disk (the Apple Music token), sealed with DPAPI so only
//! the same Windows user on this PC can read them back. Kept under
//! `%LOCALAPPDATA%\Poni\`: never roams, and uninstalling removes it.

use std::fs;
use std::io;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

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

/// Copy a DPAPI output blob and free it.
unsafe fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }
    let bytes = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
    let _ = LocalFree(HLOCAL(blob.pbData as _));
    bytes
}
