//! Plays the Prayer times add-on's adzan (`adzan.m4a`) with WinRT's
//! `MediaPlayer`. Its automatic media controls are switched off, so the
//! adzan never shows up as a playing song: not in Windows' volume flyout,
//! and not on Oyenza Studio's own Music page.
//!
//! One adzan at a time; a new one replaces it. `AppEvent::AdzanEnded` comes
//! when it finishes or fails, from a thread-pool thread.

use crate::events::{AppEvent, EventSink};
use std::path::Path;
use windows::core::HSTRING;
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Media::Core::MediaSource;
use windows::Media::Playback::MediaPlayer;

#[derive(Default)]
pub struct AdzanPlayer {
    player: Option<MediaPlayer>,
}

impl AdzanPlayer {
    /// Start the recording at `path`. An error means nothing plays (the
    /// caller plays the tone instead).
    pub fn play(&mut self, path: &Path, sink: &EventSink) -> Result<(), String> {
        self.stop();
        let uri = Uri::CreateUri(&HSTRING::from(file_uri(path))).map_err(|e| format!("bad path ({e})"))?;
        let player = MediaPlayer::new().map_err(|e| format!("no media player ({e})"))?;
        // No media session: the adzan isn't a song.
        if let Ok(cm) = player.CommandManager() {
            let _ = cm.SetIsEnabled(false);
        }
        let source = MediaSource::CreateFromUri(&uri).map_err(|e| format!("can't open the recording ({e})"))?;
        player.SetSource(&source).map_err(|e| format!("can't open the recording ({e})"))?;
        let ended = sink.clone();
        let _ = player.MediaEnded(&TypedEventHandler::new(move |_, _| {
            ended.send(AppEvent::AdzanEnded);
            Ok(())
        }));
        let failed = sink.clone();
        let _ = player.MediaFailed(&TypedEventHandler::new(move |_, _| {
            crate::pwarn!("adzan: playback failed");
            failed.send(AppEvent::AdzanEnded);
            Ok(())
        }));
        player.Play().map_err(|e| format!("can't play ({e})"))?;
        self.player = Some(player);
        Ok(())
    }

    /// Stop it now (Stop in the notice, the add-on switched off), or forget a
    /// player that finished.
    pub fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            let _ = p.Pause();
            let _ = p.Close();
        }
    }
}

/// `C:\Users\A B\x.m4a` -> `file:///C:/Users/A%20B/x.m4a` (Oyen's voice too).
pub(crate) fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let parts: Vec<String> = text
        .split(['\\', '/'])
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, s)| if i == 0 && s.ends_with(':') { s.to_string() } else { poni_core::urlenc::encode(s) })
        .collect();
    format!("file:///{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_keep_the_drive_and_encode_the_rest() {
        assert_eq!(
            file_uri(Path::new(r"C:\Users\Luca Cada\AppData\Local\Poni\addons\sholat\addon-sholat-1\adzan.m4a")),
            "file:///C:/Users/Luca%20Cada/AppData/Local/Poni/addons/sholat/addon-sholat-1/adzan.m4a"
        );
        assert_eq!(file_uri(Path::new(r"D:\Müzik\a.m4a")), "file:///D:/M%C3%BCzik/a.m4a");
    }
}
