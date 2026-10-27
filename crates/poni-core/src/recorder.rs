//! Oyenza Studio Recorder, the screen recorder built from Cap, ships as its
//! own app (its recording engine is about 120 MB, the island 2.5 MB). The
//! island is its face: the Record page starts recordings through the
//! recorder's own `oyenza-recorder:` links, built here the same way on
//! Windows and the Mac, and the recorder tells the island how the recording
//! goes with the small messages [`parse_message`] reads (on Windows a
//! `WM_COPYDATA` to the island's window). While an island answers them, the
//! recorder shows no bar and no controls of its own: the island's recording
//! pill and Record page have Pause and Stop. An island that doesn't answer
//! (an older one, or none) leaves the recorder's own bar and controls in
//! place.

use crate::urlenc;
use serde::{Deserialize, Serialize};

/// The link scheme the recorder's installer registers.
pub const SCHEME: &str = "oyenza-recorder";

/// A Studio recording of the named screen, with the camera, microphone and
/// system sound the recorder used last time. `screen` is the name the system
/// gives the display (Windows: the display device's description; Mac:
/// `NSScreen`'s localized name), which is how the recorder finds it.
pub fn record_screen_link(screen: &str) -> String {
    action_link(&serde_json::json!({ "record_screen": { "screen": screen } }).to_string())
}

/// What the Record page records.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The screen the island is on, straight away.
    #[default]
    Screen,
    /// A window, picked on the recorder's own overlay.
    Window,
    /// Part of a screen, dragged out on the recorder's own overlay.
    Area,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Screen, Target::Window, Target::Area];

    pub fn key(self) -> &'static str {
        match self {
            Target::Screen => "screen",
            Target::Window => "window",
            Target::Area => "area",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Screen => "Screen",
            Target::Window => "Window",
            Target::Area => "Area",
        }
    }
}

/// What a recording takes in besides the picture. The camera and the
/// microphone are the ones the recorder used last (else its first camera and
/// the system's microphone): the island only says on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub camera: bool,
    pub microphone: bool,
    pub system_audio: bool,
}

/// A Studio recording of `target` with `inputs`. `screen` names the display
/// for [`Target::Screen`] (as in [`record_screen_link`]); a window or an area
/// is picked on the recorder's overlay, which starts the recording.
pub fn record_link(target: Target, screen: Option<&str>, inputs: Inputs) -> String {
    let mut record = serde_json::json!({
        "target": target.key(),
        "camera": inputs.camera,
        "microphone": inputs.microphone,
        "system_audio": inputs.system_audio,
    });
    if let (Target::Screen, Some(name)) = (target, screen) {
        record["screen"] = name.into();
    }
    action_link(&serde_json::json!({ "record": record }).to_string())
}

pub fn pause_link() -> String {
    action_link("\"pause_recording\"")
}

pub fn resume_link() -> String {
    action_link("\"resume_recording\"")
}

pub fn stop_link() -> String {
    action_link("\"stop_recording\"")
}

fn action_link(value: &str) -> String {
    format!("{SCHEME}://action?value={}", urlenc::encode(value))
}

/// A recording's length as the island shows it: "0:42", "12:34", "1:02:03".
/// Whole seconds gone by, so it never shows a second early.
pub fn fmt_length(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `COPYDATASTRUCT::dwData` of the recorder's messages to the island ("OYRM").
pub const COPYDATA_ID: usize = 0x4F59_524D;
/// The longest message the island reads; anything longer isn't one of ours.
pub const MESSAGE_MAX: usize = 4096;

/// Where the recorder's recording stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Asked to record: getting the capture going (or waiting on its overlay
    /// for a window or an area).
    Starting,
    Recording,
    Paused,
    /// Stopped: the recording is being saved, then the editor opens.
    Stopping,
    /// No recording: after a stop, a cancel or a failed start.
    Idle,
}

/// One message from the recorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// The person opened the recorder itself (the Start menu, the tray):
    /// the island opens on its Record page instead of the recorder's bar.
    Show,
    /// The recording moved on. `elapsed_ms` is its length when sent (paused
    /// time left out); `error` says why a start failed.
    State { phase: Phase, elapsed_ms: u64, error: Option<String> },
}

