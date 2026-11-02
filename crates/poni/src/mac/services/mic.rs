//! The IELTS coach's microphone on the Mac: records a spoken answer as a
//! 16 kHz mono 16-bit WAV with AVFAudio's `AVAudioRecorder`, which converts
//! from the microphone's own rate, writes the file itself and keeps the
//! level meter the page shows.
//!
//! macOS asks the person once (Info.plist's NSMicrophoneUsageDescription).
//! The first `start` puts macOS's question up and returns at once; recording
//! begins when the person allows it. "Don't Allow" sends
//! `AppEvent::MicFailed`, and `start` says `Denied` from then on, until System
//! Settings › Privacy & Security › Microphone changes it. A Mac with no
//! microphone at all (a Mac mini with nothing plugged in) gets `NoDevice`
//! without being asked.
//!
//! Nothing runs unless recording: the recorder lives from `start` to `stop`.

use crate::events::{AppEvent, EventSink};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_avf_audio::{
    AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioFileTypeKey, AVAudioRecorder, AVAudioRecorderDelegate, AVFormatIDKey,
    AVLinearPCMBitDepthKey, AVLinearPCMIsBigEndianKey, AVLinearPCMIsFloatKey, AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSURL};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What the server takes: 16 kHz mono 16-bit PCM (32 KB a second).
const RATE: f64 = 16_000.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicError {
    /// macOS says no: the person turned it down (now or in System Settings),
    /// or a profile on this Mac doesn't allow it.
    Denied,
    /// No microphone: none built in, none plugged in.
    NoDevice,
    Failed(String),
}

impl std::fmt::Display for MicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MicError::Denied => f.write_str("macOS doesn't allow the microphone"),
            MicError::NoDevice => f.write_str("no microphone"),
            MicError::Failed(s) => f.write_str(s),
        }
    }
}

pub struct Mic {
    /// Shared with macOS's answer to its question, which comes later.
    state: Arc<Mutex<State>>,
}

enum State {
    /// macOS's question is up; recording starts once the person allows it.
    Asking,
    Recording(Recorder),
    /// The person said no to macOS's question.
    TurnedDown,
    Failed(String),
    /// Stopped, or the page let go of it: a late answer does nothing.
    Done,
}

struct Recorder {
    rec: Retained<AVAudioRecorder>,
    /// The recorder holds its delegate weakly.
    _delegate: Retained<MicDelegate>,
    path: PathBuf,
}

impl Mic {
    /// Records 16 kHz mono 16-bit WAV to `out`.
    pub fn start(out: &Path, sink: &EventSink) -> Result<Mic, MicError> {
        if !has_input() {
            return Err(MicError::NoDevice);
        }
        let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
        if permission == AVAudioApplicationRecordPermission::Granted {
            let rec = record(out, sink)?;
            return Ok(Mic { state: Arc::new(Mutex::new(State::Recording(rec))) });
        }
        if permission == AVAudioApplicationRecordPermission::Denied {
            return Err(MicError::Denied);
        }
        // Not asked yet: macOS asks the person, and answers on a thread of
        // its own. The recorder starts on the main thread (its callbacks need
        // a run loop), unless the page stopped it meanwhile.
        crate::pinfo!("mic: macOS asks the person for the microphone");
        let state = Arc::new(Mutex::new(State::Asking));
        let (st, out, sink) = (state.clone(), out.to_path_buf(), sink.clone());
        let answered = RcBlock::new(move |granted: Bool| {
            let (st, out, sink, granted) = (st.clone(), out.clone(), sink.clone(), granted.as_bool());
            dispatch2::DispatchQueue::main().exec_async(move || on_answer(&st, &out, &sink, granted));
        });
        unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&answered) };
        Ok(Mic { state })
    }

    /// 0..1 peak of the last ~50 ms, for the level meter (read only while recording).
    pub fn level(&self) -> f32 {
        let Ok(st) = self.state.lock() else { return 0.0 };
        let State::Recording(r) = &*st else { return 0.0 };
        // dBFS, -160 for silence.
        let db = unsafe {
            r.rec.updateMeters();
            r.rec.peakPowerForChannel(0)
        };
        if db.is_finite() {
            10f32.powf(db / 20.0).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The file and its length in seconds.
    pub fn stop(self) -> Result<(PathBuf, f32), MicError> {
        let was = self.state.lock().map(|mut s| std::mem::replace(&mut *s, State::Done)).unwrap_or(State::Done);
        match was {
            State::Recording(r) => {
                let secs = unsafe {
                    // Its own stop isn't news (the delegate would call it a failure).
                    r.rec.setDelegate(None);
                    let secs = r.rec.currentTime();
                    r.rec.stop();
                    secs
                };
                Ok((r.path, secs as f32))
            }
            State::Asking => Err(MicError::Failed("macOS is still asking for the microphone".into())),
            State::TurnedDown => Err(MicError::Denied),
            State::Failed(e) => Err(MicError::Failed(e)),
            State::Done => Err(MicError::Failed("already stopped".into())),
        }
    }
}

impl Drop for Mic {
    /// The page closed mid-answer: the recording stops, and a late "Allow"
    /// starts nothing.
    fn drop(&mut self) {
        let Ok(mut st) = self.state.lock() else { return };
        if let State::Recording(r) = std::mem::replace(&mut *st, State::Done) {
            unsafe {
                r.rec.setDelegate(None);
                r.rec.stop();
            }
        }
    }
}

impl std::fmt::Debug for Mic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Ok(st) = self.state.lock() else { return f.write_str("poisoned") };
        match &*st {
            State::Asking => f.write_str("asking macOS"),
            State::Recording(_) => f.write_str("recording"),
            State::TurnedDown => f.write_str("turned down"),
            State::Failed(e) => write!(f, "failed: {e}"),
            State::Done => f.write_str("stopped"),
        }
    }
}

