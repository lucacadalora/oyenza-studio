//! LRCLIB (https://lrclib.net) request building and response parsing.
//!
//! LRCLIB is a free, open lyrics database with an open API (no key; a
//! descriptive User-Agent is requested). HTTP itself lives in the Windows
//! crate; this module only builds URLs, cleans up messy media metadata and
//! picks the best record.

use crate::lrc::parse_lrc;
use crate::urlenc::with_query;
use serde::{Deserialize, Serialize};

pub const API_BASE: &str = "https://lrclib.net/api";

/// Lyrics with fewer lines of text than this are stubs (a title, a lone
/// "♪"), not lyrics to show.
pub const MIN_LINES: usize = 4;

/// Synced lyrics stay in sync only when the record is about as long as what
/// plays; a music video with an intro or a radio edit would otherwise light
/// up the wrong lines.
pub const MAX_SYNC_DRIFT_S: f64 = 6.0;

/// Lowest [`match_score`] that counts as the same song.
pub const MIN_SCORE: i32 = 40;

/// Separators between artist and song in video titles: hyphen, en dash, em dash.
const SEPARATORS: [&str; 3] = [" - ", " \u{2013} ", " \u{2014} "];

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackQuery {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Whole seconds; 0 when unknown.
    pub duration_s: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrclibRecord {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub track_name: String,
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub album_name: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub instrumental: bool,
    #[serde(default)]
    pub plain_lyrics: Option<String>,
    #[serde(default)]
    pub synced_lyrics: Option<String>,
}

impl LrclibRecord {
    pub fn has_synced(&self) -> bool {
        self.synced_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty())
    }
    pub fn has_plain(&self) -> bool {
        self.plain_lyrics.as_deref().is_some_and(|s| !s.trim().is_empty())
    }
    /// Synced lyrics with at least [`MIN_LINES`] timed lines of text.
    pub fn has_full_synced(&self) -> bool {
        self.synced_lyrics.as_deref().is_some_and(|s| {
            let l = parse_lrc(s);
            l.synced && l.text_lines() >= MIN_LINES
        })
    }
    /// Plain lyrics with at least [`MIN_LINES`] lines of text.
    pub fn has_full_plain(&self) -> bool {
        self.plain_lyrics.as_deref().is_some_and(|s| s.lines().filter(|l| !l.trim().is_empty()).count() >= MIN_LINES)
    }
}

/// Whether lyrics timed for a `record_s`-second recording line up with a
/// `playing_s`-second track. Unknown lengths pass.
pub fn timing_fits(record_s: Option<f64>, playing_s: u32) -> bool {
    match record_s {
        Some(d) if d > 0.0 && playing_s > 0 => (d - playing_s as f64).abs() <= MAX_SYNC_DRIFT_S,
        _ => true,
    }
}

/// `/api/get` — exact signature match (title, artist, album, duration ±2 s).
pub fn get_url(q: &TrackQuery) -> String {
    let dur = if q.duration_s > 0 { q.duration_s.to_string() } else { String::new() };
    with_query(
        &format!("{API_BASE}/get"),
        &[("track_name", &q.title), ("artist_name", &q.artist), ("album_name", &q.album), ("duration", &dur)],
    )
}

/// `/api/search` — fuzzy fallback by title + artist.
pub fn search_url(q: &TrackQuery) -> String {
    with_query(&format!("{API_BASE}/search"), &[("track_name", &q.title), ("artist_name", &q.artist)])
}

pub fn parse_get(json: &str) -> Result<LrclibRecord, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

pub fn parse_search(json: &str) -> Result<Vec<LrclibRecord>, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// Pick the best search hit: prefer synced lyrics, close duration and a
/// matching normalised title/artist. Stubs (see [`MIN_LINES`]) never win.
pub fn best_match<'a>(records: &'a [LrclibRecord], q: &TrackQuery) -> Option<&'a LrclibRecord> {
    records
        .iter()
        .filter_map(|r| {
            let synced = r.has_full_synced();
            if !(synced || r.has_full_plain() || r.instrumental) {
                return None;
            }
            match_score(q, &r.track_name, &r.artist_name, r.duration, synced).map(|s| (s, r))
        })
        .max_by_key(|(s, _)| *s)
        .filter(|(s, _)| *s >= MIN_SCORE)
        .map(|(_, r)| r)
}

