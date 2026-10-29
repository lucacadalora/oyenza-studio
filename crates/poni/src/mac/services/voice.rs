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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Default)]
pub struct VoicePlayer {
    /// The line and its delegate (NSSound holds its delegate weakly).
    playing: Option<(Retained<NSSound>, Retained<VoiceDelegate>)>,
    /// The current line couldn't play to its end (set by its delegate only).
    failed: Arc<AtomicBool>,
}

impl VoicePlayer {
    /// Plays an mp3/wav file; sends AppEvent::VoiceEnded when it finishes or fails.
    pub fn play(&mut self, path: &Path, sink: &EventSink) -> Result<(), String> {
        self.stop();
        let mtm = MainThreadMarker::new().ok_or("the voice plays from the main thread")?;
        let sound = NSSound::initWithContentsOfFile_byReference(<NSSound as AnyThread>::alloc(), &NSString::from_str(&path.to_string_lossy()), true)
            .ok_or_else(|| format!("can't open {}", path.display()))?;
        // Each line has its own flag: a stopped line's late report can't mark the next one.
        let failed = Arc::new(AtomicBool::new(false));
        let delegate = VoiceDelegate::new(mtm, Ivars { sink: sink.clone(), failed: failed.clone() });
        sound.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        if !sound.play() {
            sound.setDelegate(None);
            return Err("can't play".into());
        }
        self.playing = Some((sound, delegate));
        self.failed = failed;
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

    /// The line that just ended didn't play to its end (no sound device, a
    /// broken file).
    pub fn failed(&self) -> bool {
        self.playing.is_some() && self.failed.load(Ordering::SeqCst)
    }
}

/// What a line's delegate holds: where to say it ended, and its failed flag.
struct Ivars {
    sink: EventSink,
    failed: Arc<AtomicBool>,
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
    #[ivars = Ivars]
    struct VoiceDelegate;

    unsafe impl NSObjectProtocol for VoiceDelegate {}

    unsafe impl NSSoundDelegate for VoiceDelegate {
        /// `finished` is false when the line stopped before its end without
        /// a stop of ours (the delegate is gone by then): it couldn't play on.
        #[unsafe(method(sound:didFinishPlaying:))]
        fn did_finish(&self, _sound: &NSSound, finished: bool) {
            if !finished {
                self.ivars().failed.store(true, Ordering::SeqCst);
            }
            self.ivars().sink.send(AppEvent::VoiceEnded);
        }
    }
);

impl VoiceDelegate {
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}
