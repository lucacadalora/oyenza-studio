//! Readings for the System page (`pages::system`) on the Mac, taken the way
//! Stats takes them: straight from the kernel and IOKit through public calls,
//! with no private framework, no helper and no admin rights.
//!
//! * CPU: each core's user, system, idle and nice ticks
//!   (`host_processor_info`), each core's kind on Apple silicon (the
//!   performance levels' counts, `hw.perflevel*`), the uptime
//!   (`kern.boottime`). macOS gives apps no public speed, temperature or
//!   power reading, so the page shows those as not reported.
//! * Power, on a Mac without a battery while the page is open: the whole
//!   Mac's draw from its own sensor (the SMC's `PSTR`, `smc.rs`), as Stats
//!   reads it. macOS doesn't split it by part.
//! * GPU: the graphics processor's own statistics (IOKit's
//!   `PerformanceStatistics`): device, renderer and tiler use, memory in use.
//! * Memory: Activity Monitor's split (`sysstats::memory_split`), the swap in
//!   use (`vm.swapusage`) and the kernel's own pressure level, which the
//!   page's pressure gauge follows.
//! * Drives: the volumes Finder lists, with the space Finder calls available
//!   (purgeable space included); reads and writes from the disk drivers'
//!   statistics (`IOBlockStorageDriver`).
//! * Network: every Ethernet and Wi-Fi interface's 64-bit byte counters
//!   (`NET_RT_IFLIST2`), and the one macOS sends internet traffic through,
//!   with its name in System Settings, address, hardware address and link speed.
//! * Battery: the power sources (`IOPSCopyPowerSourcesInfo`), the battery's
//!   registry entry (cycles, capacities, voltage, current) and Low Power Mode.
//! * Apps: this account's processes (`proc_listpids`, `proc_pid_rusage`: CPU
//!   time, memory footprint as Activity Monitor counts it, disk bytes),
//!   grouped by `poni_core::sysmon::AppTracker`: every process inside an
//!   app's bundle counts as that app, and the bundle gives the icon. Other
//!   accounts' processes (system daemons, WindowServer) take admin rights to
//!   read, so the lists leave them out.
//!
//! As on Windows (`../../services/sysmon.rs`), the thread keeps the charts'
//! minute while the page is closed (`Settings::system_history`): one light
//! reading a second of the CPU ticks, memory, disk and network counters and
//! the GPU's use. Open, each second also reads the apps and the battery, and
//! every ten seconds the volumes, the battery's details and the route. While
//! the displays sleep or the Mac is locked nothing is read at all. A source
//! that fails is left out of the reading and logged once.

use super::cf::{self, CFTypeRef};
use super::{phonedrop, sysstats};
use crate::events::{AppEvent, EventSink};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSWorkspace, NSWorkspaceScreensDidSleepNotification, NSWorkspaceScreensDidWakeNotification};
use objc2_core_foundation::CFString;
use objc2_foundation::{
    NSArray, NSDistributedNotificationCenter, NSNotification, NSNotificationSuspensionBehavior, NSNumber, NSObject, NSObjectProtocol, NSProcessInfo,
    NSString, NSURLResourceKey, NSURLVolumeAvailableCapacityForImportantUsageKey, NSURLVolumeIsRemovableKey, NSURLVolumeLocalizedNameKey,
    NSURLVolumeTotalCapacityKey, NSURL,
};
use poni_core::power::PowerReading;
use poni_core::sysmon::{
    app_bundle, bundle_name, core_kinds, core_split, level_pressure, pack_mwh, parse_if_list, top_cpu, top_io, top_memory, AppTracker, AppUse,
    BatteryReading, CoreKind, CoreLoad, CoreTimes, CpuReading, DiskActivity, Drive, GpuReading, IfStats, MemoryReading, Monitor, NetReading,
    PressureLevel, ProcSample, Reading, TOP_APPS,
};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CStr};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// One reading a second, open or not.
const PERIOD: Duration = Duration::from_secs(1);
/// After the page opens, the first full reading needs a second process list to compare with; this soon.
const FIRST: Duration = Duration::from_millis(350);
/// Volumes, the route and the battery's details change rarely: look again this often.
const SLOW: Duration = Duration::from_secs(10);
/// Readings in a row the GPU may miss before it is looked for again.
const GPU_MISSES: u32 = 3;
/// Readings of each kind whose cost goes to poni.log.
const LOGGED_READINGS: u32 = 3;

const PROCESSOR_CPU_LOAD_INFO: i32 = 2;
/// `sysctl` names for the interface list.
const CTL_NET: i32 = 4;
const PF_ROUTE: i32 = 17;
const NET_RT_IFLIST2: i32 = 6;
const IFF_UP: u32 = 0x1;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
/// Ethernet, Wi-Fi included.
const IFT_ETHER: u8 = 6;
/// `proc_listpids`: the processes of one (effective) user.
const PROC_UID_ONLY: u32 = 4;
const RUSAGE_INFO_V2: i32 = 2;
/// `PROC_PIDPATHINFO_MAXSIZE`.
const PATH_BYTES: usize = 4096;
const MNT_RDONLY: u32 = 0x1;
const MNT_LOCAL: u32 = 0x1000;
/// Not a place for the person's files (the system's own volumes).
const MNT_DONTBROWSE: u32 = 0x10_0000;
const MNT_NOWAIT: i32 = 2;
const CLOCK_THREAD_CPUTIME_ID: u32 = 16;
/// How far up the registry a GPU's name and memory are looked for.
const PARENT_LEVELS: usize = 3;

#[repr(C)]
struct MachTimebase {
    numer: u32,
    denom: u32,
}

/// `struct xsw_usage` (`vm.swapusage`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XswUsage {
    total: u64,
    avail: u64,
    used: u64,
    pagesize: u32,
    encrypted: i32,
}

/// `struct rusage_info_v2` (sys/resource.h). Times are in Mach time units.
#[repr(C)]
struct RusageInfoV2 {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    pkg_idle_wkups: u64,
    interrupt_wkups: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    phys_footprint: u64,
    proc_start_abstime: u64,
    proc_exit_abstime: u64,
    child_user_time: u64,
    child_system_time: u64,
    child_pkg_idle_wkups: u64,
    child_interrupt_wkups: u64,
    child_pageins: u64,
    child_elapsed_abstime: u64,
    diskio_bytesread: u64,
    diskio_byteswritten: u64,
}

/// `struct statfs` with 64-bit inodes (sys/mount.h).
#[repr(C)]
struct StatFs {
    bsize: u32,
    iosize: i32,
    blocks: u64,
    bfree: u64,
    bavail: u64,
    files: u64,
    ffree: u64,
    fsid: [i32; 2],
    owner: u32,
    kind: u32,
    flags: u32,
    fssubtype: u32,
    fstypename: [c_char; 16],
    mntonname: [c_char; 1024],
    mntfromname: [c_char; 1024],
    flags_ext: u32,
    reserved: [u32; 7],
}

