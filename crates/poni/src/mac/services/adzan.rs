//! Plays the Prayer times add-on's adzan (`adzan.m4a`) with AppKit's
//! `NSSound`. The adzan never shows up as a playing song: NSSound doesn't
//! tell macOS's Now Playing about itself, and the Music page follows only
//! Music and Spotify.
//!
//! One adzan at a time; a new one replaces it. `AppEvent::AdzanEnded` comes
//! when it plays to its end (on the main thread), never for a stop the app
//! asked for.

use crate::events::{AppEvent, EventSink};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSSound, NSSoundDelegate};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use std::path::Path;
use std::sync::Mutex;

/// Where the delegate sends the end (AppKit calls it on the main thread).
static SINK: Mutex<Option<EventSink>> = Mutex::new(None);

#[derive(Default)]
pub struct AdzanPlayer {
    /// The sound and its delegate (NSSound holds its delegate weakly).
    playing: Option<(Retained<NSSound>, Retained<AdzanDelegate>)>,
}

impl AdzanPlayer {
    /// Start the recording at `path`. An error means nothing plays (the
    /// caller plays the tone instead).
    pub fn play(&mut self, mtm: MainThreadMarker, path: &Path, sink: &EventSink) -> Result<(), String> {
        self.stop();
        let sound = NSSound::initWithContentsOfFile_byReference(<NSSound as AnyThread>::alloc(), &NSString::from_str(&path.to_string_lossy()), true)
            .ok_or_else(|| "can't open the recording".to_string())?;
        let delegate: Retained<AdzanDelegate> = unsafe { msg_send![AdzanDelegate::alloc(mtm), init] };
        sound.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        if let Ok(mut s) = SINK.lock() {
            *s = Some(sink.clone());
        }
        if !sound.play() {
            sound.setDelegate(None);
            return Err("can't play".into());
        }
        self.playing = Some((sound, delegate));
        Ok(())
    }

    /// Stop it now (Stop in the notice, the add-on switched off), or forget a
    /// sound that finished.
    pub fn stop(&mut self) {
        if let Some((sound, _delegate)) = self.playing.take() {
            sound.setDelegate(None);
            sound.stop();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing.as_ref().is_some_and(|(s, _)| s.isPlaying())
    }
}

define_class!(
    /// Hears the adzan's end (main thread).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PoniAdzanDelegate"]
    struct AdzanDelegate;

    unsafe impl NSObjectProtocol for AdzanDelegate {}

    unsafe impl NSSoundDelegate for AdzanDelegate {
        #[unsafe(method(sound:didFinishPlaying:))]
        fn did_finish(&self, _sound: &NSSound, _finished: bool) {
            if let Some(sink) = SINK.lock().ok().and_then(|s| s.clone()) {
                sink.send(AppEvent::AdzanEnded);
            }
        }
    }
);
