//! What macOS's Now Playing reports (the player Control Center shows), as
//! MediaRemote Adapter (`third_party/mediaremote-adapter`) prints it: one JSON
//! object per line, `{"type":"data","diff":false,"payload":{…}}`.
//!
//! A full payload (`diff: false`) is everything known now; `{}` means nothing
//! plays. A diff (`diff: true`) only comes while the same track of the same
//! app plays, and carries the keys that changed; a key that went away is
//! `null`. The Mac's media service runs the adapter's `stream --micros`, so
//! times come in microseconds and the timestamp as Unix time.

use serde_json::{Map, Value};

/// The adapter's stream, as last reported.
#[derive(Clone, Debug, Default)]
pub struct NowPlayingStream {
    state: Map<String, Value>,
    /// The album art may have changed since [`NowPlayingStream::take_art_changed`].
    art_changed: bool,
}

/// One track in one app, from the stream's current state.
#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    /// The app that plays: the browser for a browser tab (macOS names its
    /// helper process as the player and the browser as its parent).
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds; 0 when unknown (live streams report none).
    pub duration_s: f64,
    /// Where the track was at `at_unix_s`.
    pub elapsed_s: f64,
    /// Unix time (seconds) when `elapsed_s` was right; `None` when the player
    /// didn't say (then `elapsed_s` is as of the line's arrival).
    pub at_unix_s: Option<f64>,
    pub playing: bool,
    /// Playback speed (1 normal), for moving the position on.
    pub rate: f64,
}

impl NowPlaying {
    /// The position at Unix time `now_s`, moved on from the last report while
    /// it plays.
    pub fn position_at(&self, now_s: f64) -> f64 {
        let mut p = self.elapsed_s;
        if let (true, Some(at)) = (self.playing, self.at_unix_s) {
            let rate = if self.rate > 0.0 { self.rate } else { 1.0 };
            p += (now_s - at).max(0.0) * rate;
        }
        if self.duration_s > 0.0 {
            p = p.min(self.duration_s);
        }
        p.max(0.0)
    }
}

impl NowPlayingStream {
    /// Read one line of the stream. `false` when it isn't a data line (then
    /// nothing changes).
    pub fn apply(&mut self, line: &str) -> bool {
        let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(line.trim()) else { return false };
        if msg.get("type").and_then(Value::as_str) != Some("data") {
            return false;
        }
        let Some(Value::Object(payload)) = msg.get("payload") else { return false };
        if msg.get("diff").and_then(Value::as_bool) != Some(true) {
            self.state = payload.clone();
            self.art_changed = true;
            return true;
        }
        self.art_changed |= payload.contains_key("artworkData");
        for (k, v) in payload {
            if v.is_null() {
                self.state.remove(k);
            } else {
                self.state.insert(k.clone(), v.clone());
            }
        }
        true
    }

    /// Forget everything (the adapter stopped).
    pub fn clear(&mut self) {
        self.state.clear();
        self.art_changed = true;
    }

    /// Whether the album art may have changed since the last call (a new
    /// track, or art that came or went), so it's decoded only then.
    pub fn take_art_changed(&mut self) -> bool {
        std::mem::take(&mut self.art_changed)
    }

