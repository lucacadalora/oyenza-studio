//! What the System page shows: CPU, GPU, memory, drives, network and battery.
//!
//! The Windows sampler (`poni::services::sysmon`) reads the kernel's own
//! counters, the way Stats reads macOS's (`host_processor_info`,
//! `host_statistics64`, IOKit), and hands them in raw; everything arithmetic
//! lives here, so it is tested on any machine: each core's load from two
//! samples of the processor times, per-app CPU, memory and I/O from two
//! process lists (grouped by executable), both lists' binary layouts, the
//! minute of history behind each chart, and the number formats the page uses.

use std::collections::{HashMap, VecDeque};

/// Samples kept per chart: one a second, so a minute.
pub const HISTORY_LEN: usize = 60;
/// Processes listed for CPU, memory and drives (Stats lists eight too).
pub const TOP_APPS: usize = 8;

/// The page's modules, in list order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Module {
    #[default]
    Cpu,
    Gpu,
    Memory,
    Drives,
    Network,
    /// A desktop's whole draw (`crate::power`): its power supply's reading,
    /// or an estimate from the parts that report their own.
    Power,
    Battery,
    /// Show over games: the frame rate and load while a game is in front.
    Games,
}

impl Module {
    pub const ALL: [Module; 8] = [Module::Cpu, Module::Gpu, Module::Memory, Module::Drives, Module::Network, Module::Power, Module::Battery, Module::Games];

    pub fn title(self) -> &'static str {
        match self {
            Module::Cpu => "CPU",
            Module::Gpu => "GPU",
            Module::Memory => "Memory",
            Module::Drives => "Drives",
            Module::Network => "Network",
            Module::Power => "Power",
            Module::Battery => "Battery",
            Module::Games => "Games",
        }
    }
}

/// What kind of core a logical processor sits on (hybrid CPUs mix two).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoreKind {
    /// Every core is the same kind.
    #[default]
    Any,
    Performance,
    Efficiency,
}

/// One logical processor's load and kind.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CoreLoad {
    pub load: f32,
    pub kind: CoreKind,
}

/// Processor load between the last two samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuReading {
    /// Busy share of all logical processors, 0..=1.
    pub load: f32,
    /// The part of `load` spent in apps (user mode) and in Windows itself (kernel mode).
    pub user: f32,
    pub kernel: f32,
    /// The part of `kernel` spent answering hardware (interrupts and deferred calls).
    pub interrupt: f32,
    /// Each logical processor in Windows' order (empty when only the total is known).
    pub cores: Vec<CoreLoad>,
    /// Logical processors (threads) across all processor groups.
    pub logical: u32,
    /// The processor's base speed, which turbo speeds pass (MHz).
    pub base_mhz: Option<u32>,
    /// Current speed: all cores, and the performance and efficiency cores of a hybrid CPU.
    pub mhz: Option<u32>,
    pub mhz_p: Option<u32>,
    pub mhz_e: Option<u32>,
    /// The warmest ACPI thermal zone, when the firmware reports a real one (°C).
    pub temp_c: Option<f32>,
    /// Package power from the processor's energy counters, when Windows exposes them (W).
    pub power_w: Option<f32>,
    pub uptime_s: u64,
}

impl CpuReading {
    /// (performance, efficiency) core counts on a hybrid CPU.
    pub fn hybrid(&self) -> Option<(usize, usize)> {
        let p = self.cores.iter().filter(|c| c.kind == CoreKind::Performance).count();
        let e = self.cores.iter().filter(|c| c.kind == CoreKind::Efficiency).count();
        (p > 0 && e > 0).then_some((p, e))
    }

    /// Average load of one kind of core.
    pub fn kind_load(&self, kind: CoreKind) -> Option<f32> {
        let v: Vec<f32> = self.cores.iter().filter(|c| c.kind == kind).map(|c| c.load).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryReading {
    pub total: u64,
    pub used: u64,
    /// Memory Windows has promised to programs, and its limit (RAM plus the
    /// page file). Committed close to the limit is Windows' memory pressure.
    /// macOS rates the pressure itself: there the limit is the RAM and
    /// committed the share the kernel's level puts the gauge at ([`level_pressure`]).
    pub committed: u64,
    pub commit_limit: u64,
    /// Files and data kept in memory that Windows hands back at once (standby and modified lists).
    pub cached: u64,
    /// Kernel memory that can't be paged out (the nonpaged pool): Stats' "wired".
    pub wired: u64,
    /// Held in the Memory Compression store.
    pub compressed: u64,
    /// Free and zeroed pages.
    pub free: u64,
    /// The page file in use and its size.
    pub page_used: u64,
    pub page_total: u64,
}

/// The parts that make up physical memory, in the order the page draws them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemPart {
    App,
    Wired,
    Compressed,
    Cached,
    Free,
}

impl MemoryReading {
    pub fn used_frac(&self) -> f32 {
        frac(self.used, self.total)
    }

    /// Committed against the limit: how hard Windows is pressed for memory.
    pub fn pressure(&self) -> f32 {
        frac(self.committed, self.commit_limit)
    }

    /// Physical memory split into its parts; they add up to `total`. Apps get
    /// what the kernel, the compression store, the cache and free pages leave.
    pub fn parts(&self) -> [(MemPart, u64); 5] {
        let wired = self.wired.min(self.total);
        let compressed = self.compressed.min(self.total - wired);
        let cached = self.cached.min(self.total - wired - compressed);
        let free = self.free.min(self.total - wired - compressed - cached);
        let app = self.total - wired - compressed - cached - free;
        [(MemPart::App, app), (MemPart::Wired, wired), (MemPart::Compressed, compressed), (MemPart::Cached, cached), (MemPart::Free, free)]
    }
}

/// Memory pressure in words, like Stats' gauge.
pub fn pressure_word(p: f32) -> &'static str {
    if p < 0.75 {
        "Normal"
    } else if p < 0.9 {
        "Warning"
    } else {
        "Critical"
    }
}

/// One fixed drive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drive {
    /// "C:".
    pub root: String,
    /// Volume label ("Windows", "Data"); may be empty.
    pub label: String,
    pub total: u64,
    pub free: u64,
    /// The drive Windows runs from.
    pub system: bool,
}

impl Drive {
    pub fn used_frac(&self) -> f32 {
        frac(self.total.saturating_sub(self.free), self.total)
    }
}

