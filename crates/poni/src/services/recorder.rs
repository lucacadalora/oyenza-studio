//! Oyenza Studio Recorder: the separate recorder app built from Cap. The
//! island starts it through the recorder's own link (`poni_core::recorder`);
//! pausing and stopping stay on the recorder's own bar. The Screen recorder
//! add-on installs and uninstalls it ([`install_job`], [`remove_job`]) on the
//! add-on thread. Nothing here runs at rest: [`installed`] is read when the
//! menu opens or a page that offers Record comes on screen.

use crate::events::{AppEvent, EventSink};
use crate::services::addons::{self, Job};
use crate::services::http::Client;
use poni_core::addons::{Status, RECORDER};
use poni_core::recorder;
use std::path::PathBuf;
use std::time::Duration;
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, GetMonitorInfoW, MonitorFromWindow, DISPLAY_DEVICEW, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
};

/// Where Windows lists installed apps (the recorder's installer adds itself
/// per user; a machine-wide install would be under HKLM).
const UNINSTALL: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall");
/// How long an uninstall may take before the island stops waiting for it.
const REMOVE_WAIT: Duration = Duration::from_secs(90);

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

/// The add-on's switch-on: download the installer, run it silently (per
/// user, no administrator) and open the recorder, whose first start asks for
/// its permissions; answer `Ready` once the recorder's link is registered,
/// or `Failed`.
pub fn install_job(sink: EventSink) -> Job {
    Box::new(move |client: Result<&Client, &str>| {
        let status = match install(client, &sink) {
            Ok(()) => {
                crate::pinfo!("recorder: installed {}", recorder::VERSION);
                Status::Ready
            }
            Err(e) => {
                crate::pwarn!("recorder: couldn't install ({e})");
                Status::Failed
            }
        };
        sink.send(AppEvent::Addon { id: RECORDER.id, status });
    })
}

fn install(client: Result<&Client, &str>, sink: &EventSink) -> Result<(), String> {
    let file = recorder::installer(false);
    let dir = crate::paths::addons_dir().join(RECORDER.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {} ({e})", dir.display()))?;
    let setup = dir.join(file.name);
    addons::fetch_installer(client?, sink, RECORDER.id, file, &recorder::installer_url(file), &setup)?;
    sink.send(AppEvent::Addon { id: RECORDER.id, status: Status::Installing });
    // `/R`: the Setup opens the recorder once it's installed.
    let ran = std::process::Command::new(&setup).args(["/S", "/R"]).status();
    let _ = std::fs::remove_dir_all(&dir);
    let code = ran.map_err(|e| format!("could not run the installer ({e})"))?;
    if !code.success() {
        return Err(format!("the installer ended with {code}"));
    }
    if installed() {
        Ok(())
    } else {
        Err("the installer ran, but the recorder's link isn't registered".into())
    }
}

/// The add-on's switch-off: run the recorder's own uninstaller silently and
/// answer `Off` once it's gone. It leaves the recordings, which live in the
/// recorder's data folder, not where the app is installed.
pub fn remove_job(sink: EventSink) -> Job {
    Box::new(move |_client: Result<&Client, &str>| {
        let _ = std::fs::remove_dir_all(crate::paths::addons_dir().join(RECORDER.id));
        let status = match remove() {
            Ok(()) => {
                crate::pinfo!("recorder: uninstalled");
                Status::Off
            }
            Err(e) => {
                crate::pwarn!("recorder: couldn't uninstall ({e})");
                if installed() {
                    Status::Ready
                } else {
                    Status::Off
                }
            }
        };
        sink.send(AppEvent::Addon { id: RECORDER.id, status });
    })
}

fn remove() -> Result<(), String> {
    if !installed() {
        return Ok(());
    }
    let uninstaller = uninstaller().ok_or("Windows lists no uninstaller for it")?;
    std::process::Command::new(&uninstaller).arg("/S").status().map_err(|e| format!("could not run {} ({e})", uninstaller.display()))?;
    // An NSIS uninstaller copies itself away and returns at once; the
    // recorder is gone once its link is.
    let until = std::time::Instant::now() + REMOVE_WAIT;
    while std::time::Instant::now() < until {
        if !installed() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("it was still installed after the uninstaller ran".into())
}

/// The recorder's uninstaller, from its entry in Windows' installed apps
/// (the one whose name is [`recorder::APP_NAME`]).
fn uninstaller() -> Option<PathBuf> {
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|root| {
        let mut list = HKEY::default();
        if unsafe { RegOpenKeyExW(root, UNINSTALL, 0, KEY_READ, &mut list) }.is_err() {
            return None;
        }
        let found = (0..).map_while(|i| subkey(list, i)).find_map(|name| {
            let name = HSTRING::from(name);
            (reg_string(list, &name, w!("DisplayName")).as_deref() == Some(recorder::APP_NAME))
                .then(|| reg_string(list, &name, w!("UninstallString")))
                .flatten()
                .and_then(|command| program(&command))
        });
        unsafe {
            let _ = RegCloseKey(list);
        }
        found
    })
}

/// The `i`th subkey's name, or None past the last.
fn subkey(key: HKEY, i: u32) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    let r = unsafe { RegEnumKeyExW(key, i, PWSTR(buf.as_mut_ptr()), &mut len, None, PWSTR::null(), None, None) };
    (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

fn reg_string(key: HKEY, subkey: &HSTRING, value: PCWSTR) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(key, PCWSTR(subkey.as_ptr()), value, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), Some(&mut size)) };
    if r.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// The program in an uninstall command line: the quoted path, or what comes
/// before the first space.
fn program(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let path = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command.split(' ').next()?,
    };
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real thing, run by hand on CI's Windows machine (`ci.yml`, input
    /// `recorder_install`): the pinned installer downloads and checks out,
    /// installs silently and opens the recorder, its link works, and the
    /// uninstall takes it away again while it runs.
    #[test]
    #[ignore]
    fn recorder_add_on_installs_and_uninstalls() {
        assert!(recorder::pinned(), "pin a published recorder release first (tools/pin_recorder.py)");
        let _com = crate::services::http::MtaGuard::new();
        let client = Client::new("OyenzaStudio-test (+https://github.com/lucacadalora/oyenza-studio)").expect("an HTTP client");
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = EventSink::new(tx, crate::events::SendHwnd(0));
        assert!(!installed(), "the machine must start without the recorder");
        let started = std::time::Instant::now();
        install(Ok(&client), &sink).expect("the install");
        println!("installed in {} s", started.elapsed().as_secs());
        assert!(installed(), "its link is registered");
        let said: Vec<String> = rx.try_iter().filter_map(|e| if let AppEvent::Addon { status, .. } = e { Some(format!("{status:?}")) } else { None }).collect();
        println!("statuses: {said:?}");
        assert!(said.iter().any(|s| s.starts_with("Downloading")) && said.iter().any(|s| s == "Installing"));
        assert!(!crate::paths::addons_dir().join(RECORDER.id).exists(), "the installer is deleted once it has run");
        std::thread::sleep(Duration::from_secs(8));
        remove().expect("the uninstall");
        assert!(!installed(), "its link is gone");
    }
}