    /// The track playing or paused now, if there is one with a title.
    pub fn now(&self) -> Option<NowPlaying> {
        let text = |k: &str| self.state.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
        let title = text("title");
        let app_id = [text("parentApplicationBundleIdentifier"), text("bundleIdentifier")].into_iter().find(|s| !s.is_empty())?;
        if title.is_empty() {
            return None;
        }
        let micros = |k: &str| self.state.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).map(|v| v / 1_000_000.0);
        // A live stream may say its length is "infinite" (the adapter drops
        // such values) or some huge number: either way, unknown.
        let duration_s = micros("durationMicros").filter(|d| *d > 0.0 && *d < 30.0 * 24.0 * 3600.0).unwrap_or(0.0);
        Some(NowPlaying {
            app_id,
            title,
            artist: text("artist"),
            album: text("album"),
            duration_s,
            elapsed_s: micros("elapsedTimeMicros").unwrap_or(0.0).max(0.0),
            at_unix_s: micros("timestampEpochMicros").filter(|t| *t > 0.0),
            playing: self.state.get("playing").and_then(Value::as_bool).unwrap_or(false),
            rate: self.state.get("playbackRate").and_then(Value::as_f64).filter(|r| r.is_finite()).unwrap_or(1.0),
        })
    }

    /// The track's album art (the image file's bytes), once the player has
    /// sent it; it can come a moment after the track.
    pub fn artwork(&self) -> Option<Vec<u8>> {
        let data = self.state.get("artworkData")?.as_str()?;
        crate::applemusic::base64url(data.trim()).filter(|b| !b.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(payload: &str) -> String {
        format!(r#"{{"type":"data","diff":false,"payload":{payload}}}"#)
    }

    fn diff(payload: &str) -> String {
        format!(r#"{{"type":"data","diff":true,"payload":{payload}}}"#)
    }

    /// What the adapter prints for a YouTube video in Safari (the browser
    /// as the parent of WebKit's player process).
    const SAFARI: &str = r#"{"processIdentifier":4242,"bundleIdentifier":"com.apple.WebKit.GPU","parentApplicationBundleIdentifier":"com.apple.Safari","playing":true,"title":"Paper Boats (Official Video)","artist":"The Lanterns","durationMicros":201000000,"elapsedTimeMicros":61500000,"timestampEpochMicros":1761480000000000,"playbackRate":1,"artworkMimeType":"image\/jpeg","artworkData":"/9j/4AAQ"}"#;

    #[test]
    fn reads_a_full_payload() {
        let mut s = NowPlayingStream::default();
        assert!(s.apply(&full(SAFARI)));
        let n = s.now().unwrap();
        assert_eq!(n.app_id, "com.apple.Safari");
        assert_eq!(n.title, "Paper Boats (Official Video)");
        assert_eq!(n.artist, "The Lanterns");
        assert_eq!(n.album, "");
        assert_eq!(n.duration_s, 201.0);
        assert_eq!(n.elapsed_s, 61.5);
        assert_eq!(n.at_unix_s, Some(1_761_480_000.0));
        assert!(n.playing);
        assert_eq!(n.rate, 1.0);
        assert_eq!(s.artwork().unwrap(), vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]);
    }

    #[test]
    fn diffs_update_and_remove_keys() {
        let mut s = NowPlayingStream::default();
        s.apply(&full(SAFARI));
        assert!(s.take_art_changed());
        assert!(s.apply(&diff(r#"{"playing":false,"playbackRate":0,"elapsedTimeMicros":75000000}"#)));
        assert!(!s.take_art_changed());
        assert!(s.apply(&diff(r#"{"artworkData":null}"#)));
        assert!(s.take_art_changed());
        assert!(!s.take_art_changed());
        let n = s.now().unwrap();
        assert!(!n.playing);
        assert_eq!(n.elapsed_s, 75.0);
        assert_eq!(n.title, "Paper Boats (Official Video)");
        assert!(s.artwork().is_none());
        // A full payload replaces everything, and an empty one means nothing plays.
        s.apply(&full(r#"{"processIdentifier":7,"bundleIdentifier":"com.spotify.client","playing":true,"title":"Blinding Lights","artist":"The Weeknd","album":"After Hours"}"#));
        let n = s.now().unwrap();
        assert_eq!(n.app_id, "com.spotify.client");
        assert_eq!(n.duration_s, 0.0);
        assert_eq!(n.at_unix_s, None);
        assert!(s.artwork().is_none());
        s.apply(&full("{}"));
        assert!(s.now().is_none());
    }

    #[test]
    fn ignores_what_isnt_a_data_line() {
        let mut s = NowPlayingStream::default();
        s.apply(&full(SAFARI));
        for line in ["", "null", "Failed to load framework", r#"{"type":"error"}"#, r#"{"type":"data","diff":true}"#, "{\"type\":\"data\",\"diff\":false,\"payl"] {
            assert!(!s.apply(line), "{line}");
        }
        assert_eq!(s.now().unwrap().app_id, "com.apple.Safari");
        s.clear();
        assert!(s.now().is_none());
    }

    #[test]
    fn needs_a_title_and_an_app() {
        let mut s = NowPlayingStream::default();
        s.apply(&full(r#"{"processIdentifier":9,"bundleIdentifier":"org.videolan.vlc","playing":true,"title":"  "}"#));
        assert!(s.now().is_none());
        s.apply(&full(r#"{"processIdentifier":9,"playing":true,"title":"No bundle"}"#));
        assert!(s.now().is_none());
        s.apply(&full(r#"{"processIdentifier":9,"bundleIdentifier":"org.videolan.vlc","title":"A file"}"#));
        let n = s.now().unwrap();
        assert_eq!(n.app_id, "org.videolan.vlc");
        assert!(!n.playing);
    }

    #[test]
    fn live_streams_have_no_length() {
        let mut s = NowPlayingStream::default();
        s.apply(&full(r#"{"processIdentifier":1,"bundleIdentifier":"com.apple.Music","playing":true,"title":"Radio","durationMicros":1.0e300}"#));
        assert_eq!(s.now().unwrap().duration_s, 0.0);
        s.apply(&full(r#"{"processIdentifier":1,"bundleIdentifier":"com.apple.Music","playing":true,"title":"Radio","durationMicros":-1}"#));
        assert_eq!(s.now().unwrap().duration_s, 0.0);
    }

    #[test]
    fn position_moves_on_while_playing() {
        let mut s = NowPlayingStream::default();
        s.apply(&full(SAFARI));
        let n = s.now().unwrap();
        assert_eq!(n.position_at(1_761_480_010.0), 71.5);
        // Never before the report, never past the end.
        assert_eq!(n.position_at(1_761_479_990.0), 61.5);
        assert_eq!(n.position_at(1_761_490_000.0), 201.0);
        let fast = NowPlaying { rate: 2.0, ..n.clone() };
        assert_eq!(fast.position_at(1_761_480_010.0), 81.5);
        let paused = NowPlaying { playing: false, ..n.clone() };
        assert_eq!(paused.position_at(1_761_480_010.0), 61.5);
        let unknown_time = NowPlaying { at_unix_s: None, ..n };
        assert_eq!(unknown_time.position_at(1_761_480_010.0), 61.5);
    }
}
