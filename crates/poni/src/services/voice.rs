//! Oyen's voice on the IELTS page: plays the server's clips (mp3, or WAV)
//! one at a time with WinRT's `MediaPlayer`, built like the adzan's
//! (`services/adzan.rs`): its automatic media controls are off, so Oyen never
//! shows up as a playing song (in Windows' volume flyout or on the Music page).
//!
//! `AppEvent::VoiceEnded` comes when a clip finishes or fails, from a
//! thread-pool thread. A clip stopped on purpose may still report; the app
//! asks [`VoicePlayer::playing`] to tell the current clip's end from a
//! stale one, and [`VoicePlayer::failed`] whether it played at all (with no
//! sound device, Oyen's words then show for their reading time). The player
//! exists only while a clip plays.

use crate::events::{AppEvent, EventSink};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::HSTRING;
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Media::Core::MediaSource;
use windows::Media::Playback::MediaPlayer;

#[derive(Default)]
pub struct VoicePlayer {
    player: Option<MediaPlayer>,
    /// The current clip ended or failed (set by its own handlers only).
    ended: Arc<AtomicBool>,
    /// The current clip couldn't play.
    failed: Arc<AtomicBool>,
}

impl VoicePlayer {
    /// Plays an mp3/wav file; sends AppEvent::VoiceEnded when it finishes or fails.
    pub fn play(&mut self, path: &Path, sink: &EventSink) -> Result<(), String> {
        self.stop();
        // A file URI needs the whole path (the self-test's home is relative).
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let uri = Uri::CreateUri(&HSTRING::from(super::adzan::file_uri(&path))).map_err(|e| format!("bad path ({e})"))?;
        let player = MediaPlayer::new().map_err(|e| format!("no media player ({e})"))?;
        // No media session: Oyen isn't a song.
        if let Ok(cm) = player.CommandManager() {
            let _ = cm.SetIsEnabled(false);
        }
        let source = MediaSource::CreateFromUri(&uri).map_err(|e| format!("can't open the clip ({e})"))?;
        player.SetSource(&source).map_err(|e| format!("can't open the clip ({e})"))?;
        // Each clip has its own flags: a stopped clip's late report can't end the next one.
        let (ended, failed) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (done, sent) = (ended.clone(), sink.clone());
        let _ = player.MediaEnded(&TypedEventHandler::new(move |_, _| {
            done.store(true, Ordering::SeqCst);
            sent.send(AppEvent::VoiceEnded);
            Ok(())
        }));
        let (done, broke, sent) = (ended.clone(), failed.clone(), sink.clone());
        let _ = player.MediaFailed(&TypedEventHandler::new(move |_, args: &Option<windows::Media::Playback::MediaPlayerFailedEventArgs>| {
            let code = args.as_ref().and_then(|a| a.ExtendedErrorCode().ok()).map_or(0, |h| h.0 as u32);
            crate::pwarn!("ielts: a clip didn't play ({code:#010x})");
            broke.store(true, Ordering::SeqCst);
            done.store(true, Ordering::SeqCst);
            sent.send(AppEvent::VoiceEnded);
            Ok(())
        }));
        player.Play().map_err(|e| format!("can't play ({e})"))?;
        self.player = Some(player);
        self.ended = ended;
        self.failed = failed;
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            let _ = p.Pause();
            let _ = p.Close();
        }
    }

    pub fn playing(&self) -> bool {
        self.player.is_some() && !self.ended.load(Ordering::SeqCst)
    }

    /// The clip that just ended didn't play (no sound device, a broken file).
    pub fn failed(&self) -> bool {
        self.player.is_some() && self.failed.load(Ordering::SeqCst)
    }
}