/// Activity of all physical disks together.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DiskActivity {
    pub read_bps: f64,
    pub write_bps: f64,
    /// Share of time the disks were busy (Task Manager's "Active time"), 0..=1.
    pub active: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetReading {
    /// A real (hardware) adapter is connected.
    pub online: bool,
    /// The adapter's name in Windows ("Wi-Fi", "Ethernet"), the busiest one when several are up.
    pub adapter: String,
    pub ipv4: Option<String>,
    /// The adapter's hardware address ("3C:22:FB:0A:11:9E") and link speed (bits/s).
    pub mac: String,
    pub link_bps: u64,
    pub down_bps: f64,
    pub up_bps: f64,
    /// Bytes received and sent since the adapters connected.
    pub received: u64,
    pub sent: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryReading {
    pub percent: u8,
    pub charging: bool,
    pub on_ac: bool,
    pub saver: bool,
    /// Windows' estimate while running on the battery.
    pub secs_left: Option<u32>,
    /// Capacity when new and at full charge today, in mWh.
    pub design_mwh: Option<u32>,
    pub full_mwh: Option<u32>,
    pub cycles: Option<u32>,
    /// Power going into (+) or out of (−) the battery, in mW.
    pub rate_mw: Option<i32>,
}

impl BatteryReading {
    /// Full-charge capacity against the design capacity (a new battery can
    /// read a little over 1).
    pub fn health(&self) -> Option<f32> {
        match (self.design_mwh, self.full_mwh) {
            (Some(d), Some(f)) if d > 0 && f > 0 => Some((f as f64 / d as f64).min(1.5) as f32),
            _ => None,
        }
    }
}

/// The graphics card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuReading {
    /// "NVIDIA GeForce RTX 4080".
    pub name: String,
    /// The busiest engine's share, 0..=1 (how Windows rates a GPU's load).
    pub util: f32,
    /// On a Mac, the renderer's share (Apple GPUs' pixel work).
    pub engine_3d: f32,
    pub copy: f32,
    pub video_decode: f32,
    pub video_encode: f32,
    /// The tiler's share (Apple GPUs' geometry work; macOS only).
    pub tiler: f32,
    /// Video memory in use and in all: the card's own (dedicated), or the
    /// share of RAM it may use on a GPU built into the processor.
    pub mem_used: u64,
    pub mem_total: u64,
    /// Built into the processor (memory shared with Windows).
    pub integrated: bool,
    /// What the driver reports, when it does: temperature, fan and the 3D engine's clock.
    pub temp_c: Option<f32>,
    pub fan_rpm: Option<u32>,
    pub mhz: Option<u32>,
    /// The whole card's draw, when its maker's library reports it (W).
    pub power_w: Option<f32>,
}

/// The processes of one executable taken together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppUse {
    /// Executable file name in lower case ("chrome.exe"): the grouping key.
    pub image: String,
    /// What the page shows: the file's description when the sampler found
    /// one ("Google Chrome"), else the file name without ".exe".
    pub name: String,
    /// Full path of the executable, for its icon (None when Windows won't tell).
    pub path: Option<String>,
    /// The group's largest process (its path stands for the group).
    pub pid: u64,
    /// Share of all logical processors, 0..=1.
    pub cpu: f32,
    /// Private working set in bytes.
    pub memory: u64,
    /// File and device reads and writes, bytes/s.
    pub io: f64,
    pub processes: u32,
}

/// One reading of everything the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub cpu: Option<CpuReading>,
    pub gpu: Option<GpuReading>,
    pub memory: Option<MemoryReading>,
    pub drives: Vec<Drive>,
    pub disk: Option<DiskActivity>,
    pub network: Option<NetReading>,
    /// None on a PC without a battery.
    pub battery: Option<BatteryReading>,
    /// The whole PC's draw, on a desktop while the page is open.
    pub power: Option<crate::power::PowerReading>,
    pub top_cpu: Vec<AppUse>,
    pub top_memory: Vec<AppUse>,
    pub top_io: Vec<AppUse>,
}

impl Reading {
    /// The drive Windows runs from, else the first one.
    pub fn system_drive(&self) -> Option<&Drive> {
        self.drives.iter().find(|d| d.system).or(self.drives.first())
    }
}

fn frac(part: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

// ---------------------------------------------------------------- history

/// The last minute of one value, oldest first. A missing sample is NaN, so a
/// chart leaves a gap instead of drawing a false zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series(VecDeque<f32>);

impl Series {
    pub fn push(&mut self, v: Option<f32>) {
        if self.0.len() == HISTORY_LEN {
            self.0.pop_front();
        }
        self.0.push_back(v.filter(|v| v.is_finite()).unwrap_or(f32::NAN));
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn values(&self) -> impl Iterator<Item = f32> + '_ {
        self.0.iter().copied()
    }

    /// The largest value in the minute (0 when there is none).
    pub fn max(&self) -> f32 {
        self.0.iter().copied().filter(|v| v.is_finite()).fold(0.0, f32::max)
    }
}

/// The history behind the charts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub cpu: Series,
    pub gpu: Series,
    pub memory: Series,
    pub disk_read: Series,
    pub disk_write: Series,
    pub net_down: Series,
    pub net_up: Series,
    /// The whole PC's draw (W).
    pub power: Series,
}

impl History {
    pub fn record(&mut self, r: &Reading) {
        self.cpu.push(r.cpu.as_ref().map(|c| c.load));
        self.gpu.push(r.gpu.as_ref().map(|g| g.util));
        self.memory.push(r.memory.as_ref().map(MemoryReading::used_frac));
        self.disk_read.push(r.disk.map(|d| d.read_bps as f32));
        self.disk_write.push(r.disk.map(|d| d.write_bps as f32));
        let net = r.network.as_ref().filter(|n| n.online);
        self.net_down.push(net.map(|n| n.down_bps as f32));
        self.net_up.push(net.map(|n| n.up_bps as f32));
        self.power.push(r.power.as_ref().map(|p| p.total_w));
    }
}

/// What the page holds: the latest reading and the minute behind it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Monitor {
    pub latest: Option<Reading>,
    pub history: History,
}

impl Monitor {
    pub fn push(&mut self, r: Reading) {
        self.history.record(&r);
        self.latest = Some(r);
    }

    pub fn clear(&mut self) {
        *self = Monitor::default();
    }
}

/// When the readings rest: nobody at the PC, or battery saver on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rest {
    /// The session is locked or the display is off: nothing is on screen, so
    /// nothing is read at all.
    pub away: bool,
    /// Battery saver: the minute kept while the page is closed rests; the open
    /// page and the readout over a game keep reading.
    pub saver: bool,
}

/// Whether the reader takes readings: for the open page or the readout over a
/// game while someone is there, and for the minute kept while the page is
/// closed (`background`) unless nobody is there or battery saver is on. When
/// it stops, the kept minute goes, and the next one starts fresh.
pub fn reads(open: bool, game: bool, background: bool, rest: Rest) -> bool {
    !rest.away && (open || game || (background && !rest.saver))
}

// ---------------------------------------------------------------- CPU

/// Cumulative processor times in 100 ns ticks, as `GetSystemTimes` reports
/// them (kernel time includes idle time).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

/// Load between two samples as (busy, user, kernel) shares of all logical
/// processors. None when no time passed or a counter went backwards.
pub fn cpu_split(prev: CpuTimes, now: CpuTimes) -> Option<(f32, f32, f32)> {
    let idle = now.idle.checked_sub(prev.idle)?;
    let kernel = now.kernel.checked_sub(prev.kernel)?;
    let user = now.user.checked_sub(prev.user)?;
    let total = kernel.checked_add(user)?;
    if total == 0 {
        return None;
    }
    let t = total as f64;
    let user_f = (user as f64 / t).clamp(0.0, 1.0) as f32;
    let kernel_f = (kernel.saturating_sub(idle) as f64 / t).clamp(0.0, 1.0) as f32;
    Some(((user_f + kernel_f).min(1.0), user_f, kernel_f))
}

/// One logical processor's cumulative times in 100 ns ticks, as
/// `NtQuerySystemInformation(SystemProcessorPerformanceInformation)` lists
/// them (the counterpart of macOS's `host_processor_info`). Kernel time
/// includes idle, deferred-call (DPC) and interrupt time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
    pub dpc: u64,
    pub interrupt: u64,
}

/// Bytes per entry of that list: five 64-bit times, a 32-bit interrupt count, padding.
const CORE_RECORD: usize = 48;

/// Read a `SystemProcessorPerformanceInformation` buffer, one entry per logical processor.
pub fn parse_core_times(buf: &[u8]) -> Vec<CoreTimes> {
    buf.chunks_exact(CORE_RECORD)
        .map(|r| CoreTimes { idle: read_u64(r, 0), kernel: read_u64(r, 8), user: read_u64(r, 16), dpc: read_u64(r, 24), interrupt: read_u64(r, 32) })
        .collect()
}