const _: () = assert!(std::mem::size_of::<RusageInfoV2>() == 160 && std::mem::size_of::<StatFs>() == 2168 && std::mem::size_of::<XswUsage>() == 32);

extern "C" {
    fn mach_host_self() -> u32;
    static mach_task_self_: u32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
    fn host_processor_info(host: u32, flavor: i32, count: *mut u32, info: *mut *mut i32, info_count: *mut u32) -> i32;
    fn vm_deallocate(task: u32, address: usize, size: usize) -> i32;
    fn mach_timebase_info(info: *mut MachTimebase) -> i32;
    fn sysctl(name: *mut i32, namelen: u32, oldp: *mut c_void, oldlenp: *mut usize, newp: *mut c_void, newlen: usize) -> i32;
    fn sysctlbyname(name: *const c_char, oldp: *mut c_void, oldlenp: *mut usize, newp: *mut c_void, newlen: usize) -> i32;
    // Intel builds pick the 64-bit inode `statfs` by its suffixed name, as the SDK headers do.
    #[cfg_attr(target_arch = "x86_64", link_name = "getfsstat$INODE64")]
    fn getfsstat(buf: *mut StatFs, bufsize: i32, flags: i32) -> i32;
    fn proc_listpids(kind: u32, typeinfo: u32, buffer: *mut c_void, buffersize: i32) -> i32;
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut c_void) -> i32;
    fn proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
    fn proc_name(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
    fn geteuid() -> u32;
    fn clock_gettime_nsec_np(clock: u32) -> u64;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    fn IOServiceGetMatchingServices(main_port: u32, matching: CFTypeRef, existing: *mut u32) -> i32;
    fn IOServiceGetMatchingService(main_port: u32, matching: CFTypeRef) -> u32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IOObjectGetClass(object: u32, name: *mut c_char) -> i32;
    fn IORegistryEntryGetRegistryEntryID(entry: u32, id: *mut u64) -> i32;
    fn IORegistryEntryCreateCFProperty(entry: u32, key: CFTypeRef, allocator: CFTypeRef, options: u32) -> CFTypeRef;
    fn IORegistryEntryGetParentEntry(entry: u32, plane: *const c_char, parent: *mut u32) -> i32;
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(blob: CFTypeRef) -> CFTypeRef;
}

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCNetworkInterfaceCopyAll() -> CFTypeRef;
    fn SCNetworkInterfaceGetBSDName(interface: CFTypeRef) -> CFTypeRef;
    fn SCNetworkInterfaceGetLocalizedDisplayName(interface: CFTypeRef) -> CFTypeRef;
}

// ---------------------------------------------------------------- the thread

pub struct SysMon {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

enum Msg {
    /// The page opened (true) or closed.
    Open(bool),
    /// Keep the history while the page is closed.
    Background(bool),
    /// Nobody can see the screen (the displays sleep, or the Mac is locked).
    Away(bool),
    Quit,
}

impl SysMon {
    pub fn start(sink: EventSink, background: bool) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new().name("poni-sysmon".into()).spawn(move || worker(sink, rx, background))?;
        Ok(Self { tx, thread: Some(thread) })
    }

    pub fn open(&self, on: bool) {
        let _ = self.tx.send(Msg::Open(on));
    }

    pub fn background(&self, on: bool) {
        let _ = self.tx.send(Msg::Background(on));
    }

    pub fn away(&self, on: bool) {
        let _ = self.tx.send(Msg::Away(on));
    }
}

impl Drop for SysMon {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
        if let Some(t) = self.thread.take() {
            // The thread answers between readings; a reading takes milliseconds.
            let _ = t.join();
        }
    }
}

fn worker(sink: EventSink, rx: Receiver<Msg>, mut background: bool) {
    let mut open = false;
    let mut away = false;
    let mut sampler: Option<Sampler> = background.then(Sampler::fresh);
    let mut monitor = Monitor::default();
    let mut due = Instant::now() + FIRST;
    loop {
        let first = if sampler.is_some() {
            match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            }
        };
        let Some(first) = first else {
            // A reading: light while closed, full while open.
            if let Some(s) = sampler.as_mut() {
                monitor.push(read(s, open));
                if open {
                    sink.send(AppEvent::System(Box::new(monitor.clone())));
                }
            }
            let now = Instant::now();
            due += PERIOD;
            if due <= now {
                due = now + PERIOD;
            }
            continue;
        };
        // Everything queued counts, the latest of each kind winning (and Quit
        // above all): flipping tabs queues several, and each would cost a sampler.
        let (mut quit, mut want_open, mut want_bg, mut want_away) = (false, None, None, None);
        for m in std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match m {
                Msg::Quit => quit = true,
                Msg::Open(on) => want_open = Some(on),
                Msg::Background(on) => want_bg = Some(on),
                Msg::Away(on) => want_away = Some(on),
            }
        }
        if quit {
            break;
        }
        if let Some(b) = want_bg {
            background = b;
        }
        if let Some(o) = want_open {
            open = o;
        }
        if let Some(a) = want_away {
            if a != away && (background || open) {
                pinfo!("system readings: {}", if a { "resting while nobody can see the screen" } else { "reading again" });
            }
            away = a;
        }
        // Not reading: no wake-ups at all until a message comes, and the kept
        // minute goes, so a gap never shows as part of it.
        if away || (!open && !background) {
            sampler = None;
            monitor.clear();
            continue;
        }
        let fresh = sampler.is_none();
        if fresh {
            sampler = Some(Sampler::fresh());
            monitor.clear();
            due = Instant::now() + FIRST;
        }
        // The page opened, or reads again after a rest while open.
        if want_open == Some(true) || (fresh && open) {
            if let Some(s) = sampler.as_mut() {
                // The kept minute shows at once; the process list to measure
                // apps from comes after, and the first full reading shortly.
                if monitor.latest.is_some() {
                    sink.send(AppEvent::System(Box::new(monitor.clone())));
                }
                s.prime_apps();
                due = Instant::now() + FIRST;
            }
        }
    }
}

// What the readings cost this thread: [light, full] × (readings, CPU nanoseconds).
static COST: [[AtomicU64; 2]; 2] = [[AtomicU64::new(0), AtomicU64::new(0)], [AtomicU64::new(0), AtomicU64::new(0)]];
static LOGGED: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// (readings, average CPU time) of the light and the full readings so far.
pub fn cost() -> [(u64, Duration); 2] {
    [0, 1].map(|k| {
        let n = COST[k][0].load(Ordering::Relaxed);
        (n, Duration::from_nanos(COST[k][1].load(Ordering::Relaxed).checked_div(n).unwrap_or(0)))
    })
}

fn thread_cpu_ns() -> u64 {
    unsafe { clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) }
}

