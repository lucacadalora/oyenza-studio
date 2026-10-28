//! The microphone, for the IELTS page's answers: WASAPI shared-mode capture
//! from the default recording device, written as 16 kHz mono 16-bit WAV
//! (`poni_core::ielts::WavWriter`).
//!
//! Why WASAPI and not WinRT's AudioGraph: `IAudioClient` and
//! `IAudioCaptureClient` come with Core Audio, which the volume watcher
//! (`services/audio.rs`) already loads, and the `windows` crate features it
//! needs are already on; AudioGraph would bring Media Foundation's pipeline
//! (several MB of DLLs and a graph thread) for one microphone, and its level
//! meter needs a frame node read through COM interop. Shared mode records in
//! Windows' mix format (usually 48 kHz float stereo), so
//! `poni_core::ielts::Downmix` averages the channels and resamples to 16 kHz
//! itself, the same on every PC (and under Wine).
//!
//! One thread, `poni-mic`, exists only while recording: [`Mic::start`]
//! starts it and [`Mic::stop`] (or dropping the `Mic`) ends it. It wakes
//! every 40 ms to take what the device has (a 500 ms buffer leaves room for
//! a late wake-up) and keeps the peak of the last 50 ms for the level meter.
//! When Windows' microphone privacy switch keeps desktop apps out,
//! [`MicError::Denied`]; a device that goes away mid-answer sends
//! `AppEvent::MicFailed`.

use crate::events::{AppEvent, EventSink};
use poni_core::ielts::{Downmix, SampleFormat, WavWriter, RATE};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::core::{w, GUID, HRESULT};
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, EDataFlow, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

/// How often the thread takes what the device has. The buffer below holds
/// 500 ms, so a wake-up that comes late loses nothing.
pub(crate) const POLL: Duration = Duration::from_millis(40);
const BUFFER_HNS: i64 = 5_000_000;
/// The level meter: the loudest sample of the last 50 ms (at 16 kHz).
const METER_SAMPLES: usize = RATE as usize / 20;
/// How long `start` waits for the device to open.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// `HRESULT_FROM_WIN32(ERROR_NOT_FOUND)`: no default recording device.
const E_NOTFOUND: HRESULT = HRESULT(0x8007_0490_u32 as i32);
/// The mix format's tags and sub-formats (Multimedia / KernelStreaming,
/// which the `windows` crate puts behind features this app doesn't need).
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MicError {
    Denied,
    NoDevice,
    Failed(String),
}

impl std::fmt::Display for MicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MicError::Denied => f.write_str("microphone access is off for desktop apps"),
            MicError::NoDevice => f.write_str("no microphone"),
            MicError::Failed(s) => f.write_str(s),
        }
    }
}

struct Shared {
    stop: AtomicBool,
    /// f32 bits: 0..1.
    level: AtomicU32,
}

/// A recording in progress.
pub struct Mic {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Result<(PathBuf, f32), MicError>>>,
}

impl Mic {
    /// Records 16 kHz mono 16-bit WAV to `out`.
    pub fn start(out: &Path, sink: &EventSink) -> Result<Mic, MicError> {
        if privacy_denies() {
            return Err(MicError::Denied);
        }
        let shared = Arc::new(Shared { stop: AtomicBool::new(false), level: AtomicU32::new(0) });
        let (ready_tx, ready_rx) = mpsc::channel();
        let (worker, path, sink) = (shared.clone(), out.to_path_buf(), sink.clone());
        let thread = std::thread::Builder::new()
            .name("poni-mic".into())
            .spawn(move || capture(&path, &worker, &sink, ready_tx))
            .map_err(|e| MicError::Failed(format!("no microphone thread ({e})")))?;
        match ready_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => Ok(Mic { shared, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                // The device didn't open in time: let the thread end on its own.
                shared.stop.store(true, Ordering::SeqCst);
                Err(MicError::Failed("the microphone didn't open".into()))
            }
        }
    }