/// How well a candidate fits the query; `None` when the title doesn't match
/// at all. Title, artist, duration and synced lyrics add up; compare with
/// [`MIN_SCORE`].
pub fn match_score(q: &TrackQuery, title: &str, artist: &str, duration_s: Option<f64>, synced: bool) -> Option<i32> {
    let want_title = fold(&q.title);
    let want_artist = fold(&q.artist);
    let t = fold(title);
    let a = fold(artist);
    let mut score = 0i32;
    if t.is_empty() || want_title.is_empty() {
        return None;
    } else if t == want_title {
        score += 40;
    } else if t.contains(&want_title) || want_title.contains(&t) {
        score += 20;
    } else {
        return None;
    }
    if !want_artist.is_empty() {
        if a == want_artist {
            score += 30;
        } else if !a.is_empty() && (a.contains(&want_artist) || want_artist.contains(&a)) {
            score += 15;
        }
    }
    if q.duration_s > 0 {
        if let Some(d) = duration_s {
            let diff = (d - q.duration_s as f64).abs();
            if diff <= 2.0 {
                score += 25;
            } else if diff <= 6.0 {
                score += 10;
            } else if diff > 20.0 {
                score -= 30;
            }
        }
    }
    if synced {
        score += 20;
    }
    Some(score)
}

/// Lowercase, strip punctuation and collapse whitespace for comparisons.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_alphanumeric() {
            out.push(c);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim_end().to_string()
}

/// Clean media metadata coming from players and browsers so it matches the
/// database: strips "(Official Video)", "[4K UPGRADE]", "M/V", "[Lyrics]",
/// "[Sub Thai]", "(Lirik dan Terjemahan)", "- Remastered 2011",
/// "VEVO"/" - Topic" channel suffixes, and splits
/// "Artist - Title" (or "Title – Artist") titles that browsers report with
/// the channel as artist.
pub fn normalize_query(q: &TrackQuery) -> TrackQuery {
    let mut artist = q.artist.trim().to_string();
    let mut title = q.title.trim().to_string();

    for suffix in [" - Topic", "VEVO", "Vevo", " Official"] {
        if let Some(stripped) = artist.strip_suffix(suffix) {
            artist = stripped.trim().to_string();
        }
    }

    // "Artist - Title" or "Title – Artist" in the title (YouTube/browser sessions).
    if let Some((left, right)) = split_title(&title) {
        if artist.is_empty() || names_artist(left, &artist) {
            (artist, title) = (strip_decorations(left), right.to_string());
        } else if is_artist(right, &artist) {
            (artist, title) = (strip_decorations(right), left.to_string());
        }
    }

    let title = clean_title(&title);
    let album = strip_decorations(&q.album);
    TrackQuery { title, artist, album, duration_s: q.duration_s }
}

/// A song name without upload noise, "feat." parts or wrapping quotes.
fn clean_title(s: &str) -> String {
    let mut title = strip_decorations(s);
    // Remove "feat." parts from the title; LRCLIB keys usually omit them.
    // ASCII-only matching keeps byte offsets valid for the original string
    // (`to_lowercase` changes the length of e.g. "İ" and the Kelvin sign).
    for marker in ["(feat.", "(ft.", "[feat.", "[ft.", "feat.", "ft.", "(with "] {
        if let Some(i) = find_marker(&title, marker) {
            title = title[..i].trim().to_string();
        }
    }
    unquote(&title)
}