/// macOS's question answered (main thread).
fn on_answer(state: &Mutex<State>, out: &Path, sink: &EventSink, granted: bool) {
    let failed = {
        let Ok(mut st) = state.lock() else { return };
        if !matches!(*st, State::Asking) {
            return;
        }
        crate::pinfo!("mic: the person {} the microphone", if granted { "allowed" } else { "turned down" });
        if !granted {
            *st = State::TurnedDown;
            "microphone access denied".to_string()
        } else {
            match record(out, sink) {
                Ok(r) => {
                    *st = State::Recording(r);
                    return;
                }
                Err(e) => {
                    let why = match e {
                        MicError::Failed(why) => why,
                        MicError::NoDevice => "the microphone went away".into(),
                        MicError::Denied => "microphone access denied".into(),
                    };
                    *st = State::Failed(why.clone());
                    why
                }
            }
        }
    };
    sink.send(AppEvent::MicFailed(failed));
}

/// Start recording to `out` (permission given).
fn record(out: &Path, sink: &EventSink) -> Result<Recorder, MicError> {
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(&out.to_string_lossy()));
    let rec = unsafe { AVAudioRecorder::initWithURL_settings_error(AVAudioRecorder::alloc(), &url, &settings()) }
        .map_err(|e| MicError::Failed(e.localizedDescription().to_string()))?;
    let delegate = MicDelegate::new(sink.clone());
    unsafe {
        rec.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        rec.setMeteringEnabled(true);
        if !rec.record() {
            rec.setDelegate(None);
            return Err(MicError::Failed("macOS didn't start the recording".into()));
        }
    }
    Ok(Recorder { rec, _delegate: delegate, path: out.to_path_buf() })
}

/// 16 kHz mono 16-bit little-endian PCM in a WAV file.
fn settings() -> Retained<NSDictionary<NSString, AnyObject>> {
    let fourcc = |s: &[u8; 4]| NSNumber::new_u32(u32::from_be_bytes(*s));
    // SAFETY: AVFAudio's own key constants, read once.
    let pairs = unsafe {
        [
            (AVFormatIDKey, fourcc(b"lpcm")),
            (AVAudioFileTypeKey, fourcc(b"WAVE")),
            (AVSampleRateKey, NSNumber::new_f64(RATE)),
            (AVNumberOfChannelsKey, NSNumber::new_u32(1)),
            (AVLinearPCMBitDepthKey, NSNumber::new_u32(16)),
            (AVLinearPCMIsFloatKey, NSNumber::new_bool(false)),
            (AVLinearPCMIsBigEndianKey, NSNumber::new_bool(false)),
        ]
    };
    let pairs: Vec<(&NSString, Retained<NSNumber>)> = pairs.into_iter().filter_map(|(k, v)| Some((k?, v))).collect();
    let keys: Vec<&NSString> = pairs.iter().map(|(k, _)| *k).collect();
    let values: Vec<&AnyObject> = pairs.iter().map(|(_, v)| v.as_ref()).collect();
    NSDictionary::from_slices(&keys, &values)
}

#[repr(C)]
struct Addr {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(id: u32, addr: *const Addr, qual_size: u32, qual: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
}

/// Whether the Mac has a microphone now: Core Audio's default input device
/// (`kAudioHardwarePropertyDefaultInputDevice` on the system object).
fn has_input() -> bool {
    let addr = Addr { selector: u32::from_be_bytes(*b"dIn "), scope: u32::from_be_bytes(*b"glob"), element: 0 };
    let (mut id, mut size) = (0u32, std::mem::size_of::<u32>() as u32);
    let r = unsafe { AudioObjectGetPropertyData(1, &addr, 0, std::ptr::null(), &mut size, (&mut id as *mut u32).cast()) };
    r == 0 && id != 0
}

define_class!(
    /// Hears the recorder stop by itself or fail to write (any thread).
    #[unsafe(super(NSObject))]
    #[name = "PoniMicDelegate"]
    #[ivars = EventSink]
    struct MicDelegate;

    unsafe impl NSObjectProtocol for MicDelegate {}

    unsafe impl AVAudioRecorderDelegate for MicDelegate {
        // `stop` lets go of the delegate first, so this is the recording
        // ending by itself (the microphone went away).
        #[unsafe(method(audioRecorderDidFinishRecording:successfully:))]
        fn did_finish(&self, _recorder: &AVAudioRecorder, _ok: bool) {
            self.ivars().send(AppEvent::MicFailed("the microphone stopped".into()));
        }

        #[unsafe(method(audioRecorderEncodeErrorDidOccur:error:))]
        fn encode_error(&self, _recorder: &AVAudioRecorder, error: Option<&NSError>) {
            let why = error.map_or_else(|| "the recording couldn't be written".to_string(), |e| e.localizedDescription().to_string());
            self.ivars().send(AppEvent::MicFailed(why));
        }
    }
);

impl MicDelegate {
    fn new(sink: EventSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(sink);
        unsafe { msg_send![super(this), init] }
    }
}
