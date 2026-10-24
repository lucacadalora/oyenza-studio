//! Apple Music's own lyrics (opt-in): the lyrics the Apple Music app shows,
//! read through Apple's web API with the user's own Apple Music token.
//!
//! This is the web player's private API, not a published one, so it can
//! change without notice; Poni then falls back to LRCLIB. HTTP lives in the
//! Windows crate; this module finds the web player's developer token, builds
//! the URLs, reads the replies and picks the song.

use crate::lrc::Lyrics;
use crate::lrclib::{match_score, LrclibRecord, TrackQuery, MIN_SCORE};
use crate::urlenc::{encode, with_query};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A web player page; its HTML names the script bundle holding the
/// developer token.
pub const WEB_PLAYER_URL: &str = "https://music.apple.com/us/new";
pub const ORIGIN: &str = "https://music.apple.com";
pub const API_BASE: &str = "https://amp-api.music.apple.com/v1";
/// Storefront when the account's own can't be read.
pub const DEFAULT_STOREFRONT: &str = "us";

/// The web player's developer token (public: it ships in Apple's script).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevToken {
    pub token: String,
    /// Unix seconds; 0 when the token doesn't say.
    pub expires_at: i64,
}

/// A song from a catalog search.
#[derive(Clone, Debug, PartialEq)]
pub struct AmSong {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub duration_s: f64,
    pub has_lyrics: bool,
    pub has_synced: bool,
}

/// Absolute URLs of the web player's main script bundle(s) named in `html`
/// (`/assets/index~1a2b3c.js`; the legacy bundle is skipped).
pub fn find_bundle_urls(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("/assets/index") {
        let tail = &rest[i..];
        let end = tail.find(|c: char| c == '"' || c == '\'' || c == '<' || c == '>' || c.is_whitespace()).unwrap_or(tail.len());
        let path = &tail[..end];
        if path.ends_with(".js") && !path.contains("legacy") {
            let url = format!("{ORIGIN}{path}");
            if !out.contains(&url) {
                out.push(url);
            }
        }
        rest = &rest[i + "/assets/index".len()..];
    }
    out
}

/// The developer token in a script bundle: the JWT whose header names the
/// web player's key (`WebPlayKid`), else any JWT issued for apple.com.
pub fn find_developer_token(js: &str) -> Option<DevToken> {
    let mut fallback: Option<DevToken> = None;
    let mut rest = js;
    while let Some(i) = rest.find("eyJ") {
        let tail = &rest[i..];
        let len = tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')).unwrap_or(tail.len());
        let candidate = &tail[..len];
        rest = &tail[3..];
        let parts: Vec<&str> = candidate.split('.').collect();
        if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
            continue;
        }
        let (Some(header), Some(payload)) = (decode_json(parts[0]), decode_json(parts[1])) else { continue };
        let token = DevToken { token: candidate.to_string(), expires_at: payload["exp"].as_i64().unwrap_or(0) };
        if header["kid"] == "WebPlayKid" || payload["iss"] == "AMPWebPlay" {
            return Some(token);
        }
        let for_apple = payload["root_https_origin"].as_array().is_some_and(|a| a.iter().any(|o| o == "apple.com"));
        if for_apple && fallback.is_none() {
            fallback = Some(token);
        }
    }
    fallback
}

/// Whether a developer token should be fetched again (expired, or within a
/// day of it).
pub fn dev_token_stale(t: &DevToken, now: i64) -> bool {
    t.expires_at > 0 && now >= t.expires_at - 24 * 3600
}

