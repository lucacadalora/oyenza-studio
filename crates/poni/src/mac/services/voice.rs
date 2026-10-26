//! The IELTS coach's voice on the Mac: plays Oyen's lines (the server's mp3
//! or wav files) with AppKit's `NSSound`, as the adzan does. NSSound reads
//! what Core Audio reads, mp3 included, and never tells macOS's Now Playing
//! about itself, so the island's music card, the media keys and the music
//! the page pauses while Oyen talks all stay with the person's own player.
//!
//! One line at a time; a new one replaces it. `AppEvent::VoiceEnded` comes
//! when a line plays to its end or can't play on (on the main thread), never
//! for a stop the page asked for, nor when `play` itself returns an error.

use crate::events::{AppEvent, EventSink};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSSound, NSSoundDelegate};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use std::path::Path;

#[derive(Default)]
pub struct VoicePlayer {
    /// The line and its delegate (NSSound holds its delegate weakly).
    playing: Option<(Retained<NSSound>, Retained<VoiceDelegate>)>,
}

impl VoicePlayer {
    /// Plays an mp3/wav file; sends AppEvent::VoiceEnded when it finishes or fails.
    pub fn play(&mut self, path: &Path, sink: &EventSink) -> Result<(), String> {
        self.stop();
        let mtm = MainThreadMarker::new().ok_or("the voice plays from the main thread")?;
        let sound = NSSound::initWithContentsOfFile_byReference(<NSSound as AnyThread>::alloc(), &NSString::from_str(&path.to_string_lossy()), true)
            .ok_or_else(|| format!("can't open {}", path.display()))?;
        let delegate = VoiceDelegate::new(mtm, sink.clone());
        sound.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        if !sound.play() {
            sound.setDelegate(None);
            return Err("can't play".into());
        }
        self.playing = Some((sound, delegate));
        Ok(())
    }

    /// Stop now (skip, the page closing), or forget a line that finished.
    pub fn stop(&mut self) {
        if let Some((sound, _delegate)) = self.playing.take() {
            sound.setDelegate(None);
            sound.stop();
        }
    }

    pub fn playing(&self) -> bool {
        self.playing.as_ref().is_some_and(|(s, _)| s.isPlaying())
    }
}

impl Drop for VoicePlayer {
    /// The page closed: Oyen stops mid-line.
    fn drop(&mut self) {
        self.stop();
    }
}

define_class!(
    /// Hears a line's end (main thread).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PoniVoiceDelegate"]
    #[ivars = EventSink]
    struct VoiceDelegate;

    unsafe impl NSObjectProtocol for VoiceDelegate {}

    unsafe impl NSSoundDelegate for VoiceDelegate {
        #[unsafe(method(sound:didFinishPlaying:))]
        fn did_finish(&self, _sound: &NSSound, _finished: bool) {
            self.ivars().send(AppEvent::VoiceEnded);
        }
    }
);

impl VoiceDelegate {
    fn new(mtm: MainThreadMarker, sink: EventSink) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(sink);
        unsafe { msg_send![super(this), init] }
    }
}
