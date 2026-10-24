//! Readings for the System page (`pages::system`): one a second on their own
//! thread, and only while the page is open. The rest of the time the thread
//! waits on its channel, so a closed page costs nothing.
//!
//! Every source works without a driver or admin rights:
//! * CPU: `GetSystemTimes` (load, and how much of it is apps or Windows); the
//!   "% Processor Performance" counter times the base speed from
//!   `CallNtPowerInformation` gives the current speed, as in Task Manager.
//! * Apps: `NtQuerySystemInformation(SystemProcessInformation)`, grouped per
//!   executable by `poni_core::sysmon::AppTracker`; each top app's path (for
//!   its icon) from `QueryFullProcessImageNameW` and its name from the file's
//!   version info, both cached.
//! * Memory: `GlobalMemoryStatusEx`, `GetPerformanceInfo` (commit and cache).
//! * Drives: every fixed drive's space; disk activity from the PhysicalDisk counters.
//! * Network: `GetIfTable2` over connected hardware adapters; the adapter and
//!   local address of the route to the internet (looked up, no packet sent).
//! * Battery: `GetSystemPowerStatus`, plus capacity, cycles and power from
//!   the battery driver (`IOCTL_BATTERY_QUERY_*`), every 10 s.
//!
//! A source that fails is left out of the reading and logged once.