/// One reading, its CPU time counted (and for the first few, logged with
/// where the time went).
fn read(s: &mut Sampler, full: bool) -> Reading {
    let k = usize::from(full);
    let log = LOGGED[k].load(Ordering::Relaxed) < LOGGED_READINGS;
    let mut laps = Laps::new(log);
    let (t0, c0) = (Instant::now(), thread_cpu_ns());
    let r = autoreleasepool(|_| s.sample(full, &mut laps));
    let (wall, cpu) = (t0.elapsed(), thread_cpu_ns().saturating_sub(c0));
    COST[k][0].fetch_add(1, Ordering::Relaxed);
    COST[k][1].fetch_add(cpu, Ordering::Relaxed);
    if log {
        LOGGED[k].fetch_add(1, Ordering::Relaxed);
        let kind = if full { "full" } else { "light" };
        pinfo!("system readings: {kind} reading took {:.2} ms of CPU, {:.2} ms in all ({})", cpu as f64 / 1e6, wall.as_secs_f64() * 1e3, laps.line());
    }
    r
}

/// Where one reading's time went (the first few readings only).
struct Laps {
    at: Option<Instant>,
    parts: Vec<(&'static str, Duration)>,
    processes: usize,
}

impl Laps {
    fn new(on: bool) -> Laps {
        Laps { at: on.then(Instant::now), parts: Vec::new(), processes: 0 }
    }

    fn lap(&mut self, name: &'static str) {
        if let Some(at) = self.at {
            let now = Instant::now();
            self.parts.push((name, now - at));
            self.at = Some(now);
        }
    }

    fn line(&self) -> String {
        let mut s: Vec<String> = self.parts.iter().map(|(n, d)| format!("{n} {:.2} ms", d.as_secs_f64() * 1e3)).collect();
        if self.processes > 0 {
            s.push(format!("{} processes", self.processes));
        }
        s.join(", ")
    }
}

// Each failing source is logged once per run, not once a second.
static WARNED: AtomicU32 = AtomicU32::new(0);

fn warn_once(bit: u32, what: &str) {
    if WARNED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        pwarn!("sysmon: {what}");
    }
}

// ---------------------------------------------------------------- the sampler

/// Reads everything; created when the history or the page needs it.
struct Sampler {
    started: Instant,
    host: u32,
    /// Mach time units to nanoseconds (Apple silicon counts process time in 24 MHz ticks).
    timebase: (u64, u64),
    ram: u64,
    logical: u32,
    kinds: Vec<CoreKind>,
    /// Each core's (user, system, idle, nice) ticks at the last reading.
    ticks: Option<Vec<[u32; 4]>>,
    apps: AppTracker,
    pids: Vec<i32>,
    /// Processes in the last list.
    listed: usize,
    /// pid → (start time, app) of each process seen, and app → (icon path, name).
    procs: HashMap<i32, (u64, String)>,
    names: HashMap<String, (Option<String>, String)>,
    gpu: Option<Gpu>,
    gpu_misses: u32,
    /// The disks' drivers by registry id, and each one's (read, written) bytes at the last reading.
    disks: Vec<(u64, Io)>,
    disk_prev: Option<(Instant, HashMap<u64, (u64, u64)>)>,
    if_buf: Vec<u8>,
    /// Each counted interface's (received, sent) bytes at the last reading, by name.
    net_prev: Option<(Instant, HashMap<String, (u64, u64)>)>,
    /// The interface internet traffic goes through, each interface's IPv4
    /// address, and their names in System Settings ("Wi-Fi").
    routed: Option<String>,
    ipv4s: Vec<(String, Ipv4Addr)>,
    labels: HashMap<String, String>,
    /// The volumes at the last full reading: light readings carry them, so
    /// the page shows them the moment it opens.
    drive_list: Vec<Drive>,
    /// The battery at the last full reading (light readings carry it too).
    battery: Option<BatteryReading>,
    details: Option<BatteryDetails>,
    /// (plugged in, charging) at the last reading: a change refreshes the details.
    power: Option<(bool, bool)>,
    slow_at: Option<Instant>,
    /// The SMC, open while the page is (a Mac without a battery only).
    smc: Option<super::smc::Smc>,
    smc_looked: bool,
}

impl Sampler {
    /// A new sampler (inside an autorelease pool: it asks Foundation too).
    fn fresh() -> Sampler {
        autoreleasepool(|_| Sampler::new())
    }

    fn new() -> Sampler {
        let mut tb = MachTimebase { numer: 1, denom: 1 };
        unsafe { mach_timebase_info(&mut tb) };
        let ram = sysstats::sysctl_u64(c"hw.memsize").unwrap_or(0);
        let mut s = Sampler {
            started: Instant::now(),
            host: unsafe { mach_host_self() },
            timebase: (u64::from(tb.numer.max(1)), u64::from(tb.denom.max(1))),
            ram,
            logical: 1,
            kinds: Vec::new(),
            ticks: None,
            apps: AppTracker::default(),
            pids: Vec::new(),
            listed: 0,
            procs: HashMap::new(),
            names: HashMap::new(),
            gpu: Gpu::open(ram),
            gpu_misses: 0,
            disks: block_drivers(),
            disk_prev: None,
            if_buf: Vec::new(),
            net_prev: None,
            routed: None,
            ipv4s: Vec::new(),
            labels: HashMap::new(),
            drive_list: Vec::new(),
            battery: battery_now(None),
            details: None,
            power: None,
            slow_at: None,
            smc: None,
            smc_looked: false,
        };
        // Baselines for everything measured between two readings.
        s.ticks = s.core_ticks();
        let cores = s.ticks.as_ref().map_or(0, |t| t.len());
        s.logical = if cores > 0 { cores as u32 } else { sysstats::sysctl_u64(c"hw.logicalcpu").unwrap_or(1).max(1) as u32 };
        s.kinds = mac_core_kinds(s.logical as usize);
        let _ = s.disk_activity();
        s.refresh_route();
        let _ = s.net_reading();
        s
    }

    /// A fresh process list to measure the next full reading's app CPU and disk use against.
    fn prime_apps(&mut self) {
        let procs = self.process_list();
        self.apps.update(self.started.elapsed().as_secs_f64(), self.logical, &procs);
    }

    /// A reading: `full` while the page is open, else light.
    fn sample(&mut self, full: bool, laps: &mut Laps) -> Reading {
        if full && self.slow_at.is_none_or(|t| t.elapsed() >= SLOW) {
            self.slow_at = Some(Instant::now());
            self.drive_list = volumes();
            laps.lap("volumes");
            self.refresh_route();
            self.details = battery_details();
            // A GPU that went away or came back (an eGPU), disks added or removed.
            if self.gpu.is_none() {
                self.gpu = Gpu::open(self.ram);
            }
            self.disks = block_drivers();
            self.prune();
            laps.lap("route, battery details, GPU and disks");
        }
        let cpu = self.cpu_reading();
        laps.lap("CPU");
        let memory = self.memory();
        laps.lap("memory");
        let disk = self.disk_activity();
        laps.lap("disks");
        let network = self.net_reading();
        laps.lap("network");
        let gpu = self.gpu_reading();
        laps.lap("GPU");
        let mut top = (Vec::new(), Vec::new(), Vec::new());
        if full {
            let procs = self.process_list();
            laps.processes = procs.len();
            let apps = self.apps.update(self.started.elapsed().as_secs_f64(), self.logical, &procs);
            top = (self.named(top_cpu(&apps, TOP_APPS)), self.named(top_memory(&apps, TOP_APPS)), self.named(top_io(&apps, TOP_APPS)));
            laps.lap("apps");
            self.battery = self.battery_reading();
            laps.lap("battery");
        }
        // The whole Mac's draw while the page is open, on a Mac without a
        // battery (a MacBook's Battery module says what it draws).
        if !full {
            self.smc = None;
            self.smc_looked = false;
        }
        let power = (full && self.battery.is_none()).then(|| self.power_reading()).flatten();
        let (top_cpu, top_memory, top_io) = top;
        Reading { cpu, gpu, memory, drives: self.drive_list.clone(), disk, network, battery: self.battery.clone(), power, top_cpu, top_memory, top_io }
    }

