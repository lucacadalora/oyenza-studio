//! What macOS's own now-playing service (MediaRemote) says is playing: every
//! player that reports to it, browsers (YouTube, YouTube Music, Spotify on
//! the web) as much as the Spotify and Music apps. Since macOS 15.4 the
//! service answers Apple's own programs only, so the Mac app asks through the
//! MediaRemote adapter (third_party/mediaremote-adapter), which
//! `/usr/bin/perl` runs for it. Its `stream` command prints one JSON object a
//! line: `{"type":"data","diff":false,"payload":{…}}` with the whole state,
//! or `"diff":true` with only what changed (a key set to null went away). An
//! empty payload: nothing is playing.
//!
//! Here, away from the Mac, so it is tested with the rest of the core.

use serde_json::{Map, Value};

/// What the service says is playing.
#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    /// The player's bundle id (`com.google.Chrome`, `com.spotify.client`):
    /// the browser for a web page, whose sound may come from a helper
    /// process. Empty for a player without one.
    pub app_id: String,
    pub pid: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds; 0 when the player doesn't say.
    pub duration_s: f64,
    /// Seconds into the track at [`Self::at_unix_s`].
    pub elapsed_s: f64,
    /// When the player last said where it was (Unix seconds).
    pub at_unix_s: Option<f64>,
    pub rate: f64,
    pub playing: bool,
}

impl Playing {
    /// Where the track is at Unix time `now`.
    pub fn position_at(&self, now: f64) -> f64 {
        let mut p = self.elapsed_s;
        if let (true, Some(at)) = (self.playing, self.at_unix_s) {
            p += (now - at).max(0.0) * self.rate;
        }
        if self.duration_s > 0.0 {
            p = p.min(self.duration_s);
        }
        p.max(0.0)
    }
}

/// The service's state as the stream has told it so far.
#[derive(Clone, Debug, Default)]
pub struct Stream {
    now: Map<String, Value>,
}

impl Stream {
    /// Take one line the stream printed. Whether anything changed.
    pub fn apply(&mut self, line: &str) -> bool {
        let Ok(Value::Object(mut msg)) = serde_json::from_str::<Value>(line.trim()) else { return false };
        if msg.get("type").and_then(Value::as_str) != Some("data") {
            return false;
        }
        let diff = msg.get("diff").and_then(Value::as_bool).unwrap_or(false);
        let payload = match msg.remove("payload") {
            Some(Value::Object(p)) => p,
            _ => Map::new(),
        };
        if !diff {
            let changed = self.now != payload;
            self.now = payload;
            return changed;
        }
        let mut changed = false;
        for (k, v) in payload {
            if v.is_null() {
                changed |= self.now.remove(&k).is_some();
            } else if self.now.get(&k) != Some(&v) {
                self.now.insert(k, v);
                changed = true;
            }
        }
        changed
    }

    /// Forget everything (the stream stopped).
    pub fn clear(&mut self) {
        self.now.clear();
    }

    /// The track, when the service names one (a title, and whether it plays).
    pub fn playing(&self) -> Option<Playing> {
        let title = self.text("title")?;
        let playing = self.now.get("playing").and_then(Value::as_bool)?;
        let micros = |k: &str| self.now.get(k).and_then(Value::as_f64).map(|v| v / 1e6);
        let rate = self.now.get("playbackRate").and_then(Value::as_f64).filter(|r| r.is_finite() && *r > 0.0).unwrap_or(1.0);
        Some(Playing {
            app_id: self.text("parentApplicationBundleIdentifier").or_else(|| self.text("bundleIdentifier")).unwrap_or_default(),
            pid: self.now.get("processIdentifier").and_then(Value::as_i64),
            title,
            artist: self.text("artist").unwrap_or_default(),
            album: self.text("album").unwrap_or_default(),
            duration_s: micros("durationMicros").unwrap_or(0.0).max(0.0),
            elapsed_s: micros("elapsedTimeMicros").unwrap_or(0.0).max(0.0),
            at_unix_s: micros("timestampEpochMicros"),
            rate,
            playing,
        })
    }

    /// The artwork's bytes, when the player gave one.
    pub fn artwork(&self) -> Option<Vec<u8>> {
        crate::update::base64(self.now.get("artworkData")?.as_str()?).filter(|b| !b.is_empty())
    }

