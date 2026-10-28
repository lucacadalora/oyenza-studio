//! Small secrets on disk (the Apple Music token, the Oyenza sign-in, the AI
//! tab's typed keys and its inference.oyenza.com key), readable only by this
//! Mac account: a file with owner-only permissions in Poni's folders. The
//! Windows app seals the same file with DPAPI, which likewise any program
//! running as that user can open; the API is the same, so the shared code
//! uses it unchanged.
//!
//! Each file is also kept out of backups: Time Machine copies
//! ~/Library/Application Support, often to a disk that isn't encrypted,
//! where a key removed here (Remove, Disconnect, Sign out) would live on.

use std::ffi::{c_char, c_int, c_void, CString};
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

extern "C" {
    fn setxattr(path: *const c_char, name: *const c_char, value: *const c_void, size: usize, position: u32, options: c_int) -> c_int;
}

pub fn save(path: &Path, secret: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp)?;
    f.write_all(secret.as_bytes())?;
    f.sync_all()?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    // Before the rename, so the file is never there without it.
    keep_out_of_backups(&tmp);
    fs::rename(&tmp, path)
}

pub fn load(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

pub fn delete(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Exclude a file from Time Machine the way `tmutil addexclusion` does: an
/// extended attribute that moves with the file. Best effort: on a disk
/// without extended attributes the secret is saved all the same.
fn keep_out_of_backups(path: &Path) {
    // The attribute's value: the string "com.apple.backupd" as a binary
    // property list, byte for byte what tmutil writes.
    const BACKUPD: &[u8] = b"bplist00_\x10\x11com.apple.backupd\x08\0\0\0\0\0\0\x01\x01\0\0\0\0\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x1c";
    let Ok(p) = CString::new(path.as_os_str().as_bytes()) else { return };
    let name = c"com.apple.metadata:com_apple_backup_excludeItem";
    if unsafe { setxattr(p.as_ptr(), name.as_ptr(), BACKUPD.as_ptr().cast(), BACKUPD.len(), 0, 0) } != 0 {
        pwarn!("secret: {} stays in backups ({})", path.display(), io::Error::last_os_error());
    }
}