    /// What the SMC says the whole Mac draws, opened with the page's first
    /// reading. A Mac without the key (Intel, virtual) has none.
    fn power_reading(&mut self) -> Option<PowerReading> {
        if !self.smc_looked {
            self.smc_looked = true;
            self.smc = super::smc::Smc::open();
            let w = self.smc.as_ref().and_then(|s| s.float(super::smc::SYSTEM_TOTAL));
            pinfo!("system: the SMC's whole-Mac power: {}", w.map_or("none".to_string(), |w| format!("{w:.1} W")));
        }
        let w = self.smc.as_ref()?.float(super::smc::SYSTEM_TOTAL).filter(|w| w.is_finite() && *w > 0.0)?;
        Some(PowerReading { total_w: w, cpu_w: None, gpu_w: None, rest_w: w, psu: None })
    }

    // ------------------------------------------------------------ CPU

    /// Each core's (user, system, idle, nice) ticks since the Mac started.
    fn core_ticks(&self) -> Option<Vec<[u32; 4]>> {
        let (mut count, mut info, mut len) = (0u32, std::ptr::null_mut::<i32>(), 0u32);
        let r = unsafe { host_processor_info(self.host, PROCESSOR_CPU_LOAD_INFO, &mut count, &mut info, &mut len) };
        if r != 0 || info.is_null() {
            warn_once(1, &format!("per-core ticks unavailable ({r})"));
            return None;
        }
        // SAFETY: the kernel handed over `len` integers, four tick counters a processor.
        let ints = unsafe { std::slice::from_raw_parts(info as *const u32, len as usize) };
        let cores = ints.chunks_exact(4).take(count as usize).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        unsafe { vm_deallocate(mach_task_self_, info as usize, len as usize * 4) };
        Some(cores)
    }

    /// Every core's load since the last reading.
    fn cpu_reading(&mut self) -> Option<CpuReading> {
        let now = self.core_ticks()?;
        let prev = self.ticks.replace(now)?;
        let now = self.ticks.as_ref()?;
        if prev.len() != now.len() {
            return None;
        }
        // The ticks in between (the counters are 32-bit and wrap), counted the
        // way Windows counts its times: kernel time includes idle time.
        let span: Vec<CoreTimes> = prev
            .iter()
            .zip(now)
            .map(|(a, b)| {
                let d = |k: usize| u64::from(b[k].wrapping_sub(a[k]));
                CoreTimes { idle: d(2), kernel: d(1) + d(2), user: d(0) + d(3), dpc: 0, interrupt: 0 }
            })
            .collect();
        let s = core_split(&vec![CoreTimes::default(); span.len()], &span)?;
        let cores = s.cores.iter().enumerate().map(|(i, load)| CoreLoad { load: *load, kind: self.kinds.get(i).copied().unwrap_or_default() }).collect();
        Some(CpuReading {
            load: s.load,
            user: s.user,
            kernel: s.kernel,
            cores,
            logical: self.logical,
            uptime_s: sysstats::uptime_s().unwrap_or(0),
            ..CpuReading::default()
        })
    }

    // ------------------------------------------------------------ memory

    /// Activity Monitor's split, the swap, and the pressure as the kernel rates it.
    fn memory(&self) -> Option<MemoryReading> {
        let total = self.ram;
        let m = sysstats::memory_split().filter(|_| total > 0);
        let Some(m) = m else {
            warn_once(2, "memory statistics unavailable");
            return None;
        };
        let used = (m.app + m.wired + m.compressed).min(total);
        let cached = m.cached.min(total - used);
        let swap = swap_usage();
        // How full memory is by the kernel's own count (what `memory_pressure`
        // prints as free), else Memory Used; the level picks the gauge's zone.
        let full = sysstats::sysctl_u64(c"kern.memorystatus_level").filter(|v| *v <= 100).map_or(used as f32 / total as f32, |free| 1.0 - free as f32 / 100.0);
        let pressure = sysstats::sysctl_u64(c"kern.memorystatus_vm_pressure_level").and_then(PressureLevel::from_kernel).map(|l| level_pressure(l, full));
        Some(MemoryReading {
            total,
            used,
            committed: pressure.map_or(0, |p| (f64::from(p) * total as f64).round() as u64),
            commit_limit: if pressure.is_some() { total } else { 0 },
            cached,
            wired: m.wired,
            compressed: m.compressed,
            free: total - used - cached,
            page_used: swap.map_or(0, |s| s.used),
            page_total: swap.map_or(0, |s| s.total),
        })
    }

    // ------------------------------------------------------------ disks

    /// Each disk driver's (read, written) bytes since it started.
    fn disk_counts(&self) -> Option<HashMap<u64, (u64, u64)>> {
        let mut out = HashMap::with_capacity(self.disks.len());
        for (id, d) in &self.disks {
            let Some(st) = prop(d.0, "Statistics") else { continue };
            if let (Some(r), Some(w)) = (cf::dict_i64(st.0, "Bytes (Read)"), cf::dict_i64(st.0, "Bytes (Write)")) {
                out.insert(*id, (r.max(0) as u64, w.max(0) as u64));
            }
        }
        if out.is_empty() {
            warn_once(4, "disk statistics unavailable");
            return None;
        }
        Some(out)
    }

    /// Reads and writes of all disks since the last reading. Only disks seen
    /// last time count: one just attached brings everything since it started.
    fn disk_activity(&mut self) -> Option<DiskActivity> {
        let now = self.disk_counts()?;
        let at = Instant::now();
        let act = self.disk_prev.as_ref().and_then(|(t, before)| {
            let dt = at.duration_since(*t).as_secs_f64();
            (dt > 0.0).then(|| {
                let (mut r, mut w) = (0u64, 0u64);
                for (id, (r1, w1)) in &now {
                    if let Some((r0, w0)) = before.get(id) {
                        r = r.saturating_add(r1.saturating_sub(*r0));
                        w = w.saturating_add(w1.saturating_sub(*w0));
                    }
                }
                DiskActivity { read_bps: r as f64 / dt, write_bps: w as f64 / dt, active: None }
            })
        });
        self.disk_prev = Some((at, now));
        act
    }

