//! Fix-ups for track metadata as players report it to Windows' media
//! controls. The media service applies them before anything else (the
//! artist line, the track key, the lyrics lookup) sees the track.

/// Apple Music for Windows reports "Artist — Album" as the artist (an em dash
/// between spaces) and leaves the album empty. Radio adds a third part
/// ("Artist — Album — Station").
const APPLE_MUSIC_SEPARATOR: &str = " \u{2014} ";

/// Apple Music for Windows reports its position about half a second behind
/// what you hear, and unevenly (Lyricify's own docs give -400 to -500 ms for
/// it). Synced lyrics run this much ahead of its reports.
pub const APPLE_MUSIC_LYRICS_LEAD_S: f64 = 0.45;

/// How far ahead of the player's reported position synced lyrics should run.
pub fn lyrics_lead_s(app_id: &str) -> f64 {
    if is_apple_music(app_id) {
        APPLE_MUSIC_LYRICS_LEAD_S
    } else {
        0.0
    }
}

/// Whether `app_id` (the session's AUMID) is Apple Music for Windows: the
/// Store app (`AppleInc.AppleMusicWin_…!App`) or `AppleMusic.exe`.
pub fn is_apple_music(app_id: &str) -> bool {
    app_id.to_ascii_lowercase().contains("applemusic")
}

/// Transport controls: what a player reports to Windows' media controls, and
/// what the island offers for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub prev: bool,
    pub next: bool,
    pub play_pause: bool,
    pub seek: bool,
}

/// What the island offers for a player that reports `reported`. Seeking needs
/// a song with a known length. Apple Music for Windows reports Previous and
/// seeking as off and declines both from outside, but its own window does
/// them: the Windows app presses its back button and moves its progress
/// slider instead, so both stay on for it.
pub fn island_controls(app_id: &str, reported: Controls, has_duration: bool) -> Controls {
    let apple = is_apple_music(app_id);
    Controls { prev: reported.prev || apple, seek: (reported.seek || apple) && has_duration, ..reported }
}

/// The value that puts a slider whose range is `min..=max` at `fraction`
/// (0..=1) of the way along; `None` when the range is unusable.
pub fn slider_value(fraction: f64, min: f64, max: f64) -> Option<f64> {
    if !(fraction.is_finite() && min.is_finite() && max.is_finite()) || max <= min {
        return None;
    }
    Some(min + fraction.clamp(0.0, 1.0) * (max - min))
}

/// Whether `app_id` is a web browser (its tabs report video titles with the
/// channel as artist). Web apps installed from a browser (`…_crx_…`, e.g.
/// YouTube Music) report proper metadata and don't count. On a Mac `app_id`
/// is the browser's bundle id (`com.apple.Safari`, `company.thebrowser.dia`).
pub fn is_browser(app_id: &str) -> bool {
    let id = app_id.trim().to_ascii_lowercase();
    if id.contains("_crx_") || id.contains(".app.") {
        return false;
    }
    // Firefox registers a 16-hex-digit hash of its install path.
    let firefox_hash = id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit());
    firefox_hash
        || ["chrome", "msedge", "microsoftedge", "brave", "opera", "firefox", "vivaldi"].iter().any(|b| id.contains(b))
        // Safari, Edge, Arc and Dia, Orion, Zen on a Mac.
        || ["com.apple.safari", "com.microsoft.edgemac", "company.thebrowser.", "com.kagi.kagimacos", "app.zen-browser."].iter().any(|b| id.starts_with(b))
}

/// The site a browser tab is playing, judged from the browser window's title
/// ("Paper Boats - YouTube - Google Chrome"); Windows' media controls only
/// name the browser. Only a window that also shows `media_title` counts, so
/// another window with YouTube open doesn't label unrelated audio.
pub fn site_from_window_title(window_title: &str, media_title: &str) -> Option<&'static str> {
    let media_title = media_title.trim();
    if media_title.is_empty() || !window_title.contains(media_title) {
        return None;
    }
    if window_title.contains(" - YouTube Music") {
        Some("YouTube Music")
    } else if window_title.contains(" - YouTube") {
        Some("YouTube")
    } else {
        None
    }
}

