//! What the System page shows: CPU, memory, drives, network and battery,
//! read once a second while the page is open.
//!
//! The Windows sampler (`poni::services::sysmon`) hands in raw counters and
//! everything arithmetic lives here, so it is tested on any machine: CPU load
//! from two `GetSystemTimes` samples, per-app usage from two process lists
//! (grouped by executable, the way Task Manager's Processes tab groups them),
//! the process list's binary layout, the minute of history behind each chart,
//! and the number formats the page uses.

use std::collections::{HashMap, VecDeque};

/// Samples kept per chart: one a second, so a minute.
pub const HISTORY_LEN: usize = 60;
/// Apps listed under the CPU and Memory charts.
pub const TOP_APPS: usize = 3;

/// The page's modules, in list order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Module {
    #[default]
    Cpu,
    Memory,
    Drives,
    Network,
    Battery,
}

impl Module {
    pub const ALL: [Module; 5] = [Module::Cpu, Module::Memory, Module::Drives, Module::Network, Module::Battery];

    pub fn title(self) -> &'static str {
        match self {
            Module::Cpu => "CPU",
            Module::Memory => "Memory",
            Module::Drives => "Drives",
            Module::Network => "Network",
            Module::Battery => "Battery",
        }
    }
}

/// Processor load between the last two samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuReading {
    /// Busy share of all logical processors, 0..=1.
    pub load: f32,
    /// The part of `load` spent in apps (user mode) and in Windows itself (kernel mode).
    pub user: f32,
    pub kernel: f32,
    /// Logical processors (threads) across all processor groups.
    pub logical: u32,
    /// Current speed, when Windows reports it (Task Manager's "Speed").
    pub mhz: Option<u32>,
    pub uptime_s: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryReading {
    pub total: u64,
    pub used: u64,
    /// Memory Windows has promised to programs, and its limit (RAM plus the
    /// page file). Committed close to the limit is Windows' memory pressure.
    pub committed: u64,
    pub commit_limit: u64,
    /// File cache Windows hands back when apps need the room.
    pub cached: u64,
}

impl MemoryReading {
    pub fn used_frac(&self) -> f32 {
        frac(self.used, self.total)
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
    /// Private working set in bytes (Task Manager's "Memory" column).
    pub memory: u64,
    pub processes: u32,
}

/// One reading of everything the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub cpu: Option<CpuReading>,
    pub memory: Option<MemoryReading>,
    pub drives: Vec<Drive>,
    pub disk: Option<DiskActivity>,
    pub network: Option<NetReading>,
    /// None on a PC without a battery.
    pub battery: Option<BatteryReading>,
    pub top_cpu: Vec<AppUse>,
    pub top_memory: Vec<AppUse>,
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
    pub memory: Series,
    pub disk_read: Series,
    pub disk_write: Series,
    pub net_down: Series,
    pub net_up: Series,
}

impl History {
    pub fn record(&mut self, r: &Reading) {
        self.cpu.push(r.cpu.as_ref().map(|c| c.load));
        self.memory.push(r.memory.as_ref().map(MemoryReading::used_frac));
        self.disk_read.push(r.disk.map(|d| d.read_bps as f32));
        self.disk_write.push(r.disk.map(|d| d.write_bps as f32));
        let net = r.network.as_ref().filter(|n| n.online);
        self.net_down.push(net.map(|n| n.down_bps as f32));
        self.net_up.push(net.map(|n| n.up_bps as f32));
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
}

/// Entries of the process list that aren't apps: the idle counter and
/// Windows' own memory bookkeeping (Task Manager's Processes tab leaves them out too).
const NOT_APPS: [&str; 6] = ["", "idle", "system idle process", "memory compression", "registry", "secure system"];

/// Turns successive process lists into per-app usage.
#[derive(Debug, Default)]
pub struct AppTracker {
    /// pid → (lower-case image, cpu time) at the previous call.
    prev: HashMap<u64, (String, u64)>,
    prev_at: Option<f64>,
}

impl AppTracker {
    /// Usage per executable since the previous call (unsorted). `now` is in
    /// seconds and `logical` is the number of logical processors. A process
    /// that is new since the last call counts 0 % CPU: its time began before it.
    pub fn update(&mut self, now: f64, logical: u32, procs: &[ProcSample]) -> Vec<AppUse> {
        let ticks = self.prev_at.map(|t| now - t).filter(|d| *d > 0.0).map(|d| d * 1e7 * f64::from(logical.max(1)));
        let mut groups: HashMap<String, AppUse> = HashMap::new();
        let mut lead: HashMap<String, u64> = HashMap::new();
        let mut next = HashMap::with_capacity(procs.len());
        for p in procs {
            let image = p.image.to_lowercase();
            next.insert(p.pid, (image.clone(), p.cpu_time));
            if p.pid == 0 || NOT_APPS.contains(&image.as_str()) {
                continue;
            }
            let cpu = match (ticks, self.prev.get(&p.pid)) {
                (Some(t), Some((img, before))) if *img == image && p.cpu_time >= *before => ((p.cpu_time - before) as f64 / t) as f32,
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
/// Bytes an entry needs for every field above.
const P_MIN: usize = 88;

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
        out.push(ProcSample { pid: read_u64(e, P_PID), image, cpu_time: user.saturating_add(kernel), memory: read_u64(e, P_PRIVATE_WS) });
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
        ProcSample { pid, image: image.into(), cpu_time, memory }
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
        assert_eq!(list[0], ProcSample { pid: 0, image: String::new(), cpu_time: 0, memory: 0 });
        assert_eq!(list[1], ProcSample { pid: 4, image: "System".into(), cpu_time: 30, memory: 4096 });
        assert_eq!(list[2], ProcSample { pid: 1234, image: "chrome.exe".into(), cpu_time: 700, memory: 512 << 20 });
    }

    #[test]
    fn process_list_stops_at_damage() {
        let base = 0x1000;
        let mut buf = process_buffer(&[(1, "a.exe", 1, 1, 1), (2, "b.exe", 1, 1, 1)], base, 128);
        // A name pointing outside the buffer reads as no name.
        buf[P_NAME_PTR..P_NAME_PTR + 8].copy_from_slice(&0xdead_beef_u64.to_le_bytes());
        let list = parse_process_list(&buf, base);
        assert_eq!((list.len(), list[0].image.as_str(), list[1].image.as_str()), (2, "", "b.exe"));
        // A buffer cut short ends the walk.
        assert_eq!(parse_process_list(&buf[..200], base).len(), 1);
        assert!(parse_process_list(&buf[..40], base).is_empty());
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
}
