//! Clocks: monotonic seconds for animation/timers, local wall time for display.

use poni_core::datetime::LocalDateTime;
use std::os::raw::{c_char, c_int, c_long};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static START: OnceLock<Instant> = OnceLock::new();

/// Monotonic seconds since the process started.
pub fn mono() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `struct tm` as macOS lays it out.
#[repr(C)]
struct Tm {
    tm_sec: c_int,
    tm_min: c_int,
    tm_hour: c_int,
    tm_mday: c_int,
    tm_mon: c_int,
    tm_year: c_int,
    tm_wday: c_int,
    tm_yday: c_int,
    tm_isdst: c_int,
    tm_gmtoff: c_long,
    tm_zone: *mut c_char,
}

extern "C" {
    fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
}

/// The wall clock for `unix` in the Mac's time zone.
pub fn local_at(unix: i64) -> LocalDateTime {
    let mut tm = Tm { tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 1, tm_mon: 0, tm_year: 70, tm_wday: 0, tm_yday: 0, tm_isdst: 0, tm_gmtoff: 0, tm_zone: std::ptr::null_mut() };
    if unsafe { localtime_r(&unix, &mut tm) }.is_null() {
        return LocalDateTime::default();
    }
    LocalDateTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u8,
        day: tm.tm_mday as u8,
        hour: tm.tm_hour as u8,
        minute: tm.tm_min as u8,
        second: tm.tm_sec.min(59) as u8,
    }
}

pub fn local_now() -> LocalDateTime {
    local_at(unix_now())
}

/// Unix timestamp of local midnight today.
pub fn day_start_unix(local: &LocalDateTime, unix: i64) -> i64 {
    unix - (local.hour as i64 * 3600 + local.minute as i64 * 60 + local.second as i64)
}
