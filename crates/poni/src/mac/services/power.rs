//! Battery and charging state (IOKit power sources), and a notification when
//! either changes. Macs without a battery report `has_battery: false`, so the
//! island shows no battery pop-ups there (and Today shows the uptime).

use super::cf::{self, CFTypeRef};
use crate::events::{AppEvent, EventSink};
use crate::state::PowerState;
use objc2_foundation::NSProcessInfo;
use std::ffi::c_void;
use std::sync::Mutex;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFTypeRef;
    fn IOPSNotificationCreateRunLoopSource(callback: extern "C" fn(*mut c_void), context: *mut c_void) -> CFTypeRef;
}

static SINK: Mutex<Option<EventSink>> = Mutex::new(None);

pub fn query() -> PowerState {
    let mut s = PowerState { saver: NSProcessInfo::processInfo().isLowPowerModeEnabled(), ..PowerState::default() };
    unsafe {
        let blob = IOPSCopyPowerSourcesInfo();
        if blob.is_null() {
            return s;
        }
        s.on_ac = cf::str_is(IOPSGetProvidingPowerSourceType(blob), "AC Power");
        let list = IOPSCopyPowerSourcesList(blob);
        if !list.is_null() {
            for i in 0..cf::CFArrayGetCount(list) {
                let desc = IOPSGetPowerSourceDescription(blob, cf::CFArrayGetValueAtIndex(list, i));
                if desc.is_null() || !cf::dict_str_is(desc, "Type", "InternalBattery") || cf::dict_bool(desc, "Is Present") == Some(false) {
                    continue;
                }
                let cur = cf::dict_i64(desc, "Current Capacity").unwrap_or(0);
                let max = cf::dict_i64(desc, "Max Capacity").unwrap_or(100).max(1);
                s.has_battery = true;
                s.percent = (cur * 100 / max).clamp(0, 100) as u8;
                s.charging = cf::dict_bool(desc, "Is Charging").unwrap_or(false);
                break;
            }
            cf::CFRelease(list);
        }
        cf::CFRelease(blob);
    }
    s
}

extern "C" fn changed(_ctx: *mut c_void) {
    let sink = SINK.lock().ok().and_then(|g| g.clone());
    if let Some(sink) = sink {
        sink.send(AppEvent::Power(query()));
    }
}

/// Send `AppEvent::Power` whenever the power source or the charge changes.
pub fn watch(sink: EventSink) {
    if let Ok(mut g) = SINK.lock() {
        *g = Some(sink);
    }
    unsafe {
        let source = IOPSNotificationCreateRunLoopSource(changed, std::ptr::null_mut());
        if source.is_null() {
            pwarn!("power: no change notifications");
            return;
        }
        // Lives as long as the app.
        cf::CFRunLoopAddSource(cf::CFRunLoopGetMain(), source, cf::kCFRunLoopCommonModes);
    }
}
