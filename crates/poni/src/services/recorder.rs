//! Oyenza Studio Recorder: the separate recorder app built from Cap. The
//! island starts, pauses and stops its recordings through the recorder's own
//! links (`poni_core::recorder`), and the recorder tells the island how they
//! go with `WM_COPYDATA` (read in `app.rs`). Nothing here runs at rest:
//! [`installed`] is read at start, as the island opens and when the menu opens.

use poni_core::recorder;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, GetMonitorInfoW, MonitorFromWindow, DISPLAY_DEVICEW, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, KEY_READ};

/// Whether the recorder is installed: its installer registers the
/// `oyenza-recorder:` link scheme (per user or for the machine; HKCR shows both).
pub fn installed() -> bool {
    let scheme = HSTRING::from(recorder::SCHEME);
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, PCWSTR(scheme.as_ptr()), 0, KEY_READ, &mut key) }.is_err() {
        return false;
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    true
}

/// The Record page's Record: `target` (the screen the island is on, or a
/// window or an area the recorder's overlay asks for) with `inputs`.
pub fn record(island: HWND, target: recorder::Target, inputs: recorder::Inputs) {
    let screen = if target == recorder::Target::Screen { screen_name(island) } else { None };
    crate::shell::open_recorder_link(&recorder::record_link(target, screen.as_deref(), inputs));
}

pub fn pause() {
    crate::shell::open_recorder_link(&recorder::pause_link());
}

pub fn resume() {
    crate::shell::open_recorder_link(&recorder::resume_link());
}

pub fn stop() {
    crate::shell::open_recorder_link(&recorder::stop_link());
}

/// Record the screen the island is on, with the camera, microphone and
/// system sound the recorder used last time.
pub fn record_screen(island: HWND) {
    match screen_name(island) {
        Some(name) => crate::shell::open_recorder_link(&recorder::record_screen_link(&name)),
        None => crate::pwarn!("recorder: the island's screen has no name"),
    }
}

/// The island's monitor, named the way the recorder names screens: the
/// display device's description for the monitor's GDI device. Two identical
/// monitors share a name, and the recorder then takes the first.
fn screen_name(hwnd: HWND) -> Option<String> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(monitor, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            return None;
        }
        let mut device = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !EnumDisplayDevicesW(PCWSTR(mi.szDevice.as_ptr()), 0, &mut device, 0).as_bool() {
            return None;
        }
        let len = device.DeviceString.iter().position(|&c| c == 0).unwrap_or(device.DeviceString.len());
        let name = String::from_utf16_lossy(&device.DeviceString[..len]);
        (!name.is_empty()).then_some(name)
    }
}