/// Loads between two samples of every core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoreSplit {
    /// Each core's busy share, in order.
    pub cores: Vec<f32>,
    /// Shares of the whole machine: busy, apps, Windows, and the hardware part of Windows.
    pub load: f32,
    pub user: f32,
    pub kernel: f32,
    pub interrupt: f32,
}

/// Every core's load between two samples. None when the core count changed
/// or no time passed; a counter that went backwards counts as no time.
pub fn core_split(prev: &[CoreTimes], now: &[CoreTimes]) -> Option<CoreSplit> {
    if prev.len() != now.len() || now.is_empty() {
        return None;
    }
    let mut cores = Vec::with_capacity(now.len());
    let (mut total, mut idle, mut user, mut hw) = (0u64, 0u64, 0u64, 0u64);
    for (a, b) in prev.iter().zip(now) {
        let k = b.kernel.saturating_sub(a.kernel);
        let u = b.user.saturating_sub(a.user);
        let t = k.saturating_add(u);
        let i = b.idle.saturating_sub(a.idle).min(t);
        cores.push(if t == 0 { 0.0 } else { (1.0 - i as f64 / t as f64) as f32 });
        total += t;
        idle += i;
        user += u;
        hw += b.dpc.saturating_sub(a.dpc).saturating_add(b.interrupt.saturating_sub(a.interrupt));
    }
    if total == 0 {
        return None;
    }
    let t = total as f64;
    let load = (1.0 - idle as f64 / t) as f32;
    let user = ((user as f64 / t) as f32).min(load);
    let kernel = (load - user).max(0.0);
    let interrupt = ((hw as f64 / t) as f32).min(kernel);
    Some(CoreSplit { cores, load, user, kernel, interrupt })
}

/// Core kinds from each logical processor's efficiency class
/// (`GetSystemCpuSetInformation`): on a hybrid CPU the highest class is the
/// performance cores, the rest efficiency cores; one class means one kind.
pub fn core_kinds(classes: &[u8]) -> Vec<CoreKind> {
    let (Some(lo), Some(hi)) = (classes.iter().min(), classes.iter().max()) else { return Vec::new() };
    if lo == hi {
        return vec![CoreKind::Any; classes.len()];
    }
    classes.iter().map(|c| if c == hi { CoreKind::Performance } else { CoreKind::Efficiency }).collect()
}

// ---------------------------------------------------------------- GPU engines

/// What a graphics engine does, from the graphics kernel's `DXGK_ENGINE_TYPE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Engine {
    #[default]
    Other,
    ThreeD,
    VideoDecode,
    VideoEncode,
    Copy,
}

impl Engine {
    pub fn from_type(t: i32) -> Engine {
        match t {
            1 => Engine::ThreeD,
            2 => Engine::VideoDecode,
            3 => Engine::VideoEncode,
            6 => Engine::Copy,
            _ => Engine::Other,
        }
    }
}

/// Engine loads of one adapter, 0..=1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuEngines {
    pub util: f32,
    pub engine_3d: f32,
    pub copy: f32,
    pub video_decode: f32,
    pub video_encode: f32,
}

/// Engine loads from two samples of every engine's running time (100 ns
/// ticks, `D3DKMT_QUERYSTATISTICS_NODE`) taken `elapsed` ticks apart. A
/// type's load is its busiest engine; the adapter's is its busiest engine of
/// all, the way Windows rates a GPU. None when the engine count changed or no
/// time passed.
pub fn gpu_busy(kinds: &[Engine], prev: &[i64], now: &[i64], elapsed: f64) -> Option<GpuEngines> {
    if elapsed <= 0.0 || prev.len() != now.len() || now.is_empty() {
        return None;
    }
    let mut e = GpuEngines::default();
    for (i, (a, b)) in prev.iter().zip(now).enumerate() {
        let busy = ((b - a).max(0) as f64 / elapsed).clamp(0.0, 1.0) as f32;
        e.util = e.util.max(busy);
        let slot = match kinds.get(i).copied().unwrap_or_default() {
            Engine::ThreeD => &mut e.engine_3d,
            Engine::Copy => &mut e.copy,
            Engine::VideoDecode => &mut e.video_decode,
            Engine::VideoEncode => &mut e.video_encode,
            Engine::Other => continue,
        };
        *slot = slot.max(busy);
    }
    Some(e)
}

// ---------------------------------------------------------------- apps

/// One process from a process list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcSample {
    pub pid: u64,
    /// Executable file name as Windows reports it ("chrome.exe").
    pub image: String,
    /// Kernel plus user time since the process started, 100 ns ticks.
    pub cpu_time: u64,
    /// Private working set in bytes.
    pub memory: u64,
    /// Bytes read and written since the process started (file and device I/O).
    pub io: u64,
}

/// Entries of the process list that aren't apps: the idle counter and
/// Windows' own memory bookkeeping (Task Manager's Processes tab leaves them out too).
const NOT_APPS: [&str; 6] = ["", "idle", "system idle process", "memory compression", "registry", "secure system"];

/// Turns successive process lists into per-app usage.
#[derive(Debug, Default)]
pub struct AppTracker {
    /// pid → (lower-case image, cpu time, I/O bytes) at the previous call.
    prev: HashMap<u64, (String, u64, u64)>,
    prev_at: Option<f64>,
}

impl AppTracker {
    /// Usage per executable since the previous call (unsorted). `now` is in
    /// seconds and `logical` is the number of logical processors. A process
    /// that is new since the last call counts 0 % CPU and no I/O: its
    /// counters began before it.
    pub fn update(&mut self, now: f64, logical: u32, procs: &[ProcSample]) -> Vec<AppUse> {
        let span = self.prev_at.map(|t| now - t).filter(|d| *d > 0.0);
        let ticks = span.map(|d| d * 1e7 * f64::from(logical.max(1)));
        let mut groups: HashMap<String, AppUse> = HashMap::new();
        let mut lead: HashMap<String, u64> = HashMap::new();
        let mut next = HashMap::with_capacity(procs.len());
        for p in procs {
            let image = p.image.to_lowercase();
            next.insert(p.pid, (image.clone(), p.cpu_time, p.io));
            if p.pid == 0 || NOT_APPS.contains(&image.as_str()) {
                continue;
            }
            let before = self.prev.get(&p.pid).filter(|(img, _, _)| *img == image);
            let cpu = match (ticks, before) {
                (Some(t), Some((_, c, _))) if p.cpu_time >= *c => ((p.cpu_time - c) as f64 / t) as f32,
                _ => 0.0,
            };
            let io = match (span, before) {
                (Some(d), Some((_, _, io))) if p.io >= *io => (p.io - io) as f64 / d,
                _ => 0.0,
            };
            let g = groups
                .entry(image.clone())
                .or_insert_with(|| AppUse { image: image.clone(), name: display_name(&p.image), pid: p.pid, ..AppUse::default() });
            let biggest = lead.entry(image).or_insert(0);
            if p.memory >= *biggest {
                *biggest = p.memory;
                g.pid = p.pid;
            }
            g.cpu += cpu;
            g.memory += p.memory;
            g.io += io;
            g.processes += 1;
        }
        self.prev = next;
        self.prev_at = Some(now);
        groups
            .into_values()
            .map(|mut g| {
                g.cpu = g.cpu.clamp(0.0, 1.0);
                g
            })
            .collect()
    }
}

/// The `n` apps using the most CPU (equal ones by memory, then by name).
pub fn top_cpu(apps: &[AppUse], n: usize) -> Vec<AppUse> {
    let mut v: Vec<&AppUse> = apps.iter().collect();
    v.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.memory.cmp(&a.memory)).then(a.image.cmp(&b.image)));
    v.into_iter().take(n).cloned().collect()
}

