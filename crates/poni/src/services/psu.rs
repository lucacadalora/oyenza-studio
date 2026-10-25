//! Power supplies that report over USB, for the System page's Power module
//! (`poni_core::power` has their protocols). The sampler looks for one among
//! the HID devices when the page opens, reads it once a second while the
//! page is open and closes it with the page: nothing runs at rest.
//!
//! Only reads go to the PSU, and the handshake its own app sends too. That
//! app (iCUE, MSI Center) may talk to it at the same time, and every input
//! report reaches every open handle: so the queue is flushed before each
//! request, and a reply counts only when it echoes the request.

use poni_core::power::{self, corsair, msi, MsiLayout, Protocol, PsuModel, PsuReading, REPORT};
use std::time::{Duration, Instant};
use windows::core::{GUID, HSTRING, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE,
    DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, HWND, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED};

/// GUID_DEVINTERFACE_HID.
const HID_INTERFACES: GUID = GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);
/// IOCTL_HID_FLUSH_QUEUE (what HidD_FlushQueue sends): drops queued input reports.
const FLUSH_QUEUE: u32 = 0x000b_0197;
/// How long a PSU gets to answer one request (Linux's drivers wait as long).
const ANSWER: Duration = Duration::from_millis(250);
/// Reports to look through for the one answering a request.
const READS_PER_ANSWER: usize = 4;

pub struct Psu {
    file: HANDLE,
    event: HANDLE,
    model: &'static PsuModel,
    /// What the page calls it: MSI's handshake name, else the model's.
    name: String,
    /// MSI: the bulk read's layout.
    layout: Option<MsiLayout>,
    /// Corsair: whether it answers the input current (some models only).
    in_amps: bool,
}

impl Psu {
    /// The first power supply this knows among the HID devices, opened and
    /// greeted. A cheap look: one walk of the HID interfaces, and only a
    /// known PSU's own interface is opened.
    pub fn find() -> Option<Psu> {
        for path in hid_paths() {
            let Some(model) = power::usb_ids(&path).and_then(|(vid, pid)| power::model(vid, pid)) else { continue };
            if let Some(psu) = Psu::open(&path, model) {
                crate::pinfo!("system power: {} reports over USB", psu.name);
                return Some(psu);
            }
        }
        None
    }

    fn open(path: &str, model: &'static PsuModel) -> Option<Psu> {
        let file = unsafe {
            CreateFileW(
                &HSTRING::from(path),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                HANDLE::default(),
            )
        }
        .ok()?;
        let Ok(event) = (unsafe { CreateEventW(None, true, false, PCWSTR::null()) }) else {
            unsafe {
                let _ = CloseHandle(file);
            }
            return None;
        };
        // Dropped (and closed) on any failure below.
        let mut psu = Psu { file, event, model, name: model.name.to_string(), layout: None, in_amps: true };
        match model.protocol {
            Protocol::Corsair => {
                // Its answer (or none, once it's had it) doesn't matter; the total must come back.
                let _ = psu.exchange(&power::corsair_hello(), |_| true);
                psu.corsair(corsair::TOTAL_OUT)?;
            }
            Protocol::Msi(layout) => {
                let reply = psu.exchange(&power::msi_hello(), |r| power::msi_name(r).is_some())?;
                let reported = power::msi_name(&reply)?;
                // MSI's software picks the layout by the name the PSU gives.
                psu.layout = Some(MsiLayout::from_name(&reported).unwrap_or(layout));
                psu.name = if reported.to_ascii_uppercase().starts_with("MSI") { reported } else { format!("MSI {reported}") };
            }
        }
        Some(psu)
    }

    /// One reading, or None when the PSU didn't answer.
    pub fn read(&mut self) -> Option<PsuReading> {
        match self.model.protocol {
            Protocol::Corsair => {
                // The total first: a PSU that doesn't answer costs one wait, not five.
                let out_w = Some(self.corsair(corsair::TOTAL_OUT)?);
                let mains_v = self.corsair(corsair::READ_VIN);
                let in_amps = if self.in_amps { self.corsair(corsair::READ_IIN) } else { None };
                self.in_amps = in_amps.is_some();
                let temp_c = self.corsair(corsair::READ_TEMP_2).or_else(|| self.corsair(corsair::READ_TEMP_1));
                let fan = self.corsair(corsair::READ_FAN);
                power::corsair_reading(&self.name, out_w, mains_v, in_amps, temp_c, fan)
            }
            Protocol::Msi(_) => {
                let layout = self.layout?;
                let all = self.exchange(&power::msi_read(msi::ALL), |r| power::msi_data(msi::ALL, r).is_some())?;
                // The MEG models also answer the mains voltage; the MPG TS ones refuse it.
                let mains_v = (layout != MsiLayout::MpgTs)
                    .then(|| self.exchange(&power::msi_read(msi::VIN), |r| r.len() > 1 && r[0] == 0x51 && (r[1] == msi::VIN || r[1] == 0xfe)))
                    .flatten()
                    .and_then(|r| power::msi_data(msi::VIN, &r).and_then(power::msi_mains));
                power::msi_reading(&self.name, layout, power::msi_data(msi::ALL, &all)?, mains_v)
            }
        }
    }