use crate::events::{AppEvent, EventSink};
use poni_core::sysmon::{
    cpu_split, display_name, parse_process_list, top_cpu, top_memory, AppTracker, AppUse, BatteryReading, CpuReading, CpuTimes, DiskActivity,
    Drive, MemoryReading, NetReading, Reading, TOP_APPS,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE,
    DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::{CloseHandle, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE, HWND};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetBestInterface, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetFileVersionInfoSizeW, GetFileVersionInfoW, GetLogicalDrives, GetVolumeInformationW,
    VerQueryValueW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT, PDH_FMT_COUNTERVALUE,
    PDH_FMT_DOUBLE,
};
use windows::Win32::System::Power::{
    CallNtPowerInformation, GetSystemPowerStatus, ProcessorInformation, BatteryInformation, BATTERY_CAPACITY_RELATIVE, BATTERY_INFORMATION,
    BATTERY_QUERY_INFORMATION, BATTERY_STATUS, BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_RATE, BATTERY_WAIT_STATUS, GUID_DEVICE_BATTERY,
    IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_STATUS, IOCTL_BATTERY_QUERY_TAG, PROCESSOR_POWER_INFORMATION, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetActiveProcessorCount, GetSystemTimes, OpenProcess, QueryFullProcessImageNameW, ALL_PROCESSOR_GROUPS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::IO::DeviceIoControl;

/// One reading a second.
const PERIOD: Duration = Duration::from_secs(1);
/// The first reading needs a second CPU sample to compare with; this soon.
const FIRST: Duration = Duration::from_millis(350);
/// Drive list, route and battery details change rarely: look again this often.
const SLOW: Duration = Duration::from_secs(10);
/// A process list rarely needs more than this; it grows when Windows asks.
const PROC_BUF: usize = 512 * 1024;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const DRIVE_FIXED: u32 = 3;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
/// `MIB_IF_ROW2.InterfaceAndOperStatusFlags` bits.
const IF_HARDWARE: u8 = 0x01;
const IF_FILTER: u8 = 0x02;
/// `PDH_FMT_NOCAP100`: percentages may pass 100 (Turbo Boost speeds).
const PDH_FMT_NOCAP100: u32 = 0x8000;
/// TEST-NET-1: the route lookup for "the internet" (nothing is sent there).
const ROUTE_PROBE: [u8; 4] = [192, 0, 2, 1];

pub struct SysMon {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

enum Msg {
    Run(bool),
    Quit,
}

impl SysMon {
    pub fn start(sink: EventSink) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new().name("poni-sysmon".into()).spawn(move || worker(sink, rx))?;
        Ok(Self { tx, thread: Some(thread) })
    }

    /// Start (the page opened) or stop (it closed) the readings.
    pub fn run(&self, on: bool) {
        let _ = self.tx.send(Msg::Run(on));
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

fn worker(sink: EventSink, rx: Receiver<Msg>) {
    let mut sampler: Option<Sampler> = None;
    let mut due = Instant::now();
    loop {
        let msg = if sampler.is_some() {
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
        match msg {
            Some(Msg::Quit) => break,
            Some(Msg::Run(true)) => {
                if sampler.is_none() {
                    sampler = Some(Sampler::new());
                    due = Instant::now() + FIRST;
                }
            }
            Some(Msg::Run(false)) => sampler = None,
            None => {
                if let Some(s) = sampler.as_mut() {
                    sink.send(AppEvent::System(Box::new(s.sample())));
                }
                let now = Instant::now();
                due += PERIOD;
                if due <= now {
                    due = now + PERIOD;
                }
            }
        }
    }
}

/// Two readings 350 ms apart from a fresh sampler (the self-test's check of
/// the real sources).
pub fn probe() -> Reading {
    let mut s = Sampler::new();
    std::thread::sleep(FIRST);
    s.sample()
}

// Each failing source is logged once per run, not once a second.
static WARNED: AtomicU32 = AtomicU32::new(0);

fn warn_once(bit: u32, what: &str) {
    if WARNED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        crate::pwarn!("sysmon: {what}");
    }
}

/// Reads everything; created when the page opens and dropped when it closes.
struct Sampler {
    started: Instant,
    logical: u32,
    base_mhz: Option<u32>,
    cpu: Option<CpuTimes>,
    apps: AppTracker,
    /// The process list buffer (u64 keeps the entries 8-byte aligned).
    procs: Vec<u64>,
    /// Image name → (path, display name), for apps that made a top list.
    names: HashMap<String, (Option<String>, String)>,
    pdh: Option<Pdh>,
    net: Option<(Instant, u64, u64)>,
    drives: Vec<(String, String)>,
    route: (Option<u32>, Option<String>),
    battery: Option<BatteryDetails>,
    slow_at: Option<Instant>,
}

impl Sampler {
    fn new() -> Self {
        let logical = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1);
        let mut s = Sampler {
            started: Instant::now(),
            logical,
            base_mhz: base_mhz(logical),
            cpu: None,
            apps: AppTracker::default(),
            procs: vec![0u64; PROC_BUF / 8],
            names: HashMap::new(),
            pdh: Pdh::open(),
            net: None,
            drives: Vec::new(),
            route: (None, None),
            battery: None,
            slow_at: None,
        };
        // Baselines for everything measured between two samples.
        s.cpu = cpu_times();
        let procs = s.process_list();
        s.apps.update(0.0, logical, &procs);
        s.net = net_counters(None).map(|n| (Instant::now(), n.received, n.sent));
        s
    }

    fn sample(&mut self) -> Reading {
        if self.slow_at.is_none_or(|t| t.elapsed() >= SLOW) {
            self.slow_at = Some(Instant::now());
            self.drives = fixed_drives();
            self.route = (route_index(), super::phonedrop::route_ipv4().map(|ip| ip.to_string()));
            self.battery = battery_details();
        }
        // (perf %, read B/s, write B/s, idle %), each when Windows had it.
        let counters = self.pdh.as_ref().filter(|p| p.collect()).map(|p| (p.value(p.perf), p.value(p.read), p.value(p.write), p.value(p.idle)));
        let (perf, read, write, idle) = counters.unwrap_or_default();

        let cpu = cpu_times().and_then(|now| {
            let split = self.cpu.and_then(|prev| cpu_split(prev, now));
            self.cpu = Some(now);
            let (load, user, kernel) = split?;
            let mhz = perf.zip(self.base_mhz).map(|(pct, base)| (pct * f64::from(base) / 100.0).round() as u32);
            Some(CpuReading { load, user, kernel, logical: self.logical, mhz: mhz.filter(|m| *m > 0), uptime_s: unsafe { GetTickCount64() } / 1000 })
        });

        let procs = self.process_list();
        let apps = self.apps.update(self.started.elapsed().as_secs_f64(), self.logical, &procs);
        let top_cpu = self.named(top_cpu(&apps, TOP_APPS));
        let top_memory = self.named(top_memory(&apps, TOP_APPS));

        let disk = read.zip(write).map(|(read, write)| DiskActivity {
            read_bps: read.max(0.0),
            write_bps: write.max(0.0),
            active: idle.map(|idle| (1.0 - idle / 100.0).clamp(0.0, 1.0) as f32),
        });

        let network = net_counters(self.route.0).map(|mut n| {
            let now = Instant::now();
            if let Some((at, rx, tx)) = self.net {
                let dt = now.duration_since(at).as_secs_f64();
                if dt > 0.0 {
                    n.down_bps = n.received.saturating_sub(rx) as f64 / dt;
                    n.up_bps = n.sent.saturating_sub(tx) as f64 / dt;
                }
            }
            self.net = Some((now, n.received, n.sent));
            n.ipv4 = self.route.1.clone().filter(|_| n.online);
            n
        });

        Reading {
            cpu,
            memory: memory(),
            drives: self.drives.iter().filter_map(|(root, label)| drive(root, label)).collect(),
            disk,
            network,
            battery: battery(self.battery.as_ref()),
            top_cpu,
            top_memory,
        }
    }

    /// The process list, or nothing when Windows won't give it.
    fn process_list(&mut self) -> Vec<poni_core::sysmon::ProcSample> {
        for _ in 0..4 {
            let bytes = (self.procs.len() * 8) as u32;
            let mut needed = 0u32;
            let status = unsafe { NtQuerySystemInformation(SystemProcessInformation, self.procs.as_mut_ptr().cast(), bytes, &mut needed) };
            if status.0 == STATUS_INFO_LENGTH_MISMATCH {
                // Room for the processes that start meanwhile.
                self.procs = vec![0u64; (needed as usize + 64 * 1024).div_ceil(8)];
                continue;
            }
            if status.0 < 0 {
                warn_once(1, &format!("process list unavailable (0x{:08X})", status.0 as u32));
                return Vec::new();
            }
            let len = (needed.min(bytes)) as usize;
            // SAFETY: the buffer holds `bytes` initialised bytes; Windows filled the first `len`.
            let buf = unsafe { std::slice::from_raw_parts(self.procs.as_ptr().cast::<u8>(), len) };
            return parse_process_list(buf, self.procs.as_ptr() as usize);
        }
        Vec::new()
    }

    /// Give each top app its path and the name from its file's version info.
    fn named(&mut self, apps: Vec<AppUse>) -> Vec<AppUse> {
        apps.into_iter()
            .map(|mut a| {
                if !self.names.contains_key(&a.image) {
                    // Only a found path is remembered: a protected process
                    // may hide it while another of the same app shows it.
                    if let Some(path) = process_path(a.pid) {
                        let name = file_description(&path).unwrap_or_else(|| display_name(&a.image));
                        self.names.insert(a.image.clone(), (Some(path), name));
                    }
                }
                if let Some((path, name)) = self.names.get(&a.image) {
                    a.path = path.clone();
                    a.name = name.clone();
                }
                a
            })
            .collect()
    }
}

// ---------------------------------------------------------------- CPU and memory

fn cpu_times() -> Option<CpuTimes> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    match unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) } {
        Ok(()) => Some(CpuTimes { idle: ticks(idle), kernel: ticks(kernel), user: ticks(user) }),
        Err(e) => {
            warn_once(2, &format!("CPU times unavailable ({e})"));
            None
        }
    }
}

