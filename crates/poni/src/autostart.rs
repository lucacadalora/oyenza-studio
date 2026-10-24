//! "Start with Windows".
//!
//! The installer build writes HKCU\Software\Microsoft\Windows\CurrentVersion\Run.
//! The Microsoft Store build (MSIX) can't: Windows keeps a packaged app's HKCU
//! writes in a private per-app copy that sign-in never reads. That build uses
//! the package's startup task instead (installer/msix/AppxManifest.xml), which
//! people also see under Settings > Apps > Startup.

use crate::events::{AppEvent, EventSink};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;
use windows::core::{w, Error, Result, RuntimeType, HSTRING, PCWSTR};
use windows::ApplicationModel::{Package, StartupTask, StartupTaskState};
use windows::Foundation::{AsyncOperationCompletedHandler, IAsyncOperation};
use windows::Win32::Foundation::{E_FAIL, ERROR_TIMEOUT};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::Registry::*;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("Poni");
/// The StartupTask TaskId in installer/msix/AppxManifest.xml.
const TASK_ID: &str = "PoniStartup";
/// Startup-task calls answer in milliseconds; never let one hang the UI thread.
const TASK_TIMEOUT: Duration = Duration::from_secs(2);

/// True in the Microsoft Store build (the process has a package identity).
pub fn packaged() -> bool {
    static PACKAGED: OnceLock<bool> = OnceLock::new();
    *PACKAGED.get_or_init(|| Package::Current().is_ok())
}

/// Set it (when `set` is Some), then read it, on a thread of its own, and
/// send what it is as `AppEvent::Autostart`. The Settings page uses this: the
/// Store build's startup task may take a moment to answer, and the island
/// keeps drawing meanwhile.
pub fn report(sink: EventSink, set: Option<bool>) {
    let spawned = std::thread::Builder::new().name("autostart".into()).spawn(move || {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if let Some(on) = set {
            if let Err(e) = set_enabled(on) {
                crate::pwarn!("autostart: turning it {} failed: {e}", if on { "on" } else { "off" });
            }
        }
        sink.send(AppEvent::Autostart(is_enabled()));
        if hr.is_ok() {
            unsafe { CoUninitialize() };
        }
    });
    if let Err(e) = spawned {
        crate::pwarn!("autostart: no thread to read it on: {e}");
    }
}

pub fn is_enabled() -> bool {
    if packaged() {
        return task_state().is_ok_and(|s| s == StartupTaskState::Enabled || s == StartupTaskState::EnabledByPolicy);
    }
    let Ok(key) = open_run_key(KEY_READ) else { return false };
    let mut size = 0u32;
    let ok = unsafe { RegQueryValueExW(key, VALUE, None, None, None, Some(&mut size)) }.is_ok();
    unsafe {
        let _ = RegCloseKey(key);
    }
    ok && size > 0
}

pub fn set_enabled(on: bool) -> Result<()> {
    if packaged() {
        return set_task_enabled(on);
    }
    let key = open_run_key(KEY_READ | KEY_WRITE)?;
    let res = if on {
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let value = format!("\"{exe}\"");
        let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
        unsafe { RegSetValueExW(key, VALUE, 0, REG_SZ, Some(bytes)).ok() }
    } else {
        match unsafe { RegDeleteValueW(key, VALUE) }.ok() {
            Err(e) if e.code() == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult() => Ok(()),
            r => r,
        }
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    res
}

/// How "Start with Windows" works in this copy, for the self-test report.
/// Err when the Store build can't reach its startup task.
pub fn describe() -> std::result::Result<String, String> {
    if !packaged() {
        let set = if is_enabled() { "set" } else { "not set" };
        return Ok(format!("installer build: Run value {set}"));
    }
    match task_state() {
        Ok(s) => Ok(format!("packaged: startup task {TASK_ID} is {}", state_name(s))),
        Err(e) => Err(format!("packaged: startup task {TASK_ID} unavailable: {e}")),
    }
}

fn open_run_key(access: REG_SAM_FLAGS) -> Result<HKEY> {
    let mut key = HKEY::default();
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, access, &mut key).ok()? };
    Ok(key)
}

// ---------------------------------------------------------------- Store build

fn startup_task() -> Result<StartupTask> {
    wait(StartupTask::GetAsync(&HSTRING::from(TASK_ID))?)
}

fn task_state() -> Result<StartupTaskState> {
    startup_task()?.State()
}

fn set_task_enabled(on: bool) -> Result<()> {
    let task = startup_task()?;
    if !on {
        return task.Disable();
    }
    // Desktop apps get no consent prompt here; Windows answers with the new state.
    match wait(task.RequestEnableAsync()?)? {
        StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy => Ok(()),
        // Turned off in Settings or Task Manager: only the user can turn it back on there.
        StartupTaskState::DisabledByUser => {
            crate::shell::open_startup_settings();
            Ok(())
        }
        s => Err(Error::new(E_FAIL, format!("startup task stays {}", state_name(s)))),
    }
}

fn state_name(s: StartupTaskState) -> &'static str {
    match s {
        StartupTaskState::Enabled => "on",
        StartupTaskState::EnabledByPolicy => "on (policy)",
        StartupTaskState::Disabled => "off",
        StartupTaskState::DisabledByUser => "off (turned off in Settings)",
        StartupTaskState::DisabledByPolicy => "off (policy)",
        _ => "unknown",
    }
}

/// Wait for a WinRT async operation for up to [`TASK_TIMEOUT`] (cancelling it
/// on timeout).
fn wait<T: RuntimeType + 'static>(op: IAsyncOperation<T>) -> Result<T> {
    let (tx, rx) = mpsc::channel();
    op.SetCompleted(&AsyncOperationCompletedHandler::<T>::new(move |_, status| {
        let _ = tx.send(status);
        Ok(())
    }))?;
    match rx.recv_timeout(TASK_TIMEOUT) {
        // A failed or cancelled operation returns its error here.
        Ok(_) => op.GetResults(),
        Err(_) => {
            let _ = op.Cancel();
            Err(Error::new(ERROR_TIMEOUT.to_hresult(), "no answer from the startup task"))
        }
    }
}