/// The artist and album to use for a track reported by `app_id`.
pub fn artist_album(app_id: &str, artist: String, album: String) -> (String, String) {
    if !is_apple_music(app_id) {
        return (artist, album);
    }
    let mut parts = artist.split(APPLE_MUSIC_SEPARATOR).map(str::trim);
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) if !a.is_empty() => {
            // An album the player reported itself wins over the guessed one.
            let album = if album.trim().is_empty() { b.to_string() } else { album };
            (a.to_string(), album)
        }
        _ => (artist, album),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPLE: &str = "AppleInc.AppleMusicWin_nzyj5cx40ttqa!App";

    fn fix(app_id: &str, artist: &str, album: &str) -> (String, String) {
        artist_album(app_id, artist.into(), album.into())
    }

    fn pair(artist: &str, album: &str) -> (String, String) {
        (artist.into(), album.into())
    }

    #[test]
    fn recognises_apple_music() {
        assert!(is_apple_music(APPLE));
        assert!(is_apple_music("AppleMusic.exe"));
        assert!(!is_apple_music("Spotify.exe"));
        assert!(!is_apple_music("MSEdge"));
    }

    #[test]
    fn apple_music_keeps_previous_and_seeking() {
        // What Apple Music for Windows reports: Next and play/pause only.
        let reported = Controls { prev: false, next: true, play_pause: true, seek: false };
        let all = Controls { prev: true, next: true, play_pause: true, seek: true };
        assert_eq!(island_controls(APPLE, reported, true), all);
        // A song with no known length (radio) can't be seeked.
        assert_eq!(island_controls(APPLE, reported, false), Controls { seek: false, ..all });
        // Other players get what they report.
        assert_eq!(island_controls("Spotify.exe", reported, true), reported);
        assert_eq!(island_controls("Spotify.exe", all, true), all);
        assert_eq!(island_controls("Spotify.exe", all, false), Controls { seek: false, ..all });
    }

    #[test]
    fn slider_values_follow_the_slider_range() {
        // Apple Music's slider counts seconds.
        assert_eq!(slider_value(0.5, 0.0, 240.0), Some(120.0));
        assert_eq!(slider_value(0.25, 10.0, 50.0), Some(20.0));
        assert_eq!(slider_value(1.5, 0.0, 100.0), Some(100.0));
        assert_eq!(slider_value(-1.0, 0.0, 100.0), Some(0.0));
        assert_eq!(slider_value(0.5, 0.0, 0.0), None);
        assert_eq!(slider_value(0.5, 5.0, 1.0), None);
        assert_eq!(slider_value(f64::NAN, 0.0, 100.0), None);
        assert_eq!(slider_value(0.5, 0.0, f64::INFINITY), None);
    }

    #[test]
    fn lyrics_run_ahead_of_apple_music_only() {
        assert_eq!(lyrics_lead_s(APPLE), APPLE_MUSIC_LYRICS_LEAD_S);
        assert_eq!(lyrics_lead_s("Spotify.exe"), 0.0);
    }

    #[test]
    fn recognises_browsers() {
        for id in ["Chrome", "MSEdge", "308046B0AF4A39CB", "Brave", "Opera", "C:\\Apps\\Vivaldi\\vivaldi.exe"] {
            assert!(is_browser(id), "{id}");
        }
        // The Mac's bundle ids.
        for id in ["com.google.Chrome", "com.apple.Safari", "com.microsoft.edgemac", "company.thebrowser.Browser", "company.thebrowser.dia", "org.mozilla.firefox", "com.brave.Browser"] {
            assert!(is_browser(id), "{id}");
        }
        for id in [APPLE, "Spotify.exe", "Chrome._crx_cinhimbnkkaeohfgghhklpknlkffjgod", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"] {
            assert!(!is_browser(id), "{id}");
        }
        // A web app Chrome installed on a Mac (YouTube Music) is no browser tab.
        for id in ["com.spotify.client", "com.apple.Music", "com.google.Chrome.app.cinhimbnkkaeohfgghhklpknlkffjgod"] {
            assert!(!is_browser(id), "{id}");
        }
    }

    #[test]
    fn names_the_site_from_the_window_title() {
        let t = "The Lanterns - Paper Boats (Official Video)";
        assert_eq!(site_from_window_title(&format!("(2) {t} - YouTube - Google Chrome"), t), Some("YouTube"));
        assert_eq!(
            site_from_window_title(&format!("{t} - YouTube and 3 more pages - Personal - Microsoft\u{200b} Edge"), t),
            Some("YouTube")
        );
        assert_eq!(site_from_window_title("Paper Boats - YouTube Music - Google Chrome", "Paper Boats"), Some("YouTube Music"));
        // Another tab or site in front, or no title to compare.
        assert_eq!(site_from_window_title("Other Video - YouTube - Google Chrome", t), None);
        assert_eq!(site_from_window_title(&format!("{t} - Vimeo - Google Chrome"), t), None);
        assert_eq!(site_from_window_title("Something - YouTube - Google Chrome", " "), None);
    }

    #[test]
    fn splits_apple_music_artist() {
        assert_eq!(fix(APPLE, "The Lanterns \u{2014} Harbor", ""), pair("The Lanterns", "Harbor"));
        assert_eq!(fix("AppleMusic.exe", "The Lanterns \u{2014} Harbor", ""), pair("The Lanterns", "Harbor"));
        // Radio: "Artist — Album — Station".
        assert_eq!(fix(APPLE, "The Lanterns \u{2014} Harbor - EP \u{2014} Night Radio", ""), pair("The Lanterns", "Harbor - EP"));
        // An album the player reported itself is kept.
        assert_eq!(fix(APPLE, "The Lanterns \u{2014} Harbor", "Harbor (Deluxe)"), pair("The Lanterns", "Harbor (Deluxe)"));
    }

    #[test]
    fn leaves_other_metadata_alone() {
        // No separator (local files, some radio).
        assert_eq!(fix(APPLE, "The Lanterns", ""), pair("The Lanterns", ""));
        // A hyphen or an em dash without spaces is part of the name.
        assert_eq!(fix(APPLE, "Up - Down", ""), pair("Up - Down", ""));
        assert_eq!(fix(APPLE, "Up\u{2014}Down", ""), pair("Up\u{2014}Down", ""));
        // Nothing before the dash: keep the raw value rather than an empty artist.
        assert_eq!(fix(APPLE, " \u{2014} Harbor", ""), pair(" \u{2014} Harbor", ""));
        // Other players can have an em dash in a real artist name.
        assert_eq!(fix("Spotify.exe", "The Lanterns \u{2014} Harbor", ""), pair("The Lanterns \u{2014} Harbor", ""));
    }
}