/// The `n` apps using the most memory (equal ones by name).
pub fn top_memory(apps: &[AppUse], n: usize) -> Vec<AppUse> {
    let mut v: Vec<&AppUse> = apps.iter().collect();
    v.sort_by(|a, b| b.memory.cmp(&a.memory).then(a.image.cmp(&b.image)));
    v.into_iter().take(n).cloned().collect()
}

/// The `n` apps reading and writing the most (equal ones by name), only those doing any.
pub fn top_io(apps: &[AppUse], n: usize) -> Vec<AppUse> {
    let mut v: Vec<&AppUse> = apps.iter().filter(|a| a.io >= 1.0).collect();
    v.sort_by(|a, b| b.io.total_cmp(&a.io).then(a.image.cmp(&b.image)));
    v.into_iter().take(n).cloned().collect()
}

/// "chrome.exe" → "chrome" (what the page shows until it finds the file's description).
pub fn display_name(image: &str) -> String {
    let n = image.len();
    if n > 4 && image.is_char_boundary(n - 4) && image[n - 4..].eq_ignore_ascii_case(".exe") {
        image[..n - 4].to_string()
    } else {
        image.to_string()
    }
}

// ---------------------------------------------------------------- process list layout

// `SYSTEM_PROCESS_INFORMATION` as `NtQuerySystemInformation(SystemProcessInformation)`
// fills it on 64-bit Windows (Poni ships x64 only). Offsets of the fields read:
const P_NEXT: usize = 0; // u32, bytes to the next entry, 0 on the last
const P_PRIVATE_WS: usize = 8; // i64, private working set
const P_USER_TIME: usize = 40; // i64, 100 ns
const P_KERNEL_TIME: usize = 48; // i64, 100 ns
const P_NAME_LEN: usize = 56; // u16, image name length in bytes (UNICODE_STRING)
const P_NAME_PTR: usize = 64; // pointer to the UTF-16 image name, inside the same buffer
const P_PID: usize = 80; // HANDLE, the process id
const P_IO_READ: usize = 232; // ReadTransferCount, bytes
const P_IO_WRITE: usize = 240; // WriteTransferCount, bytes
/// Bytes an entry needs for every field above.
const P_MIN: usize = 248;

/// Read a process list buffer. `base` is the address of `buf[0]`: image names
/// point into the buffer by address. The walk stops at the last entry, or at
/// one that would run past the buffer or go backwards.
pub fn parse_process_list(buf: &[u8], base: usize) -> Vec<ProcSample> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off.checked_add(P_MIN).is_some_and(|end| end <= buf.len()) {
        let e = &buf[off..];
        let next = read_u32(e, P_NEXT) as usize;
        let name_len = read_u16(e, P_NAME_LEN) as usize;
        let name_ptr = read_u64(e, P_NAME_PTR) as usize;
        let image = name_ptr
            .checked_sub(base)
            .filter(|start| name_len > 0 && start.checked_add(name_len).is_some_and(|end| end <= buf.len()))
            .map(|start| utf16le(&buf[start..start + name_len]))
            .unwrap_or_default();
        let user = read_u64(e, P_USER_TIME);
        let kernel = read_u64(e, P_KERNEL_TIME);
        let io = read_u64(e, P_IO_READ).saturating_add(read_u64(e, P_IO_WRITE));
        out.push(ProcSample { pid: read_u64(e, P_PID), image, cpu_time: user.saturating_add(kernel), memory: read_u64(e, P_PRIVATE_WS), io });
        if next < P_MIN {
            break;
        }
        off += next;
    }
    out
}

fn read_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]))
}

fn read_u64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]))
}

fn utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

// ---------------------------------------------------------------- formats

const KIB: f64 = 1024.0;
const MIB: f64 = KIB * 1024.0;
const GIB: f64 = MIB * 1024.0;

/// "23%".
pub fn fmt_pct(f: f32) -> String {
    format!("{}%", (f * 100.0).round().clamp(0.0, 100.0) as i32)
}

/// An app's CPU share: "0%", "4.2%", "23%".
pub fn fmt_app_cpu(f: f32) -> String {
    let p = f64::from(f.clamp(0.0, 1.0)) * 100.0;
    if p < 0.05 {
        "0%".into()
    } else if p < 9.95 {
        format!("{p:.1}%")
    } else {
        format!("{p:.0}%")
    }
}

/// Sizes the way Windows shows them (1 GB = 1024³ bytes): "512 MB", "9.6 GB", "212 GB", "1.8 TB".
pub fn fmt_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / GIB;
    if gb >= 1000.0 {
        let tb = gb / 1024.0;
        if tb >= 10.0 {
            format!("{tb:.0} TB")
        } else {
            format!("{tb:.1} TB")
        }
    } else if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / MIB)
    }
}

/// Transfer speeds: "0 KB/s", "4.2 KB/s", "84 KB/s", "1.2 MB/s", "120 MB/s", "1.1 GB/s".
pub fn fmt_rate(bytes_per_s: f64) -> String {
    let b = if bytes_per_s.is_finite() { bytes_per_s.max(0.0) } else { 0.0 };
    let kb = b / KIB;
    if kb < 0.5 {
        "0 KB/s".into()
    } else if kb < 9.95 {
        format!("{kb:.1} KB/s")
    } else if kb < 999.5 {
        format!("{kb:.0} KB/s")
    } else if b < 9.95 * MIB {
        format!("{:.1} MB/s", b / MIB)
    } else if b < 999.5 * MIB {
        format!("{:.0} MB/s", b / MIB)
    } else {
        format!("{:.1} GB/s", b / GIB)
    }
}

/// "2.6 GHz", "800 MHz".
pub fn fmt_speed(mhz: u32) -> String {
    if mhz >= 1000 {
        format!("{:.1} GHz", f64::from(mhz) / 1000.0)
    } else {
        format!("{mhz} MHz")
    }
}

/// Time since Windows started: "3 d 4 h", "4 h 12 min", "12 min".
pub fn fmt_uptime(secs: u64) -> String {
    let mins = secs / 60;
    let (d, h, m) = (mins / 1440, mins / 60 % 24, mins % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

/// Battery time left: "2 h 14 min left", "35 min left".
pub fn fmt_time_left(secs: u32) -> String {
    let mins = secs / 60;
    if mins >= 60 {
        format!("{} h {} min left", mins / 60, mins % 60)
    } else {
        format!("{} min left", mins.max(1))
    }
}

/// Watts from milliwatts, without the sign: "8.2 W", "45 W".
pub fn fmt_watts(mw: i32) -> String {
    let w = f64::from(mw.unsigned_abs()) / 1000.0;
    if w < 9.95 {
        format!("{w:.1} W")
    } else {
        format!("{w:.0} W")
    }
}

/// A temperature: "54°".
pub fn fmt_temp(c: f32) -> String {
    format!("{}°", c.round() as i32)
}

/// A link speed in bits per second: "1 Gbps", "866 Mbps", "2.5 Gbps".
pub fn fmt_link(bps: u64) -> String {
    let m = bps as f64 / 1e6;
    if m >= 1000.0 {
        let g = m / 1000.0;
        if (g - g.round()).abs() < 0.05 {
            format!("{:.0} Gbps", g)
        } else {
            format!("{g:.1} Gbps")
        }
    } else {
        format!("{m:.0} Mbps")
    }
}

/// How busy the processor is, in words (the same words as Today's CPU ring).
pub fn load_word(f: f32) -> &'static str {
    match f {
        f if f < 0.10 => "Idle",
        f if f < 0.35 => "Light load",
        f if f < 0.70 => "Moderate",
        f if f < 0.90 => "Busy",
        _ => "Very busy",
    }
}

// ---------------------------------------------------------------- macOS

/// Memory pressure as a kernel that rates it in levels has it
/// (`kern.memorystatus_vm_pressure_level` on macOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressureLevel {
    Normal,
    Warning,
    Critical,
}