    // ------------------------------------------------------------ network

    /// The kernel's interface list (one `sysctl`; the buffer is kept).
    fn interfaces(&mut self) -> Option<Vec<IfStats>> {
        let mut mib = [CTL_NET, PF_ROUTE, 0, 0, NET_RT_IFLIST2, 0];
        for _ in 0..3 {
            if !self.if_buf.is_empty() {
                let mut got = self.if_buf.len();
                let r = unsafe { sysctl(mib.as_mut_ptr(), 6, self.if_buf.as_mut_ptr().cast(), &mut got, std::ptr::null_mut(), 0) };
                if r == 0 {
                    return Some(parse_if_list(&self.if_buf[..got.min(self.if_buf.len())]));
                }
            }
            // The first time, or the list outgrew the buffer: ask how big it is.
            let mut need = 0usize;
            if unsafe { sysctl(mib.as_mut_ptr(), 6, std::ptr::null_mut(), &mut need, std::ptr::null_mut(), 0) } != 0 || need == 0 {
                warn_once(8, "network counters unavailable");
                return None;
            }
            self.if_buf = vec![0u8; need + 2048];
        }
        None
    }

    /// Totals and speeds over the connected Ethernet and Wi-Fi interfaces;
    /// the one shown is the route's, else the busiest.
    fn net_reading(&mut self) -> Option<NetReading> {
        let list = self.interfaces()?;
        let up: Vec<&IfStats> = list.iter().filter(|i| counted(i)).collect();
        let mut n = NetReading { online: !up.is_empty(), ..NetReading::default() };
        for i in &up {
            n.received = n.received.saturating_add(i.received);
            n.sent = n.sent.saturating_add(i.sent);
        }
        let now = Instant::now();
        // Only interfaces seen last time count: one that just came up brings
        // its whole total, which isn't this second's traffic.
        if let Some((at, before)) = &self.net_prev {
            let dt = now.duration_since(*at).as_secs_f64();
            if dt > 0.0 {
                let (mut rx, mut tx) = (0u64, 0u64);
                for i in &up {
                    if let Some((r0, t0)) = before.get(&i.name) {
                        rx = rx.saturating_add(i.received.saturating_sub(*r0));
                        tx = tx.saturating_add(i.sent.saturating_sub(*t0));
                    }
                }
                n.down_bps = rx as f64 / dt;
                n.up_bps = tx as f64 / dt;
            }
        }
        let routed = up.iter().find(|i| self.routed.as_deref() == Some(i.name.as_str()));
        if let Some(i) = routed.or_else(|| up.iter().max_by_key(|i| i.received.saturating_add(i.sent))) {
            n.adapter = self.labels.get(&i.name).cloned().unwrap_or_else(|| i.name.clone());
            n.mac = i.mac.clone();
            n.link_bps = i.baud;
            n.ipv4 = self.ipv4s.iter().find(|(name, _)| *name == i.name).map(|(_, ip)| ip.to_string());
        }
        self.net_prev = Some((now, up.iter().map(|i| (i.name.clone(), (i.received, i.sent))).collect()));
        Some(n)
    }

    /// Where internet traffic goes (the address macOS would send it from),
    /// each interface's address, and the interfaces' names in System Settings.
    fn refresh_route(&mut self) {
        self.ipv4s = phonedrop::interface_ipv4s();
        let ip = phonedrop::route_ipv4();
        self.routed = ip.and_then(|ip| self.ipv4s.iter().find(|(_, a)| *a == ip).map(|(n, _)| n.clone()));
        if self.labels.is_empty() || self.ipv4s.iter().any(|(n, _)| n.starts_with("en") && !self.labels.contains_key(n)) {
            self.labels = interface_labels();
        }
    }

    // ------------------------------------------------------------ GPU

    fn gpu_reading(&mut self) -> Option<GpuReading> {
        let r = self.gpu.as_ref()?.reading();
        if r.is_some() {
            self.gpu_misses = 0;
        } else {
            self.gpu_misses += 1;
            if self.gpu_misses >= GPU_MISSES {
                warn_once(16, "GPU stopped answering; looking for it again with the page");
                self.gpu = None;
            }
        }
        r
    }

    // ------------------------------------------------------------ apps

    /// This account's processes: CPU time, memory footprint and disk bytes.
    fn process_list(&mut self) -> Vec<ProcSample> {
        let uid = unsafe { geteuid() };
        let need = unsafe { proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
        if need <= 0 {
            warn_once(32, "process list unavailable");
            return Vec::new();
        }
        let room = need as usize / 4 + 64;
        if self.pids.len() < room {
            self.pids.resize(room, 0);
        }
        let got = unsafe { proc_listpids(PROC_UID_ONLY, uid, self.pids.as_mut_ptr().cast(), (self.pids.len() * 4) as i32) };
        self.listed = (got.max(0) as usize / 4).min(self.pids.len());
        let mut out = Vec::with_capacity(self.listed);
        for k in 0..self.listed {
            let pid = self.pids[k];
            if pid <= 0 {
                continue;
            }
            // SAFETY: all zeroes is a valid value of this plain C struct.
            let mut ri: RusageInfoV2 = unsafe { std::mem::zeroed() };
            if unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, (&mut ri as *mut RusageInfoV2).cast()) } != 0 {
                continue;
            }
            let image = self.app_key(pid, ri.proc_start_abstime);
            let cpu_time = self.to_100ns(ri.user_time.saturating_add(ri.system_time));
            let io = ri.diskio_bytesread.saturating_add(ri.diskio_byteswritten);
            out.push(ProcSample { pid: pid as u64, image, cpu_time, memory: ri.phys_footprint, io });
        }
        out
    }

    /// Mach time units to the 100 ns ticks `AppTracker` counts in.
    fn to_100ns(&self, t: u64) -> u64 {
        (u128::from(t) * u128::from(self.timebase.0) / u128::from(self.timebase.1) / 100) as u64
    }

    /// The app a process belongs to (the key it is grouped by), looked up once
    /// a process: the app bundle it sits in, else its file's name.
    fn app_key(&mut self, pid: i32, start: u64) -> String {
        if let Some((s, key)) = self.procs.get(&pid) {
            if *s == start {
                return key.clone();
            }
        }
        let path = process_path(pid);
        let (key, icon, name) = match path.as_deref().and_then(app_bundle) {
            Some(bundle) => (bundle.to_lowercase(), Some(bundle.to_string()), bundle_name(bundle).to_string()),
            None => match &path {
                Some(p) => {
                    let file = p.rsplit('/').next().unwrap_or(p);
                    (file.to_lowercase(), Some(p.clone()), file.to_string())
                }
                None => {
                    let n = process_name(pid).unwrap_or_default();
                    (n.to_lowercase(), None, n)
                }
            },
        };
        self.names.entry(key.clone()).or_insert((icon, name));
        self.procs.insert(pid, (start, key.clone()));
        key
    }

    /// Give each top app its name and the path its icon comes from.
    fn named(&self, apps: Vec<AppUse>) -> Vec<AppUse> {
        apps.into_iter()
            .map(|mut a| {
                if let Some((path, name)) = self.names.get(&a.image) {
                    a.path = path.clone();
                    a.name = name.clone();
                }
                a
            })
            .collect()
    }

    /// Forget processes that have ended, and apps none of whose processes run.
    fn prune(&mut self) {
        let live: HashSet<i32> = self.pids[..self.listed].iter().copied().collect();
        self.procs.retain(|pid, _| live.contains(pid));
        let keys: HashSet<&String> = self.procs.values().map(|(_, k)| k).collect();
        self.names.retain(|k, _| keys.contains(k));
    }

    // ------------------------------------------------------------ battery

    /// The battery now; plugging in or out refreshes its details at once
    /// instead of at the next slow refresh.
    fn battery_reading(&mut self) -> Option<BatteryReading> {
        let mut r = battery_now(self.details.as_ref())?;
        let state = (r.on_ac, r.charging);
        if self.power.is_some_and(|p| p != state) {
            self.details = battery_details();
            r = battery_now(self.details.as_ref())?;
        }
        self.power = Some(state);
        Some(r)
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        // mach_host_self() handed this sampler a send right of its own.
        unsafe { mach_port_deallocate(mach_task_self_, self.host) };
    }
}

