//! Oyenza Studio Recorder, the screen recorder built from Cap
//! (lucacadalora/oyen-rekam), ships as its own app. The island starts it
//! through the recorder's own `oyenza-recorder:` links; this builds them, the
//! same way on Windows and the Mac.

use crate::addons::AddonFile;
use crate::urlenc;

/// The link scheme the recorder's installer registers.
pub const SCHEME: &str = "oyenza-recorder";

/// The app's name: its folder on Windows, `<APP_NAME>.app` on the Mac, and
/// the name its entry in Windows' installed apps shows.
pub const APP_NAME: &str = "Oyenza Studio Recorder";

/// The recorder release the Screen recorder add-on installs
/// (`addons::RECORDER`). The recorder updates itself after that, from its
/// own releases, so a new island release only moves this when new installs
/// should start from a newer recorder. `tools/pin_recorder.py` writes this
/// block from the release.
pub const VERSION: &str = "1.0.0";
/// Where the recorder's releases live: `<RELEASES>/v<VERSION>/<name>`.
pub const RELEASES: &str = "https://github.com/lucacadalora/oyen-rekam/releases/download";
/// The Windows installer (NSIS, per user: no administrator needed).
pub const WINDOWS_SETUP: AddonFile = AddonFile { name: "Oyenza-Studio-Recorder-1.0.0-setup.exe", size: 0, sha256: "" };
/// The Mac disk image (Apple silicon).
pub const MAC_DMG: AddonFile = AddonFile { name: "Oyenza-Studio-Recorder-1.0.0-mac.dmg", size: 0, sha256: "" };

/// This system's installer (`mac`: the Mac app's).
pub fn installer(mac: bool) -> &'static AddonFile {
    if mac {
        &MAC_DMG
    } else {
        &WINDOWS_SETUP
    }
}

pub fn installer_url(file: &AddonFile) -> String {
    format!("{RELEASES}/v{VERSION}/{}", file.name)
}

/// Whether the installers are pinned yet (sizes and SHA-256 written in by
/// `tools/pin_recorder.py`). Until then the Add-ons page leaves the Screen
/// recorder out, so nothing can download an installer it can't check.
pub const fn pinned() -> bool {
    WINDOWS_SETUP.size > 0 && MAC_DMG.size > 0 && WINDOWS_SETUP.sha256.len() == 64 && MAC_DMG.sha256.len() == 64
}

/// A Studio recording of the named screen, with the camera, microphone and
/// system sound the recorder used last time. `screen` is the name the system
/// gives the display (Windows: the display device's description; Mac:
/// `NSScreen`'s localized name), which is how the recorder finds it.
pub fn record_screen_link(screen: &str) -> String {
    action_link(&serde_json::json!({ "record_screen": { "screen": screen } }).to_string())
}

fn action_link(value: &str) -> String {
    format!("{SCHEME}://action?value={}", urlenc::encode(value))
}

/// Whether `link` has the shape [`record_screen_link`] builds: our scheme,
/// the `action` host and one percent-encoded value, nothing else.
pub fn is_action_link(link: &str) -> bool {
    let prefix = format!("{SCHEME}://action?value=");
    link.strip_prefix(prefix.as_str())
        .is_some_and(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'%')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn matches_the_link_the_recorder_parses() {
        // The value in the recorder's own test, parses_record_screen_action_url
        // (apps/desktop-gpui/src/deeplink.rs in lucacadalora/oyen-rekam).
        assert_eq!(
            record_screen_link("Odyssey G93SC"),
            "oyenza-recorder://action?value=%7B%22record_screen%22%3A%7B%22screen%22%3A%22Odyssey%20G93SC%22%7D%7D"
        );
    }

    #[test]
    fn quotes_and_non_ascii_names_stay_one_value() {
        let name = "LG \"UltraFine\" 5K \\ Layar kiri — 2";
        let link = record_screen_link(name);
        assert!(is_action_link(&link), "{link}");
        let value: serde_json::Value = serde_json::from_str(&decode(link.split_once("value=").unwrap().1)).unwrap();
        assert_eq!(value["record_screen"]["screen"], name);
    }

    #[test]
    fn installers_come_from_the_recorder_release() {
        assert_eq!(installer(false).name, WINDOWS_SETUP.name);
        assert_eq!(installer(true).name, MAC_DMG.name);
        assert_eq!(
            installer_url(&WINDOWS_SETUP),
            format!("https://github.com/lucacadalora/oyen-rekam/releases/download/v{VERSION}/{}", WINDOWS_SETUP.name)
        );
        for f in [WINDOWS_SETUP, MAC_DMG] {
            assert!(f.name.contains(VERSION), "{} names the pinned version", f.name);
            let hex = f.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            assert!(hex, "{}: the SHA-256 is lowercase hex", f.name);
            assert_eq!(f.size > 0, f.sha256.len() == 64, "{}: pinned with both a size and a SHA-256, or neither", f.name);
        }
    }

    #[test]
    fn only_our_shape_passes() {
        assert!(is_action_link("oyenza-recorder://action?value=%22stop_recording%22"));
        for bad in [
            "",
            "https://example.com/?value=%22stop_recording%22",
            "cap-desktop://action?value=%22stop_recording%22",
            "oyenza-recorder://action?value=",
            "oyenza-recorder://action?value=%22stop%22&next=1",
            "oyenza-recorder://action?value=a b",
            "oyenza-recorder://auth?value=%22x%22",
        ] {
            assert!(!is_action_link(bad), "{bad:?} should be refused");
        }
    }
}
