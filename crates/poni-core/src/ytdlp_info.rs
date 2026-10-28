//! What a pasted link holds, before anything downloads: the picture, title,
//! site, length and the qualities it comes in, as the Downloads page shows
//! it. Read from yt-dlp's `-J --flat-playlist` reply: one JSON object, a
//! video with its formats or a playlist whose items are only listed. Pure, so
//! it is tested here against yt-dlp's replies (`tests/fixtures/ytdlp_info/`;
//! the YouTube and SoundCloud ones are hand-written in yt-dlp's shape).

use serde_json::Value;

/// A link, as the island shows it before downloading.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    /// The site's id for it (a playlist's own id for a playlist).
    pub id: String,
    pub title: String,
    /// The site's name ([`site_name`]).
    pub site: String,
    /// Who put it up: the channel, else the account.
    pub uploader: Option<String>,
    pub duration_s: Option<f64>,
    /// The picture: the widest one yt-dlp lists.
    pub thumbnail: Option<String>,
    /// The picture heights it comes in, tallest first. Empty for sound
    /// alone, and for a playlist (its items aren't read yet).
    pub qualities: Vec<Quality>,
    /// Whether it has sound to save (the Audio choice). A playlist, whose
    /// items aren't read yet, says yes.
    pub audio: bool,
    /// Set for a playlist or a channel: its items.
    pub playlist: Option<PlaylistInfo>,
}

/// One picture height, and about how big the download is at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quality {
    /// In pixels: 1080 for 1920x1080 ([`quality_label`]). An upright video
    /// counts its long side (1920 for 1080x1920).
    pub height: u32,
    /// The picture's file plus the best sound's, the ones yt-dlp picks.
    /// None when neither the site nor the bitrate tells.
    pub approx_bytes: Option<u64>,
}

/// A playlist's items, as listed.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistInfo {
    /// How many items the site says it has, else how many are listed.
    pub count: Option<u32>,
    pub entries: Vec<Entry>,
}

/// One item of a playlist.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub title: Option<String>,
    /// Its own page, to download it by.
    pub url: String,
    pub duration_s: Option<f64>,
}

/// Reads yt-dlp's `-J --flat-playlist` reply. The error (for the log, not
/// the page) says why it isn't one.
pub fn parse(json: &str) -> Result<Preview, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if !v.is_object() {
        return Err("yt-dlp's reply isn't a JSON object".into());
    }
    let id = text(&v["id"]).unwrap_or_default();
    let Some(title) = text(&v["title"]).or_else(|| Some(id.clone()).filter(|id| !id.is_empty())) else {
        return Err("no id or title in yt-dlp's reply".into());
    };
    let entries = v["entries"].as_array();
    let formats = v["formats"].as_array().map(Vec::as_slice).unwrap_or_default();
    let duration_s = num(&v["duration"]);
    Ok(Preview {
        id,
        title,
        site: site_name(v["extractor"].as_str().unwrap_or(""), v["webpage_url_domain"].as_str().unwrap_or("")),
        uploader: text(&v["channel"]).or_else(|| text(&v["uploader"])),
        duration_s,
        // A playlist without a picture of its own shows its first item's.
        thumbnail: picture(&v).or_else(|| entries?.iter().find_map(picture)),
        qualities: qualities(formats, duration_s),
        audio: entries.is_some() || formats.iter().any(|f| !ignored(f) && !lacks(f, "acodec")),
        playlist: entries.map(|e| PlaylistInfo {
            count: num(&v["playlist_count"]).map(|n| n as u32).or(u32::try_from(e.len()).ok()),
            entries: e.iter().filter_map(entry).collect(),
        }),
    })
}

/// "1080p", "1440p"; "4K" and "8K" for 2160 and 4320, as people say them.
pub fn quality_label(height: u32) -> String {
    match height {
        2160 => "4K".to_string(),
        4320 => "8K".to_string(),
        h => format!("{h}p"),
    }
}

/// "9:56", "1:02:03": whole seconds, like a recording's length.
pub fn duration_label(s: f64) -> String {
    crate::recorder::fmt_length(s)
}

/// The site's name as people know it, from yt-dlp's `extractor`
/// ("youtube:tab") and the page's `domain`: a few sites whose extractor
/// names read badly by name, the rest by their address ("vimeo.com").
pub fn site_name(extractor: &str, domain: &str) -> String {
    let known = match extractor.split(':').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "youtube" => "YouTube",
        "soundcloud" => "SoundCloud",
        "archive.org" => "Internet Archive",
        "peertube" => "PeerTube",
        _ => "",
    };
    let domain = domain.strip_prefix("www.").unwrap_or(domain);
    [known, domain, extractor].into_iter().find(|s| !s.is_empty()).unwrap_or_default().to_string()
}