fn ticks(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// The processor's base speed (MHz), which "% Processor Performance" scales.
fn base_mhz(logical: u32) -> Option<u32> {
    let mut info = vec![PROCESSOR_POWER_INFORMATION::default(); logical as usize];
    let bytes = (info.len() * std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()) as u32;
    let status = unsafe { CallNtPowerInformation(ProcessorInformation, None, 0, Some(info.as_mut_ptr().cast()), bytes) };
    (status.0 >= 0).then(|| info[0].MaxMhz).filter(|m| *m > 0)
}

fn memory() -> Option<MemoryReading> {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if let Err(e) = unsafe { GlobalMemoryStatusEx(&mut m) } {
        warn_once(4, &format!("memory status unavailable ({e})"));
        return None;
    }
    let mut r = MemoryReading { total: m.ullTotalPhys, used: m.ullTotalPhys.saturating_sub(m.ullAvailPhys), ..MemoryReading::default() };
    let cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
    let mut p = PERFORMANCE_INFORMATION { cb, ..Default::default() };
    if unsafe { GetPerformanceInfo(&mut p, cb) }.is_ok() {
        let page = p.PageSize as u64;
        r.committed = p.CommitTotal as u64 * page;
        r.commit_limit = p.CommitLimit as u64 * page;
        r.cached = p.SystemCache as u64 * page;
    }
    Some(r)
}

// ---------------------------------------------------------------- apps

fn process_path(pid: u64) -> Option<String> {
    let pid = u32::try_from(pid).ok()?;
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let r = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) };
    unsafe {
        let _ = CloseHandle(h);
    }
    r.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// The executable's own description ("Google Chrome", "Visual Studio Code").