    /// Tells one artwork from another without decoding it (0: none).
    pub fn artwork_sig(&self) -> u64 {
        let Some(s) = self.now.get("artworkData").and_then(Value::as_str) else { return 0 };
        // FNV-1a over the base64 text.
        s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)) | 1
    }

    fn text(&self, k: &str) -> Option<String> {
        let s = self.now.get(k)?.as_str()?.trim();
        (!s.is_empty()).then(|| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{"type":"data","diff":false,"payload":{"bundleIdentifier":"company.thebrowser.dia","processIdentifier":4242,"playing":true,"title":"This Song Saved My Life","artist":"Simple Plan","album":"Get Your Heart On!","durationMicros":192000000,"elapsedTimeMicros":12500000,"timestampEpochMicros":1792834860000000,"playbackRate":1,"artworkMimeType":"image/jpeg","artworkData":"aGVsbG8="}}"#;

    #[test]
    fn a_whole_payload_is_the_track() {
        let mut s = Stream::default();
        assert!(s.apply(FULL));
        let p = s.playing().unwrap();
        assert_eq!(p.app_id, "company.thebrowser.dia");
        assert_eq!(p.pid, Some(4242));
        assert_eq!((p.title.as_str(), p.artist.as_str(), p.album.as_str()), ("This Song Saved My Life", "Simple Plan", "Get Your Heart On!"));
        assert_eq!((p.duration_s, p.elapsed_s, p.at_unix_s), (192.0, 12.5, Some(1_792_834_860.0)));
        assert!(p.playing);
        assert_eq!(s.artwork().unwrap(), b"hello");
        // The same line again changes nothing.
        assert!(!s.apply(FULL));
    }

    #[test]
    fn a_diff_changes_only_what_it_names_and_null_removes() {
        let mut s = Stream::default();
        s.apply(FULL);
        let sig = s.artwork_sig();
        assert!(s.apply(r#"{"type":"data","diff":true,"payload":{"playing":false,"elapsedTimeMicros":13000000,"artworkData":null,"artworkMimeType":null}}"#));
        let p = s.playing().unwrap();
        assert!(!p.playing);
        assert_eq!(p.elapsed_s, 13.0);
        assert_eq!(p.title, "This Song Saved My Life");
        assert_eq!(s.artwork(), None);
        assert_eq!(s.artwork_sig(), 0);
        assert_ne!(sig, 0);
        assert!(!s.apply(r#"{"type":"data","diff":true,"payload":{"playing":false}}"#));
    }

    #[test]
    fn an_empty_payload_is_nothing_playing() {
        let mut s = Stream::default();
        s.apply(FULL);
        assert!(s.apply(r#"{"type":"data","diff":false,"payload":{}}"#));
        assert_eq!(s.playing(), None);
        // A null payload, or one without a title or a state, names nothing either.
        s.apply(FULL);
        assert!(s.apply(r#"{"type":"data","diff":false,"payload":null}"#));
        assert_eq!(s.playing(), None);
        s.apply(r#"{"type":"data","diff":false,"payload":{"playing":true,"title":"  "}}"#);
        assert_eq!(s.playing(), None);
        s.apply(r#"{"type":"data","diff":false,"payload":{"title":"Paper Boats"}}"#);
        assert_eq!(s.playing(), None);
    }

    #[test]
    fn lines_that_are_not_data_are_ignored() {
        let mut s = Stream::default();
        assert!(!s.apply("Failed to load framework"));
        assert!(!s.apply(r#"{"type":"error","payload":{}}"#));
        assert!(!s.apply(""));
        assert_eq!(s.playing(), None);
    }

    #[test]
    fn a_web_page_is_its_browser() {
        let mut s = Stream::default();
        s.apply(r#"{"type":"data","diff":false,"payload":{"bundleIdentifier":"com.apple.WebKit.GPU","parentApplicationBundleIdentifier":"com.apple.Safari","playing":true,"title":"Paper Boats"}}"#);
        assert_eq!(s.playing().unwrap().app_id, "com.apple.Safari");
        // A player with no bundle (the adapter's test player) has no app id.
        s.apply(r#"{"type":"data","diff":false,"payload":{"processIdentifier":7,"playing":true,"title":"Is It Broken Yet?"}}"#);
        let p = s.playing().unwrap();
        assert_eq!((p.app_id.as_str(), p.pid, p.duration_s, p.at_unix_s), ("", Some(7), 0.0, None));
    }

    #[test]
    fn the_position_runs_on_from_the_report_while_playing() {
        let mut s = Stream::default();
        s.apply(FULL);
        let p = s.playing().unwrap();
        let at = 1_792_834_860.0;
        assert_eq!(p.position_at(at + 2.0), 14.5);
        // Never before the report, never past the end.
        assert_eq!(p.position_at(at - 5.0), 12.5);
        assert_eq!(p.position_at(at + 1000.0), 192.0);
        // At double speed, twice as far; paused, where it stopped.
        s.apply(r#"{"type":"data","diff":true,"payload":{"playbackRate":2}}"#);
        assert_eq!(s.playing().unwrap().position_at(at + 2.0), 16.5);
        s.apply(r#"{"type":"data","diff":true,"payload":{"playing":false,"playbackRate":0}}"#);
        let paused = s.playing().unwrap();
        assert_eq!((paused.rate, paused.position_at(at + 2.0)), (1.0, 12.5));
    }
}