/// The first credited artist ("A, B & C" → "A").
pub fn primary_artist(artist: &str) -> String {
    artist
        .split([',', '&', ';', '/'])
        .next()
        .unwrap_or(artist)
        .split(" x ")
        .next()
        .unwrap_or(artist)
        .trim()
        .to_string()
}

/// For a browser tab whose channel names neither side of "A - B" (a lyric
/// channel, a record label): the title read as "Artist - Song", the usual
/// order, then as "Song - Artist", to try in turn when the query as reported
/// finds nothing. Only for browser sessions: player titles like
/// "Song - Live" have no artist in them.
pub fn title_guesses(q: &TrackQuery) -> Vec<TrackQuery> {
    let Some((left, right)) = split_title(&q.title) else { return Vec::new() };
    [(left, right), (right, left)]
        .into_iter()
        .filter_map(|(artist, song)| {
            let artist = strip_decorations(artist);
            let title = clean_title(song);
            (!artist.is_empty() && !title.is_empty())
                .then(|| TrackQuery { title, artist, album: String::new(), duration_s: q.duration_s })
        })
        .collect()
}

/// The title split at its first artist/song separator.
fn split_title(title: &str) -> Option<(&str, &str)> {
    SEPARATORS
        .iter()
        .filter_map(|sep| title.find(sep).map(|i| (i, sep.len())))
        .min_by_key(|(i, _)| *i)
        .map(|(i, n)| (title[..i].trim(), title[i + n..].trim()))
        .filter(|(l, r)| !l.is_empty() && !r.is_empty())
}

/// Folded and without spaces: channel names are often "TheLanterns".
fn squash(s: &str) -> String {
    fold(s).replace(' ', "")
}

/// Whether the left side of "A - B" names `artist` (often the channel:
/// "TheLanternsVEVO", "The Lanterns Music").
fn names_artist(side: &str, artist: &str) -> bool {
    let (side, artist) = (squash(&strip_decorations(side)), squash(artist));
    !side.is_empty() && !artist.is_empty() && (artist.contains(&side) || side.contains(&artist))
}

/// Whether the right side of "A - B" is the artist itself, so the title is
/// "Song – Artist". Stricter than [`names_artist`]: "Song - Band Remix" by
/// "Band" stays one title.
fn is_artist(side: &str, artist: &str) -> bool {
    let (side, artist) = (squash(&strip_decorations(side)), squash(artist));
    !side.is_empty() && (side == artist || artist.starts_with(&side))
}

/// Words that mark a bracketed part ("(Official Video)", "[4K UPGRADE]") as
/// upload noise rather than part of the song's name.
const NOISE_WORDS: &[&str] = &[
    "official", "oficial", "officiel", "video", "videoclip", "lyric", "lyrics", "lirik", "visualizer", "visualiser",
    "audio", "hd", "hq", "4k", "8k", "mv", "explicit",
    // Lyrics, subtitles and translations on fan and label uploads: "[Sub Thai]",
    // "(Lirik dan Terjemahan)", "[Vietsub]", "(Legendado)", "(Letra)".
    "letra", "paroles", "testo", "romanized", "romaji", "sub", "subs", "subbed", "subtitle", "subtitles", "subtitled",
    "subtítulos", "engsub", "thaisub", "indosub", "subindo", "vietsub", "terjemahan", "translation", "translated",
    "legendado", "legendada", "subtitulado", "subtitulada", "traducida", "traducido", "traducción", "tradução",
    "traduction", "traduzione", "übersetzung",
];

/// The same tags where words aren't split by spaces, found anywhere in the
/// bracket: Thai "ซับไทย" (Thai subs), "แปลไทย" / "คำแปล" (translation),
/// "เนื้อเพลง" (lyrics); Korean "자막" (subtitles), "가사" (lyrics), "해석" /
/// "번역" (translation); Chinese and Japanese "字幕" / "中字" (subtitles),
/// "歌詞" / "歌词" (lyrics), "和訳" / "翻譯" / "翻译" (translation). Vietnamese
/// "lời bài hát" (lyrics) is a phrase.
const NOISE_PARTS: &[&str] = &[
    "ซับไทย", "แปลไทย", "คำแปล", "เนื้อเพลง", "자막", "가사", "해석", "번역", "字幕", "中字", "歌詞", "歌词", "和訳", "翻譯", "翻译",
    "lời bài hát",
];

