//! The Mac's side of what the pages ask the system for. Slice 1 has no
//! background services (music, Bluetooth, clipboard history and the Shelf's
//! thumbnails come in later slices); this holds the few direct calls the
//! island makes: the accent colour, opening links, the clipboard for paste.

use objc2_app_kit::{NSColor, NSColorSpace, NSPasteboard, NSPasteboardTypeString, NSSound, NSWorkspace};
use objc2_foundation::{NSString, NSURL};
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

/// Open a URL (or a file path) with its default app.
pub fn open(target: &str) {
    let s = NSString::from_str(target);
    let url = if target.contains("://") { NSURL::URLWithString(&s) } else { Some(NSURL::fileURLWithPath(&s)) };
    match url {
        Some(u) => {
            if !NSWorkspace::sharedWorkspace().openURL(&u) {
                pwarn!("could not open {target}");
            }
        }
        None => pwarn!("not a URL: {target}"),
    }
}

/// Plain text on the clipboard (for paste into a text field).
pub fn clipboard_text() -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    let s = pb.stringForType(unsafe { NSPasteboardTypeString })?;
    Some(s.to_string())
}

/// Put text on the clipboard.
pub fn set_clipboard_text(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let _ = pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
}

/// The gentle chime (timer finished): one of macOS's own alert sounds.
pub fn play(chime: bool) {
    let name = if chime { "Glass" } else { "Tink" };
    if let Some(s) = NSSound::soundNamed(&NSString::from_str(name)) {
        s.play();
    }
}