    /// 0..1 peak of the last ~50 ms, for the level meter (read only while recording).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// The file and its length in seconds.
    pub fn stop(mut self) -> Result<(PathBuf, f32), MicError> {
        self.shared.stop.store(true, Ordering::SeqCst);
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(r)) => r,
            _ => Err(MicError::Failed("the recording stopped unexpectedly".into())),
        }
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        // Dropped without `stop`: end the recording and give the device back.
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Windows' privacy switches: "Microphone access" off, or "Let desktop apps
/// access your microphone" off. Shared-mode capture then either fails with
/// E_ACCESSDENIED or delivers silence, depending on the Windows version, so
/// ask first.
pub(crate) fn privacy_denies() -> bool {
    let base = w!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone");
    let nonpackaged = w!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone\\NonPackaged");
    [base, nonpackaged].into_iter().any(|key| consent(key).is_some_and(|v| v.eq_ignore_ascii_case("Deny")))
}

fn consent(key: windows::core::PCWSTR) -> Option<String> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let mut buf = [0u16; 16];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, key, w!("Value"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), Some(&mut size)) };
    if r.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

pub(crate) fn mic_error(what: &str, e: &windows::core::Error) -> MicError {
    match e.code() {
        E_ACCESSDENIED => MicError::Denied,
        E_NOTFOUND | AUDCLNT_E_DEVICE_INVALIDATED => MicError::NoDevice,
        c => MicError::Failed(format!("{what} (0x{:08X})", c.0 as u32)),
    }
}

/// The open device, ready to start.
pub(crate) struct Device {
    pub(crate) client: IAudioClient,
    pub(crate) capture: IAudioCaptureClient,
    pub(crate) downmix: Downmix,
    pub(crate) block: usize,
}

/// The default recording device, opened in shared mode in its own format.
fn open() -> Result<Device, MicError> {
    open_endpoint(eCapture, "ielts: microphone")
}

/// The default device of `flow`, opened in shared mode in its own format:
/// a recording device (`eCapture`), or the playback device's own sound
/// (`eRender`, WASAPI loopback: what the PC plays, a call's voices
/// included). `what` names it in the log.
pub(crate) fn open_endpoint(flow: EDataFlow, what: &str) -> Result<Device, MicError> {
    let loopback = flow != eCapture;
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| mic_error("no Core Audio", &e))?;
        let device = devices.GetDefaultAudioEndpoint(flow, eConsole).map_err(|e| mic_error(if loopback { "no playback device" } else { "no recording device" }, &e))?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| mic_error("can't open the microphone", &e))?;
        let mix = client.GetMixFormat().map_err(|e| mic_error("no recording format", &e))?;
        let format = sample_format(mix);
        let wf = std::ptr::read_unaligned(mix);
        let flags = if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, mix, None);
        CoTaskMemFree(Some(mix as *const core::ffi::c_void));
        init.map_err(|e| mic_error("can't start the microphone", &e))?;
        let format = format.ok_or_else(|| MicError::Failed(format!("recording format {} with {} bits isn't one this app reads", { wf.wFormatTag }, { wf.wBitsPerSample })))?;
        let capture: IAudioCaptureClient = client.GetService().map_err(|e| mic_error("can't read the microphone", &e))?;
        let (rate, channels, block) = (wf.nSamplesPerSec, wf.nChannels, wf.nBlockAlign as usize);
        if block != format.bytes() * channels as usize {
            return Err(MicError::Failed(format!("recording frames of {block} bytes for {channels} channels")));
        }
        crate::pinfo!("{what} at {rate} Hz, {channels} channel(s), {format:?}");
        Ok(Device { client, capture, downmix: Downmix::new(rate, channels, format), block })
    }
}

