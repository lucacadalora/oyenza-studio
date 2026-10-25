//! Oyenza Studio Recorder on the Mac: the separate recorder app built from
//! Cap, started through its own link (`poni_core::recorder`). Pausing and
//! stopping stay on the recorder's own bar. Nothing here runs at rest.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_foundation::{NSString, NSURL};
use poni_core::recorder;

/// Whether some app opens `oyenza-recorder:` links, i.e. the recorder is
/// installed. Read when the menu bar's menu is built.
pub fn installed() -> bool {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&format!("{}://", recorder::SCHEME))) else {
        return false;
    };
    NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url).is_some()
}

/// Record the island's screen with the camera, microphone and system sound
/// the recorder used last time.
pub fn record_screen(mtm: MainThreadMarker) {
    match screen_name(mtm) {
        Some(name) => super::open(&recorder::record_screen_link(&name)),
        None => crate::pwarn!("recorder: the island's screen has no name"),
    }
}

/// The screen the island sits on (`window::pick_screen`: the notch screen,
/// else the menu-bar screen), by the name the recorder gives screens.
fn screen_name(mtm: MainThreadMarker) -> Option<String> {
    let island = crate::window::pick_screen(mtm)?;
    NSScreen::screens(mtm)
        .iter()
        .find(|s| {
            let f = s.frame();
            f.origin.x == island.x && f.origin.y == island.y && f.size.width == island.w && f.size.height == island.h
        })
        .map(|s| s.localizedName().to_string())
}