// ---------------------------------------------------------------- IOKit and Core Foundation

/// A Core Foundation object this file owns (released when dropped).
struct Cf(CFTypeRef);

impl Cf {
    fn new(p: CFTypeRef) -> Option<Cf> {
        (!p.is_null()).then(|| Cf(p))
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { cf::CFRelease(self.0) };
    }
}

/// An IOKit object this file holds (released when dropped).
struct Io(u32);

impl Drop for Io {
    fn drop(&mut self) {
        unsafe { IOObjectRelease(self.0) };
    }
}

fn key_ptr(k: &CFString) -> CFTypeRef {
    (k as *const CFString).cast()
}

/// A registry entry's property.
fn prop(entry: u32, key: &str) -> Option<Cf> {
    let k = cf::key(key);
    Cf::new(unsafe { IORegistryEntryCreateCFProperty(entry, key_ptr(&k), std::ptr::null(), 0) })
}

/// A property of the entry or of one of its nearest parents (a graphics
/// card's PCI device names it), never the Mac's own (whose "model" is the
/// Mac's model number).
fn search(entry: u32, key: &str) -> Option<Cf> {
    if let Some(v) = prop(entry, key) {
        return Some(v);
    }
    let mut at: Option<Io> = None;
    for _ in 0..PARENT_LEVELS {
        let mut parent = 0u32;
        let from = at.as_ref().map_or(entry, |e| e.0);
        if unsafe { IORegistryEntryGetParentEntry(from, c"IOService".as_ptr(), &mut parent) } != 0 || parent == 0 {
            return None;
        }
        let parent = Io(parent);
        if class_name(parent.0).contains("PlatformExpert") {
            return None;
        }
        if let Some(v) = prop(parent.0, key) {
            return Some(v);
        }
        at = Some(parent);
    }
    None
}

/// Every registry entry of an IOKit class.
fn services(class: &CStr) -> Vec<Io> {
    let matching = unsafe { IOServiceMatching(class.as_ptr()) };
    let mut it = 0u32;
    // The call takes the matching dictionary over.
    if matching.is_null() || unsafe { IOServiceGetMatchingServices(0, matching, &mut it) } != 0 {
        return Vec::new();
    }
    let it = Io(it);
    std::iter::from_fn(|| {
        let e = unsafe { IOIteratorNext(it.0) };
        (e != 0).then(|| Io(e))
    })
    .collect()
}