fn file_description(path: &str) -> Option<String> {
    let file = HSTRING::from(path);
    let size = unsafe { GetFileVersionInfoSizeW(&file, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    unsafe { GetFileVersionInfoW(&file, 0, size, data.as_mut_ptr().cast()) }.ok()?;
    let block = data.as_ptr().cast();
    let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // The file's own languages first, then US English and language-neutral.
    let mut keys = Vec::new();
    if unsafe { VerQueryValueW(block, w!("\\VarFileInfo\\Translation"), &mut p, &mut len) }.as_bool() && !p.is_null() {
        // SAFETY: `p` points at `len` bytes inside `data`.
        let t = unsafe { std::slice::from_raw_parts(p as *const u16, len as usize / 2) };
        keys.extend(t.chunks_exact(2).map(|c| format!("{:04x}{:04x}", c[0], c[1])));
    }
    keys.extend(["040904b0", "040904e4", "000004b0"].map(String::from));
    for k in keys {
        let sub = HSTRING::from(format!("\\StringFileInfo\\{k}\\FileDescription"));
        if unsafe { VerQueryValueW(block, &sub, &mut p, &mut len) }.as_bool() && !p.is_null() && len > 1 {
            // SAFETY: a string value: `len` UTF-16 units inside `data`.
            let s = unsafe { std::slice::from_raw_parts(p as *const u16, len as usize) };
            let text = String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- PDH counters

/// The few Performance Data Helper counters Task Manager also shows.
struct Pdh {
    query: isize,
    perf: Option<isize>,
    read: Option<isize>,
    write: Option<isize>,
    idle: Option<isize>,
}

impl Pdh {
    fn open() -> Option<Pdh> {
        let mut query = 0isize;
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            warn_once(8, "performance counters unavailable");
            return None;
        }
        let add = |path: PCWSTR| {
            let mut c = 0isize;
            (unsafe { PdhAddEnglishCounterW(query, path, 0, &mut c) } == 0).then_some(c)
        };
        let pdh = Pdh {
            query,
            perf: add(w!("\\Processor Information(_Total)\\% Processor Performance")),
            read: add(w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec")),
            write: add(w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec")),
            idle: add(w!("\\PhysicalDisk(_Total)\\% Idle Time")),
        };
        if pdh.perf.is_none() && pdh.read.is_none() {
            warn_once(8, "performance counters unavailable");
            return None;
        }
        // Rates need two collections: this one is the baseline.
        unsafe { PdhCollectQueryData(query) };
        Some(pdh)
    }

    fn collect(&self) -> bool {
        unsafe { PdhCollectQueryData(self.query) == 0 }
    }

    fn value(&self, counter: Option<isize>) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let r = unsafe { PdhGetFormattedCounterValue(counter?, PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100), None, &mut v) };
        // CStatus 0 = valid data, 1 = new data (also valid).
        (r == 0 && v.CStatus <= 1).then(|| unsafe { v.Anonymous.doubleValue }).filter(|x| x.is_finite())
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

// ---------------------------------------------------------------- drives

/// ("C:", label) for every fixed drive.
fn fixed_drives() -> Vec<(String, String)> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let root = format!("{}:\\", (b'A' + i) as char);
            if unsafe { GetDriveTypeW(&HSTRING::from(&root)) } != DRIVE_FIXED {
                return None;
            }
            let mut name = [0u16; 261];
            let label = match unsafe { GetVolumeInformationW(&HSTRING::from(&root), Some(&mut name), None, None, None, None) } {
                Ok(()) => String::from_utf16_lossy(&name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())]),
                Err(_) => String::new(),
            };
            Some((root[..2].to_string(), label))
        })
        .collect()
}

fn drive(root: &str, label: &str) -> Option<Drive> {
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(format!("{root}\\")), Some(&mut free), Some(&mut total), None) }.ok()?;
    let system = std::env::var("SystemDrive").map_or(root.eq_ignore_ascii_case("C:"), |d| d.trim_end_matches('\\').eq_ignore_ascii_case(root));
    Some(Drive { root: root.into(), label: label.into(), total, free, system })
}