/// The mix format's samples, if this app reads them.
unsafe fn sample_format(f: *const WAVEFORMATEX) -> Option<SampleFormat> {
    let wf = std::ptr::read_unaligned(f);
    let tag = if wf.wFormatTag == WAVE_FORMAT_EXTENSIBLE && wf.cbSize >= 22 {
        let ext = std::ptr::read_unaligned(f as *const WAVEFORMATEXTENSIBLE);
        // KSDATAFORMAT_SUBTYPE_PCM / _IEEE_FLOAT: the old tag in the GUID's first field.
        let sub: GUID = ext.SubFormat;
        sub.data1 as u16
    } else {
        wf.wFormatTag
    };
    match (tag, wf.wBitsPerSample) {
        (WAVE_FORMAT_IEEE_FLOAT, 32) => Some(SampleFormat::F32),
        (WAVE_FORMAT_PCM, 16) => Some(SampleFormat::I16),
        (WAVE_FORMAT_PCM, 24) => Some(SampleFormat::I24),
        (WAVE_FORMAT_PCM, 32) => Some(SampleFormat::I32),
        _ => None,
    }
}

/// The `poni-mic` thread: open the device, say whether it opened, then
/// record until told to stop.
fn capture(path: &Path, shared: &Shared, sink: &EventSink, ready: mpsc::Sender<Result<(), MicError>>) -> Result<(PathBuf, f32), MicError> {
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let out = (|| {
        let started = open().and_then(|d| {
            let wav = WavWriter::create(path).map_err(|e| MicError::Failed(format!("can't write the answer ({e})")))?;
            unsafe { d.client.Start() }.map_err(|e| mic_error("the microphone didn't start", &e))?;
            Ok((d, wav))
        });
        let (mut d, mut wav) = match started {
            Ok(v) => {
                let _ = ready.send(Ok(()));
                v
            }
            Err(e) => {
                let _ = ready.send(Err(e.clone()));
                let _ = std::fs::remove_file(path);
                return Err(e);
            }
        };
        let result = record(&mut d, &mut wav, shared);
        unsafe {
            let _ = d.client.Stop();
        }
        shared.level.store(0, Ordering::Relaxed);
        match result {
            Ok(()) => wav.finish().map_err(|e| MicError::Failed(format!("can't finish the answer ({e})"))),
            Err(e) => {
                crate::pwarn!("ielts: the microphone stopped mid-answer ({e})");
                sink.send(AppEvent::MicFailed(e.to_string()));
                drop(wav);
                let _ = std::fs::remove_file(path);
                Err(e)
            }
        }
    })();
    if com.is_ok() {
        unsafe { CoUninitialize() };
    }
    out
}

/// Take what the device has every [`POLL`] until `shared.stop`.
fn record(d: &mut Device, wav: &mut WavWriter, shared: &Shared) -> Result<(), MicError> {
    let mut pcm: Vec<i16> = Vec::with_capacity(RATE as usize / 4);
    // The last 50 ms, packet by packet: (samples, loudest).
    let mut meter: VecDeque<(usize, f32)> = VecDeque::new();
    loop {
        let last = shared.stop.load(Ordering::SeqCst);
        loop {
            let frames = unsafe { d.capture.GetNextPacketSize() }.map_err(|e| mic_error("the microphone went away", &e))?;
            if frames == 0 {
                break;
            }
            let (mut data, mut n, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
            unsafe { d.capture.GetBuffer(&mut data, &mut n, &mut flags, None, None) }.map_err(|e| mic_error("the microphone went away", &e))?;
            pcm.clear();
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                d.downmix.silence(n as usize, &mut pcm);
            } else {
                let bytes = unsafe { std::slice::from_raw_parts(data, n as usize * d.block) };
                d.downmix.push(bytes, &mut pcm);
            }
            unsafe { d.capture.ReleaseBuffer(n) }.map_err(|e| mic_error("the microphone went away", &e))?;
            wav.write(&pcm).map_err(|e| MicError::Failed(format!("can't write the answer ({e})")))?;
            meter.push_back((pcm.len(), poni_core::ielts::peak(&pcm)));
            while meter.len() > 1 && meter.iter().skip(1).map(|m| m.0).sum::<usize>() >= METER_SAMPLES {
                meter.pop_front();
            }
            let level = meter.iter().map(|m| m.1).fold(0.0f32, f32::max);
            shared.level.store(level.to_bits(), Ordering::Relaxed);
        }
        if last {
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

