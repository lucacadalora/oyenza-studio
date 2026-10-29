//! Where Poni keeps its files on a Mac.
//!
//! - Settings + data: `~/Library/Application Support/Poni/`
//! - Cache:           `~/Library/Caches/Poni/`
//! - Log:             `~/Library/Logs/Poni/poni.log` (Console.app lists it)
//!
//! `PONI_HOME` overrides all three (used by `--render` and `--smoke` to stay
//! isolated).

use std::path::PathBuf;

fn home_override() -> Option<PathBuf> {
    std::env::var_os("PONI_HOME").map(PathBuf::from)
}

fn library() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Library")
}

pub fn data_dir() -> PathBuf {
    home_override().unwrap_or_else(|| library().join("Application Support").join("Poni"))
}

pub fn cache_dir() -> PathBuf {
    home_override().map(|h| h.join("cache")).unwrap_or_else(|| library().join("Caches").join("Poni"))
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn store_path() -> PathBuf {
    data_dir().join("data.json")
}

pub fn log_path() -> PathBuf {
    home_override().map(|h| h.join("poni.log")).unwrap_or_else(|| library().join("Logs").join("Poni").join("poni.log"))
}

/// Held while Poni runs, so a second copy quits at start.
pub fn lock_path() -> PathBuf {
    cache_dir().join("poni.lock")
}

/// Where files from a phone land (Phone drop): `~/Downloads/Oyenza Studio`, or
/// `~/Downloads/Oyen Poni` where a copy from before the rename made it: Shelf
/// items link to the files in it.
pub fn phone_inbox_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("phone-drop");
    }
    let downloads = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Downloads");
    let before_rename = downloads.join("Oyen Poni");
    if before_rename.is_dir() { before_rename } else { downloads.join("Oyenza Studio") }
}

/// Where the Downloader saves videos and sound: `~/Downloads`, as a browser
/// would. An app may create files there without asking (macOS asks before
/// it reads one it didn't make). Under `PONI_HOME` for the smoke run.
pub fn media_downloads_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("downloads");
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Downloads")
}

/// A meeting's audio while it is recorded and handed over (Meeting notes),
/// deleted once its notes are saved.
pub fn notes_audio_dir() -> PathBuf {
    cache_dir().join("notes")
}

/// Where meeting notes are saved, as Markdown: `~/Documents/Oyenza Studio/Notes`.
/// Under `PONI_HOME` for the smoke run.
pub fn notes_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("notes-saved");
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Documents").join("Oyenza Studio").join("Notes")
}

pub fn lyrics_cache_dir() -> PathBuf {
    cache_dir().join("lyrics")
}

/// Add-on files, one folder per add-on (`services::addons`), only there
/// while the add-on is switched on. A Mac that clears its caches downloads
/// them again.
pub fn addons_dir() -> PathBuf {
    cache_dir().join("addons")
}

/// The IELTS page's audio: Oyen's clips and the answer being recorded, each
/// deleted once used.
pub fn ielts_dir() -> PathBuf {
    cache_dir().join("ielts")
}

/// The Oyenza sign-in (this Mac's key and the masked account), in a file
/// only this Mac account can read (`services::secret`). Signing out removes
/// it. With the settings, not the cache: macOS may empty caches by itself.
pub fn oyenza_link_path() -> PathBuf {
    data_dir().join("oyenza-link.bin")
}

/// A key signed out while Oyenza couldn't be reached, kept only to tell the
/// server once it can.
pub fn oyenza_revoke_path() -> PathBuf {
    data_dir().join("oyenza-revoke.bin")
}

/// A key typed into the AI tab for one service, or the inference.oyenza.com
/// key (`usage-<name>.bin`), in a file only this Mac account can read
/// (`services::secret`). With the settings, not the cache: macOS may empty
/// caches by itself.
pub fn usage_key_path(name: &str) -> PathBuf {
    data_dir().join(format!("usage-{name}.bin"))
}

/// The person's Apple Music token (owner-only file; uninstalling with the
/// cache folder removes it).
pub fn apple_music_token_path() -> PathBuf {
    cache_dir().join("apple-music-token.bin")
}

/// The Apple Music web player's (public) developer token, cached until it expires.
pub fn apple_music_web_token_path() -> PathBuf {
    cache_dir().join("apple-music-web.json")
}
