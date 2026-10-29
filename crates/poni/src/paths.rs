//! Where Poni keeps its files.
//!
//! - Settings + data: `%APPDATA%\Poni\` (roams with the user profile)
//! - Cache + log:     `%LOCALAPPDATA%\Poni\`
//!
//! `PONI_HOME` overrides both (used by `--selftest` to stay isolated).

use std::path::PathBuf;

fn home_override() -> Option<PathBuf> {
    std::env::var_os("PONI_HOME").map(PathBuf::from)
}

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn data_dir() -> PathBuf {
    home_override().unwrap_or_else(|| env_dir("APPDATA").join("Poni"))
}

pub fn cache_dir() -> PathBuf {
    home_override().map(|h| h.join("cache")).unwrap_or_else(|| env_dir("LOCALAPPDATA").join("Poni"))
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn store_path() -> PathBuf {
    data_dir().join("data.json")
}

pub fn log_path() -> PathBuf {
    cache_dir().join("poni.log")
}

pub fn lyrics_cache_dir() -> PathBuf {
    cache_dir().join("lyrics")
}

/// The user's Apple Music token, sealed with DPAPI (never roams; uninstalling
/// removes it with the cache).
pub fn apple_music_token_path() -> PathBuf {
    cache_dir().join("apple-music-token.bin")
}

/// The Apple Music web player's (public) developer token, cached until it expires.
pub fn apple_music_web_token_path() -> PathBuf {
    cache_dir().join("apple-music-web.json")
}

/// Where files sent from a phone land: `Downloads\Oyenza Studio` (wherever the
/// user moved Downloads to), or `Downloads\Oyen Poni` where an install from
/// before the rename made it: Shelf items link to the files in it. Under
/// `PONI_HOME` for the self-test.
pub fn phone_inbox_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("phone-drop");
    }
    let downloads = downloads_dir().unwrap_or_else(|| env_dir("USERPROFILE").join("Downloads"));
    let before_rename = downloads.join("Oyen Poni");
    if before_rename.is_dir() { before_rename } else { downloads.join("Oyenza Studio") }
}

fn downloads_dir() -> Option<PathBuf> {
    known_folder(&windows::Win32::UI::Shell::FOLDERID_Downloads)
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, HANDLE::default()).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty()).map(PathBuf::from)
    }
}

/// The Oyenza sign-in (this PC's key and the masked account), sealed with
/// DPAPI. Signing out or uninstalling removes it.
pub fn oyenza_link_path() -> PathBuf {
    cache_dir().join("oyenza-link.bin")
}

/// Add-on files, one folder per add-on (`services::addons`). Local, never
/// roaming: a PC that hasn't them downloads them again. Switching an add-on
/// off or uninstalling removes them.
pub fn addons_dir() -> PathBuf {
    cache_dir().join("addons")
}

/// The IELTS page's audio: Oyen's clips and the answer being recorded, each
/// deleted once used (local, never roaming).
pub fn ielts_dir() -> PathBuf {
    cache_dir().join("ielts")
}

/// A meeting being recorded or handed over for notes: its audio parts, one
/// folder a meeting (local, never roaming). A folder goes once its notes are
/// saved or the recording is thrown away.
pub fn notes_audio_dir() -> PathBuf {
    cache_dir().join("notes")
}

/// Where meeting notes are saved, as Markdown: `Documents\Oyenza Studio\Notes`
/// (wherever the user moved Documents to). Under `PONI_HOME` for the self-test.
pub fn notes_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("notes-saved");
    }
    let docs = known_folder(&windows::Win32::UI::Shell::FOLDERID_Documents).unwrap_or_else(|| env_dir("USERPROFILE").join("Documents"));
    docs.join("Oyenza Studio").join("Notes")
}

/// Paired phones (their pairing secrets and names), sealed with DPAPI.
/// Removing every phone deletes the file.
pub fn paired_phones_path() -> PathBuf {
    cache_dir().join("paired-phones.bin")
}

/// A key signed out while Oyenza couldn't be reached, sealed, kept only to
/// tell the server once it can.
pub fn oyenza_revoke_path() -> PathBuf {
    cache_dir().join("oyenza-revoke.bin")
}

/// A key typed into the AI tab for one service, or the inference.oyenza.com
/// key (`usage-<name>.bin`), sealed with DPAPI: never roams, and removing the
/// key or uninstalling deletes it.
pub fn usage_key_path(name: &str) -> PathBuf {
    cache_dir().join(format!("usage-{name}.bin"))
}
