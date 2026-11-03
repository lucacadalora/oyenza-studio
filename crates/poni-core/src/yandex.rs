//! Yandex Music's own lyrics (opt-in, needs the user's OAuth token).
//!
//! Search is anonymous: `/search?text=…&type=track` lists tracks with
//! `lyricsInfo.hasAvailableSyncLyrics`, which is the only reliable signal that
//! a timed lyrics file exists. The lyrics file itself needs a signed request:
//!
//! ```text
//! GET /tracks/{id}/lyrics?format=LRC&timeStamp={unix}&sign={base64(hmac)}
//! sign = base64(HMAC-SHA256(key, "{id}{unix}"))
//! ```
//!
//! The reply carries a presigned `downloadUrl` (24 h) and the text behind it is
//! standard LRC (`[mm:ss.xx] line`). Without a Plus subscription Yandex answers
//! 200 with a paywall notice instead of the lyrics, so the body is checked
//! before it is trusted.
//!
//! Signing key and client header are a pair: the desktop pair below is the one
//! that answers in 2026, the Android pair stopped working for other clients.

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

pub const API_BASE: &str = "https://api.music.yandex.net";
/// Sent as `X-Yandex-Music-Client`; the signature only validates with the
/// matching key.
pub const CLIENT_HEADER: &str = "YandexMusicDesktopAppWindows/5.95.0";
const SIGN_KEY: &str = "kzqU4XhfCaY6B6JTHODeq5";
/// Yandex's canned answer for accounts without Plus.
const PAYWALL: &str = "Подключите подписку";

/// `base64(HMAC-SHA256(SIGN_KEY, "{track_id}{ts}"))`, standard alphabet with
/// padding. `ts` is Unix seconds; `track_id` is the numeric id.
/// `base64(HMAC-SHA256(key, "{track_id}{ts}"))`, standard alphabet with
/// padding. `ts` is Unix seconds; `track_id` is the numeric id.
pub fn sign_with(key: &str, track_id: i64, ts: i64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes any key");
    mac.update(format!("{track_id}{ts}").as_bytes());
    base64(&mac.finalize().into_bytes())
}

/// The signature with the key this client sends (`CLIENT_HEADER`).
pub fn sign(track_id: i64, ts: i64) -> String {
    sign_with(SIGN_KEY, track_id, ts)
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b1 = chunk[0] as u32;
        let b2 = *chunk.get(1).unwrap_or(&0) as u32;
        let b3 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b1 << 16) | (b2 << 8) | b3;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// One track from `/search`.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub duration_s: f64,
    pub has_sync: bool,
}