fn class_name(entry: u32) -> String {
    let mut name = [0 as c_char; 128];
    if unsafe { IOObjectGetClass(entry, name.as_mut_ptr()) } != 0 {
        return String::new();
    }
    unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- CPU kinds

/// Each logical processor's kind, in `host_processor_info`'s order, from
/// the performance levels' counts (`hw.perflevel0` is the performance
/// cores, `hw.perflevel1` the efficiency cores): Apple numbers the
/// efficiency cores first on its chips (an Ultra, two chips in one, repeats
/// the pattern per chip, which this reads as one). One kind on Intel.
fn mac_core_kinds(n: usize) -> Vec<CoreKind> {
    match level_classes(n) {
        Some(classes) => core_kinds(&classes),
        None => vec![CoreKind::Any; n],
    }
}

/// Efficiency classes (1 performance, 0 efficiency), efficiency cores first.
fn level_classes(n: usize) -> Option<Vec<u8>> {
    if sysstats::sysctl_u64(c"hw.nperflevels")? != 2 {
        return None;
    }
    let p = sysstats::sysctl_u64(c"hw.perflevel0.logicalcpu")? as usize;
    let e = sysstats::sysctl_u64(c"hw.perflevel1.logicalcpu")? as usize;
    (p + e == n && p > 0 && e > 0).then(|| [vec![0u8; e], vec![1u8; p]].concat())
}

// ---------------------------------------------------------------- memory

fn swap_usage() -> Option<XswUsage> {
    let mut s = XswUsage::default();
    let mut len = std::mem::size_of::<XswUsage>();
    let r = unsafe { sysctlbyname(c"vm.swapusage".as_ptr(), (&mut s as *mut XswUsage).cast(), &mut len, std::ptr::null_mut(), 0) };
    (r == 0).then_some(s)
}

// ---------------------------------------------------------------- GPU

/// The Mac's graphics processor, read through its accelerator's statistics.
struct Gpu {
    entry: Io,
    name: String,
    integrated: bool,
    mem_total: u64,
}

impl Gpu {
    /// Apple silicon's GPU; else a graphics card over the processor's own Intel graphics.
    fn open(ram: u64) -> Option<Gpu> {
        let mut best: Option<(u8, Gpu)> = None;
        for e in services(c"IOAccelerator") {
            if prop(e.0, "PerformanceStatistics").and_then(|s| device_use(s.0)).is_none() {
                continue;
            }
            let class = class_name(e.0);
            let lower = class.to_ascii_lowercase();
            let (apple, intel) = (lower.contains("agx"), lower.contains("intel"));
            let rank = if apple {
                3
            } else if intel {
                1
            } else {
                2
            };
            if best.as_ref().is_some_and(|(r, _)| *r >= rank) {
                continue;
            }
            let name = search(e.0, "model").and_then(|v| cf::text(v.0)).filter(|n| !n.is_empty()).unwrap_or(class);
            let integrated = apple || intel;
            // Apple silicon's GPU shares all of the Mac's memory; a card has its own.
            let vram = search(e.0, "VRAM,totalMB").and_then(|v| cf::number_i64(v.0)).map(|mb| mb.max(0) as u64 * 1024 * 1024);
            let mem_total = if integrated { ram } else { vram.unwrap_or(0) };
            best = Some((rank, Gpu { entry: e, name, integrated, mem_total }));
        }
        if best.is_none() {
            warn_once(64, "no GPU statistics");
        }
        best.map(|(_, g)| g)
    }

    fn reading(&self) -> Option<GpuReading> {
        let s = prop(self.entry.0, "PerformanceStatistics")?;
        let get = |k: &str| cf::dict_i64(s.0, k);
        let pct = |v: i64| (v as f32 / 100.0).clamp(0.0, 1.0);
        let util = pct(device_use(s.0)?);
        let mem_used = ["vramUsedBytes", "In use system memory", "inUseVidMemoryBytes"].iter().find_map(|k| get(*k)).map_or(0, |b| b.max(0) as u64);
        Some(GpuReading {
            name: self.name.clone(),
            util,
            engine_3d: get("Renderer Utilization %").map_or(util, pct),
            tiler: get("Tiler Utilization %").map_or(0.0, pct),
            mem_used,
            mem_total: self.mem_total,
            integrated: self.integrated,
            ..GpuReading::default()
        })
    }
}

/// The GPU's busy share in percent, as its driver reports it.
fn device_use(stats: CFTypeRef) -> Option<i64> {
    cf::dict_i64(stats, "Device Utilization %").or_else(|| cf::dict_i64(stats, "GPU Activity(%)"))
}

// ---------------------------------------------------------------- drives

/// The disks' drivers, each with its registry id.
fn block_drivers() -> Vec<(u64, Io)> {
    services(c"IOBlockStorageDriver")
        .into_iter()
        .map(|e| {
            let mut id = 0u64;
            unsafe { IORegistryEntryGetRegistryEntryID(e.0, &mut id) };
            (id, e)
        })
        .collect()
}

/// The volumes Finder lists, the startup disk first: local and browsable,
/// at "/" or in /Volumes, but not removable media (USB sticks, SD cards) or
/// read-only disk images.
fn volumes() -> Vec<Drive> {
    let n = unsafe { getfsstat(std::ptr::null_mut(), 0, MNT_NOWAIT) };
    if n <= 0 {
        warn_once(128, "volume list unavailable");
        return Vec::new();
    }
    // SAFETY: all zeroes is a valid value of this plain C struct.
    let mut buf: Vec<StatFs> = (0..n as usize + 4).map(|_| unsafe { std::mem::zeroed() }).collect();
    let bytes = i32::try_from(buf.len() * std::mem::size_of::<StatFs>()).unwrap_or(i32::MAX);
    let got = unsafe { getfsstat(buf.as_mut_ptr(), bytes, MNT_NOWAIT) };
    let mut drives = Vec::new();
    for f in &buf[..(got.max(0) as usize).min(buf.len())] {
        let mount = unsafe { CStr::from_ptr(f.mntonname.as_ptr()) }.to_string_lossy().into_owned();
        let root = mount == "/";
        // The startup disk is read-only (its system half); others that are read-only are images.
        let place = root || mount.starts_with("/Volumes/");
        if !place || f.flags & MNT_LOCAL == 0 || f.flags & MNT_DONTBROWSE != 0 || (!root && f.flags & MNT_RDONLY != 0) {
            continue;
        }
        let bsize = u64::from(f.bsize);
        if let Some(d) = volume(&mount, root, (f.blocks.saturating_mul(bsize), f.bavail.saturating_mul(bsize))) {
            drives.push(d);
        }
    }
    drives.sort_by_key(|d| !d.system);
    drives
}

/// One volume as Finder shows it: its name, size and the space available
/// for important use (purgeable space counts as free); `fallback` is
/// `statfs`'s (total, free) when Foundation doesn't say.
fn volume(mount: &str, system: bool, fallback: (u64, u64)) -> Option<Drive> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(mount));
    let keys: [&NSURLResourceKey; 4] =
        unsafe { [NSURLVolumeLocalizedNameKey, NSURLVolumeIsRemovableKey, NSURLVolumeTotalCapacityKey, NSURLVolumeAvailableCapacityForImportantUsageKey] };
    let vals = url.resourceValuesForKeys_error(&NSArray::from_slice(&keys)).ok();
    let value = |k: &NSURLResourceKey| vals.as_ref().and_then(|v| v.objectForKey(k));
    let num = |k: &NSURLResourceKey| value(k).and_then(|v| v.downcast::<NSNumber>().ok());
    if num(keys[1]).is_some_and(|n| n.boolValue()) {
        return None;
    }
    let bytes = |k: &NSURLResourceKey| num(k).map(|n| n.longLongValue().max(0) as u64);
    let total = bytes(keys[2]).filter(|t| *t > 0).unwrap_or(fallback.0);
    let free = bytes(keys[3]).filter(|f| *f > 0).unwrap_or(fallback.1);
    let name = value(keys[0]).and_then(|v| v.downcast::<NSString>().ok()).map(|s| s.to_string()).filter(|s| !s.is_empty());
    let name = name.unwrap_or_else(|| mount.rsplit('/').find(|s| !s.is_empty()).unwrap_or("Macintosh HD").to_string());
    (total > 0).then(|| Drive { root: name, label: String::new(), total, free: free.min(total), system })
}

// ---------------------------------------------------------------- network

/// An Ethernet or Wi-Fi interface that is up and connected ("en0", "en1", …;
/// not Apple's own peer-to-peer links, bridges, tunnels or the loopback).
fn counted(i: &IfStats) -> bool {
    i.name.starts_with("en") && i.kind == IFT_ETHER && i.flags & (IFF_UP | IFF_RUNNING) == (IFF_UP | IFF_RUNNING) && i.flags & IFF_LOOPBACK == 0
}

/// Each interface's name in System Settings ("Wi-Fi", "Ethernet"), by its BSD name.
fn interface_labels() -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(all) = Cf::new(unsafe { SCNetworkInterfaceCopyAll() }) else { return out };
    for i in 0..unsafe { cf::CFArrayGetCount(all.0) } {
        let iface = unsafe { cf::CFArrayGetValueAtIndex(all.0, i) };
        let bsd = cf::text(unsafe { SCNetworkInterfaceGetBSDName(iface) });
        let label = cf::text(unsafe { SCNetworkInterfaceGetLocalizedDisplayName(iface) });
        if let (Some(b), Some(l)) = (bsd, label.filter(|l| !l.is_empty())) {
            out.insert(b, l);
        }
    }
    out
}

// ---------------------------------------------------------------- apps

fn process_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; PATH_BYTES];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr().cast(), PATH_BYTES as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..(n as usize).min(PATH_BYTES)]).into_owned())
}

fn process_name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 64];
    let n = unsafe { proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..(n as usize).min(buf.len())]).into_owned())
}

// ---------------------------------------------------------------- battery

#[derive(Clone, Copy, Debug, Default)]
struct BatteryDetails {
    design_mwh: Option<u32>,
    full_mwh: Option<u32>,
    cycles: Option<u32>,
    rate_mw: Option<i32>,
}

