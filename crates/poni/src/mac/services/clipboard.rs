//! The clipboard on the Mac: plain text, read when the person pastes into a
//! text field or the Script page (or uses an Apple Music token), and written
//! to clear that token. Nothing watches the pasteboard (the history that did
//! left the app after 0.2.8).

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

/// Plain text on the clipboard (for paste into a text field).
pub fn read_text() -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    let s = pb.stringForType(unsafe { NSPasteboardTypeString })?;
    Some(s.to_string())
}

/// Put text on the clipboard.
pub fn set_text(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let _ = pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
}