// ---------------------------------------------------------------- network

/// The interface Windows would use to reach the internet (a table lookup).
fn route_index() -> Option<u32> {
    let mut index = 0u32;
    (unsafe { GetBestInterface(u32::from_ne_bytes(ROUTE_PROBE), &mut index) } == 0).then_some(index)
}

/// Totals over every connected hardware adapter (speeds are filled in by the
/// caller); the adapter shown is the route's, else the busiest.
fn net_counters(route: Option<u32>) -> Option<NetReading> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if let Err(e) = unsafe { GetIfTable2(&mut table) }.ok() {
        warn_once(16, &format!("network counters unavailable ({e})"));
        return None;
    }
    // SAFETY: Windows allocated `NumEntries` rows after the header; freed below.
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize) };
    let mut n = NetReading::default();
    let mut busiest: Option<(u64, String)> = None;
    let mut routed: Option<String> = None;
    for r in rows {
        let flags = r.InterfaceAndOperStatusFlags._bitfield;
        if flags & IF_HARDWARE == 0 || flags & IF_FILTER != 0 || r.OperStatus != IfOperStatusUp || r.Type == IF_TYPE_SOFTWARE_LOOPBACK {
            continue;
        }
        n.received = n.received.saturating_add(r.InOctets);
        n.sent = n.sent.saturating_add(r.OutOctets);
        let alias = String::from_utf16_lossy(&r.Alias[..r.Alias.iter().position(|c| *c == 0).unwrap_or(r.Alias.len())]);
        if route == Some(r.InterfaceIndex) {
            routed = Some(alias.clone());
        }
        let traffic = r.InOctets.saturating_add(r.OutOctets);
        if busiest.as_ref().is_none_or(|(t, _)| traffic > *t) {
            busiest = Some((traffic, alias));
        }
    }
    unsafe { FreeMibTable(table as *const core::ffi::c_void) };
    n.online = busiest.is_some();
    n.adapter = routed.or(busiest.map(|(_, a)| a)).unwrap_or_default();
    Some(n)
}

// ---------------------------------------------------------------- battery

#[derive(Clone, Copy, Debug, Default)]
struct BatteryDetails {
    design_mwh: Option<u32>,
    full_mwh: Option<u32>,
    cycles: Option<u32>,
    rate_mw: Option<i32>,
}

fn battery(details: Option<&BatteryDetails>) -> Option<BatteryReading> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    // BatteryFlag 128 = no system battery, 255 = unknown (as in services::power).
    if s.BatteryFlag == 128 || s.BatteryFlag == 255 || s.BatteryLifePercent > 100 {
        return None;
    }
    let on_ac = s.ACLineStatus == 1;
    let d = details.copied().unwrap_or_default();
    Some(BatteryReading {
        percent: s.BatteryLifePercent,
        charging: s.BatteryFlag & 8 != 0,
        on_ac,
        saver: s.SystemStatusFlag == 1,
        secs_left: (!on_ac && s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime),
        design_mwh: d.design_mwh,
        full_mwh: d.full_mwh,
        cycles: d.cycles,
        rate_mw: d.rate_mw,
    })
}

