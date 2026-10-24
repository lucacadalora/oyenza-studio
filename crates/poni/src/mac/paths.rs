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

/// Where files from a phone land (Phone drop): `~/Downloads/Oyen Poni`.
pub fn phone_inbox_dir() -> PathBuf {
    if let Some(h) = home_override() {
        return h.join("phone-drop");
    }
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Downloads").join("Oyen Poni")
}

pub fn lyrics_cache_dir() -> PathBuf {
    cache_dir().join("lyrics")
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
