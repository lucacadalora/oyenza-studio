//! Clocks: monotonic seconds for animation/timers, local wall time for display.

use poni_core::datetime::LocalDateTime;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::System::SystemInformation::GetLocalTime;

static START: OnceLock<Instant> = OnceLock::new();
/// Self-test loop recordings hold `mono()` here (f64 bits; `u64::MAX` = not held).
static HELD: AtomicU64 = AtomicU64::new(u64::MAX);
/// Self-test pictures of a set time of day hold `local_shown()` here (the
/// year, then a byte each for the month, day, hour, minute and second;
/// `u64::MAX` = not held).
static HELD_LOCAL: AtomicU64 = AtomicU64::new(u64::MAX);

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

/// The local date and time the island shows: the PC's ([`local_now`]),
/// unless a self-test picture holds another ([`hold_local`]). The log and
/// the minute timer keep to the PC's.
pub fn local_shown() -> LocalDateTime {
    let held = HELD_LOCAL.load(Ordering::Relaxed);
    if held != u64::MAX {
        return unpack(held);
    }
    local_now()
}

/// Self-test only: hold `local_shown()` at `t` (so a picture shows a set
/// time of day whenever it is taken), or let it follow the PC's clock again
/// with `None`.
pub fn hold_local(t: Option<LocalDateTime>) {
    HELD_LOCAL.store(t.map_or(u64::MAX, pack), Ordering::Relaxed);
}

/// A date and time as [`HELD_LOCAL`] keeps it.
fn pack(t: LocalDateTime) -> u64 {
    (t.year.clamp(0, 9999) as u64) << 40 | (t.month as u64) << 32 | (t.day as u64) << 24 | (t.hour as u64) << 16 | (t.minute as u64) << 8 | t.second as u64
}

fn unpack(held: u64) -> LocalDateTime {
    let byte = |shift: u32| (held >> shift) as u8;
    LocalDateTime { year: (held >> 40) as i32, month: byte(32), day: byte(24), hour: byte(16), minute: byte(8), second: byte(0) }
}

/// Unix timestamp of local midnight today.
pub fn day_start_unix(local: &LocalDateTime, unix: i64) -> i64 {
    unix - (local.hour as i64 * 3600 + local.minute as i64 * 60 + local.second as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_local_time_reads_back_as_it_was_set() {
        // A picture's time, the last second of a leap day, and year 1.
        for t in [
            LocalDateTime { year: 2026, month: 10, day: 29, hour: 6, minute: 10, second: 0 },
            LocalDateTime { year: 2028, month: 2, day: 29, hour: 23, minute: 59, second: 59 },
            LocalDateTime { year: 1, month: 1, day: 1, hour: 0, minute: 0, second: 0 },
        ] {
            assert_eq!(unpack(pack(t)), t, "{t:?}");
            // Never the value that means "not held".
            assert_ne!(pack(t), u64::MAX);
        }
    }
}
