//! Readings for the System page (`pages::system`), taken the way Stats takes
//! them on macOS: straight from the kernel's own counters, never from another
//! app, with no driver and no admin rights.
//!
//! * CPU: every logical processor's idle, kernel, user, deferred-call and
//!   interrupt time (`NtQuerySystemInformation(SystemProcessorPerformance-
//!   Information)`, the counterpart of `host_processor_info`), each core's
//!   kind on hybrid CPUs (`GetSystemCpuSetInformation`), each core's speed
//!   (the "% Processor Performance" counters times the base speed from
//!   `CallNtPowerInformation`), the warmest ACPI thermal zone and the
//!   package's energy counters when the firmware has them.
//! * GPU: for the card with the most video memory (named by DXGI), the
//!   graphics kernel's own accounting: each engine's running time and the
//!   memory segments in use (`D3DKMTQueryStatistics`), and what the driver
//!   reports of temperature, fan and clock (`D3DKMTQueryAdapterInfo`).
//! * Memory: `GlobalMemoryStatusEx`, `GetPerformanceInfo` and the memory
//!   manager's lists (standby, modified, free, nonpaged pool, page file);
//!   compressed memory is the Memory Compression process's working set.
//! * Apps: the kernel's process list (`SystemProcessInformation`), grouped
//!   per executable by `poni_core::sysmon::AppTracker`; names from each
//!   file's version info, icons from its path.
//! * Drives, network and battery: fixed drives' space and the physical
//!   disks' counters; `GetIfTable2` per adapter plus the route's adapter,
//!   address and link speed; `GetSystemPowerStatus` and the battery driver.
//! * Power, on a desktop while the page is open: a power supply that
//!   reports over USB (`services::psu`), else an estimate from the CPU
//!   package's energy counter and the graphics card's own reading (NVML,
//!   the NVIDIA driver's library), as `poni_core::power` adds them up. Both
//!   readers open with the page and close with it.
//!
//! The thread keeps the charts' minute while the page is closed (Settings ›
//! keep System history): one light reading a second of the processor times,
//! memory status, disk and network counters and the GPU's engines, touching
//! no window and no drive. Open, each second also walks the process list and
//! reads the sensors, drives, battery and the driver's GPU readings. Under
//! Show over games, while a game is in front, the light reading adds the
//! driver's GPU readings (the temperature) and goes to the island too. A
//! source that fails is left out of the reading and logged once.