    fn corsair(&mut self, cmd: u8) -> Option<f32> {
        // An unsupported command answers too, with 0 for the command: it ends the wait.
        let reply = self.exchange(&power::corsair_read(cmd), |r| r.len() > 1 && r[0] == 0x03 && (r[1] == cmd || r[1] == 0))?;
        power::corsair_value(cmd, &reply)
    }

    /// Sends `request` and waits for the report `answers` picks out.
    fn exchange(&mut self, request: &[u8; REPORT], answers: impl Fn(&[u8]) -> bool) -> Option<[u8; REPORT]> {
        // Answers to another app's requests, or to one of ours that timed out.
        let mut got = 0u32;
        unsafe {
            let _ = DeviceIoControl(self.file, FLUSH_QUEUE, None, 0, None, 0, Some(&mut got), None);
        }
        let mut out = [0u8; REPORT + 1];
        out[1..].copy_from_slice(request);
        let deadline = Instant::now() + ANSWER;
        self.io(deadline, |ov| unsafe { WriteFile(self.file, Some(&out), None, Some(ov)) })?;
        for _ in 0..READS_PER_ANSWER {
            let mut buf = [0u8; REPORT + 1];
            let n = self.io(deadline, |ov| unsafe { ReadFile(self.file, Some(&mut buf), None, Some(ov)) })? as usize;
            // Byte 0 is the report id.
            if n > 1 && answers(&buf[1..n.min(REPORT + 1)]) {
                let mut reply = [0u8; REPORT];
                reply[..n - 1].copy_from_slice(&buf[1..n]);
                return Some(reply);
            }
        }
        None
    }

    /// Runs one overlapped read or write, waiting until `deadline`; cancels
    /// it past the deadline. The bytes moved, or None.
    fn io(&self, deadline: Instant, start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>) -> Option<u32> {
        let mut ov = OVERLAPPED { hEvent: self.event, ..Default::default() };
        unsafe {
            let _ = ResetEvent(self.event);
        }
        match start(&mut ov) {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(_) => return None,
        }
        let wait = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
        let mut n = 0u32;
        unsafe {
            if WaitForSingleObject(self.event, wait) != WAIT_OBJECT_0 {
                let _ = CancelIoEx(self.file, Some(&ov));
                // The cancelled request finishes before its buffer and `ov` go.
                let _ = GetOverlappedResult(self.file, &ov, &mut n, true);
                return None;
            }
            GetOverlappedResult(self.file, &ov, &mut n, false).ok()?;
        }
        Some(n)
    }
}

impl Drop for Psu {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.file);
            let _ = CloseHandle(self.event);
        }
    }
}

/// Every present HID interface's path.
fn hid_paths() -> Vec<String> {
    let mut paths = Vec::new();
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(Some(&HID_INTERFACES), PCWSTR::null(), HWND::default(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
            return paths;
        };
        for i in 0.. {
            let mut iface = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInterfaces(set, None, &HID_INTERFACES, i, &mut iface).is_err() {
                break;
            }
            if let Some(p) = interface_path(set, &iface) {
                paths.push(p);
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    paths
}

unsafe fn interface_path(set: HDEVINFO, iface: &SP_DEVICE_INTERFACE_DATA) -> Option<String> {
    let mut need = 0u32;
    let _ = SetupDiGetDeviceInterfaceDetailW(set, iface, None, 0, Some(&mut need), None);
    if need < 8 {
        return None;
    }
    let mut buf = vec![0u64; (need as usize).div_ceil(8)];
    let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
    SetupDiGetDeviceInterfaceDetailW(set, iface, Some(detail), need, None, None).ok()?;
    let chars = (need as usize - std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath)) / 2;
    // The path runs past the declared one-character array.
    let path = std::slice::from_raw_parts(std::ptr::addr_of!((*detail).DevicePath).cast::<u16>(), chars);
    Some(String::from_utf16_lossy(&path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())]))
}