/// Tracks in a `/search?type=track` reply, in Yandex's order.
pub fn parse_search(body: &str) -> Vec<Hit> {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let results = v
        .pointer("/result/tracks/results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    results
        .iter()
        .filter_map(|t| {
            // `id` is a number here and a string in `/tracks/{id}`.
            let id = t.get("id").and_then(|x| match x {
                Value::Number(n) => n.as_i64(),
                Value::String(s) => s.parse().ok(),
                _ => None,
            })?;
            let title = t.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
            let artist = t
                .pointer("/artists/0/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let duration_s = t.get("durationMs").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
            let has_sync = t
                .pointer("/lyricsInfo/hasAvailableSyncLyrics")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(Hit { id, title, artist, duration_s, has_sync })
        })
        .collect()
}

/// The presigned link to the lyrics file in a `/lyrics` reply.
pub fn download_url(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.pointer("/result/downloadUrl").and_then(Value::as_str).map(str::to_string)
}

/// True when Yandex sent its "subscribe to Plus" notice instead of lyrics.
pub fn is_paywall(text: &str) -> bool {
    text.contains(PAYWALL)
}

/// The OAuth token in what the user copied: the bare token, or the whole
/// `access_token=…&token_type=…` fragment the browser puts in the address bar.
pub fn clean_token(text: &str) -> Option<String> {
    let raw = text.trim();
    let after = raw.split("access_token=").nth(1).unwrap_or(raw);
    let token = after.split(['&', '#', ' ', '\n']).next().unwrap_or("");
    let usable = token.len() >= 20
        && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    usable.then(|| token.to_string())
}

/// True when the body carries at least one `[mm:ss` timestamp.
pub fn looks_synced(text: &str) -> bool {
    text.match_indices('[').any(|(i, _)| {
        let b = &text.as_bytes()[i + 1..];
        b.len() >= 5 && b[0].is_ascii_digit() && b[1].is_ascii_digit() && b[2] == b':'
    })
}

/// The lyrics file as the pipeline expects it: the raw LRC text, ready for
/// `lrc` to parse. `duration` stays unknown on purpose: the file comes from
/// Yandex for this very track id, and a length mismatch would otherwise
/// downgrade it to plain text (the drift check is for cross-source matches).
pub fn record(hit: &Hit, lrc: String) -> crate::lrclib::LrclibRecord {
    crate::lrclib::LrclibRecord {
        id: hit.id,
        track_name: hit.title.clone(),
        artist_name: hit.artist.clone(),
        album_name: None,
        duration: None,
        instrumental: false,
        plain_lyrics: None,
        synced_lyrics: Some(lrc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_the_published_vector() {
        // MarshalX/yandex-music-api issue #565; the vector is the Android key's.
        assert_eq!(
            sign_with("p93jhgh689SBReK6ghtw62", 565378, 1668329814),
            "tNv3p9w458vwJtXEDPbEV9JtBlm7Mo3N8/Y1iKTU5Mw="
        );
    }

    #[test]
    fn search_reply_gives_tracks_and_the_sync_flag() {
        let body = r#"{"result":{"tracks":{"results":[
            {"id":157548631,"title":"Красивая мелодия","durationMs":240000,
             "artists":[{"name":"КУОК"},{"name":"SABU"}],
             "lyricsInfo":{"hasAvailableSyncLyrics":true,"hasAvailableTextLyrics":true}},
            {"id":"42","title":"ПУШКА","durationMs":120000,
             "artists":[{"name":"КУОК"}],"lyricsInfo":{"hasAvailableSyncLyrics":false}}
        ]}}}"#;
        let hits = parse_search(body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, 157548631);
        assert_eq!(hits[0].artist, "КУОК");
        assert_eq!(hits[0].duration_s, 240.0);
        assert!(hits[0].has_sync);
        assert_eq!(hits[1].id, 42, "id may arrive as a string");
        assert!(!hits[1].has_sync);
    }

    #[test]
    fn paywall_and_lrc_are_told_apart() {
        assert!(is_paywall("Подключите подписку на Плюс, чтобы видеть тексты песен"));
        assert!(!is_paywall("[00:13.11] Когда я прихожу сюда"));
        assert!(looks_synced("[00:13.11] Когда я прихожу сюда\n[00:16.26] Мне кажется"));
        assert!(!looks_synced("просто текст без таймингов"));
    }

    #[test]
    fn download_url_is_read_from_the_reply() {
        let body = r#"{"result":{"downloadUrl":"https://x/y?z=1","lyricId":1}}"#;
        assert_eq!(download_url(body).as_deref(), Some("https://x/y?z=1"));
        assert_eq!(download_url("{}"), None);
    }

    #[test]
    fn a_copied_token_is_cleaned() {
        let token = "y0__wgBEJPbnu0CGN74BiDmncTQGTDbiJSaCO5Y3bCPl4W5oFmS_5jzJ";
        assert_eq!(clean_token(token).as_deref(), Some(token));
        let fragment = format!("{token}&token_type=bearer&expires_in=31536000&cid=abc");
        assert_eq!(clean_token(&fragment).as_deref(), Some(token));
        let with_key = format!("access_token={token}");
        assert_eq!(clean_token(&with_key).as_deref(), Some(token));
        assert_eq!(clean_token("короткий"), None);
        assert_eq!(clean_token(""), None);
    }
}