use crate::events::{AppEvent, EventSink};
use poni_core::power::PowerReading;
use poni_core::sysmon::{
    core_kinds, core_split, cpu_split, display_name, gpu_busy, parse_core_times, parse_process_list, reads, top_cpu, top_io, top_memory, AppTracker,
    AppUse, BatteryReading, CoreKind, CoreLoad, CoreTimes, CpuReading, CpuTimes, DiskActivity, Drive, Engine, GpuReading, MemoryReading, Monitor,
    NetReading, ProcSample, Reading, Rest, TOP_APPS,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{s, w, HSTRING, PCWSTR, PWSTR};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics, D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER,
    D3DKMT_NODEMETADATA, D3DKMT_NODE_PERFDATA, D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS,
    D3DKMT_QUERYSTATISTICS_ADAPTER, D3DKMT_QUERYSTATISTICS_NODE, D3DKMT_QUERYSTATISTICS_RESULT, D3DKMT_QUERYSTATISTICS_SEGMENT,
    D3DKMT_QUERYSTATISTICS_TYPE, KMTQAITYPE_ADAPTERPERFDATA, KMTQAITYPE_NODEMETADATA, KMTQAITYPE_NODEPERFDATA, KMTQUERYADAPTERINFOTYPE,
};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation, SystemProcessorPerformanceInformation};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE,
    DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Foundation::{CloseHandle, FreeLibrary, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE, HMODULE, HWND, LUID};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetBestInterface, GetIfEntry2, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::{IfOperStatusUp, NET_LUID_LH};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetFileVersionInfoSizeW, GetFileVersionInfoW, GetLogicalDrives, GetVolumeInformationW,
    VerQueryValueW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT,
    PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
};
use windows::Win32::System::Power::{
    BatteryInformation, CallNtPowerInformation, GetSystemPowerStatus, ProcessorInformation, BATTERY_CAPACITY_RELATIVE, BATTERY_INFORMATION,
    BATTERY_QUERY_INFORMATION, BATTERY_STATUS, BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_RATE, BATTERY_WAIT_STATUS, GUID_DEVICE_BATTERY,
    IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_STATUS, IOCTL_BATTERY_QUERY_TAG, PROCESSOR_POWER_INFORMATION, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
use windows::Win32::System::SystemInformation::{
    CpuSetInformation, GetSystemCpuSetInformation, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_CPU_SET_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetActiveProcessorCount, GetActiveProcessorGroupCount, GetCurrentThread, GetSystemTimes, OpenProcess, QueryFullProcessImageNameW, ALL_PROCESSOR_GROUPS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;
use windows::Win32::System::IO::DeviceIoControl;

/// One reading a second, open or not.
const PERIOD: Duration = Duration::from_secs(1);
/// After the page opens, the first full reading needs a second process list to compare with; this soon.
const FIRST: Duration = Duration::from_millis(350);
/// Drive list, route and battery details change rarely: look again this often.
const SLOW: Duration = Duration::from_secs(10);
/// A process list rarely needs more than this; it grows when Windows asks.
const PROC_BUF: usize = 512 * 1024;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const PDH_MORE_DATA: u32 = 0x8000_07D2;
const DRIVE_FIXED: u32 = 3;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
/// `MIB_IF_ROW2.InterfaceAndOperStatusFlags` bits.
const IF_HARDWARE: u8 = 0x01;
const IF_FILTER: u8 = 0x02;
/// `PDH_FMT_NOCAP100`: percentages may pass 100 (Turbo Boost speeds).
const PDH_FMT_NOCAP100: u32 = 0x8000;
/// TEST-NET-1: the route lookup for "the internet" (nothing is sent there).
const ROUTE_PROBE: [u8; 4] = [192, 0, 2, 1];
/// A thermal zone that hasn't moved this much since the page first opened is a fixed placeholder, not a sensor.
const ZONE_STILL_C: f32 = 0.5;
/// One logical processor's entry in the processor times list.
const CORE_BYTES: usize = 48;
/// Readings in a row a GPU may miss before its adapter is opened again (a
/// driver update or a graphics card switched off changes or ends it).
const GPU_MISSES: u32 = 3;
/// The PCI vendor id DXGI gives Microsoft's software and basic adapters.
const MICROSOFT_VENDOR: u32 = 0x1414;
const NVIDIA_VENDOR: u32 = 0x10de;
/// Unanswered PSU readings in a row before the page stops asking it.
const PSU_MISSES: u32 = 3;
/// This much dedicated memory or less is a GPU built into the processor (its
/// slice of RAM, often exactly 512 MB), when the segments don't say.
const INTEGRATED_UP_TO: u64 = 512 * 1024 * 1024;

pub struct SysMon {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

enum Msg {
    /// The page opened (true) or closed.
    Open(bool),
    /// Keep the history while the page is closed.
    Background(bool),
    /// A game is in front under Show over games: readings for the island.
    Game(bool),
    /// Nobody at the PC, or battery saver: see [`reads`].
    Rest(Rest),
    Quit,
}

impl SysMon {
    pub fn start(sink: EventSink, background: bool, rest: Rest) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new().name("poni-sysmon".into()).spawn(move || worker(sink, rx, background, rest))?;
        Ok(Self { tx, thread: Some(thread) })
    }

    pub fn open(&self, on: bool) {
        let _ = self.tx.send(Msg::Open(on));
    }

    pub fn background(&self, on: bool) {
        let _ = self.tx.send(Msg::Background(on));
    }

    pub fn game(&self, on: bool) {
        let _ = self.tx.send(Msg::Game(on));
    }

    pub fn rest(&self, rest: Rest) {
        let _ = self.tx.send(Msg::Rest(rest));
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

fn worker(sink: EventSink, rx: Receiver<Msg>, mut background: bool, mut rest: Rest) {
    let mut open = false;
    let mut game = false;
    let mut sampler: Option<Sampler> = reads(open, game, background, rest).then(Sampler::new);
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
                monitor.push(s.sample(open, game));
                if open || game {
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
        let (mut quit, mut want_open, mut want_bg, mut want_game, mut want_rest) = (false, None, None, None, None);
        for m in std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match m {
                Msg::Quit => quit = true,
                Msg::Open(on) => want_open = Some(on),
                Msg::Background(on) => want_bg = Some(on),
                Msg::Game(on) => want_game = Some(on),
                Msg::Rest(r) => want_rest = Some(r),
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
        if let Some(g) = want_game {
            game = g;
        }
        if let Some(r) = want_rest {
            if r != rest && (background || open || game) {
                crate::pinfo!("system readings: {}", if r.away { "resting while nobody is at the PC" } else if r.saver { "kept minute resting on battery saver" } else { "reading again" });
            }
            rest = r;
        }
        // Not reading: no thread wake-ups at all until a message comes, and
        // the kept minute goes so a gap never shows as part of it.
        if !reads(open, game, background, rest) {
            sampler = None;
            monitor.clear();
            continue;
        }
        let fresh = sampler.is_none();
        if fresh {
            sampler = Some(Sampler::new());
            monitor.clear();
            due = Instant::now() + FIRST;
        }
        // A game just came to the front: the kept reading at once, a fresh
        // one shortly, then each second.
        if want_game == Some(true) && !open {
            if monitor.latest.is_some() {
                sink.send(AppEvent::System(Box::new(monitor.clone())));
            }
            due = due.min(Instant::now() + FIRST);
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

/// The self-test's check of the real sources: a fresh sampler's light
/// reading, its first full reading (which also lists drives and battery)
/// and a second full one, 350 ms apart, and how long each took.
pub struct Probe {
    pub reading: Reading,
    pub light: Duration,
    pub first_full: Duration,
    pub full: Duration,
    /// What one more light reading spent on each source (and the process list).
    pub parts: Vec<(&'static str, Duration)>,
    /// Processor cycles one light reading takes, averaged over a few (what
    /// keeping the history costs each second).
    pub light_cycles: Option<u64>,
}

pub fn probe() -> Probe {
    let mut s = Sampler::new();
    s.prime_apps();
    let mut timed = |full: bool| {
        std::thread::sleep(FIRST);
        let t = Instant::now();
        let r = s.sample(full, false);
        (r, t.elapsed())
    };
    let (_, light) = timed(false);
    let (_, first_full) = timed(true);
    let (reading, full) = timed(true);
    std::thread::sleep(FIRST);
    let mut parts = Vec::new();
    let t = Instant::now();
    let _ = s.core_times();
    parts.push(("processor times", t.elapsed()));
    let t = Instant::now();
    let _ = s.pdh.as_ref().filter(|p| p.collect(false)).map(|p| p.read(false));
    parts.push(("performance counters", t.elapsed()));
    let t = Instant::now();
    let _ = s.gpu.as_mut().map(|g| g.reading(false));
    parts.push(("GPU statistics", t.elapsed()));
    let t = Instant::now();
    let _ = memory(false);
    parts.push(("memory status", t.elapsed()));
    let t = Instant::now();
    let _ = if_rows(&s.ifs).map(|rows| net_reading(&rows, s.route.0));
    parts.push(("network adapters", t.elapsed()));
    let t = Instant::now();
    let _ = if_table().map(|rows| net_reading(&rows, s.route.0));
    parts.push(("interface table (every 10 s)", t.elapsed()));
    let t = Instant::now();
    let _ = s.process_list();
    parts.push(("process list (open only)", t.elapsed()));
    // The thread's own cycle count, exact where its CPU time is counted in clock ticks.
    const RUNS: u64 = 5;
    let cycles = || {
        let mut c = 0u64;
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) }.ok().map(|_| c)
    };
    let mut spent = Some(0u64);
    for _ in 0..RUNS {
        std::thread::sleep(Duration::from_millis(200));
        let c0 = cycles();
        let _ = s.sample(false, false);
        spent = spent.zip(c0.zip(cycles())).map(|(sum, (a, b))| sum + b.saturating_sub(a));
    }
    Probe { reading, light, first_full, full, parts, light_cycles: spent.map(|c| c / RUNS) }
}

// Each failing source is logged once per run, not once a second.
static WARNED: AtomicU32 = AtomicU32::new(0);

fn warn_once(bit: u32, what: &str) {
    if WARNED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        crate::pwarn!("sysmon: {what}");
    }
}

/// Reads everything; created when the history or the page needs it.
struct Sampler {
    started: Instant,
    logical: u32,
    /// Active logical processors in each processor group (Windows makes groups of up to 64).
    groups: Vec<u32>,
    base_mhz: Option<u32>,
    /// Each logical processor's kind, group after group in Windows' order.
    kinds: Vec<CoreKind>,
    cores: Option<Vec<CoreTimes>>,
    cores_buf: Vec<u64>,
    cpu: Option<CpuTimes>,
    apps: AppTracker,
    /// The process list buffer (u64 keeps the entries 8-byte aligned).
    procs: Vec<u64>,
    /// Image name → (path, display name), for apps that made a top list.
    names: HashMap<String, (Option<String>, String)>,
    pdh: Option<Pdh>,
    gpu: Option<Gpu>,
    /// When the adapters were last read, and each one's (received, sent) bytes by LUID.
    net: Option<(Instant, HashMap<u64, (u64, u64)>)>,
    /// The connected hardware adapters (LUIDs) at the last read of the whole
    /// interface table, and when: light readings ask for just these.
    ifs: Vec<u64>,
    ifs_at: Option<Instant>,
    drives: Vec<(String, String)>,
    route: (Option<u32>, Option<String>),
    battery: Option<BatteryDetails>,
    /// (plugged in, charging) at the last reading: a change refreshes the battery details.
    power: Option<(bool, bool)>,
    /// The drives at the last full reading: light readings carry them, so
    /// the page shows them the moment it opens (drives aren't read while closed).
    drive_list: Vec<Drive>,
    /// The Memory Compression store, from the last process list.
    compressed: u64,
    /// The thermal zone reading, its lowest and highest so far (placeholders never move).
    zone: Option<(f32, f32, f32)>,
    slow_at: Option<Instant>,
    /// A power supply that reports over USB, open while the page is, and
    /// whether this opening looked for one (once per opening).
    psu: Option<super::psu::Psu>,
    psu_looked: bool,
    psu_misses: u32,
}

impl Sampler {
    fn new() -> Self {
        let logical = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }.max(1);
        let groups: Vec<u32> = (0..unsafe { GetActiveProcessorGroupCount() }.max(1)).map(|g| unsafe { GetActiveProcessorCount(g) }).collect();
        let mut s = Sampler {
            started: Instant::now(),
            logical,
            base_mhz: base_mhz(logical),
            kinds: core_kinds(&efficiency_classes(&groups)),
            groups,
            cores: None,
            cores_buf: Vec::new(),
            cpu: None,
            apps: AppTracker::default(),
            procs: vec![0u64; PROC_BUF / 8],
            names: HashMap::new(),
            pdh: Pdh::open(),
            gpu: Gpu::open(),
            net: None,
            drives: Vec::new(),
            ifs: Vec::new(),
            ifs_at: None,
            route: (None, None),
            battery: None,
            power: None,
            drive_list: Vec::new(),
            compressed: 0,
            zone: None,
            slow_at: None,
            psu: None,
            psu_looked: false,
            psu_misses: 0,
        };
        // Baselines for everything measured between two samples.
        s.cores = s.core_times();
        s.cpu = cpu_times();
        if let Some(rows) = if_table() {
            let (_, adapters) = net_reading(&rows, None);
            s.ifs = adapters.iter().map(|a| a.0).collect();
            s.ifs_at = Some(Instant::now());
            s.net = Some((Instant::now(), adapters.into_iter().map(|(luid, rx, tx)| (luid, (rx, tx))).collect()));
        }
        s
    }

    /// A fresh process list to measure the next full reading's app CPU and I/O against.
    fn prime_apps(&mut self) {
        let procs = self.process_list();
        self.apps.update(self.started.elapsed().as_secs_f64(), self.logical, &procs);
    }

    /// A reading: `full` while the page is open, else light; `sensors` adds
    /// the driver's GPU readings to a light one (the island over a game).
    fn sample(&mut self, full: bool, sensors: bool) -> Reading {
        // Under the readout over a game, a graphics card that stopped
        // answering (a driver reset) is looked for again too.
        if sensors && !full && self.gpu.is_none() && self.slow_at.is_none_or(|t| t.elapsed() >= SLOW) {
            self.slow_at = Some(Instant::now());
            self.gpu = Gpu::open();
        }
        if full && self.slow_at.is_none_or(|t| t.elapsed() >= SLOW) {
            self.slow_at = Some(Instant::now());
            self.drives = fixed_drives();
            self.route = (route_index(), super::phonedrop::route_ipv4().map(|ip| ip.to_string()));
            self.battery = battery_details();
            // A graphics card that went away, came back or was added (an external one).
            if self.gpu.is_none() {
                self.gpu = Gpu::open();
            }
        }
        // The power readers live while the page is open.
        if let Some(g) = self.gpu.as_mut() {
            g.page_open(full);
        }
        if !full {
            self.psu = None;
            self.psu_looked = false;
        }
        let pdh = self.pdh.as_ref().filter(|p| p.collect(full));
        let counters = pdh.map(|p| p.read(full)).unwrap_or_default();

        let mut cpu = self.cpu_reading(&counters);
        if let Some(c) = cpu.as_mut() {
            c.temp_c = self.zone_temp(counters.zone_c);
            c.power_w = counters.package_w;
        }

        let gpu = self.gpu.as_mut().and_then(|g| g.reading(full || sensors));
        if self.gpu.as_ref().is_some_and(|g| g.misses >= GPU_MISSES) {
            warn_once(1 << 11, "GPU stopped answering; opening it again with the page");
            self.gpu = None;
        }

        let mut top = (Vec::new(), Vec::new(), Vec::new());
        if full {
            let procs = self.process_list();
            if let Some(mc) = procs.iter().find(|p| p.image.eq_ignore_ascii_case("Memory Compression")) {
                self.compressed = mc.memory;
            }
            let apps = self.apps.update(self.started.elapsed().as_secs_f64(), self.logical, &procs);
            top = (self.named(top_cpu(&apps, TOP_APPS)), self.named(top_memory(&apps, TOP_APPS)), self.named(top_io(&apps, TOP_APPS)));
        }

        // The commit charge from the counters; GetPerformanceInfo (which counts
        // every process and thread on the way) only when they're missing.
        let commit = counters.committed.zip(counters.commit_limit);
        let memory = memory(commit.is_none() || (full && counters.standby.is_none())).map(|mut m| {
            if let Some((committed, limit)) = commit {
                m.committed = committed;
                m.commit_limit = limit;
            }
            if let Some(standby) = counters.standby {
                m.cached = standby + counters.modified.unwrap_or(0);
            }
            m.wired = counters.nonpaged.unwrap_or(0);
            m.free = counters.free.unwrap_or(0);
            m.compressed = self.compressed;
            m.page_total = m.commit_limit.saturating_sub(m.total);
            m.page_used = counters.page_pct.map_or(0, |p| (p / 100.0 * m.page_total as f64) as u64);
            m
        });

        let disk = counters.read.zip(counters.write).map(|(read, write)| DiskActivity {
            read_bps: read.max(0.0),
            write_bps: write.max(0.0),
            active: counters.idle.map(|idle| (1.0 - idle / 100.0).clamp(0.0, 1.0) as f32),
        });

        // The whole interface table (dozens of filters and tunnels) when open
        // and every ten seconds; in between just the adapters it listed.
        let route = self.route.0;
        let known = (!full && self.ifs_at.is_some_and(|t| t.elapsed() < SLOW)).then(|| if_rows(&self.ifs)).flatten();
        let (rows, from_table) = match known {
            Some(rows) => (Some(rows), false),
            None => (if_table(), true),
        };
        let network = rows.map(|rows| net_reading(&rows, route)).map(|(mut n, adapters)| {
            let now = Instant::now();
            if from_table {
                self.ifs = adapters.iter().map(|a| a.0).collect();
                self.ifs_at = Some(now);
            }
            // Only adapters seen last time count: one that just (re)connected
            // brings its whole total since startup, which isn't this second's traffic.
            if let Some((at, before)) = &self.net {
                let dt = now.duration_since(*at).as_secs_f64();
                if dt > 0.0 {
                    let (mut rx, mut tx) = (0u64, 0u64);
                    for (luid, r, t) in &adapters {
                        if let Some((r0, t0)) = before.get(luid) {
                            rx = rx.saturating_add(r.saturating_sub(*r0));
                            tx = tx.saturating_add(t.saturating_sub(*t0));
                        }
                    }
                    n.down_bps = rx as f64 / dt;
                    n.up_bps = tx as f64 / dt;
                }
            }
            self.net = Some((now, adapters.into_iter().map(|(luid, r, t)| (luid, (r, t))).collect()));
            n.ipv4 = self.route.1.clone().filter(|_| n.online);
            n
        });

        if full {
            self.drive_list = self.drives.iter().filter_map(|(root, label)| drive(root, label)).collect();
        }
        // The power status is one cheap call; the battery's details come from the last slow refresh.
        let battery = if full { self.battery_reading() } else { battery(self.battery.as_ref()) };
        // A laptop's battery page already says what it draws.
        let power = (full && battery.is_none()).then(|| self.power_reading(cpu.as_ref(), gpu.as_ref())).flatten();
        let (top_cpu, top_memory, top_io) = top;
        Reading { cpu, gpu, memory, drives: self.drive_list.clone(), disk, network, battery, power, top_cpu, top_memory, top_io }
    }

    /// The whole PC's draw: what a power supply that reports over USB says,
    /// else the estimate from the CPU package and the graphics card.
    fn power_reading(&mut self, cpu: Option<&CpuReading>, gpu: Option<&GpuReading>) -> Option<PowerReading> {
        let cpu_w = cpu.and_then(|c| c.power_w);
        let gpu_w = gpu.and_then(|g| g.power_w);
        if !self.psu_looked {
            self.psu_looked = true;
            self.psu_misses = 0;
            self.psu = super::psu::Psu::find();
        }
        if let Some(psu) = self.psu.as_mut() {
            match psu.read() {
                Some(r) => {
                    self.psu_misses = 0;
                    return Some(PowerReading::from_psu(r, cpu_w, gpu_w));
                }
                None => {
                    self.psu_misses += 1;
                    if self.psu_misses >= PSU_MISSES {
                        warn_once(1 << 12, "power supply stopped answering; estimating until the page opens again");
                        self.psu = None;
                    }
                }
            }
        }
        PowerReading::estimate(cpu_w, gpu_w, gpu.is_some_and(|g| !g.integrated))
    }

    /// Every core's load since the last reading (the processor times by
    /// core), or the machine's total when Windows won't list the cores.
    fn cpu_reading(&mut self, counters: &Counters) -> Option<CpuReading> {
        let uptime_s = unsafe { GetTickCount64() } / 1000;
        let mhz = counters.perf.zip(self.base_mhz).map(|(pct, base)| (pct * f64::from(base) / 100.0).round() as u32).filter(|m| *m > 0);
        let (mhz_p, mhz_e) = self.kind_speeds(&counters.perf_cores);
        if let Some(now) = self.core_times() {
            let split = self.cores.as_ref().and_then(|prev| core_split(prev, &now));
            self.cores = Some(now);
            let s = split?;
            let cores = s.cores.iter().enumerate().map(|(i, load)| CoreLoad { load: *load, kind: self.kinds.get(i).copied().unwrap_or_default() }).collect();
            return Some(CpuReading {
                load: s.load,
                user: s.user,
                kernel: s.kernel,
                interrupt: s.interrupt,
                cores,
                logical: self.logical,
                base_mhz: self.base_mhz,
                mhz,
                mhz_p,
                mhz_e,
                uptime_s,
                ..CpuReading::default()
            });
        }
        let now = cpu_times()?;
        let split = self.cpu.and_then(|prev| cpu_split(prev, now));
        self.cpu = Some(now);
        let (load, user, kernel) = split?;
        Some(CpuReading { load, user, kernel, logical: self.logical, base_mhz: self.base_mhz, mhz, uptime_s, ..CpuReading::default() })
    }

    /// Each logical processor's times, straight from the kernel, group after
    /// group: the plain query answers for the calling thread's group only, so
    /// a PC with more than one group is asked group by group. None when not
    /// every processor answered (the totals then come from `GetSystemTimes`).
    fn core_times(&mut self) -> Option<Vec<CoreTimes>> {
        let bytes = (self.logical as usize * CORE_BYTES).max(CORE_BYTES);
        if self.cores_buf.len() * 8 < bytes {
            self.cores_buf = vec![0u64; bytes.div_ceil(8)];
        }
        let len = (self.cores_buf.len() * 8) as u32;
        let mut cores = Vec::with_capacity(self.logical as usize);
        for group in 0..self.groups.len() {
            let mut got = 0u32;
            let status = if self.groups.len() == 1 {
                unsafe { NtQuerySystemInformation(SystemProcessorPerformanceInformation, self.cores_buf.as_mut_ptr().cast(), len, &mut got) }.0
            } else {
                let query = nt_query_ex()?;
                let g = group as u16;
                unsafe { query(SystemProcessorPerformanceInformation.0, (&g as *const u16).cast(), 2, self.cores_buf.as_mut_ptr().cast(), len, &mut got) }
            };
            if status < 0 || (got as usize) < CORE_BYTES {
                warn_once(1 << 9, &format!("per-core times unavailable (0x{:08X})", status as u32));
                return None;
            }
            // SAFETY: Windows filled `got` bytes of the buffer.
            let buf = unsafe { std::slice::from_raw_parts(self.cores_buf.as_ptr().cast::<u8>(), (got as usize).min(len as usize)) };
            cores.extend(parse_core_times(buf));
        }
        (cores.len() == self.logical as usize).then_some(cores)
    }

    /// Average speed of the performance and efficiency cores ("1,5" = group 1, core 5).
    fn kind_speeds(&self, perf: &[(String, f64)]) -> (Option<u32>, Option<u32>) {
        let Some(base) = self.base_mhz else { return (None, None) };
        let mut sums: [(f64, u32); 2] = [(0.0, 0); 2];
        for (name, pct) in perf {
            let Some((group, index)) = name.split_once(',') else { continue };
            let (Ok(g), Ok(i)) = (group.parse::<usize>(), index.parse::<usize>()) else { continue };
            let Some(first) = (g < self.groups.len()).then(|| self.groups[..g].iter().sum::<u32>() as usize) else { continue };
            let slot = match self.kinds.get(first + i) {
                Some(CoreKind::Performance) => 0,
                Some(CoreKind::Efficiency) => 1,
                _ => continue,
            };
            sums[slot].0 += pct * f64::from(base) / 100.0;
            sums[slot].1 += 1;
        }
        let avg = |(sum, n): (f64, u32)| (n > 0).then(|| (sum / f64::from(n)).round() as u32).filter(|m| *m > 0);
        (avg(sums[0]), avg(sums[1]))
    }

    /// The thermal zone's temperature, once it has shown it is a live sensor.
    fn zone_temp(&mut self, c: Option<f32>) -> Option<f32> {
        let c = c.filter(|c| (5.0..=115.0).contains(c))?;
        let (lo, hi) = self.zone.map_or((c, c), |(_, lo, hi)| (lo.min(c), hi.max(c)));
        self.zone = Some((c, lo, hi));
        (hi - lo >= ZONE_STILL_C).then_some(c)
    }

    /// The process list, or nothing when Windows won't give it.
    fn process_list(&mut self) -> Vec<ProcSample> {
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
                    // The file name must match: the process may have exited and its id been reused.
                    let path = process_path(a.pid)
                        .filter(|p| p.rsplit(['\\', '/']).next().is_some_and(|f| f.eq_ignore_ascii_case(&a.image)))
                        .or_else(|| windows_file(&a.image));
                    if let Some(path) = path {
                        // Our own file's description is a whole line ("Oyenza Studio - a living notch for Windows").
                        let own = std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().eq_ignore_ascii_case(&path));
                        let name = if own { "Oyenza Studio".to_string() } else { file_description(&path).unwrap_or_else(|| display_name(&a.image)) };
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

    /// The battery now; plugging in or out refreshes its power reading at once
    /// instead of at the next slow refresh.
    fn battery_reading(&mut self) -> Option<BatteryReading> {
        let mut r = battery(self.battery.as_ref())?;
        let state = (r.on_ac, r.charging);
        if self.power.is_some_and(|p| p != state) {
            self.battery = battery_details();
            r = battery(self.battery.as_ref())?;
        }
        self.power = Some(state);
        Some(r)
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

/// `NtQuerySystemInformationEx` (not in the bindings): ntdll's, looked up once.
type QueryEx = unsafe extern "system" fn(i32, *const core::ffi::c_void, u32, *mut core::ffi::c_void, u32, *mut u32) -> i32;

fn nt_query_ex() -> Option<QueryEx> {
    static ADDR: OnceLock<Option<usize>> = OnceLock::new();
    let addr = *ADDR.get_or_init(|| unsafe {
        let ntdll = GetModuleHandleW(w!("ntdll.dll")).ok()?;
        GetProcAddress(ntdll, s!("NtQuerySystemInformationEx")).map(|f| f as usize)
    });
    // SAFETY: the export has this signature on every Windows that has it.
    addr.map(|a| unsafe { std::mem::transmute::<usize, QueryEx>(a) })
}

/// Each logical processor's efficiency class, group after group in index order.
fn efficiency_classes(groups: &[u32]) -> Vec<u8> {
    let mut need = 0u32;
    let _ = unsafe { GetSystemCpuSetInformation(None, 0, &mut need, HANDLE::default(), 0) };
    if need == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u64; (need as usize).div_ceil(8)];
    if !unsafe { GetSystemCpuSetInformation(Some(buf.as_mut_ptr().cast()), need, &mut need, HANDLE::default(), 0) }.as_bool() {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), (need as usize).min(buf.len() * 8)) };
    let first: Vec<usize> = groups.iter().scan(0usize, |sum, n| Some(std::mem::replace(sum, *sum + *n as usize))).collect();
    let mut classes = vec![0u8; groups.iter().sum::<u32>() as usize];
    let entry = std::mem::size_of::<SYSTEM_CPU_SET_INFORMATION>();
    let mut off = 0usize;
    // Each entry starts with its own size; read only whole entries that lie inside `bytes`.
    while off + 4 <= bytes.len() {
        let size = u32::from_ne_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]) as usize;
        if size < 8 || off + size > bytes.len() {
            break;
        }
        if size >= entry {
            // SAFETY: `entry` bytes from `off` are inside `bytes`; the read needs no alignment.
            let e = unsafe { std::ptr::read_unaligned(bytes.as_ptr().add(off).cast::<SYSTEM_CPU_SET_INFORMATION>()) };
            if e.Type == CpuSetInformation {
                let set = unsafe { e.Anonymous.CpuSet };
                if let Some(i) = first.get(set.Group as usize).map(|f| f + set.LogicalProcessorIndex as usize).filter(|i| *i < classes.len()) {
                    classes[i] = set.EfficiencyClass;
                }
            }
        }
        off += size;
    }
    classes
}

/// Physical memory in use, and with `perf_info` the commit charge and the
/// file cache from `GetPerformanceInfo`.
fn memory(perf_info: bool) -> Option<MemoryReading> {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if let Err(e) = unsafe { GlobalMemoryStatusEx(&mut m) } {
        warn_once(4, &format!("memory status unavailable ({e})"));
        return None;
    }
    let mut r = MemoryReading { total: m.ullTotalPhys, used: m.ullTotalPhys.saturating_sub(m.ullAvailPhys), ..MemoryReading::default() };
    if !perf_info {
        return Some(r);
    }
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

/// A process as the System page names it: its file's description
/// ("Counter-Strike 2"), else its file name without ".exe". None when
/// Windows won't open it (a game running as admin can refuse).
pub fn app_name(pid: u32) -> Option<String> {
    let path = process_path(u64::from(pid))?;
    let image = path.rsplit('\\').next().unwrap_or(&path).to_string();
    Some(file_description(&path).unwrap_or_else(|| display_name(&image)))
}

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
    // The UTF-16 units at `p` that lie inside `data`: a value's length comes
    // from the file itself, and some tools write it in bytes, not characters.
    let units = |p: *mut core::ffi::c_void, len: u32| -> &[u16] {
        let start = (p as usize).wrapping_sub(data.as_ptr() as usize);
        if p.is_null() || start >= data.len() || start % 2 != 0 {
            return &[];
        }
        let n = (len as usize).min((data.len() - start) / 2);
        // SAFETY: `n` units starting at `p` lie inside `data`, which outlives this slice.
        unsafe { std::slice::from_raw_parts(p as *const u16, n) }
    };
    let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // The file's own languages first, then US English and language-neutral.
    let mut keys = Vec::new();
    if unsafe { VerQueryValueW(block, w!("\\VarFileInfo\\Translation"), &mut p, &mut len) }.as_bool() {
        let t = units(p, len / 2);
        keys.extend(t.chunks_exact(2).map(|c| format!("{:04x}{:04x}", c[0], c[1])));
    }
    keys.extend(["040904b0", "040904e4", "000004b0"].map(String::from));
    for k in keys {
        let sub = HSTRING::from(format!("\\StringFileInfo\\{k}\\FileDescription"));
        if unsafe { VerQueryValueW(block, &sub, &mut p, &mut len) }.as_bool() {
            let s = units(p, len);
            let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
            let text = String::from_utf16_lossy(&s[..end]).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Where a Windows component whose process hides its path lives
/// ("WmiPrvSE.exe" in System32\wbem): for its icon and name.
fn windows_file(image: &str) -> Option<String> {
    if !image.to_ascii_lowercase().ends_with(".exe") || image.contains(['\\', '/']) {
        return None;
    }
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    [r"System32", r"System32\wbem", r"", r"SysWOW64"]
        .iter()
        .map(|dir| if dir.is_empty() { format!(r"{root}\{image}") } else { format!(r"{root}\{dir}\{image}") })
        .find(|p| std::path::Path::new(p).is_file())
}

// ---------------------------------------------------------------- performance counters

/// What one collection of the performance counters gave.
#[derive(Clone, Debug, Default)]
struct Counters {
    perf: Option<f64>,
    perf_cores: Vec<(String, f64)>,
    read: Option<f64>,
    write: Option<f64>,
    idle: Option<f64>,
    standby: Option<u64>,
    modified: Option<u64>,
    free: Option<u64>,
    nonpaged: Option<u64>,
    committed: Option<u64>,
    commit_limit: Option<u64>,
    page_pct: Option<f64>,
    zone_c: Option<f32>,
    package_w: Option<f32>,
}

/// The kernel's performance counters this page reads: one query for every
/// reading, and one for the sensors, read only while the page is open.
struct Pdh {
    query: isize,
    sensors: isize,
    perf: Option<isize>,
    perf_cores: Option<isize>,
    read: Option<isize>,
    write: Option<isize>,
    idle: Option<isize>,
    standby: [Option<isize>; 3],
    modified: Option<isize>,
    free: Option<isize>,
    nonpaged: Option<isize>,
    committed: Option<isize>,
    commit_limit: Option<isize>,
    page_pct: Option<isize>,
    /// Thermal zones in tenths of a kelvin (High Precision), else whole kelvins.
    zone: Option<(isize, bool)>,
    energy: Option<isize>,
}

impl Pdh {
    fn open() -> Option<Pdh> {
        let (mut query, mut sensors) = (0isize, 0isize);
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            warn_once(8, "performance counters unavailable");
            return None;
        }
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut sensors) } != 0 {
            sensors = 0;
        }
        let add_to = |q: isize, path: PCWSTR| {
            let mut c = 0isize;
            (q != 0 && unsafe { PdhAddEnglishCounterW(q, path, 0, &mut c) } == 0).then_some(c)
        };
        let add = |path: PCWSTR| add_to(query, path);
        let zone = add_to(sensors, w!("\\Thermal Zone Information(*)\\High Precision Temperature"))
            .map(|c| (c, true))
            .or_else(|| add_to(sensors, w!("\\Thermal Zone Information(*)\\Temperature")).map(|c| (c, false)));
        let pdh = Pdh {
            query,
            sensors,
            perf: add(w!("\\Processor Information(_Total)\\% Processor Performance")),
            perf_cores: add(w!("\\Processor Information(*)\\% Processor Performance")),
            read: add(w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec")),
            write: add(w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec")),
            idle: add(w!("\\PhysicalDisk(_Total)\\% Idle Time")),
            standby: [
                add(w!("\\Memory\\Standby Cache Core Bytes")),
                add(w!("\\Memory\\Standby Cache Normal Priority Bytes")),
                add(w!("\\Memory\\Standby Cache Reserve Bytes")),
            ],
            modified: add(w!("\\Memory\\Modified Page List Bytes")),
            free: add(w!("\\Memory\\Free & Zero Page List Bytes")),
            nonpaged: add(w!("\\Memory\\Pool Nonpaged Bytes")),
            committed: add(w!("\\Memory\\Committed Bytes")),
            commit_limit: add(w!("\\Memory\\Commit Limit")),
            page_pct: add(w!("\\Paging File(_Total)\\% Usage")),
            zone,
            energy: add_to(sensors, w!("\\Energy Meter(*)\\Power")),
        };
        if pdh.perf.is_none() && pdh.read.is_none() && pdh.free.is_none() {
            warn_once(8, "performance counters unavailable");
            return None;
        }
        // Rates need two collections: this one is the baseline.
        unsafe { PdhCollectQueryData(query) };
        Some(pdh)
    }

    fn collect(&self, sensors: bool) -> bool {
        if sensors && self.sensors != 0 {
            unsafe { PdhCollectQueryData(self.sensors) };
        }
        unsafe { PdhCollectQueryData(self.query) == 0 }
    }

    fn read(&self, sensors: bool) -> Counters {
        let bytes = |c: Option<isize>| self.value(c).map(|v| v.max(0.0) as u64);
        let standby: Vec<u64> = self.standby.iter().filter_map(|c| bytes(*c)).collect();
        let zone_c = self.zone.filter(|_| sensors).and_then(|(c, precise)| {
            let k = self.array(Some(c)).into_iter().map(|(_, v)| if precise { v / 10.0 } else { v }).fold(f64::NAN, f64::max);
            k.is_finite().then_some((k - 273.15) as f32)
        });
        // The package's power in milliwatts (Intel/AMD RAPL domains); only a sane value counts.
        let package_w = {
            let e = if sensors { self.array(self.energy) } else { Vec::new() };
            let pick = e.iter().find(|(n, _)| n.to_ascii_uppercase().contains("PKG")).or_else(|| e.iter().max_by(|a, b| a.1.total_cmp(&b.1)));
            pick.map(|(_, mw)| (*mw / 1000.0) as f32).filter(|w| (0.5..=500.0).contains(w))
        };
        Counters {
            perf: self.value(self.perf),
            perf_cores: self.array(self.perf_cores),
            read: self.value(self.read),
            write: self.value(self.write),
            idle: self.value(self.idle),
            standby: (!standby.is_empty()).then(|| standby.iter().sum()),
            modified: bytes(self.modified),
            free: bytes(self.free),
            nonpaged: bytes(self.nonpaged),
            committed: bytes(self.committed),
            commit_limit: bytes(self.commit_limit).filter(|l| *l > 0),
            page_pct: self.value(self.page_pct),
            zone_c,
            package_w,
        }
    }

    fn value(&self, counter: Option<isize>) -> Option<f64> {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        let r = unsafe { PdhGetFormattedCounterValue(counter?, PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100), None, &mut v) };
        // CStatus 0 = valid data, 1 = new data (also valid).
        (r == 0 && v.CStatus <= 1).then(|| unsafe { v.Anonymous.doubleValue }).filter(|x| x.is_finite())
    }

    /// Every instance of a wildcard counter, as (instance name, value).
    fn array(&self, counter: Option<isize>) -> Vec<(String, f64)> {
        let Some(c) = counter else { return Vec::new() };
        let fmt = PDH_FMT(PDH_FMT_DOUBLE.0 | PDH_FMT_NOCAP100);
        for _ in 0..3 {
            let (mut size, mut count) = (0u32, 0u32);
            let r = unsafe { PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, None) };
            if r != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let r = unsafe { PdhGetFormattedCounterArrayW(c, fmt, &mut size, &mut count, Some(items)) };
            if r == PDH_MORE_DATA {
                // New instances appeared in between: ask again.
                continue;
            }
            if r != 0 {
                return Vec::new();
            }
            return (0..count as usize)
                .filter_map(|i| {
                    // SAFETY: Windows wrote `count` items (and their names) into `buf`.
                    let it = unsafe { &*items.add(i) };
                    if it.FmtValue.CStatus > 1 {
                        return None;
                    }
                    let name = unsafe { it.szName.to_string() }.ok()?;
                    let v = unsafe { it.FmtValue.Anonymous.doubleValue };
                    v.is_finite().then_some((name, v))
                })
                .collect();
        }
        Vec::new()
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
            if self.sensors != 0 {
                PdhCloseQuery(self.sensors);
            }
        }
    }
}

// ---------------------------------------------------------------- GPU

/// The graphics card with the most video memory, as DXGI names it, read
/// through the graphics kernel.
struct Gpu {
    name: String,
    luid: LUID,
    mem_total: u64,
    integrated: bool,
    /// The kernel handle for the driver's readings (temperature, fan, clock).
    adapter: Option<u32>,
    /// Each engine's type, by engine (node) number.
    engines: Vec<Engine>,
    segments: u32,
    /// Each engine's running time (100 ns) at the last reading, and when.
    prev: (Instant, Vec<i64>),
    /// The last good reading, shown while the kernel misses one, and the misses in a row.
    last: Option<GpuReading>,
    misses: u32,
    /// (vendor, device, subsystem) PCI ids, as DXGI gives them.
    pci: (u32, u32, u32),
    /// The card's own power reading (NVIDIA's library), while the page is
    /// open, and whether this opening tried to load it.
    nvml: Option<Nvml>,
    nvml_tried: bool,
}

impl Gpu {
    fn open() -> Option<Gpu> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
        let mut best: Option<(u64, u64, String, LUID)> = None;
        let mut pci = (0, 0, 0);
        for i in 0.. {
            let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
            // Windows' own software renderers (WARP, the basic display and
            // render drivers, Hyper-V's) aren't graphics cards.
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || desc.VendorId == MICROSOFT_VENDOR {
                continue;
            }
            let mem = desc.DedicatedVideoMemory as u64;
            if best.as_ref().is_none_or(|(m, ..)| mem > *m) {
                let name = String::from_utf16_lossy(&desc.Description[..desc.Description.iter().position(|c| *c == 0).unwrap_or(desc.Description.len())]);
                best = Some((mem, desc.SharedSystemMemory as u64, name.trim().to_string(), desc.AdapterLuid));
                pci = (desc.VendorId, desc.DeviceId, desc.SubSysId);
            }
        }
        let (dedicated, shared, name, luid) = best?;
        let mut gpu = Gpu {
            name,
            luid,
            mem_total: dedicated,
            integrated: false,
            adapter: None,
            engines: Vec::new(),
            segments: 0,
            prev: (Instant::now(), Vec::new()),
            last: None,
            misses: 0,
            pci,
            nvml: None,
            nvml_tried: false,
        };
        // SAFETY (all unions below): the kernel filled the member the query's type names.
        let info = gpu.stats(D3DKMT_QUERYSTATISTICS_ADAPTER, |_| {}).map(|r| unsafe { r.AdapterInformation });
        let Some(info) = info.filter(|i| i.NodeCount > 0) else {
            warn_once(1 << 10, "GPU engine statistics unavailable");
            return None;
        };
        gpu.segments = info.NbSegments;
        // Built into the processor: its own memory is RAM the firmware set
        // aside (SegmentProperties: system memory, reserved by firmware), or
        // too little to be a card's. Its real memory is then the shared RAM too.
        let ram_backed = (0..gpu.segments).filter_map(|id| gpu.segment(id)).any(|seg| seg.Aperture == 0 && seg.SegmentProperties._bitfield & 0b11 != 0);
        gpu.integrated = ram_backed || dedicated <= INTEGRATED_UP_TO;
        if gpu.integrated {
            gpu.mem_total = dedicated + shared;
        }
        let mut open = D3DKMT_OPENADAPTERFROMLUID { AdapterLuid: luid, hAdapter: 0 };
        gpu.adapter = (unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.0 >= 0).then_some(open.hAdapter);
        gpu.engines = (0..info.NodeCount)
            .map(|node| {
                let mut meta = D3DKMT_NODEMETADATA { NodeOrdinalAndAdapterIndex: node, ..Default::default() };
                if gpu.adapter_info(KMTQAITYPE_NODEMETADATA, &mut meta) {
                    let t = meta.NodeData.EngineType;
                    Engine::from_type(t.0)
                } else {
                    Engine::Other
                }
            })
            .collect();
        gpu.prev = (Instant::now(), gpu.running_times()?);
        Some(gpu)
    }

    /// The card's power reader opens with the page (once per opening) and
    /// goes when it closes.
    fn page_open(&mut self, open: bool) {
        if !open {
            self.nvml = None;
            self.nvml_tried = false;
        } else if !self.nvml_tried {
            self.nvml_tried = true;
            self.nvml = Nvml::open(self.pci);
        }
    }

    fn segment(&self, id: u32) -> Option<windows::Wdk::Graphics::Direct3D::D3DKMT_QUERYSTATISTICS_SEGMENT_INFORMATION> {
        let r = self.stats(D3DKMT_QUERYSTATISTICS_SEGMENT, |q| q.Anonymous.QuerySegment.SegmentId = id)?;
        // SAFETY: a segment query fills the segment member.
        Some(unsafe { r.SegmentInformation })
    }

    fn stats(&self, kind: D3DKMT_QUERYSTATISTICS_TYPE, query: impl FnOnce(&mut D3DKMT_QUERYSTATISTICS)) -> Option<D3DKMT_QUERYSTATISTICS_RESULT> {
        let mut q = D3DKMT_QUERYSTATISTICS { Type: kind, AdapterLuid: self.luid, ..Default::default() };
        query(&mut q);
        // The kernel writes its answer into the same structure (the header calls it const).
        let p: *mut D3DKMT_QUERYSTATISTICS = &mut q;
        (unsafe { D3DKMTQueryStatistics(p) }.0 >= 0).then_some(q.QueryResult)
    }

    /// One of the driver's answers about the adapter (`T` is the structure `kind` fills).
    fn adapter_info<T>(&self, kind: KMTQUERYADAPTERINFOTYPE, data: &mut T) -> bool {
        let Some(h) = self.adapter else { return false };
        let mut q = D3DKMT_QUERYADAPTERINFO {
            hAdapter: h,
            Type: kind,
            pPrivateDriverData: (data as *mut T).cast(),
            PrivateDriverDataSize: std::mem::size_of::<T>() as u32,
        };
        unsafe { D3DKMTQueryAdapterInfo(&mut q) }.0 >= 0
    }

    /// Each engine's running time, or None when the kernel didn't answer for one.
    fn running_times(&self) -> Option<Vec<i64>> {
        (0..self.engines.len() as u32)
            .map(|node| {
                let r = self.stats(D3DKMT_QUERYSTATISTICS_NODE, |q| q.Anonymous.QueryNode.NodeId = node)?;
                // SAFETY: a node query fills the node member.
                Some(unsafe { r.NodeInformation.GlobalInformation.RunningTime })
            })
            .collect()
    }

    /// A missed answer shows the last reading and keeps the last good times
    /// to measure from, so the next good one isn't the engines' whole life.
    fn reading(&mut self, full: bool) -> Option<GpuReading> {
        let at = Instant::now();
        let Some(times) = self.running_times() else {
            self.misses += 1;
            return self.last.clone();
        };
        self.misses = 0;
        let busy = gpu_busy(&self.engines, &self.prev.1, &times, at.duration_since(self.prev.0).as_secs_f64() * 1e7);
        self.prev = (at, times);
        let e = busy?;
        // Memory in use: the card's own segments, or everything a GPU built into the processor holds.
        let (mut own, mut shared) = (0u64, 0u64);
        for seg in (0..self.segments).filter_map(|id| self.segment(id)) {
            if seg.Aperture != 0 {
                shared += seg.BytesResident;
            } else {
                own += seg.BytesResident;
            }
        }
        let mut r = GpuReading {
            name: self.name.clone(),
            util: e.util,
            engine_3d: e.engine_3d,
            copy: e.copy,
            video_decode: e.video_decode,
            video_encode: e.video_encode,
            mem_used: if self.integrated { own + shared } else { own },
            mem_total: self.mem_total,
            integrated: self.integrated,
            ..GpuReading::default()
        };
        if full {
            let mut p = D3DKMT_ADAPTER_PERFDATA::default();
            if self.adapter_info(KMTQAITYPE_ADAPTERPERFDATA, &mut p) {
                // Tenths of a degree; drivers without a sensor report 0.
                r.temp_c = (p.Temperature > 0).then(|| p.Temperature as f32 / 10.0).filter(|c| *c < 130.0);
                r.fan_rpm = (p.FanRPM > 0).then_some(p.FanRPM);
            }
            if let Some(node) = self.engines.iter().position(|e| *e == Engine::ThreeD) {
                let mut n = D3DKMT_NODE_PERFDATA { NodeOrdinal: node as u32, ..Default::default() };
                if self.adapter_info(KMTQAITYPE_NODEPERFDATA, &mut n) {
                    r.mhz = u32::try_from(n.Frequency / 1_000_000).ok().filter(|m| *m > 0);
                }
            }
            r.power_w = self.nvml.as_ref().and_then(Nvml::watts);
        }
        self.last = Some(r.clone());
        Some(r)
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some(h) = self.adapter {
            unsafe {
                let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER { hAdapter: h });
            }
        }
    }
}