/// Unbracketed tags at the end of a title ("GANGNAM STYLE M/V"). Single words
/// like "Video" are left alone: "Home Video" is a song. Longer tags come
/// before the tags they end with.
const NOISE_TAILS: &[&str] = &[
    " official music video",
    " official lyric video",
    " official video",
    " official audio",
    " official mv",
    " lyric video",
    " m/v",
    " mv",
    " lirik dan terjemahan",
    " lirik terjemahan",
    " lyrics + vietsub",
    " sub thai",
    " thai sub",
    " sub indo",
    " indo sub",
    " eng sub",
    " engsub",
    " thaisub",
    " indosub",
    " vietsub",
];

fn is_noise(inner: &str) -> bool {
    let f = fold(inner);
    // Thai tone marks aren't alphanumeric, so `fold` would split "เนื้อเพลง":
    // match the parts against the bracket as written.
    let lower = inner.to_lowercase();
    f.contains("remaster")
        || f == "m v"
        || f.split(' ').any(|w| NOISE_WORDS.contains(&w))
        || NOISE_PARTS.iter().any(|p| lower.contains(p))
}

fn strip_decorations(s: &str) -> String {
    let mut out = s.trim().to_string();
    // Bracketed noise anywhere: (Official Video), [Lyrics], (4K Remaster), …
    'groups: loop {
        for (open, close) in [('(', ')'), ('[', ']'), ('\u{3010}', '\u{3011}')] {
            let mut end = out.len();
            while let Some(a) = out[..end].rfind(open) {
                let inner_start = a + open.len_utf8();
                if let Some(len) = out[inner_start..].find(close) {
                    if is_noise(&out[inner_start..inner_start + len]) {
                        let rest = &out[inner_start + len + close.len_utf8()..];
                        out = format!("{} {}", out[..a].trim_end(), rest.trim_start()).trim().to_string();
                        continue 'groups;
                    }
                }
                end = a;
            }
        }
        break;
    }
    // "Song | Official Video".
    if let Some(i) = out.rfind(" | ") {
        if is_noise(&out[i + 3..]) {
            out = out[..i].trim().to_string();
        }
    }
    // Unbracketed tags at the end; never down to nothing.
    while let Some(rest) = NOISE_TAILS.iter().find_map(|t| strip_suffix_ascii_ci(&out, t)) {
        if rest.trim().is_empty() {
            break;
        }
        out = rest.trim().to_string();
    }
    // " - Remastered 2011" style suffixes. The separator is searched in `out`
    // itself so the index is valid for slicing it.
    if let Some(i) = out.rfind(" - ") {
        let tail = out[i + 3..].to_lowercase();
        if tail.contains("remaster") || tail.contains("live") && tail.contains("version") || tail.starts_with("mono") {
            out = out[..i].trim().to_string();
        }
    }
    out
}

/// `s` without a trailing ASCII `suffix`, compared case-insensitively.
fn strip_suffix_ascii_ci<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let at = s.len().checked_sub(suffix.len())?;
    (s.is_char_boundary(at) && s.as_bytes()[at..].eq_ignore_ascii_case(suffix.as_bytes())).then(|| &s[..at])
}

/// A song name wrapped in quotes ("'Paper Boats'", K-pop uploads) without them.
fn unquote(s: &str) -> String {
    for (open, close) in [('\'', '\''), ('"', '"'), ('\u{2018}', '\u{2019}'), ('\u{201C}', '\u{201D}'), ('\u{300C}', '\u{300D}')] {
        if let Some(inner) = s.strip_prefix(open).and_then(|r| r.strip_suffix(close)) {
            if !inner.trim().is_empty() && !inner.contains(close) {
                return inner.trim().to_string();
            }
        }
    }
    s.to_string()
}