/// The internal battery as the menu bar shows it, with `details` from its
/// registry entry; None on a Mac without one.
fn battery_now(details: Option<&BatteryDetails>) -> Option<BatteryReading> {
    let blob = Cf::new(unsafe { IOPSCopyPowerSourcesInfo() })?;
    let on_ac = cf::str_is(unsafe { IOPSGetProvidingPowerSourceType(blob.0) }, "AC Power");
    let list = Cf::new(unsafe { IOPSCopyPowerSourcesList(blob.0) })?;
    for i in 0..unsafe { cf::CFArrayGetCount(list.0) } {
        let desc = unsafe { IOPSGetPowerSourceDescription(blob.0, cf::CFArrayGetValueAtIndex(list.0, i)) };
        if desc.is_null() || !cf::dict_str_is(desc, "Type", "InternalBattery") || cf::dict_bool(desc, "Is Present") == Some(false) {
            continue;
        }
        let cur = cf::dict_i64(desc, "Current Capacity").unwrap_or(0);
        let max = cf::dict_i64(desc, "Max Capacity").unwrap_or(100).max(1);
        // Minutes; -1 while macOS is still working it out.
        let left = cf::dict_i64(desc, "Time to Empty").filter(|m| *m > 0 && !on_ac);
        let d = details.copied().unwrap_or_default();
        return Some(BatteryReading {
            percent: (cur * 100 / max).clamp(0, 100) as u8,
            charging: cf::dict_bool(desc, "Is Charging").unwrap_or(false),
            on_ac,
            saver: NSProcessInfo::processInfo().isLowPowerModeEnabled(),
            secs_left: left.and_then(|m| u32::try_from(m * 60).ok()),
            design_mwh: d.design_mwh,
            full_mwh: d.full_mwh,
            cycles: d.cycles,
            rate_mw: d.rate_mw,
        });
    }
    None
}

/// Cycles, capacities and the power in or out, from the battery's registry
/// entry. macOS reports charge (mAh), which becomes energy at the pack's
/// nominal voltage (`pack_mwh`); the power is the present voltage times the
/// current (negative while discharging).
fn battery_details() -> Option<BatteryDetails> {
    let matching = unsafe { IOServiceMatching(c"AppleSmartBattery".as_ptr()) };
    if matching.is_null() {
        return None;
    }
    // The call takes the matching dictionary over.
    let e = unsafe { IOServiceGetMatchingService(0, matching) };
    if e == 0 {
        return None;
    }
    let e = Io(e);
    let num = |k: &str| prop(e.0, k).and_then(|v| cf::number_i64(v.0));
    let mv = num("Voltage").or_else(|| num("AppleRawBatteryVoltage")).filter(|v| *v > 0);
    let ma = num("Amperage").or_else(|| num("InstantAmperage"));
    let design = num("DesignCapacity").filter(|v| *v > 0);
    // Apple silicon's MaxCapacity is a percentage; Intel's is mAh.
    let full = num("NominalChargeCapacity").or_else(|| num("AppleRawMaxCapacity")).or_else(|| num("MaxCapacity").filter(|v| *v > 100)).filter(|v| *v > 0);
    let energy = |mah: Option<i64>| mah.zip(mv).and_then(|(c, v)| pack_mwh(u32::try_from(c).ok()?, u32::try_from(v).ok()?));
    Some(BatteryDetails {
        design_mwh: energy(design),
        full_mwh: energy(full),
        cycles: num("CycleCount").and_then(|c| u32::try_from(c).ok()).filter(|c| *c > 0),
        rate_mw: mv.zip(ma).and_then(|(v, a)| i32::try_from(v.saturating_mul(a) / 1000).ok()),
    })
}

/// The Mac's model ("Mac14,2"), as its registry names it (the smoke test's report).
pub fn mac_model() -> Option<String> {
    let matching = unsafe { IOServiceMatching(c"IOPlatformExpertDevice".as_ptr()) };
    if matching.is_null() {
        return None;
    }
    // The call takes the matching dictionary over.
    let e = unsafe { IOServiceGetMatchingService(0, matching) };
    if e == 0 {
        return None;
    }
    let e = Io(e);
    prop(e.0, "model").and_then(|v| cf::text(v.0)).filter(|m| !m.is_empty())
}

// ---------------------------------------------------------------- nobody there

static AWAY_SINK: Mutex<Option<EventSink>> = Mutex::new(None);

fn tell(ev: AppEvent) {
    let sink = AWAY_SINK.lock().ok().and_then(|g| g.clone());
    if let Some(s) = sink {
        s.send(ev);
    }
}

define_class!(
    /// Hears the displays sleep and wake and the Mac lock and unlock (main thread).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PoniAwayObserver"]
    struct AwayObserver;

    unsafe impl NSObjectProtocol for AwayObserver {}

    impl AwayObserver {
        #[unsafe(method(screensSlept:))]
        fn screens_slept(&self, _n: &NSNotification) {
            tell(AppEvent::ScreensAsleep(true));
        }

        #[unsafe(method(screensWoke:))]
        fn screens_woke(&self, _n: &NSNotification) {
            tell(AppEvent::ScreensAsleep(false));
        }

        #[unsafe(method(screenLocked:))]
        fn screen_locked(&self, _n: &NSNotification) {
            tell(AppEvent::Locked(true));
        }

        #[unsafe(method(screenUnlocked:))]
        fn screen_unlocked(&self, _n: &NSNotification) {
            tell(AppEvent::Locked(false));
        }
    }
);

/// Tell the app when nobody can see the screen: the displays sleep and wake
/// (NSWorkspace), the Mac locks and unlocks (macOS's distributed
/// notifications, asked for at once: Poni is never the app in front).
/// The observer stays for the app's life.
pub fn watch_away(mtm: MainThreadMarker, sink: EventSink) {
    if let Ok(mut g) = AWAY_SINK.lock() {
        *g = Some(sink);
    }
    let observer: Retained<AwayObserver> = unsafe { msg_send![AwayObserver::alloc(mtm), init] };
    let obs: &AnyObject = observer.as_ref();
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    unsafe {
        workspace.addObserver_selector_name_object(obs, sel!(screensSlept:), Some(NSWorkspaceScreensDidSleepNotification), None);
        workspace.addObserver_selector_name_object(obs, sel!(screensWoke:), Some(NSWorkspaceScreensDidWakeNotification), None);
    }
    let center = NSDistributedNotificationCenter::defaultCenter();
    for (name, sel) in [("com.apple.screenIsLocked", sel!(screenLocked:)), ("com.apple.screenIsUnlocked", sel!(screenUnlocked:))] {
        let name = NSString::from_str(name);
        unsafe { center.addObserver_selector_name_object_suspensionBehavior(obs, sel, Some(&name), None, NSNotificationSuspensionBehavior::DeliverImmediately) };
    }
    std::mem::forget(observer);
}
