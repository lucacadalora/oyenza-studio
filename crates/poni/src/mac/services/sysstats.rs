//! CPU / memory / disk / uptime for the Today page, sampled once a second
//! while it shows (as on Windows).
//!
//! * CPU: the busy share of all cores between this sample and the previous
//!   one (`host_statistics`, cumulative ticks), so the first call reports 0.
//! * Memory: what Activity Monitor calls "Memory Used" (app memory + wired +
//!   compressed) of the installed RAM.
//! * Disk: the startup disk's space "available for important use" (what
//!   Finder shows, counting purgeable space) and its size. Looked up every
//!   ten seconds at most: it is the slow part.
//! * Uptime: since the Mac started (`kern.boottime`).

use crate::state::SysStats;
use objc2_foundation::{NSArray, NSNumber, NSString, NSURLResourceKey, NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeTotalCapacityKey, NSURL};
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const HOST_CPU_LOAD_INFO: i32 = 3;
const HOST_VM_INFO64: i32 = 4;

/// `vm_statistics64` (mach/vm_statistics.h).
#[repr(C, align(8))]
#[derive(Default)]
struct VmStatistics64 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
}

#[repr(C)]
struct Timeval {
    tv_sec: i64,
    tv_usec: i32,
}

extern "C" {
    fn mach_host_self() -> u32;
    fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    fn host_statistics64(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    fn sysctlbyname(name: *const c_char, oldp: *mut c_void, oldlenp: *mut usize, newp: *mut c_void, newlen: usize) -> i32;
}

/// The previous CPU ticks: user, system, idle, nice.
static PREV_CPU: Mutex<Option<[u32; 4]>> = Mutex::new(None);
/// The disk's (free, total) bytes and when they were read.
static DISK: Mutex<Option<(u64, u64, Instant)>> = Mutex::new(None);
static WARNED: AtomicBool = AtomicBool::new(false);

fn warn_once(what: &str) {
    if !WARNED.swap(true, Ordering::Relaxed) {
        pwarn!("sysstats: {what} unavailable");
    }
}

pub fn sample() -> SysStats {
    let mut s = SysStats { valid: true, ..SysStats::default() };
    match cpu_ticks() {
        Some(now) => {
            let mut prev = PREV_CPU.lock().unwrap_or_else(|p| p.into_inner());
            s.cpu = prev.map(|p| cpu_load(p, now)).unwrap_or(0.0);
            *prev = Some(now);
        }
        None => warn_once("CPU load"),
    }
    s.mem_total = sysctl_u64(c"hw.memsize").unwrap_or(0);
    match memory_split() {
        Some(m) => {
            let used = m.app + m.wired + m.compressed;
            s.mem_used = if s.mem_total > 0 { used.min(s.mem_total) } else { used };
        }
        None => warn_once("memory"),
    }
    let (free, total) = disk();
    s.disk_free = free;
    s.disk_total = total;
    s.uptime_s = uptime_s().unwrap_or(0);
    s
}

fn cpu_ticks() -> Option<[u32; 4]> {
    let mut ticks = [0u32; 4];
    let mut count = 4u32;
    let r = unsafe { host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, ticks.as_mut_ptr() as *mut i32, &mut count) };
    (r == 0).then_some(ticks)
}

/// Busy share of the ticks between two samples (counters wrap).
fn cpu_load(prev: [u32; 4], now: [u32; 4]) -> f32 {
    let d: Vec<u64> = (0..4).map(|i| now[i].wrapping_sub(prev[i]) as u64).collect();
    let total = d.iter().sum::<u64>();
    if total == 0 {
        return 0.0;
    }
    let busy = d[0] + d[1] + d[3];
    (busy as f64 / total as f64).clamp(0.0, 1.0) as f32
}

/// Physical memory the way Activity Monitor splits it, in bytes (the System
/// page shows the parts, Today their sum as "Memory Used").
pub(super) struct MemSplit {
    pub app: u64,
    pub wired: u64,
    pub compressed: u64,
    /// Cached files: file-backed and purgeable pages.
    pub cached: u64,
}

pub(super) fn memory_split() -> Option<MemSplit> {
    let vm = vm_stats()?;
    let page = sysctl_u64(c"hw.pagesize").unwrap_or(16384);
    let pages = |n: u32| n as u64 * page;
    Some(MemSplit {
        app: pages(vm.internal_page_count).saturating_sub(pages(vm.purgeable_count)),
        wired: pages(vm.wire_count),
        compressed: pages(vm.compressor_page_count),
        cached: pages(vm.external_page_count) + pages(vm.purgeable_count),
    })
}

fn vm_stats() -> Option<VmStatistics64> {
    let mut vm = VmStatistics64::default();
    let mut count = (std::mem::size_of::<VmStatistics64>() / 4) as u32;
    let r = unsafe { host_statistics64(mach_host_self(), HOST_VM_INFO64, &mut vm as *mut VmStatistics64 as *mut i32, &mut count) };
    (r == 0).then_some(vm)
}

pub(super) fn sysctl_u64(name: &CStr) -> Option<u64> {
    let mut v: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let r = unsafe { sysctlbyname(name.as_ptr(), &mut v as *mut u64 as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    match (r, len) {
        (0, 8) => Some(v),
        (0, 4) => Some(v & 0xffff_ffff),
        _ => None,
    }
}

pub(super) fn uptime_s() -> Option<u64> {
    let mut tv = Timeval { tv_sec: 0, tv_usec: 0 };
    let mut len = std::mem::size_of::<Timeval>();
    let r = unsafe { sysctlbyname(c"kern.boottime".as_ptr(), &mut tv as *mut Timeval as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    if r != 0 || tv.tv_sec <= 0 {
        return None;
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((now - tv.tv_sec).max(0) as u64)
}

fn disk() -> (u64, u64) {
    let mut cache = DISK.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((free, total, at)) = *cache {
        if at.elapsed().as_secs() < 10 {
            return (free, total);
        }
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str("/"));
    let keys: [&NSURLResourceKey; 2] = unsafe { [NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeTotalCapacityKey] };
    let read = url.resourceValuesForKeys_error(&NSArray::from_slice(&keys)).ok().map(|vals| {
        let num = |k: &NSURLResourceKey| vals.objectForKey(k).and_then(|v| v.downcast::<NSNumber>().ok()).map(|n| n.longLongValue().max(0) as u64);
        (num(keys[0]).unwrap_or(0), num(keys[1]).unwrap_or(0))
    });
    let (free, total) = read.unwrap_or((0, 0));
    if total == 0 {
        warn_once("disk space");
    }
    *cache = Some((free, total, Instant::now()));
    (free, total)
}
