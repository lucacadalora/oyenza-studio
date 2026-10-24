//! Battery / charging state (`GetSystemPowerStatus`) and change notifications.

use crate::state::PowerState;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Power::{GetSystemPowerStatus, RegisterPowerSettingNotification, POWERBROADCAST_SETTING, SYSTEM_POWER_STATUS};
use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
use windows::Win32::System::SystemServices::{GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_CONSOLE_DISPLAY_STATE, GUID_POWER_SAVING_STATUS};
use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_WINDOW_HANDLE, PBT_POWERSETTINGCHANGE};

pub fn query() -> PowerState {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return PowerState::default();
    }
    // BatteryFlag: 128 = no system battery, 255 = unknown.
    let has_battery = s.BatteryFlag != 128 && s.BatteryFlag != 255 && s.BatteryLifePercent <= 100;
    PowerState {
        has_battery,
        percent: if s.BatteryLifePercent <= 100 { s.BatteryLifePercent } else { 0 },
        charging: has_battery && (s.BatteryFlag & 8) != 0,
        on_ac: s.ACLineStatus == 1,
        saver: s.SystemStatusFlag == 1,
    }
}

/// Ask Windows to send `WM_POWERBROADCAST` to `hwnd` when the power source,
/// the battery percentage or battery saver changes (battery saver can be
/// switched by hand at any charge).
pub fn register(hwnd: HWND) {
    unsafe {
        for guid in [&GUID_ACDC_POWER_SOURCE, &GUID_BATTERY_PERCENTAGE_REMAINING, &GUID_POWER_SAVING_STATUS] {
            if let Err(e) = RegisterPowerSettingNotification(HANDLE(hwnd.0), guid, DEVICE_NOTIFY_WINDOW_HANDLE) {
                crate::pwarn!("RegisterPowerSettingNotification failed: {e}");
            }
        }
    }
}

/// Ask for session lock/unlock (`WM_WTSSESSION_CHANGE`) and display on/off
/// (`WM_POWERBROADCAST`) notifications so ambient animation can pause.
pub fn register_session(hwnd: HWND) {
    unsafe {
        if let Err(e) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
            crate::pwarn!("WTSRegisterSessionNotification failed: {e}");
        }
        if let Err(e) = RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE) {
            crate::pwarn!("display-state notification unavailable: {e}");
        }
    }
}

/// For `WM_POWERBROADCAST`: `Some(display_on)` when the message reports a
/// console display state change (0 = off, 1 = on, 2 = dimmed counts as on).
pub fn display_state_change(wp: WPARAM, lp: LPARAM) -> Option<bool> {
    if wp.0 as u32 != PBT_POWERSETTINGCHANGE || lp.0 == 0 {
        return None;
    }
    // SAFETY: for PBT_POWERSETTINGCHANGE, lParam points to a POWERBROADCAST_SETTING
    // valid for the duration of the message.
    let s = unsafe { &*(lp.0 as *const POWERBROADCAST_SETTING) };
    if s.PowerSetting != GUID_CONSOLE_DISPLAY_STATE || s.DataLength < 4 {
        return None;
    }
    let v = unsafe { std::ptr::read_unaligned(s.Data.as_ptr() as *const u32) };
    Some(v != 0)
}
