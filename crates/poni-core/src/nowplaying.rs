//! Fix-ups for track metadata as players report it to Windows' media
//! controls. The media service applies them before anything else (the
//! artist line, the track key, the lyrics lookup) sees the track.

/// Apple Music for Windows reports "Artist — Album" as the artist (an em dash
/// between spaces) and leaves the album empty. Radio adds a third part
/// ("Artist — Album — Station").
const APPLE_MUSIC_SEPARATOR: &str = " \u{2014} ";

/// Whether `app_id` (the session's AUMID) is Apple Music for Windows: the
/// Store app (`AppleInc.AppleMusicWin_…!App`) or `AppleMusic.exe`.
pub fn is_apple_music(app_id: &str) -> bool {
    app_id.to_ascii_lowercase().contains("applemusic")
}

/// Whether `app_id` is a web browser (its tabs report video titles with the
/// channel as artist). Web apps installed from a browser (`…_crx_…`, e.g.
/// YouTube Music) report proper metadata and don't count.
pub fn is_browser(app_id: &str) -> bool {
    let id = app_id.trim().to_ascii_lowercase();
    if id.contains("_crx_") {
        return false;
    }
    // Firefox registers a 16-hex-digit hash of its install path.
    let firefox_hash = id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit());
    firefox_hash || ["chrome", "msedge", "microsoftedge", "brave", "opera", "firefox", "vivaldi"].iter().any(|b| id.contains(b))
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
    fn recognises_browsers() {
        for id in ["Chrome", "MSEdge", "308046B0AF4A39CB", "Brave", "Opera", "C:\\Apps\\Vivaldi\\vivaldi.exe"] {
            assert!(is_browser(id), "{id}");
        }
        for id in [APPLE, "Spotify.exe", "Chrome._crx_cinhimbnkkaeohfgghhklpknlkffjgod", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"] {
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
