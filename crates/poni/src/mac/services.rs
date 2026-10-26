//! The Mac's side of what the island asks the system for: what plays (from
//! macOS's own now playing, or Music and Spotify themselves), lyrics, the add-ons (Japanese readings, and Prayer times'
//! location and adzan), power, volume, system stats, the System page's
//! readings, the clipboard, Shelf tiles, Phone drop, full-screen apps and
//! start at login (one file each), plus a few direct calls (the accent
//! colour, opening and revealing files, sounds). Oyenza sign-in is the
//! Windows app's worker, shared as is.

// This file is loaded with #[path], so its modules sit in services/ by path.
#[path = "services/adzan.rs"]
pub mod adzan;
#[path = "services/audio.rs"]
pub mod audio;
#[path = "services/cf.rs"]
mod cf;
#[path = "services/clipboard.rs"]
pub mod clipboard;
#[path = "services/fullscreen.rs"]
pub mod fullscreen;
#[path = "services/http.rs"]
pub mod http;
#[path = "services/media.rs"]
pub mod media;
#[path = "services/mediaremote.rs"]
mod mediaremote;
#[path = "services/romanize.rs"]
pub mod romanize;
#[path = "services/secret.rs"]
pub mod secret;
#[path = "services/smc.rs"]
pub mod smc;
// Lyrics (LRCLIB, and Apple Music's own with the person's token) are the
// Windows app's code, shared as is: only its HTTP client, token store and
// romaji readings above are the Mac's.
#[path = "../services/applemusic.rs"]
pub mod applemusic;
#[path = "../services/lyrics.rs"]
pub mod lyrics;
// So is the add-ons' downloader: the Mac offers every add-on with it.
#[path = "../services/addons.rs"]
pub mod addons;
#[path = "../services/cities.rs"]
pub mod cities;
// And Oyenza sign-in: its key goes through secret.rs above.
#[path = "../services/oyenza.rs"]
pub mod oyenza;
// And the update file's reader, which runs on the add-ons' thread.
#[path = "../services/update.rs"]
pub mod update;
// And the Stocks page's prices from oyenza.trade.
#[path = "../services/stocks.rs"]
pub mod stocks;
#[path = "services/location.rs"]
pub mod location;
#[path = "services/login.rs"]
pub mod login;
#[path = "services/phonedrop.rs"]
pub mod phonedrop;
#[path = "services/power.rs"]
pub mod power;
#[path = "services/recorder.rs"]
pub mod recorder;
#[path = "services/sysmon.rs"]
pub mod sysmon;
#[path = "services/sysstats.rs"]
pub mod sysstats;
#[path = "services/thumbs.rs"]
pub mod thumbs;

use objc2_app_kit::{NSColor, NSColorSpace, NSSound, NSWorkspace};
use objc2_foundation::{NSArray, NSFileManager, NSString, NSURL};
use poni_core::geom::Color;

pub mod imaging {
    //! What the Shelf page asks about its files.

    /// Size (DIPs) at which the Shelf draws a thumbnail tile.
    pub const THUMB_DIP: f32 = 52.0;

    #[allow(dead_code)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PathKind {
        File,
        Folder,
        Missing,
    }

    /// What is at a shelf path right now (a file system lookup; the Windows
    /// app caches this off the UI thread because its paths can be on a network).
    pub fn path_kind(path: &str) -> Option<PathKind> {
        Some(match std::fs::metadata(path) {
            Ok(m) if m.is_dir() => PathKind::Folder,
            Ok(_) => PathKind::File,
            Err(_) => PathKind::Missing,
        })
    }
}

/// The accent colour chosen in System Settings > Appearance.
pub fn system_accent() -> Option<Color> {
    let c = NSColor::controlAccentColor();
    let c = c.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    Some(Color::rgba(c.redComponent() as f32, c.greenComponent() as f32, c.blueComponent() as f32, 1.0))
}

/// Show a file selected in a Finder window.
pub fn reveal(path: &str) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
}

/// The app was called Oyen Poni before 0.2.0, and a copy from then has the same
/// bundle id, so Spotlight or the login item could start it instead of this one.
/// Moves "Oyen Poni.app" from /Applications or ~/Applications to the Trash, where
/// it can be put back, and returns the folder it was in.
pub fn trash_app_from_before_rename() -> Option<String> {
    let running = std::env::current_exe().ok()?;
    let mut dirs = vec![std::path::PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(std::path::PathBuf::from(home).join("Applications"));
    }
    for dir in dirs {
        let old = dir.join("Oyen Poni.app");
        if running.starts_with(&old) {
            continue;
        }
        let plist = std::fs::read_to_string(old.join("Contents/Info.plist")).unwrap_or_default();
        if !plist.contains("<string>com.oyenza.poni</string>") {
            continue;
        }
        let url = NSURL::fileURLWithPath(&NSString::from_str(&old.to_string_lossy()));
        match NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, None) {
            Ok(()) => {
                pinfo!("moved {} to the Trash (the app from before the rename)", old.display());
                return Some(dir.to_string_lossy().into_owned());
            }
            Err(e) => pwarn!("couldn't move {} to the Trash: {}", old.display(), e.localizedDescription()),
        }
    }
    None
}

/// Open a URL (or a file path) with its default app. System Settings'
/// panes have URLs without "//" (`x-apple.systempreferences:…`).
pub fn open(target: &str) {
    let s = NSString::from_str(target);
    let is_url = target.contains("://") || target.starts_with("x-apple.systempreferences:");
    let url = if is_url { NSURL::URLWithString(&s) } else { Some(NSURL::fileURLWithPath(&s)) };
    match url {
        Some(u) => {
            if !NSWorkspace::sharedWorkspace().openURL(&u) {
                pwarn!("could not open {target}");
            }
        }
        None => pwarn!("not a URL: {target}"),
    }
}

/// The gentle chime (timer finished): one of macOS's own alert sounds.
pub fn play(chime: bool) {
    let name = if chime { "Glass" } else { "Tink" };
    if let Some(s) = NSSound::soundNamed(&NSString::from_str(name)) {
        s.play();
    }
}
