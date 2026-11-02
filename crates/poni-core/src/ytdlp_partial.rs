//! What yt-dlp leaves of a download it didn't finish, so the runner can
//! delete it once the person stops one (`services::downloader`): the files it
//! writes into while it downloads. They sit in the download folder, named
//! with the site's id for the video in brackets ("Big Buck Bunny
//! [BigBuckBunny_124].ogv.part", `ytdlp::args`' file names), and yt-dlp's
//! info line gives that id. Pure, so it is tested here against yt-dlp's real
//! lines (`tests/fixtures/ytdlp/`).

use serde_json::Value;

/// The site's id for the video an info line is about,
/// `{"t":"info","id":"BigBuckBunny_124",…}`, which `ytdlp::args` has yt-dlp
/// print before each download (`ytdlp::Line::Info` leaves the id out). None
/// for any other line.
pub fn video_id(line: &str) -> Option<String> {
    let line = line.trim();
    // A field without `|null` prints NA, which isn't JSON (as `ytdlp::parse` reads it).
    let v: Value = serde_json::from_str(line).or_else(|_| serde_json::from_str(&line.replace(":NA,", ":null,").replace(":NA}", ":null}"))).ok()?;
    if v["t"] != "info" {
        return None;
    }
    match &v["id"] {
        Value::String(s) => Some(s.trim()).filter(|s| !s.is_empty()).map(str::to_string),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Whether `name`, a file in the download folder, is one yt-dlp left of
/// video `id`: the file it writes into (`….mp4.part`), and for a download in
/// fragments (HLS, DASH) the fragments (`….mp4.part-Frag3`,
/// `….mp4.part-Frag3.part`) and its note of them (`….mp4.ytdl`). Never a
/// finished file, or another video's. An id that yt-dlp had to shorten or
/// change in the name matches nothing, so those files stay.
pub fn is_partial(name: &str, id: &str) -> bool {
    !id.is_empty() && name.contains(&format!("[{id}]")) && (name.ends_with(".part") || name.ends_with(".ytdl") || name.contains(".part-Frag"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded with yt-dlp 2026.10.26: Stop 2 s into Blender's HLS film, and
    // the first draft's template, which printed NA.
    const HLS_STOPPED: &str = include_str!("../tests/fixtures/ytdlp/peertube_hls_interrupted.stdout");
    const NA: &str = include_str!("../tests/fixtures/ytdlp/original_template_na.stdout");
    const FILM: &str = "b29290cc-dc51-4a12-bcb2-2aa5fece7605";

    #[test]
    fn the_info_line_names_the_video() {
        let ids: Vec<String> = HLS_STOPPED.lines().filter_map(video_id).collect();
        assert_eq!(ids, [FILM], "from the info line alone, not the progress lines");
        assert_eq!(NA.lines().find_map(video_id).as_deref(), Some("bbb_30fps"), "NA beside it");
        assert_eq!(video_id(r#"  {"t":"info","id":12345,"title":"x"}  "#).as_deref(), Some("12345"));
        for other in [
            "",
            "[download] Destination: <dir>/Big Buck Bunny [BigBuckBunny_124].ogv",
            r#"{"t":"dl","id":"BigBuckBunny_124","st":"downloading","done":1024}"#,
            r#"{"t":"info","id":null,"title":"x"}"#,
            r#"{"t":"info","id":" ","title":"x"}"#,
            r#"{"t":"info","title":"x"}"#,
            "{not json",
        ] {
            assert_eq!(video_id(other), None, "{other:?}");
        }
    }

    #[test]
    fn only_what_yt_dlp_left_of_that_video_goes() {
        let left = [
            // One file, and a download in fragments (HLS).
            ("Big Buck Bunny [BigBuckBunny_124].ogv.part", "BigBuckBunny_124"),
            ("OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4.part", FILM),
            ("OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4.part-Frag3", FILM),
            ("OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4.part-Frag3.part", FILM),
            ("OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4.ytdl", FILM),
            // The picture of a video whose sound comes after, and the sound alone.
            ("bbb_30fps [bbb_30fps].fbbb_30fps_320x180_200k.mp4.part", "bbb_30fps"),
            ("bbb_30fps [bbb_30fps] (audio).m4a.part", "bbb_30fps"),
        ];
        for (name, id) in left {
            assert!(is_partial(name, id), "{name}");
        }
        let kept = [
            "OVERGROWN： a feature film project by Blender Studio [b29290cc-dc51-4a12-bcb2-2aa5fece7605].mp4",
            "Sprite Fright [5e1a4d5e-0000-4000-8000-000000000000].mp4.part",
            "OVERGROWN.mp4.part",
            "b29290cc-dc51-4a12-bcb2-2aa5fece7605.part",
            "notes [b29290cc-dc51-4a12-bcb2-2aa5fece7605].txt",
        ];
        for name in kept {
            assert!(!is_partial(name, FILM), "{name}");
        }
        assert!(!is_partial("Untitled [].mp4.part", ""), "no id, nothing goes");
    }
}