/// Byte index of the first ASCII-case-insensitive occurrence of `needle` in
/// `hay`. Only ASCII letters are folded, so the index always lies on a char
/// boundary of `hay` and can be used to slice it.
pub fn find_ascii_ci(hay: &str, needle: &str) -> Option<usize> {
    find_ascii_ci_from(hay, needle, 0)
}

fn find_ascii_ci_from(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() {
        return (from <= h.len()).then_some(from);
    }
    if n.len() > h.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Where a "feat." style `marker` starts in `title`, if it does so after some
/// text and at a word start ("Defeat." and "Left." are not markers).
fn find_marker(title: &str, marker: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = find_ascii_ci_from(title, marker, from) {
        let at_word_start = marker.starts_with(['(', '['])
            || title[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric());
        if i > 0 && at_word_start {
            return Some(i);
        }
        from = i + 1;
        // Step to the next char boundary (the match start is ASCII, so i + 1 is one).
        while from < title.len() && !title.is_char_boundary(from) {
            from += 1;
        }
    }
    None
}

/// Stable file-name-safe cache key for a query.
pub fn cache_key(q: &TrackQuery) -> String {
    // FNV-1a 64-bit over the folded fields.
    let mut h: u64 = 0xcbf29ce484222325;
    for part in [fold(&q.artist), fold(&q.title), q.duration_s.to_string()] {
        for b in part.bytes().chain(std::iter::once(0u8)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(title: &str, artist: &str, album: &str, d: u32) -> TrackQuery {
        TrackQuery { title: title.into(), artist: artist.into(), album: album.into(), duration_s: d }
    }

    #[test]
    fn builds_urls() {
        let u = get_url(&q("Paper Boats", "The Lanterns", "Harbor", 201));
        assert_eq!(
            u,
            "https://lrclib.net/api/get?track_name=Paper%20Boats&artist_name=The%20Lanterns&album_name=Harbor&duration=201"
        );
        let s = search_url(&q("Paper Boats", "", "", 0));
        assert_eq!(s, "https://lrclib.net/api/search?track_name=Paper%20Boats");
    }

    #[test]
    fn normalizes_browser_metadata() {
        let n = normalize_query(&q("The Lanterns - Paper Boats (Official Video)", "TheLanternsVEVO", "", 0));
        assert_eq!(n.artist, "The Lanterns");
        assert_eq!(n.title, "Paper Boats");
        let n = normalize_query(&q("Paper Boats - Remastered 2011", "The Lanterns", "", 0));
        assert_eq!(n.title, "Paper Boats");
        let n = normalize_query(&q("Paper Boats (feat. Someone)", "The Lanterns - Topic", "", 0));
        assert_eq!(n.title, "Paper Boats");
        assert_eq!(n.artist, "The Lanterns");
        // A hyphen in a real title with a non-matching artist stays put.
        let n = normalize_query(&q("Up - Down", "Other Band", "", 0));
        assert_eq!(n.title, "Up - Down");
    }

    #[test]
    fn normalizes_video_titles() {
        let n = |title: &str, channel: &str| {
            let n = normalize_query(&q(title, channel, "", 0));
            (n.artist, n.title)
        };
        let pair = |a: &str, t: &str| (a.to_string(), t.to_string());
        // Song first, en dash, extra tags.
        assert_eq!(n("Paper Boats (Official Music Video) [4K UPGRADE] \u{2013} The Lanterns", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("The Lanterns \u{2013} Paper Boats (Official Video Remastered)", "The Lanterns Official"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("The Lanterns \u{2014} Paper Boats", "TheLanternsVEVO"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("The Lanterns - Paper Boats (Official Video) (4K Remaster)", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(
            n("The Lanterns - Paper Boats ft. Someone [Official Video] Harbor Soundtrack", "The Lanterns Music"),
            pair("The Lanterns", "Paper Boats")
        );
        assert_eq!(n("LANTERNS - PAPER BOATS(\u{c885}\u{c774}\u{bc30}) M/V", "officiallanterns"), pair("LANTERNS", "PAPER BOATS(\u{c885}\u{c774}\u{bc30})"));
        assert_eq!(n("The Lanterns - 'Paper Boats' M/V", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("Paper Boats | Official Video", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        // Noise anywhere, not only in the last bracket; real parts stay.
        assert_eq!(n("Paper Boats (Official Video) (Live)", "The Lanterns").1, "Paper Boats (Live)");
        assert_eq!(
            n("Paper Boats (This Time for the Harbor) (The Official 2010 Harbor Song)", "The Lanterns").1,
            "Paper Boats (This Time for the Harbor)"
        );
        // Titles that only look like tags or "Song - Artist" stay whole.
        assert_eq!(n("Home Video", "Someone").1, "Home Video");
        assert_eq!(n("Official", "Someone").1, "Official");
        assert_eq!(n("Official Video", "Someone").1, "Official Video");
        assert_eq!(n("Paper Boats - Lanterns Remix", "Lanterns"), pair("Lanterns", "Paper Boats - Lanterns Remix"));
        assert_eq!(n("Paper Boats - Live", "The Lanterns"), pair("The Lanterns", "Paper Boats - Live"));
    }

    #[test]
    fn strips_subtitle_and_translation_tags() {
        let n = |title: &str, channel: &str| {
            let n = normalize_query(&q(title, channel, "", 0));
            (n.artist, n.title)
        };
        let pair = |a: &str, t: &str| (a.to_string(), t.to_string());
        assert_eq!(n("[Sub Thai] The Lanterns - Paper Boats", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("[THAISUB] Paper Boats - The Lanterns", "TheLanternsVEVO"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("Paper Boats - The Lanterns (ซับไทย)", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("The Lanterns - Paper Boats (Lirik dan Terjemahan)", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        assert_eq!(n("The Lanterns - Paper Boats | Lirik Terjemahan Indonesia", "The Lanterns"), pair("The Lanterns", "Paper Boats"));
        // A record label's channel names neither side: the tag goes, the
        // title stays whole for `title_guesses`.
        assert_eq!(
            n("[Sub Thai] Paper Boats - The Lanterns", "Harbor Music Thailand"),
            pair("Harbor Music Thailand", "Paper Boats - The Lanterns")
        );
        for title in [
            "Paper Boats (Lyrics + Vietsub)",
            "Paper Boats [Eng Sub]",
            "Paper Boats (Sub Español)",
            "Paper Boats (Legendado)",
            "Paper Boats (Letra)",
            "Paper Boats (แปลไทย)",
            "Paper Boats [เนื้อเพลง]",
            "Paper Boats [가사/해석]",
            "Paper Boats [한글자막]",
            "Paper Boats\u{3010}中字\u{3011}",
            "Paper Boats\u{3010}和訳\u{3011}",
            "Paper Boats (Lời bài hát)",
            // Unbracketed at the end.
            "Paper Boats Sub Indo",
            "Paper Boats Lyrics + Vietsub",
            "Paper Boats Lirik dan Terjemahan",
        ] {
            assert_eq!(n(title, "The Lanterns").1, "Paper Boats", "{title}");
        }
        // Words that only start like a tag, and tags outside brackets that
        // could be a song's name, stay.
        assert_eq!(n("Paper Boats (Submarine Mix)", "The Lanterns").1, "Paper Boats (Submarine Mix)");
        assert_eq!(n("Sub Rosa", "Someone").1, "Sub Rosa");
        assert_eq!(n("Vietsub", "Someone").1, "Vietsub");
    }

    #[test]
    fn reads_unknown_channel_titles_both_ways() {
        let read = |g: &TrackQuery| (g.artist.clone(), g.title.clone());
        let pair = |a: &str, t: &str| (a.to_string(), t.to_string());
        // A lyric-video channel names neither side: "Artist - Song" first.
        let n = normalize_query(&q("The Lanterns - Paper Boats (Lyrics)", "Lyric Channel", "", 180));
        assert_eq!(n.title, "The Lanterns - Paper Boats");
        let g = title_guesses(&n);
        assert_eq!(g.iter().map(read).collect::<Vec<_>>(), [pair("The Lanterns", "Paper Boats"), pair("Paper Boats", "The Lanterns")]);
        assert!(g.iter().all(|g| g.duration_s == 180 && g.album.is_empty()));
        // A record label's upload with the song first: the second reading finds it.
        let n = normalize_query(&q("[Sub Thai] Paper Boats - The Lanterns", "Harbor Music Thailand", "", 236));
        assert_eq!(read(&title_guesses(&n)[1]), pair("The Lanterns", "Paper Boats"));
        // Nothing to read as "A - B".
        assert!(title_guesses(&q("Paper Boats", "The Lanterns", "", 0)).is_empty());
        assert!(title_guesses(&q(" - Paper Boats", "", "", 0)).is_empty());
    }

    #[test]
    fn stubs_never_win() {
        let rec = |id: i64, synced: Option<&str>, plain: Option<&str>| LrclibRecord {
            id,
            track_name: "Paper Boats".into(),
            artist_name: "The Lanterns".into(),
            duration: Some(201.0),
            synced_lyrics: synced.map(Into::into),
            plain_lyrics: plain.map(Into::into),
            ..Default::default()
        };
        let full = "[00:01.00]one\n[00:02.00]two\n[00:03.00]three\n[00:04.00]four\n";
        let stub = rec(1, Some("[00:00.00]Paper Boats"), None);
        assert!(!stub.has_full_synced());
        let records = vec![stub.clone(), rec(2, Some(full), None)];
        assert_eq!(best_match(&records, &q("Paper Boats", "The Lanterns", "", 201)).unwrap().id, 2);
        assert!(best_match(&[stub], &q("Paper Boats", "The Lanterns", "", 201)).is_none());
        // Blank rows don't count as lines; plain-only records need text too.
        assert!(!rec(3, Some("[00:01.00]one\n[00:02.00]\n[00:03.00]\n[00:04.00]two"), None).has_full_synced());
        assert!(rec(4, None, Some("one\ntwo\n\nthree\nfour")).has_full_plain());
        assert!(!rec(5, None, Some("one\n\ntwo")).has_full_plain());
    }

    #[test]
    fn timing_fits_close_lengths_only() {
        assert!(timing_fits(Some(201.0), 205));
        assert!(timing_fits(Some(201.0), 195));
        assert!(!timing_fits(Some(201.0), 230));
        assert!(!timing_fits(Some(201.0), 180));
        // Unknown on either side: nothing to compare.
        assert!(timing_fits(None, 230));
        assert!(timing_fits(Some(201.0), 0));
        assert!(timing_fits(Some(0.0), 230));
    }

    #[test]
    fn non_ascii_case_folding_does_not_panic() {
        // "İ" lowercases to 3 bytes (from 2) and the Kelvin sign "K" to 1 byte
        // (from 3), so lowercase indexes do not fit the original string.
        let n = normalize_query(&q("İzmir - Çiçek", "Other", "", 0));
        assert_eq!(n.title, "İzmir - Çiçek");
        let n = normalize_query(&q("İİ - X", "", "", 0));
        assert_eq!((n.artist.as_str(), n.title.as_str()), ("İİ", "X"));
        let n = normalize_query(&q("İİ - X", "Someone", "", 0));
        assert_eq!(n.title, "İİ - X");
        let n = normalize_query(&q("Kelvin \u{212A} feat. Y", "Kelvin", "", 0));
        assert_eq!(n.title, "Kelvin \u{212A}");
        let n = normalize_query(&q("\u{212A}\u{212A}\u{212A} (Ft. Y)", "", "", 0));
        assert_eq!(n.title, "\u{212A}\u{212A}\u{212A}");
        let n = normalize_query(&q("GİTME (Official Video)", "Artist", "", 0));
        assert_eq!(n.title, "GİTME");
        let n = normalize_query(&q("GİTME - Remastered 2011", "Artist", "İİ (Official Audio)", 0));
        assert_eq!(n.title, "GİTME");
        assert_eq!(n.album, "İİ");
        assert_eq!(strip_decorations("İİ - X"), "İİ - X");
        assert_eq!(strip_decorations("İİ - Remastered"), "İİ");
        assert_eq!(strip_decorations("ÇİÇEK [Lyrics]"), "ÇİÇEK");
    }

    #[test]
    fn feat_markers_need_a_word_start() {
        assert_eq!(normalize_query(&q("Accept Defeat. Again", "A", "", 0)).title, "Accept Defeat. Again");
        assert_eq!(normalize_query(&q("Turn Left. Now", "A", "", 0)).title, "Turn Left. Now");
        assert_eq!(normalize_query(&q("Defeat. Song ft. B", "A", "", 0)).title, "Defeat. Song");
        assert_eq!(normalize_query(&q("Song FEAT. B", "A", "", 0)).title, "Song");
        assert_eq!(normalize_query(&q("Song [ft. B]", "A", "", 0)).title, "Song");
        assert_eq!(normalize_query(&q("Song (with B)", "A", "", 0)).title, "Song");
        assert_eq!(normalize_query(&q("feat. Only", "A", "", 0)).title, "feat. Only");
    }

    #[test]
    fn ascii_ci_find() {
        assert_eq!(find_ascii_ci("Song FEAT. x", "feat."), Some(5));
        assert_eq!(find_ascii_ci("İİ feat. x", "feat."), Some(5));
        assert_eq!(find_ascii_ci("\u{212A} ft.", "ft."), Some(4));
        assert_eq!(find_ascii_ci("abc", "abcd"), None);
        assert_eq!(find_ascii_ci("abc", ""), Some(0));
        // The Kelvin sign is not ASCII "k".
        assert_eq!(find_ascii_ci("\u{212A}", "k"), None);
    }

    #[test]
    fn primary_artist_split() {
        assert_eq!(primary_artist("A, B & C"), "A");
        assert_eq!(primary_artist("Solo"), "Solo");
    }

    #[test]
    fn parses_and_picks_best() {
        let json = r#"[
            {"id":1,"trackName":"Paper Boats","artistName":"The Lanterns","duration":240,"instrumental":false,"plainLyrics":"a\nb\nc\nd","syncedLyrics":null},
            {"id":2,"trackName":"Paper Boats","artistName":"The Lanterns","duration":201.5,"instrumental":false,"plainLyrics":"a\nb\nc\nd","syncedLyrics":"[00:01.00]a\n[00:02.00]b\n[00:03.00]c\n[00:04.00]d"},
            {"id":3,"trackName":"Something Else","artistName":"The Lanterns","duration":201,"instrumental":false,"plainLyrics":"a\nb\nc\nd","syncedLyrics":"[00:01.00]a\n[00:02.00]b\n[00:03.00]c\n[00:04.00]d"}
        ]"#;
        let recs = parse_search(json).unwrap();
        let best = best_match(&recs, &q("Paper Boats", "The Lanterns", "", 201)).unwrap();
        assert_eq!(best.id, 2);
        assert!(best_match(&recs, &q("Nope", "Nobody", "", 0)).is_none());
    }

    #[test]
    fn parse_get_tolerates_missing_fields() {
        let r = parse_get(r#"{"id":7,"trackName":"T","artistName":"A"}"#).unwrap();
        assert_eq!(r.id, 7);
        assert!(!r.has_synced());
    }

    #[test]
    fn cache_key_is_stable_and_safe() {
        let a = cache_key(&q("Paper Boats", "The Lanterns", "x", 201));
        let b = cache_key(&q("paper boats!", "the lanterns", "y", 201));
        assert_eq!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