/// `nvmlPciInfo_t`, as NVML's v3 query fills it.
#[repr(C)]
struct NvmlPci {
    bus_id_legacy: [u8; 16],
    domain: u32,
    bus: u32,
    device: u32,
    /// Device id in the high half, vendor id in the low.
    pci_device_id: u32,
    pci_subsystem_id: u32,
    bus_id: [u8; 32],
}

type NvmlCall = unsafe extern "C" fn() -> i32;
type NvmlCount = unsafe extern "C" fn(*mut u32) -> i32;
type NvmlByIndex = unsafe extern "C" fn(u32, *mut usize) -> i32;
type NvmlPciInfo = unsafe extern "C" fn(usize, *mut NvmlPci) -> i32;
type NvmlPower = unsafe extern "C" fn(usize, *mut u32) -> i32;

/// An NVIDIA card's own power reading, from NVML: the library the NVIDIA
/// driver puts in System32 (what nvidia-smi reads). Windows' graphics kernel
/// gives no watts. Loaded while the page is open, released when it closes.
struct Nvml {
    lib: HMODULE,
    device: usize,
    power: NvmlPower,
    shutdown: NvmlCall,
}

impl Nvml {
    /// The NVML device for the card DXGI picked (matched by its PCI ids; the
    /// only one when there's just one).
    fn open((vendor, device, subsys): (u32, u32, u32)) -> Option<Nvml> {
        if vendor != NVIDIA_VENDOR {
            return None;
        }
        unsafe {
            let lib = LoadLibraryExW(w!("nvml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
            let found = Nvml::device(lib, (device << 16) | vendor, subsys);
            if found.is_none() {
                warn_once(1 << 13, "the NVIDIA card's power reading is unavailable");
                let _ = FreeLibrary(lib);
            }
            found
        }
    }

    unsafe fn device(lib: HMODULE, pci_id: u32, subsys: u32) -> Option<Nvml> {
        let init: NvmlCall = std::mem::transmute(GetProcAddress(lib, s!("nvmlInit_v2"))?);
        let shutdown: NvmlCall = std::mem::transmute(GetProcAddress(lib, s!("nvmlShutdown"))?);
        let count: NvmlCount = std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetCount_v2"))?);
        let by_index: NvmlByIndex = std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetHandleByIndex_v2"))?);
        let pci_info: NvmlPciInfo = std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetPciInfo_v3"))?);
        let power: NvmlPower = std::mem::transmute(GetProcAddress(lib, s!("nvmlDeviceGetPowerUsage"))?);
        if init() != 0 {
            return None;
        }
        let mut n = 0u32;
        let mut pick = None;
        if count(&mut n) == 0 {
            for i in 0..n {
                let mut d = 0usize;
                if by_index(i, &mut d) != 0 {
                    continue;
                }
                let mut info: NvmlPci = std::mem::zeroed();
                let matches = pci_info(d, &mut info) == 0 && info.pci_device_id == pci_id && info.pci_subsystem_id == subsys;
                if matches || n == 1 {
                    pick = Some(d);
                    break;
                }
            }
        }
        match pick {
            Some(device) => Some(Nvml { lib, device, power, shutdown }),
            None => {
                shutdown();
                None
            }
        }
    }

    /// The whole board's draw (W): NVML averages it over a second on cards
    /// from Ampere on.
    fn watts(&self) -> Option<f32> {
        let mut mw = 0u32;
        (unsafe { (self.power)(self.device, &mut mw) } == 0).then(|| mw as f32 / 1000.0).filter(|w| (0.5..=3000.0).contains(w))
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
            let _ = FreeLibrary(self.lib);
        }
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

/// Every interface Windows has: adapters, and dozens of filters and tunnels.
fn if_table() -> Option<Vec<MIB_IF_ROW2>> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if let Err(e) = unsafe { GetIfTable2(&mut table) }.ok() {
        warn_once(16, &format!("network counters unavailable ({e})"));
        return None;
    }
    // SAFETY: Windows allocated `NumEntries` rows after the header (the
    // declared one-row array is only the first of them); freed below.
    let rows = unsafe { std::slice::from_raw_parts(std::ptr::addr_of!((*table).Table).cast::<MIB_IF_ROW2>(), (*table).NumEntries as usize) }.to_vec();
    unsafe { FreeMibTable(table as *const core::ffi::c_void) };
    Some(rows)
}

/// Just these interfaces (by LUID); None when one has gone.
fn if_rows(luids: &[u64]) -> Option<Vec<MIB_IF_ROW2>> {
    luids
        .iter()
        .map(|luid| {
            let mut row = MIB_IF_ROW2 { InterfaceLuid: NET_LUID_LH { Value: *luid }, ..Default::default() };
            unsafe { GetIfEntry2(&mut row) }.is_ok().then_some(row)
        })
        .collect()
}

/// Totals over the connected hardware adapters among `rows`, and each one's
/// (LUID, received, sent) so the caller can measure speeds per adapter; the
/// adapter shown is the route's, else the busiest, with its address and link speed.
fn net_reading(rows: &[MIB_IF_ROW2], route: Option<u32>) -> (NetReading, Vec<(u64, u64, u64)>) {
    let mut n = NetReading::default();
    let mut adapters = Vec::new();
    let mut busiest: Option<(u64, &MIB_IF_ROW2)> = None;
    let mut routed: Option<&MIB_IF_ROW2> = None;
    for r in rows {
        let flags = r.InterfaceAndOperStatusFlags._bitfield;
        if flags & IF_HARDWARE == 0 || flags & IF_FILTER != 0 || r.OperStatus != IfOperStatusUp || r.Type == IF_TYPE_SOFTWARE_LOOPBACK {
            continue;
        }
        n.received = n.received.saturating_add(r.InOctets);
        n.sent = n.sent.saturating_add(r.OutOctets);
        // SAFETY: NET_LUID_LH is a 64-bit value with a bit-field view.
        adapters.push((unsafe { r.InterfaceLuid.Value }, r.InOctets, r.OutOctets));
        if route == Some(r.InterfaceIndex) {
            routed = Some(r);
        }
        let traffic = r.InOctets.saturating_add(r.OutOctets);
        if busiest.is_none_or(|(t, _)| traffic > t) {
            busiest = Some((traffic, r));
        }
    }
    if let Some(r) = routed.or(busiest.map(|(_, r)| r)) {
        n.adapter = String::from_utf16_lossy(&r.Alias[..r.Alias.iter().position(|c| *c == 0).unwrap_or(r.Alias.len())]);
        let len = (r.PhysicalAddressLength as usize).min(r.PhysicalAddress.len());
        n.mac = r.PhysicalAddress[..len].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
        n.link_bps = r.ReceiveLinkSpeed.max(r.TransmitLinkSpeed);
    }
    n.online = busiest.is_some();
    (n, adapters)
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
    // The path runs past the declared one-character array.
    let path = std::slice::from_raw_parts(std::ptr::addr_of!((*detail).DevicePath).cast::<u16>(), chars);
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