/// The heights `formats` come in, tallest first, each with the size of the
/// format yt-dlp picks for it, plus the best sound's when that format has
/// none. yt-dlp lists formats worst first, so the last of a kind is its pick;
/// one whose size can't be told leaves the next best's.
fn qualities(formats: &[Value], duration_s: Option<f64>) -> Vec<Quality> {
    let formats: Vec<&Value> = formats.iter().filter(|f| !ignored(f)).collect();
    let sound = formats.iter().rev().filter(|f| lacks(f, "vcodec") && !lacks(f, "acodec")).find_map(|f| bytes(f, duration_s));
    let mut out: Vec<Quality> = Vec::new();
    for f in formats.iter().filter(|f| !lacks(f, "vcodec")) {
        let Some(height) = num(&f["height"]).map(|h| h.round() as u32) else { continue };
        let size = bytes(f, duration_s).map(|b| if lacks(f, "acodec") { b + sound.unwrap_or(0) } else { b });
        match out.iter_mut().find(|q| q.height == height) {
            Some(q) => q.approx_bytes = size.or(q.approx_bytes),
            None => out.push(Quality { height, approx_bytes: size }),
        }
    }
    out.sort_by_key(|q| std::cmp::Reverse(q.height));
    out
}

/// A format's size: the site's own figure, else its bitrate (kbit/s) over
/// the whole length, as yt-dlp estimates it.
fn bytes(f: &Value, duration_s: Option<f64>) -> Option<u64> {
    let size = num(&f["filesize"]).or_else(|| num(&f["filesize_approx"])).or_else(|| Some(num(&f["tbr"])? * duration_s? * 125.0))?;
    Some(size.round() as u64)
}

/// Not something to download: a storyboard (the seek bar's strips of
/// stills, `mhtml`), any other format with neither picture nor sound
/// (yt-dlp's "images"), or one locked by DRM.
fn ignored(f: &Value) -> bool {
    (lacks(f, "vcodec") && lacks(f, "acodec")) || f["ext"] == "mhtml" || f["format_note"] == "storyboard" || f["has_drm"] == true
}

/// yt-dlp writes "none" for the part a format hasn't got. A codec it doesn't
/// know is left out, and counts as there.
fn lacks(f: &Value, codec: &str) -> bool {
    f[codec] == "none"
}

/// The widest of `thumbnails`, else `thumbnail`, else the last of
/// `thumbnails` (yt-dlp lists them worst first).
fn picture(v: &Value) -> Option<String> {
    let all = v["thumbnails"].as_array().map(Vec::as_slice).unwrap_or_default();
    let widest = all.iter().filter_map(|t| Some((num(&t["width"])?, text(&t["url"])?))).max_by(|a, b| a.0.total_cmp(&b.0));
    widest.map(|(_, url)| url).or_else(|| text(&v["thumbnail"])).or_else(|| all.iter().rev().find_map(|t| text(&t["url"])))
}

/// A playlist item; one without an address to download it by is left out.
fn entry(e: &Value) -> Option<Entry> {
    Some(Entry {
        id: text(&e["id"]).unwrap_or_default(),
        title: text(&e["title"]),
        url: text(&e["webpage_url"]).or_else(|| text(&e["url"]))?,
        duration_s: num(&e["duration"]),
    })
}

