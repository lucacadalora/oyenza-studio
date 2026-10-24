//! Clocks: monotonic seconds for animation/timers, local wall time for display.

use poni_core::datetime::LocalDateTime;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::System::SystemInformation::GetLocalTime;

static START: OnceLock<Instant> = OnceLock::new();
/// Self-test loop recordings hold `mono()` here (f64 bits; `u64::MAX` = not held).
static HELD: AtomicU64 = AtomicU64::new(u64::MAX);

/// Monotonic seconds since the process started.
pub fn mono() -> f64 {
    let held = HELD.load(Ordering::Relaxed);
    if held != u64::MAX {
        return f64::from_bits(held);
    }
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Self-test only: hold `mono()` at `t` (so a recording steps it exactly), or
/// let it run again with `None`.
pub fn hold(t: Option<f64>) {
    HELD.store(t.map_or(u64::MAX, f64::to_bits), Ordering::Relaxed);
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 100-ns ticks since 1601-01-01 UTC (Windows FILETIME / WinRT DateTime).
pub fn filetime_now() -> i64 {
    let unix_100ns = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64 / 100).unwrap_or(0);
    unix_100ns + 116_444_736_000_000_000
}

pub fn local_now() -> LocalDateTime {
    let st = unsafe { GetLocalTime() };
    LocalDateTime {
        year: st.wYear as i32,
        month: st.wMonth as u8,
        day: st.wDay as u8,
        hour: st.wHour as u8,
        minute: st.wMinute as u8,
        second: st.wSecond as u8,
    }
}

/// Unix timestamp of local midnight today.
pub fn day_start_unix(local: &LocalDateTime, unix: i64) -> i64 {
    unix - (local.hour as i64 * 3600 + local.minute as i64 * 60 + local.second as i64)
}
