//! Start at login: the app registers itself as a login item with macOS's
//! ServiceManagement (`SMAppService.mainAppService`, macOS 13+). It shows in
//! System Settings > General > Login Items, where it can also be turned off.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::NSError;

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {
    /// Referenced so the framework (and its SMAppService class) gets linked.
    static SMAppServiceErrorDomain: *const std::ffi::c_void;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    On,
    /// Registered, but the person has to allow it in System Settings.
    NeedsApproval,
    /// macOS can't find this app's service (SMAppService says "not found").
    NotFound,
    /// No SMAppService on this macOS (before 13).
    Unavailable,
}

fn service() -> Option<Retained<AnyObject>> {
    let _linked = unsafe { SMAppServiceErrorDomain };
    let cls = AnyClass::get(c"SMAppService")?;
    let svc: Option<Retained<AnyObject>> = unsafe { msg_send![cls, mainAppService] };
    svc
}

pub fn status() -> Status {
    match raw_status() {
        -1 => Status::Unavailable,
        1 => Status::On,
        2 => Status::NeedsApproval,
        3 => Status::NotFound,
        _ => Status::Off,
    }
}

/// SMAppServiceStatus as macOS reports it: 0 not registered, 1 enabled,
/// 2 requires approval, 3 not found (-1: no SMAppService).
pub fn raw_status() -> isize {
    let Some(svc) = service() else { return -1 };
    unsafe { msg_send![&*svc, status] }
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    let svc = service().ok_or_else(|| "login items need macOS 13 or later".to_string())?;
    let mut err: *mut NSError = std::ptr::null_mut();
    let ok: bool = unsafe {
        if on {
            msg_send![&*svc, registerAndReturnError: &mut err]
        } else {
            msg_send![&*svc, unregisterAndReturnError: &mut err]
        }
    };
    if ok {
        return Ok(());
    }
    let why = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "unknown error".into());
    Err(why)
}

/// System Settings > General > Login Items.
pub fn open_settings() {
    if let Some(cls) = AnyClass::get(c"SMAppService") {
        let _: () = unsafe { msg_send![cls, openSystemSettingsLoginItems] };
    }
}