impl PressureLevel {
    /// macOS's numbers: 1 normal, 2 warning, 4 critical.
    pub fn from_kernel(v: u64) -> Option<PressureLevel> {
        match v {
            1 => Some(PressureLevel::Normal),
            2 => Some(PressureLevel::Warning),
            4 => Some(PressureLevel::Critical),
            _ => None,
        }
    }
}

/// The pressure gauge's value for a kernel that rates pressure in levels:
/// the level picks the zone [`pressure_word`] names, and `used` (how full
/// memory is by the kernel's own count, 0..=1) places the needle inside it,
/// a little in from its edges, so the gauge always says what the kernel
/// (and Activity Monitor) says.
pub fn level_pressure(level: PressureLevel, used: f32) -> f32 {
    let (lo, hi) = match level {
        PressureLevel::Normal => (0.0, 0.72),
        PressureLevel::Warning => (0.78, 0.86),
        PressureLevel::Critical => (0.92, 1.0),
    };
    if used.is_finite() {
        used.clamp(lo, hi)
    } else {
        lo
    }
}

/// The outermost app bundle an executable sits in: "/Applications/Google
/// Chrome.app" for Chrome's helpers deep inside it, so every process of an
/// app counts as that app. None outside a bundle.
pub fn app_bundle(path: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(i) = path[from..].find(".app/") {
        let end = from + i + 4;
        // ".app" must end a real name ("/Foo.app/"), not be the whole of one.
        if !path[..from + i].ends_with('/') && from + i > 0 {
            return Some(&path[..end]);
        }
        from = end;
    }
    None
}

/// What an app is called, from its bundle's path: "/Applications/Google
/// Chrome.app" → "Google Chrome".
pub fn bundle_name(bundle: &str) -> &str {
    let file = bundle.trim_end_matches('/').rsplit('/').next().unwrap_or(bundle);
    file.strip_suffix(".app").filter(|n| !n.is_empty()).unwrap_or(file)
}

/// Energy (mWh) in a battery holding `mah`, at its pack's nominal voltage:
/// 3.8 V a cell, the cells in series counted from the present voltage `mv`
/// (a lithium cell never rises past 4.4 V). For batteries that report their
/// charge, not their energy (macOS's), so they read in watt-hours as on Windows.
pub fn pack_mwh(mah: u32, mv: u32) -> Option<u32> {
    if mah == 0 || mv == 0 {
        return None;
    }
    let cells = (f64::from(mv) / 4400.0).ceil().max(1.0);
    u32::try_from((f64::from(mah) * cells * 3.8).round() as u64).ok()
}

/// One network interface as macOS's interface list (`sysctl` `NET_RT_IFLIST2`) has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IfStats {
    pub index: u16,
    /// "en0".
    pub name: String,
    /// `IFF_*` flags (up, running, loopback, …).
    pub flags: u32,
    /// `IFT_*` type: 6 is Ethernet, Wi-Fi included.
    pub kind: u8,
    /// Link speed, bits/s.
    pub baud: u64,
    /// Bytes received and sent since the interface came up (64-bit counters).
    pub received: u64,
    pub sent: u64,
    /// The hardware address ("3C:22:FB:0A:11:9E"); empty when it has none.
    pub mac: String,
}

/// `RTM_IFINFO2`: an interface's entry in the list (its addresses follow as other messages).
const RTM_IFINFO2: u8 = 0x12;
/// `if_msghdr2`: the header, then `if_data64` at 32; the interface's link
/// address (`sockaddr_dl`) follows it.
const IFM2_LEN: usize = 160;
const IFM_FLAGS: usize = 8;
const IFM_INDEX: usize = 12;
const IFM_DATA: usize = 32;
const IFD_TYPE: usize = IFM_DATA;
const IFD_BAUD: usize = IFM_DATA + 16;
const IFD_IBYTES: usize = IFM_DATA + 64;
const IFD_OBYTES: usize = IFM_DATA + 72;
/// `sockaddr_dl`: name length at 5, address length at 6, name then address from 8.
const AF_LINK: u8 = 18;
const SDL_DATA: usize = 8;