/// Read one message (UTF-8 JSON, [`MESSAGE_MAX`] bytes at most):
/// `{"v":1,"event":"show"}` or
/// `{"v":1,"event":"state","state":"recording","elapsed_ms":42000}`.
/// Unknown fields are left alone; an unknown event or state is `None`, so
/// the recorder falls back to its own window for it.
pub fn parse_message(bytes: &[u8]) -> Option<Message> {
    if bytes.len() > MESSAGE_MAX {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if v.get("v")?.as_u64()? < 1 {
        return None;
    }
    match v.get("event")?.as_str()? {
        "show" => Some(Message::Show),
        "state" => {
            let phase = match v.get("state")?.as_str()? {
                "starting" => Phase::Starting,
                "recording" => Phase::Recording,
                "paused" => Phase::Paused,
                "stopping" => Phase::Stopping,
                "idle" => Phase::Idle,
                _ => return None,
            };
            let elapsed_ms = v.get("elapsed_ms").and_then(|e| e.as_u64()).unwrap_or(0);
            let error = v.get("error").and_then(|e| e.as_str()).map(|e| e.chars().take(120).collect::<String>()).filter(|e| !e.trim().is_empty());
            Some(Message::State { phase, elapsed_ms, error })
        }
        _ => None,
    }
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

    #[test]
    fn lengths_read_like_a_clock() {
        assert_eq!(fmt_length(0.0), "0:00");
        assert_eq!(fmt_length(42.9), "0:42");
        assert_eq!(fmt_length(754.0), "12:34");
        assert_eq!(fmt_length(3723.5), "1:02:03");
        assert_eq!(fmt_length(-3.0), "0:00");
    }

    fn value_of(link: &str) -> serde_json::Value {
        assert!(is_action_link(link), "{link}");
        serde_json::from_str(&decode(link.split_once("value=").unwrap().1)).unwrap()
    }

    #[test]
    fn record_links_carry_the_target_and_inputs() {
        let link = record_link(Target::Screen, Some("Odyssey G93SC"), Inputs { camera: false, microphone: true, system_audio: false });
        assert_eq!(
            value_of(&link),
            serde_json::json!({ "record": { "target": "screen", "screen": "Odyssey G93SC", "camera": false, "microphone": true, "system_audio": false } })
        );
        // A window or an area is picked on the recorder's overlay: no screen name.
        for target in [Target::Window, Target::Area] {
            let value = value_of(&record_link(target, Some("Odyssey G93SC"), Inputs { camera: true, microphone: false, system_audio: true }));
            assert_eq!(value["record"]["target"], target.key());
            assert!(value["record"].get("screen").is_none(), "{value}");
            assert_eq!(value["record"]["camera"], true);
            assert_eq!(value["record"]["microphone"], false);
            assert_eq!(value["record"]["system_audio"], true);
        }
    }

    #[test]
    fn pause_resume_and_stop_are_the_recorders_own_actions() {
        for (link, action) in [(pause_link(), "pause_recording"), (resume_link(), "resume_recording"), (stop_link(), "stop_recording")] {
            assert_eq!(value_of(&link), serde_json::Value::String(action.into()));
        }
    }

    #[test]
    fn reads_the_recorders_messages() {
        assert_eq!(parse_message(br#"{"v":1,"event":"show"}"#), Some(Message::Show));
        assert_eq!(
            parse_message(br#"{"v":1,"event":"state","state":"recording","elapsed_ms":42000,"later":"field"}"#),
            Some(Message::State { phase: Phase::Recording, elapsed_ms: 42_000, error: None })
        );
        // A newer recorder's messages still read, and a failed start says why.
        assert_eq!(
            parse_message(br#"{"v":2,"event":"state","state":"idle","error":"No screen with that name"}"#),
            Some(Message::State { phase: Phase::Idle, elapsed_ms: 0, error: Some("No screen with that name".into()) })
        );
        for (state, phase) in [("starting", Phase::Starting), ("paused", Phase::Paused), ("stopping", Phase::Stopping)] {
            let m = format!(r#"{{"v":1,"event":"state","state":"{state}","elapsed_ms":5}}"#);
            assert_eq!(parse_message(m.as_bytes()), Some(Message::State { phase, elapsed_ms: 5, error: None }));
        }
    }

    #[test]
    fn refuses_what_isnt_a_message() {
        let long = format!(r#"{{"v":1,"event":"show","pad":"{}"}}"#, "x".repeat(MESSAGE_MAX));
        for bad in [
            &b""[..],
            b"not json",
            br#"{"event":"show"}"#,
            br#"{"v":0,"event":"show"}"#,
            br#"{"v":1,"event":"dance"}"#,
            br#"{"v":1,"event":"state","state":"flying"}"#,
            br#"{"v":1,"event":"state"}"#,
            long.as_bytes(),
        ] {
            assert_eq!(parse_message(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