/// A string (trimmed, not empty) or a number, as text.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim()).filter(|s| !s.is_empty()).map(str::to_string),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A number above zero.
fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| *n > 0.0 && n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded with yt-dlp 2026.10.26 (Linux) and trimmed to the fields read
    // here plus a few that aren't.
    const PEERTUBE: &str = include_str!("../tests/fixtures/ytdlp_info/peertube-video.json");
    const PLAYLIST: &str = include_str!("../tests/fixtures/ytdlp_info/peertube-playlist.json");
    const ARCHIVE: &str = include_str!("../tests/fixtures/ytdlp_info/archive-org.json");
    // Hand-written in yt-dlp's shape: YouTube refuses the VM they were made
    // on, and SoundCloud's robots.txt closes track pages to Claude.
    const YOUTUBE: &str = include_str!("../tests/fixtures/ytdlp_info/youtube-handwritten.json");
    const SOUNDCLOUD: &str = include_str!("../tests/fixtures/ytdlp_info/soundcloud-handwritten.json");

    fn heights(p: &Preview) -> Vec<u32> {
        p.qualities.iter().map(|q| q.height).collect()
    }

    #[test]
    fn a_peertube_video_comes_in_1080p_and_480p_with_its_sound() {
        let p = parse(PEERTUBE).unwrap();
        assert_eq!(p.id, "b29290cc-dc51-4a12-bcb2-2aa5fece7605");
        assert_eq!(p.title, "OVERGROWN: a feature film project by Blender Studio");
        assert_eq!((p.site.as_str(), p.uploader.as_deref()), ("PeerTube", Some("Blender Studio")), "the channel, not the account");
        assert_eq!(p.duration_s.map(duration_label).as_deref(), Some("1:10"));
        assert_eq!(p.thumbnail.as_deref(), Some("https://video.blender.org/lazy-static/thumbnails/2d0a6dff-ad41-4e1c-a921-53338b30759c.jpg"));
        assert_eq!(heights(&p), [1080, 480], "the sound-only file adds no height");
        // yt-dlp picks the HLS copies, which only give a bitrate: bitrate
        // times length lands within a few bytes of the plain files' sizes.
        let near = |q: &Quality, file: u64| q.approx_bytes.is_some_and(|b| b.abs_diff(file) < 1_000);
        assert!(near(&p.qualities[0], 23_256_496) && near(&p.qualities[1], 8_315_403), "{:?}", p.qualities);
        assert!(p.audio && p.playlist.is_none());
    }

    #[test]
    fn an_archive_org_item_offers_each_file_at_its_size() {
        let p = parse(ARCHIVE).unwrap();
        assert_eq!((p.title.as_str(), p.site.as_str(), p.uploader.as_deref()), ("Big Buck Bunny", "Internet Archive", Some("jake@archive.org")));
        assert_eq!(p.duration_s.map(duration_label).as_deref(), Some("9:56"));
        assert_eq!(
            p.qualities,
            [
                Quality { height: 720, approx_bytes: Some(332_243_668) },
                Quality { height: 360, approx_bytes: Some(61_878_609) },
                Quality { height: 300, approx_bytes: Some(46_935_223) },
            ]
        );
        // No codecs named: like yt-dlp, the files count as having sound.
        assert!(p.audio);
        // No picture says its width: yt-dlp's own pick.
        assert!(p.thumbnail.unwrap().ends_with("/big_buck_bunny_720p_surround_000585.jpg"));
    }

    #[test]
    fn a_playlist_lists_its_videos_without_reading_them() {
        let p = parse(PLAYLIST).unwrap();
        assert_eq!((p.id.as_str(), p.title.as_str(), p.site.as_str()), ("j5y8znJnMkYKVpRUwtHTUQ", "BCON20", "PeerTube"));
        assert_eq!(p.thumbnail.as_deref(), Some("https://video.blender.org/lazy-static/thumbnails/playlist-926745ab-c50d-4cd9-88b9-c6dec96d407c.jpg"));
        let list = p.playlist.as_ref().unwrap();
        assert_eq!((list.count, list.entries.len()), (Some(3), 3));
        assert_eq!(
            list.entries[0],
            Entry {
                id: "6mpVJBp4AWggD7k1gvhNvc".into(),
                title: Some("Blender Conference 2020 - Pre Party".into()),
                url: "https://video.blender.org/w/6mpVJBp4AWggD7k1gvhNvc".into(),
                duration_s: None,
            }
        );
        assert!(p.qualities.is_empty() && p.duration_s.is_none());
        assert!(p.audio, "Audio stays on offer: the items aren't read yet");
    }

    #[test]
    fn youtube_comes_in_4k_to_144p_each_with_the_best_sound_added() {
        let p = parse(YOUTUBE).unwrap();
        assert_eq!((p.site.as_str(), p.uploader.as_deref(), p.duration_s), ("YouTube", Some("An Example Channel"), Some(213.0)));
        // The storyboard (320x180 strips of stills) is no quality.
        assert_eq!(heights(&p), [2160, 1440, 1080, 720, 480, 360, 240, 144]);
        let labels: Vec<String> = p.qualities.iter().map(|q| quality_label(q.height)).collect();
        assert_eq!(labels, ["4K", "1440p", "1080p", "720p", "480p", "360p", "240p", "144p"]);
        // 251, the last sound-only format, is yt-dlp's best sound.
        const SOUND: u64 = 3_512_345;
        let size = |h: u32| p.qualities.iter().find(|q| q.height == h).and_then(|q| q.approx_bytes);
        assert_eq!(size(1080), Some(25_300_000 + SOUND), "399, the last 1080p, not 137 or the HLS 270");
        assert_eq!(size(2160), Some(166_500_000 + SOUND), "401 has only yt-dlp's estimate");
        assert_eq!(size(360), Some(4_620_000 + SOUND), "396, not 18 with its own sound");
        assert_eq!(size(144), Some(1_290_000 + SOUND));
        // The widest picture that says its width, not yt-dlp's pick (a WebP of no stated size).
        assert_eq!(p.thumbnail.as_deref(), Some("https://i.ytimg.com/vi/aBcDeFgHiJk/maxresdefault.jpg"));
        assert!(p.audio && p.playlist.is_none());
    }

    #[test]
    fn a_soundcloud_track_is_sound_alone() {
        let p = parse(SOUNDCLOUD).unwrap();
        assert_eq!((p.site.as_str(), p.uploader.as_deref()), ("SoundCloud", Some("An Example Artist")));
        assert!(p.qualities.is_empty() && p.audio, "no picture heights, only Audio");
        assert_eq!(p.duration_s.map(duration_label).as_deref(), Some("3:08"));
        // The 500 px artwork; the original (yt-dlp's pick) gives no size.
        assert_eq!(p.thumbnail.as_deref(), Some("https://i1.sndcdn.com/artworks-000000000000-abcdef-t500x500.jpg"));
    }

    #[test]
    fn playlist_items_keep_what_yt_dlp_lists() {
        // A channel's first items: the site counts more than are listed.
        let p = parse(
            r#"{"_type": "playlist", "id": "UC0", "title": "An Example Channel - Videos", "extractor": "youtube:tab",
                "webpage_url_domain": "youtube.com", "playlist_count": 120, "entries": [
                {"_type": "url", "ie_key": "Youtube", "id": "aBcDeFgHiJk", "url": "https://www.youtube.com/watch?v=aBcDeFgHiJk",
                 "title": "An example video", "duration": 213.0,
                 "thumbnails": [{"url": "https://i.ytimg.com/vi/aBcDeFgHiJk/hqdefault.jpg", "height": 188, "width": 336}]},
                {"_type": "url", "id": 42, "url": "https://example.org/v/42"},
                {"_type": "url", "id": "gone", "title": "[Private video]"},
                null]}"#,
        )
        .unwrap();
        assert_eq!(p.site, "YouTube");
        assert_eq!(p.thumbnail.as_deref(), Some("https://i.ytimg.com/vi/aBcDeFgHiJk/hqdefault.jpg"), "no picture of its own: its first item's");
        let list = p.playlist.unwrap();
        assert_eq!(list.count, Some(120));
        assert_eq!(list.entries.len(), 2, "items without an address are left out");
        assert_eq!(list.entries[0].duration_s, Some(213.0));
        assert_eq!((list.entries[1].id.as_str(), list.entries[1].title.as_deref()), ("42", None));
    }

    #[test]
    fn labels_read_the_way_people_say_them() {
        assert_eq!([2160, 4320, 1440, 1080, 300].map(quality_label), ["4K", "8K", "1440p", "1080p", "300p"]);
        assert_eq!([596.46, 70.0, 3723.0, 5.0, -1.0].map(duration_label), ["9:56", "1:10", "1:02:03", "0:05", "0:00"]);
        assert_eq!(site_name("generic", "www.example.org"), "example.org");
        assert_eq!(site_name("vimeo", "vimeo.com"), "vimeo.com", "the rest by their address");
        assert_eq!(site_name("SomeSite", ""), "SomeSite");
    }

    #[test]
    fn a_reply_that_names_nothing_is_an_error() {
        for bad in ["", "null", "[]", "{}", r#"{"id": " ", "title": ""}"#] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        // Only a storyboard and a DRM-locked format: nothing to download.
        let p = parse(
            r#"{"id": "x", "formats": [{"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none", "height": 180},
                {"format_id": "drm", "vcodec": "avc1", "acodec": "mp4a", "height": 1080, "has_drm": true}]}"#,
        )
        .unwrap();
        assert_eq!((p.title.as_str(), p.qualities.len(), p.audio), ("x", 0, false), "the id stands in for the title");
    }
}