/// Read the interface list: one entry per interface, its name and hardware
/// address from the link address after it. Other messages (addresses) are
/// skipped; the walk stops at a message that would run past the buffer.
pub fn parse_if_list(buf: &[u8]) -> Vec<IfStats> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 4 <= buf.len() {
        let len = usize::from(read_u16(buf, off));
        if len < 4 || off + len > buf.len() {
            break;
        }
        let m = &buf[off..off + len];
        off += len;
        if m[3] != RTM_IFINFO2 || len < IFM2_LEN {
            continue;
        }
        let mut i = IfStats {
            index: read_u16(m, IFM_INDEX),
            flags: read_u32(m, IFM_FLAGS),
            kind: m[IFD_TYPE],
            baud: read_u64(m, IFD_BAUD),
            received: read_u64(m, IFD_IBYTES),
            sent: read_u64(m, IFD_OBYTES),
            ..IfStats::default()
        };
        let sdl = &m[IFM2_LEN..];
        if sdl.len() > SDL_DATA && sdl[1] == AF_LINK {
            let (nlen, alen) = (usize::from(sdl[5]), usize::from(sdl[6]));
            let end = (usize::from(sdl[0]).max(SDL_DATA)).min(sdl.len());
            let data = &sdl[SDL_DATA..end];
            if nlen <= data.len() {
                i.name = String::from_utf8_lossy(&data[..nlen]).into_owned();
                if alen == 6 && nlen + alen <= data.len() {
                    i.mac = data[nlen..nlen + alen].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":");
                }
            }
        }
        out.push(i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn times(idle: u64, kernel: u64, user: u64) -> CpuTimes {
        CpuTimes { idle, kernel, user }
    }

    #[test]
    fn cpu_split_separates_apps_from_windows() {
        // kernel 100 (of which idle 50) + user 100: busy 150 of 200, apps 100, Windows 50.
        let (load, user, kernel) = cpu_split(times(1_000, 2_000, 3_000), times(1_050, 2_100, 3_100)).unwrap();
        assert!((load - 0.75).abs() < 1e-6, "{load}");
        assert!((user - 0.5).abs() < 1e-6, "{user}");
        assert!((kernel - 0.25).abs() < 1e-6, "{kernel}");
    }

    #[test]
    fn cpu_split_needs_time_to_pass_and_forward_counters() {
        let a = times(10, 20, 30);
        assert_eq!(cpu_split(a, a), None);
        assert_eq!(cpu_split(times(100, 200, 300), a), None);
        // An idle machine: kernel time is all idle.
        assert_eq!(cpu_split(times(0, 0, 0), times(40, 40, 0)), Some((0.0, 0.0, 0.0)));
        // Inconsistent counters never go past 0..=1.
        let (load, _, _) = cpu_split(times(0, 0, 0), times(500, 100, 100)).unwrap();
        assert!((0.0..=1.0).contains(&load));
    }

    fn proc(pid: u64, image: &str, cpu_time: u64, memory: u64) -> ProcSample {
        ProcSample { pid, image: image.into(), cpu_time, memory, io: 0 }
    }

    #[test]
    fn apps_group_processes_by_executable() {
        let mut t = AppTracker::default();
        let first = [proc(0, "", 0, 0), proc(4, "System", 100, 1), proc(10, "chrome.exe", 1_000, 300), proc(11, "Chrome.exe", 2_000, 500), proc(20, "Code.exe", 0, 50)];
        let apps = t.update(10.0, 4, &first);
        // The first list has nothing to compare with: memory only.
        let chrome = apps.iter().find(|a| a.image == "chrome.exe").unwrap();
        assert_eq!((chrome.processes, chrome.memory, chrome.cpu, chrome.pid), (2, 800, 0.0, 11));
        assert_eq!(chrome.name, "chrome");
        assert!(apps.iter().all(|a| a.image != ""), "the idle counter is not an app");

        // One second later on 4 logical processors: 4e7 ticks of capacity.
        let second = [proc(4, "System", 100, 1), proc(10, "chrome.exe", 1_000 + 2_000_000, 300), proc(11, "chrome.exe", 2_000 + 2_000_000, 500), proc(20, "Code.exe", 1_000_000, 50), proc(30, "new.exe", 9_999_999, 5)];
        let apps = t.update(11.0, 4, &second);
        let get = |img: &str| apps.iter().find(|a| a.image == img).unwrap().cpu;
        assert!((get("chrome.exe") - 0.1).abs() < 1e-6);
        assert!((get("code.exe") - 0.025).abs() < 1e-6);
        assert_eq!(get("new.exe"), 0.0, "a new process has no earlier time to compare with");
        assert_eq!(get("system"), 0.0);

        let top = top_cpu(&apps, 2);
        assert_eq!(top.iter().map(|a| a.image.as_str()).collect::<Vec<_>>(), ["chrome.exe", "code.exe"]);
        let top = top_memory(&apps, 3);
        assert_eq!(top.iter().map(|a| a.image.as_str()).collect::<Vec<_>>(), ["chrome.exe", "code.exe", "new.exe"]);
    }

    #[test]
    fn a_reused_process_id_starts_over() {
        let mut t = AppTracker::default();
        t.update(1.0, 1, &[proc(7, "old.exe", 5_000_000, 1)]);
        let apps = t.update(2.0, 1, &[proc(7, "other.exe", 9_000_000, 1)]);
        assert_eq!(apps[0].cpu, 0.0);
    }

    #[test]
    fn app_cpu_never_exceeds_the_whole_machine() {
        let mut t = AppTracker::default();
        t.update(1.0, 1, &[proc(1, "a.exe", 0, 1)]);
        let apps = t.update(1.1, 1, &[proc(1, "a.exe", 50_000_000, 1)]);
        assert_eq!(apps[0].cpu, 1.0);
    }

    /// A process list the way Windows lays it out, with names after the entries.
    fn process_buffer(entries: &[(u64, &str, u64, u64, u64)], base: usize, entry_size: usize) -> Vec<u8> {
        let names_at = entries.len() * entry_size;
        let mut buf = vec![0u8; names_at + 256];
        let mut name_off = names_at;
        for (i, (pid, name, user, kernel, private)) in entries.iter().enumerate() {
            let at = i * entry_size;
            let next = if i + 1 == entries.len() { 0 } else { entry_size as u32 };
            buf[at..at + 4].copy_from_slice(&next.to_le_bytes());
            buf[at + P_PRIVATE_WS..at + P_PRIVATE_WS + 8].copy_from_slice(&private.to_le_bytes());
            // Reads of 1 KB per pid, writes of 2 KB per pid.
            buf[at + P_IO_READ..at + P_IO_READ + 8].copy_from_slice(&(pid * 1024).to_le_bytes());
            buf[at + P_IO_WRITE..at + P_IO_WRITE + 8].copy_from_slice(&(pid * 2048).to_le_bytes());
            buf[at + P_USER_TIME..at + P_USER_TIME + 8].copy_from_slice(&user.to_le_bytes());
            buf[at + P_KERNEL_TIME..at + P_KERNEL_TIME + 8].copy_from_slice(&kernel.to_le_bytes());
            buf[at + P_PID..at + P_PID + 8].copy_from_slice(&pid.to_le_bytes());
            let units: Vec<u16> = name.encode_utf16().collect();
            if !units.is_empty() {
                let len = (units.len() * 2) as u16;
                buf[at + P_NAME_LEN..at + P_NAME_LEN + 2].copy_from_slice(&len.to_le_bytes());
                buf[at + P_NAME_PTR..at + P_NAME_PTR + 8].copy_from_slice(&((base + name_off) as u64).to_le_bytes());
                for u in units {
                    buf[name_off..name_off + 2].copy_from_slice(&u.to_le_bytes());
                    name_off += 2;
                }
            }
        }
        buf
    }

    #[test]
    fn process_list_reads_every_entry() {
        let base = 0x7ff0_0000_0000;
        let buf = process_buffer(&[(0, "", 0, 0, 0), (4, "System", 10, 20, 4096), (1234, "chrome.exe", 300, 400, 512 << 20)], base, 256);
        let list = parse_process_list(&buf, base);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], ProcSample { pid: 0, image: String::new(), cpu_time: 0, memory: 0, io: 0 });
        assert_eq!(list[1], ProcSample { pid: 4, image: "System".into(), cpu_time: 30, memory: 4096, io: 4 * 3072 });
        assert_eq!(list[2], ProcSample { pid: 1234, image: "chrome.exe".into(), cpu_time: 700, memory: 512 << 20, io: 1234 * 3072 });
    }

    #[test]
    fn process_list_stops_at_damage() {
        let base = 0x1000;
        let mut buf = process_buffer(&[(1, "a.exe", 1, 1, 1), (2, "b.exe", 1, 1, 1)], base, 256);
        // A name pointing outside the buffer reads as no name.
        buf[P_NAME_PTR..P_NAME_PTR + 8].copy_from_slice(&0xdead_beef_u64.to_le_bytes());
        let list = parse_process_list(&buf, base);
        assert_eq!((list.len(), list[0].image.as_str(), list[1].image.as_str()), (2, "", "b.exe"));
        // A buffer cut short ends the walk.
        assert_eq!(parse_process_list(&buf[..400], base).len(), 1);
        assert!(parse_process_list(&buf[..200], base).is_empty());
        // A next offset too small to hold an entry can't loop.
        buf[0..4].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(parse_process_list(&buf, base).len(), 1);
    }

    #[test]
    fn history_keeps_a_minute_with_gaps() {
        let mut m = Monitor::default();
        for i in 0..70 {
            let cpu = (i % 2 == 0).then(|| CpuReading { load: 0.5, ..CpuReading::default() });
            m.push(Reading { cpu, ..Reading::default() });
        }
        assert_eq!(m.history.cpu.len(), HISTORY_LEN);
        let v: Vec<f32> = m.history.cpu.values().collect();
        // Samples 10..=69 remain: the even ones have a value, the odd ones are gaps.
        assert!(v[0] == 0.5 && v[1].is_nan(), "oldest first, missing samples are gaps");
        assert_eq!(m.history.cpu.max(), 0.5);
        assert_eq!(m.history.memory.max(), 0.0);
        m.clear();
        assert!(m.latest.is_none() && m.history.cpu.is_empty());
    }

    #[test]
    fn offline_network_records_gaps() {
        let mut h = History::default();
        h.record(&Reading { network: Some(NetReading { online: false, down_bps: 9.0, ..NetReading::default() }), ..Reading::default() });
        assert!(h.net_down.values().next().unwrap().is_nan());
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_pct(0.234), "23%");
        assert_eq!(fmt_pct(1.7), "100%");
        assert_eq!((fmt_app_cpu(0.0004), fmt_app_cpu(0.0912), fmt_app_cpu(0.234)), ("0%".into(), "9.1%".into(), "23%".into()));
        assert_eq!(fmt_bytes(212 << 30), "212 GB");
        assert_eq!(fmt_bytes(9_600 << 20), "9.4 GB");
        assert_eq!(fmt_bytes(512 << 20), "512 MB");
        assert_eq!(fmt_bytes(1_900 << 30), "1.9 TB");
        assert_eq!(fmt_rate(0.0), "0 KB/s");
        assert_eq!(fmt_rate(4_300.0), "4.2 KB/s");
        assert_eq!(fmt_rate(86_000.0), "84 KB/s");
        assert_eq!(fmt_rate(1_258_291.0), "1.2 MB/s");
        assert_eq!(fmt_rate(125_829_120.0), "120 MB/s");
        assert_eq!(fmt_rate(f64::NAN), "0 KB/s");
        assert_eq!((fmt_speed(2_600), fmt_speed(800)), ("2.6 GHz".into(), "800 MHz".into()));
        assert_eq!(fmt_uptime(3 * 86_400 + 4 * 3_600 + 59), "3 d 4 h");
        assert_eq!(fmt_uptime(4 * 3_600 + 12 * 60), "4 h 12 min");
        assert_eq!(fmt_uptime(30), "0 min");
        assert_eq!((fmt_time_left(8_040), fmt_time_left(20)), ("2 h 14 min left".into(), "1 min left".into()));
        assert_eq!((fmt_watts(-8_200), fmt_watts(45_000)), ("8.2 W".into(), "45 W".into()));
        assert_eq!((load_word(0.05), load_word(0.23), load_word(0.95)), ("Idle", "Light load", "Very busy"));
        assert_eq!((display_name("chrome.exe"), display_name("Code.EXE"), display_name("System")), ("chrome".into(), "Code".into(), "System".into()));
    }

    fn core_buffer(cores: &[(u64, u64, u64, u64, u64)]) -> Vec<u8> {
        let mut b = Vec::new();
        for (idle, kernel, user, dpc, int) in cores {
            for v in [idle, kernel, user, dpc, int] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&[0u8; 8]); // interrupt count + padding
        }
        b
    }

    #[test]
    fn core_times_parse_and_split() {
        let a = parse_core_times(&core_buffer(&[(0, 0, 0, 0, 0), (0, 0, 0, 0, 0)]));
        // Core 0: 100 of kernel (60 idle, 10 hardware) + 100 user. Core 1: all idle.
        let b = parse_core_times(&core_buffer(&[(60, 100, 100, 6, 4), (200, 200, 0, 0, 0)]));
        assert_eq!(b[0], CoreTimes { idle: 60, kernel: 100, user: 100, dpc: 6, interrupt: 4 });
        let s = core_split(&a, &b).unwrap();
        assert!((s.cores[0] - 0.7).abs() < 1e-6 && s.cores[1] == 0.0, "{:?}", s.cores);
        // Machine: 400 ticks, 260 idle -> busy 0.35, apps 0.25, Windows 0.10, of which hardware 0.025.
        assert!((s.load - 0.35).abs() < 1e-6 && (s.user - 0.25).abs() < 1e-6 && (s.kernel - 0.10).abs() < 1e-6);
        assert!((s.interrupt - 0.025).abs() < 1e-6);
        // A partial trailing record is ignored; a changed core count or no time gives nothing.
        assert_eq!(parse_core_times(&[0u8; 50]).len(), 1);
        assert_eq!(core_split(&a, &b[..1]), None);
        assert_eq!(core_split(&a, &a), None);
    }

    #[test]
    fn hybrid_cores_by_efficiency_class() {
        assert_eq!(core_kinds(&[1, 1, 0, 0]), [CoreKind::Performance, CoreKind::Performance, CoreKind::Efficiency, CoreKind::Efficiency]);
        assert_eq!(core_kinds(&[0, 0]), [CoreKind::Any, CoreKind::Any]);
        assert!(core_kinds(&[]).is_empty());
        let cores = [(0.8, CoreKind::Performance), (0.4, CoreKind::Performance), (0.1, CoreKind::Efficiency)]
            .map(|(load, kind)| CoreLoad { load, kind })
            .to_vec();
        let c = CpuReading { cores, ..CpuReading::default() };
        assert_eq!(c.hybrid(), Some((2, 1)));
        assert!((c.kind_load(CoreKind::Performance).unwrap() - 0.6).abs() < 1e-6);
        assert_eq!(CpuReading::default().hybrid(), None);
    }

    #[test]
    fn app_io_rate_and_top_io() {
        let mut t = AppTracker::default();
        let p = |pid, img: &str, io| ProcSample { pid, image: img.into(), cpu_time: 0, memory: 1, io };
        t.update(1.0, 1, &[p(1, "a.exe", 1_000), p(2, "b.exe", 5_000)]);
        let apps = t.update(3.0, 1, &[p(1, "a.exe", 9_000), p(2, "b.exe", 5_000), p(3, "c.exe", 99_999)]);
        let io = |img: &str| apps.iter().find(|a| a.image == img).unwrap().io;
        assert_eq!((io("a.exe"), io("b.exe"), io("c.exe")), (4_000.0, 0.0, 0.0));
        let top = top_io(&apps, 5);
        assert_eq!(top.iter().map(|a| a.image.as_str()).collect::<Vec<_>>(), ["a.exe"], "idle apps are left out");
    }

    #[test]
    fn memory_parts_add_up() {
        let m = MemoryReading { total: 100, used: 60, wired: 5, compressed: 3, cached: 30, free: 10, committed: 80, commit_limit: 100, ..MemoryReading::default() };
        let parts = m.parts();
        assert_eq!(parts.map(|(_, v)| v), [52, 5, 3, 30, 10]);
        assert_eq!(parts.iter().map(|(_, v)| v).sum::<u64>(), 100);
        // Parts that overrun the total are cut, never negative.
        let odd = MemoryReading { total: 10, wired: 8, compressed: 8, cached: 8, free: 8, ..MemoryReading::default() };
        assert_eq!(odd.parts().map(|(_, v)| v), [0, 8, 2, 0, 0]);
        assert_eq!((pressure_word(m.pressure()), pressure_word(0.8), pressure_word(0.95)), ("Warning", "Warning", "Critical"));
        assert_eq!(pressure_word(0.5), "Normal");
    }

    #[test]
    fn new_formats() {
        assert_eq!((fmt_temp(54.4), fmt_temp(-3.6)), ("54°".into(), "-4°".into()));
        assert_eq!((fmt_link(1_000_000_000), fmt_link(2_500_000_000), fmt_link(866_700_000)), ("1 Gbps".into(), "2.5 Gbps".into(), "867 Mbps".into()));
    }

    #[test]
    fn gpu_engine_loads() {
        let kinds = [Engine::ThreeD, Engine::Copy, Engine::VideoDecode, Engine::ThreeD, Engine::Other];
        assert_eq!(Engine::from_type(1), Engine::ThreeD);
        assert_eq!(Engine::from_type(6), Engine::Copy);
        assert_eq!(Engine::from_type(42), Engine::Other);
        // One second (10^7 ticks): 3D engines busy 30 % and 55 %, copy 5 %, decode 12 %, the other one 70 %.
        let prev = [0, 100, 1_000, 0, 0];
        let now = [3_000_000, 500_100, 1_201_000, 5_500_000, 7_000_000];
        let e = gpu_busy(&kinds, &prev, &now, 1e7).unwrap();
        assert!((e.engine_3d - 0.55).abs() < 1e-6 && (e.copy - 0.05).abs() < 1e-6);
        assert!((e.video_decode - 0.12).abs() < 1e-6 && e.video_encode == 0.0);
        assert!((e.util - 0.7).abs() < 1e-6, "the busiest engine of all, of any type");
        // A counter that went backwards is idle; past the clock is full.
        let e = gpu_busy(&kinds[..2], &[5, 0], &[0, 30_000_000], 1e7).unwrap();
        assert_eq!((e.engine_3d, e.copy), (0.0, 1.0));
        assert_eq!(gpu_busy(&kinds, &prev, &now[..4], 1e7), None);
        assert_eq!(gpu_busy(&kinds, &prev, &now, 0.0), None);
    }

    #[test]
    fn readings_rest_when_nobody_is_there() {
        let here = Rest::default();
        let away = Rest { away: true, saver: false };
        let saver = Rest { away: false, saver: true };
        // The kept minute: read while someone is there, rest when away or on battery saver.
        assert!(reads(false, false, true, here));
        assert!(!reads(false, false, true, away));
        assert!(!reads(false, false, true, saver));
        assert!(!reads(false, false, true, Rest { away: true, saver: true }));
        // The open page and the readout over a game: rest only when nobody can see them.
        for (open, game) in [(true, false), (false, true), (true, true)] {
            assert!(reads(open, game, false, here));
            assert!(reads(open, game, false, saver));
            assert!(!reads(open, game, true, away));
        }
        // Nothing open and no history kept: never.
        assert!(!reads(false, false, false, here));
    }

    #[test]
    fn readings_helpers() {
        let d = Drive { root: "C:".into(), total: 400, free: 100, ..Drive::default() };
        assert_eq!(d.used_frac(), 0.75);
        let r = Reading { drives: vec![Drive { root: "D:".into(), ..Drive::default() }, Drive { system: true, ..d.clone() }], ..Reading::default() };
        assert_eq!(r.system_drive().unwrap().root, "C:");
        let b = BatteryReading { design_mwh: Some(57_000), full_mwh: Some(51_870), ..BatteryReading::default() };
        assert!((b.health().unwrap() - 0.91).abs() < 1e-3);
        assert_eq!(BatteryReading::default().health(), None);
        assert_eq!(MemoryReading { total: 16, used: 4, ..MemoryReading::default() }.used_frac(), 0.25);
    }

    #[test]
    fn mac_pressure_says_the_kernels_word() {
        use PressureLevel::*;
        let from = PressureLevel::from_kernel;
        assert_eq!((from(1), from(2), from(4), from(0)), (Some(Normal), Some(Warning), Some(Critical), None));
        // The needle follows how full memory is, inside the level's zone.
        assert_eq!(level_pressure(Normal, 0.4), 0.4);
        for (level, word) in [(Normal, "Normal"), (Warning, "Warning"), (Critical, "Critical")] {
            for used in [0.0, 0.5, 0.75, 0.8, 0.9, 1.0, f32::NAN] {
                let p = level_pressure(level, used);
                assert_eq!(pressure_word(p), word, "{level:?} at {used}");
                // As the page reads it back: a share of the RAM, in whole bytes.
                let total = 17_179_869_184u64;
                let m = MemoryReading { total, commit_limit: total, committed: (f64::from(p) * total as f64).round() as u64, ..MemoryReading::default() };
                assert_eq!(pressure_word(m.pressure()), word);
            }
        }
    }

    #[test]
    fn apps_by_bundle() {
        let helper = "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/130/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)";
        assert_eq!(app_bundle(helper), Some("/Applications/Google Chrome.app"));
        assert_eq!(app_bundle("/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"), Some("/System/Library/CoreServices/Finder.app"));
        assert_eq!(app_bundle("/usr/libexec/trustd"), None);
        assert_eq!(app_bundle("/Users/me/tools.apps/bin/x"), None);
        assert_eq!(app_bundle("/Users/me/.app/x"), None);
        assert_eq!(bundle_name("/Applications/Google Chrome.app"), "Google Chrome");
        assert_eq!(bundle_name("/Applications/Safari.app/"), "Safari");
        assert_eq!(bundle_name("/Applications/.app"), ".app");
    }

    #[test]
    fn battery_energy_at_the_packs_voltage() {
        // A MacBook Air's 3-cell pack (49.9 Wh, 4382 mAh), full or nearly empty.
        assert_eq!(pack_mwh(4382, 12_655), Some(49_955));
        assert_eq!(pack_mwh(4382, 9_400), Some(49_955));
        // A 2-cell pack.
        assert_eq!(pack_mwh(5_000, 8_100), Some(38_000));
        assert_eq!((pack_mwh(0, 12_000), pack_mwh(4_000, 0)), (None, None));
    }

    /// An interface's entry the way `NET_RT_IFLIST2` lays it out, its link
    /// address after it, padded to four bytes.
    #[allow(clippy::too_many_arguments)]
    fn if_entry(index: u16, name: &str, flags: u32, kind: u8, baud: u64, rx: u64, tx: u64, mac: &[u8]) -> Vec<u8> {
        let mut sdl = vec![0u8; SDL_DATA];
        sdl.extend_from_slice(name.as_bytes());
        sdl.extend_from_slice(mac);
        sdl.resize(sdl.len().max(20), 0);
        sdl[0] = sdl.len() as u8;
        sdl[1] = AF_LINK;
        sdl[5] = name.len() as u8;
        sdl[6] = mac.len() as u8;
        sdl.resize(sdl.len().div_ceil(4) * 4, 0);
        let mut m = vec![0u8; IFM2_LEN];
        let len = (IFM2_LEN + sdl.len()) as u16;
        m[0..2].copy_from_slice(&len.to_le_bytes());
        m[2] = 5;
        m[3] = RTM_IFINFO2;
        m[IFM_FLAGS..IFM_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        m[IFM_INDEX..IFM_INDEX + 2].copy_from_slice(&index.to_le_bytes());
        m[IFD_TYPE] = kind;
        m[IFD_BAUD..IFD_BAUD + 8].copy_from_slice(&baud.to_le_bytes());
        m[IFD_IBYTES..IFD_IBYTES + 8].copy_from_slice(&rx.to_le_bytes());
        m[IFD_OBYTES..IFD_OBYTES + 8].copy_from_slice(&tx.to_le_bytes());
        m.extend_from_slice(&sdl);
        m
    }

    #[test]
    fn interface_list_reads_every_interface() {
        let mut buf = if_entry(1, "lo0", 0x8049, 24, 0, 1_000, 1_000, &[]);
        // An address message between interfaces is skipped.
        let mut addr = vec![0u8; 24];
        addr[0] = 24;
        addr[3] = 0x0c;
        buf.extend_from_slice(&addr);
        buf.extend(if_entry(4, "en0", 0x8863, 6, 1_000_000_000, 5 << 32, 77, &[0x3c, 0x22, 0xfb, 0x0a, 0x11, 0x9e]));
        let list = parse_if_list(&buf);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[0].kind, list[0].mac.as_str()), ("lo0", 24, ""));
        let en0 = &list[1];
        assert_eq!((en0.index, en0.name.as_str(), en0.flags, en0.kind), (4, "en0", 0x8863, 6));
        assert_eq!((en0.baud, en0.received, en0.sent), (1_000_000_000, 5 << 32, 77));
        assert_eq!(en0.mac, "3C:22:FB:0A:11:9E");
        // Cut short, the walk ends at the last whole message.
        assert_eq!(parse_if_list(&buf[..buf.len() - 1]).len(), 1);
        assert!(parse_if_list(&buf[..3]).is_empty());
        // A zero length can't loop.
        assert!(parse_if_list(&[0u8; 64]).is_empty());
    }
}