/// Capacity, cycles and power of the first battery, from its driver.
fn battery_details() -> Option<BatteryDetails> {
    unsafe {
        let set = SetupDiGetClassDevsW(Some(&GUID_DEVICE_BATTERY), PCWSTR::null(), HWND::default(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE).ok()?;
        let path = device_path(set);
        let _ = SetupDiDestroyDeviceInfoList(set);
        let path = path?;
        let h = CreateFileW(
            &HSTRING::from(path),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
        .ok()?;
        let d = query_battery(h);
        let _ = CloseHandle(h);
        d
    }
}

unsafe fn device_path(set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO) -> Option<String> {
    let mut iface = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
    SetupDiEnumDeviceInterfaces(set, None, &GUID_DEVICE_BATTERY, 0, &mut iface).ok()?;
    let mut need = 0u32;
    let _ = SetupDiGetDeviceInterfaceDetailW(set, &iface, None, 0, Some(&mut need), None);
    if need < 8 {
        return None;
    }
    let mut buf = vec![0u64; (need as usize).div_ceil(8)];
    let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
    SetupDiGetDeviceInterfaceDetailW(set, &iface, Some(detail), need, None, None).ok()?;
    let chars = (need as usize - std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath)) / 2;
    let path = std::slice::from_raw_parts((*detail).DevicePath.as_ptr(), chars);
    Some(String::from_utf16_lossy(&path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())]))
}

unsafe fn query_battery(h: HANDLE) -> Option<BatteryDetails> {
    let wait = 0u32;
    let mut tag = 0u32;
    let mut got = 0u32;
    let size = |n: usize| n as u32;
    DeviceIoControl(h, IOCTL_BATTERY_QUERY_TAG, Some((&wait as *const u32).cast()), 4, Some((&mut tag as *mut u32).cast()), 4, Some(&mut got), None).ok()?;
    if tag == 0 {
        return None;
    }
    let q = BATTERY_QUERY_INFORMATION { BatteryTag: tag, InformationLevel: BatteryInformation, AtRate: 0 };
    let mut info = BATTERY_INFORMATION::default();
    let mut d = BatteryDetails::default();
    let known = |v: u32| (v != 0 && v != BATTERY_UNKNOWN_CAPACITY).then_some(v);
    if DeviceIoControl(
        h,
        IOCTL_BATTERY_QUERY_INFORMATION,
        Some((&q as *const BATTERY_QUERY_INFORMATION).cast()),
        size(std::mem::size_of::<BATTERY_QUERY_INFORMATION>()),
        Some((&mut info as *mut BATTERY_INFORMATION).cast()),
        size(std::mem::size_of::<BATTERY_INFORMATION>()),
        Some(&mut got),
        None,
    )
    .is_ok()
    {
        // Relative capacities are percentages, not mWh: leave them out.
        if info.Capabilities & BATTERY_CAPACITY_RELATIVE == 0 {
            d.design_mwh = known(info.DesignedCapacity);
            d.full_mwh = known(info.FullChargedCapacity);
        }
        d.cycles = (info.CycleCount > 0).then_some(info.CycleCount);
    }
    let ws = BATTERY_WAIT_STATUS { BatteryTag: tag, ..Default::default() };
    let mut st = BATTERY_STATUS::default();
    if DeviceIoControl(
        h,
        IOCTL_BATTERY_QUERY_STATUS,
        Some((&ws as *const BATTERY_WAIT_STATUS).cast()),
        size(std::mem::size_of::<BATTERY_WAIT_STATUS>()),
        Some((&mut st as *mut BATTERY_STATUS).cast()),
        size(std::mem::size_of::<BATTERY_STATUS>()),
        Some(&mut got),
        None,
    )
    .is_ok()
        && st.Rate as u32 != BATTERY_UNKNOWN_RATE
        && d.design_mwh.is_some()
    {
        d.rate_mw = Some(st.Rate);
    }
    Some(d)
}
