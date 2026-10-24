//! Small secrets on disk (the Apple Music token), readable only by this Mac
//! account: a file with owner-only permissions in Poni's cache folder. The
//! Windows app seals the same file with DPAPI, which likewise any program
//! running as that user can open; the API is the same, so the shared
//! Apple Music code uses it unchanged.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub fn save(path: &Path, secret: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp)?;
    f.write_all(secret.as_bytes())?;
    f.sync_all()?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
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