/// The user's `media-user-token` as copied from the browser: accepts the bare
/// value, `media-user-token=…`, or a whole cookie header. `None` when the
/// text doesn't look like a token.
pub fn clean_user_token(text: &str) -> Option<String> {
    const NAME: &str = "media-user-token";
    let text = text.trim();
    let value = match text.find(NAME) {
        Some(i) => {
            let after = text[i + NAME.len()..].trim_start();
            let after = after.strip_prefix('=').or_else(|| after.strip_prefix(':'))?;
            after.split(';').next().unwrap_or("")
        }
        None => text,
    };
    let value = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let ok_char = |c: char| c.is_ascii_alphanumeric() || "+/=_-.%".contains(c);
    (value.len() >= 80 && value.len() <= 8192 && value.chars().all(ok_char)).then(|| value.to_string())
}

pub fn storefront_url() -> String {
    format!("{API_BASE}/me/storefront")
}

pub fn search_url(storefront: &str, q: &TrackQuery) -> String {
    let term = format!("{} {}", q.artist, q.title);
    with_query(&format!("{API_BASE}/catalog/{}/search", encode(storefront)), &[("term", term.trim()), ("types", "songs"), ("limit", "10")])
}

pub fn lyrics_url(storefront: &str, song_id: &str) -> String {
    format!("{API_BASE}/catalog/{}/songs/{}/lyrics", encode(storefront), encode(song_id))
}

/// The account's storefront ("id", "us", …) from `/me/storefront`.
pub fn parse_storefront(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    let id = v["data"][0]["id"].as_str()?.trim();
    (!id.is_empty() && id.len() <= 8 && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| id.to_ascii_lowercase())
}

pub fn parse_search(json: &str) -> Result<Vec<AmSong>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let Some(results) = v.get("results") else { return Err("no \"results\" in the reply".into()) };
    let songs = results["songs"]["data"].as_array().map(Vec::as_slice).unwrap_or_default();
    Ok(songs
        .iter()
        .filter_map(|s| {
            let a = &s["attributes"];
            Some(AmSong {
                id: s["id"].as_str()?.to_string(),
                name: a["name"].as_str()?.to_string(),
                artist: a["artistName"].as_str().unwrap_or("").to_string(),
                album: a["albumName"].as_str().unwrap_or("").to_string(),
                duration_s: a["durationInMillis"].as_f64().unwrap_or(0.0) / 1000.0,
                has_lyrics: a["hasLyrics"].as_bool().unwrap_or(false),
                has_synced: a["hasTimeSyncedLyrics"].as_bool().unwrap_or(false),
            })
        })
        .collect())
}

/// The search hit that is the queried song and has time-synced lyrics
/// (plain ones are left to LRCLIB, which may have them synced).
pub fn best_song<'a>(songs: &'a [AmSong], q: &TrackQuery) -> Option<&'a AmSong> {
    songs
        .iter()
        .filter(|s| s.has_synced)
        .filter_map(|s| match_score(q, &s.name, &s.artist, Some(s.duration_s).filter(|d| *d > 0.0), true).map(|score| (score, s)))
        .max_by_key(|(score, _)| *score)
        .filter(|(score, _)| *score >= MIN_SCORE)
        .map(|(_, s)| s)
}

/// The TTML document in a `/lyrics` reply.
pub fn parse_lyrics(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    let ttml = v["data"][0]["attributes"]["ttml"].as_str()?;
    (!ttml.trim().is_empty()).then(|| ttml.to_string())
}

/// Apple's lyrics as an LRCLIB-style record, so the cache and the page treat
/// both sources alike. Synced lyrics become LRC text.
pub fn to_record(song: &AmSong, lyrics: &Lyrics) -> LrclibRecord {
    let text = |timed: bool| {
        lyrics
            .lines
            .iter()
            .map(|l| if timed { format!("[{}]{}", lrc_stamp(l.time_s), l.text) } else { l.text.clone() })
            .collect::<Vec<_>>()
            .join("\n")
    };
    LrclibRecord {
        id: song.id.parse().unwrap_or(0),
        track_name: song.name.clone(),
        artist_name: song.artist.clone(),
        album_name: Some(song.album.clone()).filter(|a| !a.is_empty()),
        duration: Some(song.duration_s).filter(|d| *d > 0.0),
        instrumental: false,
        plain_lyrics: Some(text(false)),
        synced_lyrics: lyrics.synced.then(|| text(true)),
    }
}

/// `mm:ss.mmm` for an LRC timestamp.
fn lrc_stamp(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// base64url (no padding) → JSON.
fn decode_json(part: &str) -> Option<Value> {
    serde_json::from_slice(&base64url(part)?).ok()
}

fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lrc::{parse_lrc, LyricLine};

    fn b64(s: &str) -> String {
        const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = s.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    }

    fn jwt(header: &str, payload: &str) -> String {
        format!("{}.{}.c2lnbmF0dXJl", b64(header), b64(payload))
    }

    #[test]
    fn base64url_roundtrip() {
        for s in ["", "a", "ab", "abc", "{\"kid\":\"WebPlayKid\"}", "\u{e9}t\u{e9}"] {
            assert_eq!(base64url(&b64(s)).unwrap(), s.as_bytes());
        }
        assert!(base64url("not base64!").is_none());
    }

    #[test]
    fn finds_the_web_player_token() {
        let other = jwt(r#"{"alg":"ES256","kid":"OTHER"}"#, r#"{"iss":"X","exp":100,"root_https_origin":["apple.com"]}"#);
        let web = jwt(r#"{"alg":"ES256","kid":"WebPlayKid"}"#, r#"{"iss":"AMPWebPlay","exp":1798583676}"#);
        let js = format!(r#"var a="eyJnot.a.token";const t="{other}",u="{web}";"#);
        let t = find_developer_token(&js).unwrap();
        assert_eq!(t.token, web);
        assert_eq!(t.expires_at, 1798583676);
        // Without the web player key, any apple.com token will do.
        let t = find_developer_token(&format!("x='{other}'")).unwrap();
        assert_eq!(t.token, other);
        assert!(find_developer_token("no tokens here eyJ").is_none());

        assert!(!dev_token_stale(&DevToken { token: "t".into(), expires_at: 1798583676 }, 1_790_000_000));
        assert!(dev_token_stale(&DevToken { token: "t".into(), expires_at: 1000 }, 1000 - 3600));
        assert!(!dev_token_stale(&DevToken { token: "t".into(), expires_at: 0 }, i64::MAX));
    }

    #[test]
    fn finds_bundle_urls() {
        let html = r#"<script type="module" crossorigin src="/assets/index~ea66e33958.js"></script><link href="/assets/index~ea66e33958.css"><script src="/assets/index-legacy~1.js"></script>"#;
        assert_eq!(find_bundle_urls(html), vec!["https://music.apple.com/assets/index~ea66e33958.js".to_string()]);
        assert!(find_bundle_urls("<html></html>").is_empty());
    }

    #[test]
    fn cleans_user_tokens() {
        let raw = "Ak".to_string() + &"x".repeat(120) + "+/=";
        assert_eq!(clean_user_token(&raw).as_deref(), Some(raw.as_str()));
        assert_eq!(clean_user_token(&format!("  \"{raw}\"\n")).as_deref(), Some(raw.as_str()));
        assert_eq!(clean_user_token(&format!("media-user-token={raw}")).as_deref(), Some(raw.as_str()));
        assert_eq!(clean_user_token(&format!("itre=1; media-user-token={raw}; pldfltcid=abc")).as_deref(), Some(raw.as_str()));
        assert_eq!(clean_user_token(&format!("media-user-token: {raw}")).as_deref(), Some(raw.as_str()));
        assert!(clean_user_token("hello").is_none());
        assert!(clean_user_token(&format!("{raw} {raw}")).is_none());
        assert!(clean_user_token("media-user-token=").is_none());
    }

    #[test]
    fn urls() {
        let q = TrackQuery { title: "Paper Boats".into(), artist: "The Lanterns".into(), album: String::new(), duration_s: 201 };
        assert_eq!(
            search_url("id", &q),
            "https://amp-api.music.apple.com/v1/catalog/id/search?term=The%20Lanterns%20Paper%20Boats&types=songs&limit=10"
        );
        assert_eq!(lyrics_url("id", "1234"), "https://amp-api.music.apple.com/v1/catalog/id/songs/1234/lyrics");
        assert_eq!(storefront_url(), "https://amp-api.music.apple.com/v1/me/storefront");
    }

    #[test]
    fn reads_replies() {
        assert_eq!(parse_storefront(r#"{"data":[{"id":"id","type":"storefronts"}]}"#).as_deref(), Some("id"));
        assert_eq!(parse_storefront(r#"{"data":[]}"#), None);
        assert_eq!(parse_storefront(r#"{"data":[{"id":"../x"}]}"#), None);

        let search = r#"{"results":{"songs":{"data":[
            {"id":"11","attributes":{"name":"Paper Boats","artistName":"The Lanterns","albumName":"Harbor","durationInMillis":201300,"hasLyrics":true,"hasTimeSyncedLyrics":false}},
            {"id":"12","attributes":{"name":"Paper Boats","artistName":"The Lanterns","albumName":"Harbor (Deluxe)","durationInMillis":201500,"hasLyrics":true,"hasTimeSyncedLyrics":true}},
            {"id":"13","attributes":{"name":"Paper Boats","artistName":"Cover Band","albumName":"Covers","durationInMillis":250000,"hasLyrics":true,"hasTimeSyncedLyrics":true}},
            {"attributes":{"name":"no id"}}
        ]}}}"#;
        let songs = parse_search(search).unwrap();
        assert_eq!(songs.len(), 3);
        assert_eq!(songs[1].album, "Harbor (Deluxe)");
        assert!((songs[1].duration_s - 201.5).abs() < 1e-9);
        let q = TrackQuery { title: "Paper Boats".into(), artist: "The Lanterns".into(), album: String::new(), duration_s: 201 };
        // The synced one by the right artist, not the plain-only or the cover.
        assert_eq!(best_song(&songs, &q).unwrap().id, "12");
        let other = TrackQuery { title: "Other Song".into(), ..q.clone() };
        assert!(best_song(&songs, &other).is_none());
        assert!(parse_search(r#"{"results":{}}"#).unwrap().is_empty());
        assert!(parse_search(r#"{"errors":[]}"#).is_err());
        assert!(parse_search("<html>").is_err());

        assert_eq!(parse_lyrics(r#"{"data":[{"attributes":{"ttml":"<tt/>"}}]}"#).as_deref(), Some("<tt/>"));
        assert_eq!(parse_lyrics(r#"{"data":[]}"#), None);
    }

    #[test]
    fn records_roundtrip_through_lrc() {
        let song = AmSong {
            id: "12".into(),
            name: "Paper Boats".into(),
            artist: "The Lanterns".into(),
            album: "Harbor".into(),
            duration_s: 201.5,
            has_lyrics: true,
            has_synced: true,
        };
        let line = |t: f64, s: &str| LyricLine { time_s: t, text: s.into() };
        let lyrics = Lyrics { lines: vec![line(10.5, "Paper boats drift"), line(16.0, ""), line(62.25, "Harbor lights"), line(3725.0, "Late row")], synced: true };
        let r = to_record(&song, &lyrics);
        assert_eq!(r.id, 12);
        assert_eq!(r.duration, Some(201.5));
        let back = parse_lrc(r.synced_lyrics.as_deref().unwrap());
        assert!(back.synced);
        assert_eq!(back.lines, lyrics.lines);
        assert_eq!(r.plain_lyrics.as_deref(), Some("Paper boats drift\n\nHarbor lights\nLate row"));

        let unsynced = Lyrics { lines: vec![line(0.0, "One row")], synced: false };
        let r = to_record(&song, &unsynced);
        assert_eq!(r.synced_lyrics, None);
        assert_eq!(r.plain_lyrics.as_deref(), Some("One row"));
    }
}
