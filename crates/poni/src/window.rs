//! Window creation, monitor selection and the vsync frame clock.

use crate::clock;
use crate::events::{SendHwnd, WM_APP_FRAME};
use poni_core::frametime::{period_for_hz, VsyncTimeline, OFF_RHYTHM_LIMIT};
use poni_core::settings::MonitorChoice;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromPoint, DEVMODEW, ENUM_CURRENT_SETTINGS, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const ISLAND_CLASS: PCWSTR = w!("Poni.Island");
pub const HELPER_CLASS: PCWSTR = w!("Poni.Helper");
pub const VISUAL_CLASS: PCWSTR = w!("Poni.Visual");

pub type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> windows::Win32::Foundation::LRESULT;

/// The visual window takes no input and has next to nothing to handle.
unsafe extern "system" fn visual_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    match msg {
        // WS_EX_TRANSPARENT already keeps hit tests off it; should one land
        // here anyway, it goes on to the island underneath (same thread).
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        // The renderer places and sizes it; moving to another display must
        // not resize it.
        WM_DPICHANGED => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub fn register_classes(island_proc: WndProc, helper_proc: WndProc) -> Result<()> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let icon = LoadIconW(hinst, PCWSTR(1 as _)).unwrap_or_default();
        let cursor = LoadCursorW(None, IDC_ARROW)?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(island_proc),
            hInstance: hinst.into(),
            hIcon: icon,
            hCursor: cursor,
            lpszClassName: ISLAND_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        let wc2 = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(helper_proc),
            hInstance: hinst.into(),
            hIcon: icon,
            lpszClassName: HELPER_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&wc2) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        let wc3 = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(visual_wndproc),
            hInstance: hinst.into(),
            lpszClassName: VISUAL_CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&wc3) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        Ok(())
    }
}

/// The island: a borderless, top-most, non-activating, per-pixel-alpha
/// layered tool window (no taskbar button, never steals focus on click).
pub fn create_island_window() -> Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            ISLAND_CLASS,
            w!("Poni"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            hinst,
            None,
        )
    }
}

/// The GPU renderer's window, made with its first frame: it shows what the
/// renderer draws (DirectComposition) and never takes input. The island
/// window then only carries the hit mask, a layered bitmap, and there mouse,
/// pen and touch all follow its alpha. On one window with DirectComposition
/// content, touch ignores that alpha: the window's whole rectangle (the room
/// for the largest page, far wider and taller than the closed island)
/// swallowed taps meant for the windows under it. Owned by the island, so
/// it stays just above it whenever the island is raised.
pub fn create_visual_window(island: HWND) -> Result<HWND> {
    let visual = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            VISUAL_CLASS,
            w!("Poni"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            island,
            None,
            GetModuleHandleW(None)?,
            None,
        )?
    };
    VISUAL.with(|v| v.set(visual.0 as isize));
    // It holds the pixels: out of capture too while an owner wants the island out.
    if CAPTURE.with(|c| c.get().1) {
        unsafe {
            let _ = SetWindowDisplayAffinity(visual, WDA_EXCLUDEFROMCAPTURE);
        }
    }
    Ok(visual)
}

/// Who wants the island left out of screen capture. The Glass theme does
/// while it copies the screen under the island, so the copy never contains
/// the island itself; the Script page does while it shows, so the script
/// stays out of screen shares and recordings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureOwner {
    Glass = 1,
    Script = 2,
    /// A recording with Oyenza Studio Recorder: the island isn't in its video.
    Recording = 4,
}

thread_local! {
    // (owners that want it, what Windows was last told). UI thread only.
    static CAPTURE: std::cell::Cell<(u8, bool)> = const { std::cell::Cell::new((0, false)) };
    // The visual window, once the GPU renderer made it (0 before).
    static VISUAL: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// The visual window, if the GPU renderer made one.
fn visual() -> Option<HWND> {
    let v = VISUAL.with(|v| v.get());
    (v != 0).then(|| HWND(v as *mut core::ffi::c_void))
}

/// Leave the island out of screenshots, screen recordings and screen sharing
/// for `who` (`on`), or stop wanting it. The island stays out while any
/// owner wants it, so one owner letting go never shows it under the other.
/// True when Windows holds what the owners want now.
pub fn exclude_from_capture(hwnd: HWND, who: CaptureOwner, on: bool) -> bool {
    CAPTURE.with(|c| {
        let (mut wants, told) = c.get();
        if on {
            wants |= who as u8;
        } else {
            wants &= !(who as u8);
        }
        let want = wants != 0;
        let ok = want == told || set_capture_excluded(hwnd, want);
        c.set((wants, if ok { want } else { told }));
        ok
    })
}

/// Whether Windows leaves the island out of screen capture right now (its
/// visual window too, where the GPU renderer draws).
pub fn capture_excluded(hwnd: HWND) -> bool {
    let excluded = |h: HWND| {
        let mut got = 0u32;
        unsafe { GetWindowDisplayAffinity(h, &mut got).is_ok() && got == WDA_EXCLUDEFROMCAPTURE.0 }
    };
    excluded(hwnd) && visual().map_or(true, excluded)
}

/// Tell Windows (see [`exclude_from_capture`]), for the island and its visual
/// window. True when it took the setting for both.
fn set_capture_excluded(hwnd: HWND, on: bool) -> bool {
    let want = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    let set = |h: HWND| unsafe {
        if SetWindowDisplayAffinity(h, want).is_err() {
            return false;
        }
        let mut got = 0u32;
        GetWindowDisplayAffinity(h, &mut got).is_ok() && got == want.0
    };
    // Both, even when the first refuses.
    let island = set(hwnd);
    visual().map_or(true, set) && island
}

/// Windows' build number (19041 = Windows 10 version 2004, the first that can
/// leave a window out of capture instead of blacking it out).
pub fn windows_build() -> Option<u32> {
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut buf = [0u16; 32];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if r.is_err() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len]).trim().parse().ok()
}

/// Hidden top-level helper window: owns the tray icon, the AppBar
/// registration (full-screen notifications) and power messages.
pub fn create_helper_window() -> Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        CreateWindowExW(WS_EX_TOOLWINDOW, HELPER_CLASS, w!("Poni Helper"), WS_POPUP, 0, 0, 0, 0, None, None, hinst, None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    pub handle: isize,
    /// Full monitor rectangle in physical pixels.
    pub rect: RECT,
    /// Work area (excludes the taskbar).
    pub work: RECT,
    pub dpi: u32,
}

impl MonitorInfo {
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
    /// Y where the island hangs from: below a top-docked taskbar, else the monitor top.
    pub fn top(&self) -> i32 {
        if self.work.top > self.rect.top && self.work.left <= self.rect.left && self.work.right >= self.rect.right {
            self.work.top
        } else {
            self.rect.top
        }
    }
    pub fn center_x(&self) -> i32 {
        (self.rect.left + self.rect.right) / 2
    }
}

pub fn monitor_info(hmon: HMONITOR) -> Option<MonitorInfo> {
    unsafe {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Some(MonitorInfo { handle: hmon.0 as isize, rect: mi.rcMonitor, work: mi.rcWork, dpi: dx.max(96) })
    }
}

pub fn pick_monitor(choice: MonitorChoice) -> MonitorInfo {
    unsafe {
        let point = match choice {
            MonitorChoice::Primary => POINT { x: 0, y: 0 },
            MonitorChoice::Cursor => {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                p
            }
        };
        let hmon = MonitorFromPoint(point, MONITOR_DEFAULTTOPRIMARY);
        monitor_info(hmon).unwrap_or(MonitorInfo {
            handle: 0,
            rect: RECT { left: 0, top: 0, right: 1920, bottom: 1080 },
            work: RECT { left: 0, top: 0, right: 1920, bottom: 1040 },
            dpi: 96,
        })
    }
}

// ---------------------------------------------------------------- frame clock

/// Refresh rate of `hmon` in Hz from its current display mode (0 if unknown).
pub fn refresh_hz(hmon: HMONITOR) -> u32 {
    unsafe {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            return 0;
        }
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if EnumDisplaySettingsW(PCWSTR(mi.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() {
            dm.dmDisplayFrequency
        } else {
            0
        }
    }
}

/// The DXGI output (display) showing `hmon`, if any adapter drives it.
fn output_for(hmon: HMONITOR) -> Option<IDXGIOutput> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                if output.GetDesc().is_ok_and(|d| d.Monitor == hmon) {
                    return Some(output);
                }
                j += 1;
            }
            i += 1;
        }
        None
    }
}

/// What the clock waits on for each vertical blank.
enum Vblank {
    /// `WaitForVBlank` on the island's display: the real refresh of that
    /// monitor at whatever rate it runs (60, 144, 240 Hz).
    Output(IDXGIOutput),
    /// No DXGI output (some remote sessions): wait for DWM to compose.
    Dwm,
    /// Neither blocks properly: sleep one period (high-resolution timer).
    Timer,
}

impl Vblank {
    fn for_monitor(hmon: HMONITOR) -> Self {
        match output_for(hmon) {
            Some(o) => Vblank::Output(o),
            None => Vblank::Dwm,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Vblank::Output(_) => "display vblank",
            Vblank::Dwm => "DWM flush",
            Vblank::Timer => "timer",
        }
    }

    /// Block until the next vblank. False when the source failed. The timer
    /// keeps its own steady schedule in `next_due` (mono seconds).
    fn wait(&self, period: f64, next_due: &mut f64) -> bool {
        match self {
            Vblank::Output(o) => unsafe { o.WaitForVBlank().is_ok() },
            Vblank::Dwm => unsafe { DwmFlush().is_ok() },
            Vblank::Timer => {
                let now = clock::mono();
                if *next_due < now || *next_due > now + 2.0 * period {
                    *next_due = now + period;
                }
                std::thread::sleep(Duration::from_secs_f64(*next_due - now));
                *next_due += period;
                true
            }
        }
    }
}

fn f64_load(a: &AtomicU64) -> f64 {
    f64::from_bits(a.load(Ordering::Acquire))
}

fn f64_store(a: &AtomicU64, v: f64) {
    a.store(v.to_bits(), Ordering::Release);
}

struct ClockShared {
    active: Mutex<bool>,
    cv: Condvar,
    /// A WM_APP_FRAME is queued and not yet handled.
    pending: AtomicBool,
    stop: AtomicBool,
    /// Post a frame on every n-th vblank (1 = the full refresh rate).
    divisor: AtomicU32,
    /// Locked time (mono seconds) of the newest vblank.
    latest: AtomicU64,
    /// Refresh period estimate (seconds).
    period: AtomicU64,
    /// The island's monitor (HMONITOR); the clock follows its display.
    monitor: AtomicIsize,
    /// The clock restarted after being parked (cleared by the UI).
    resumed: AtomicBool,
    /// Vblanks between the previous posted frame and the newest one.
    periods: AtomicU32,
    /// The display mode's refresh rate (Hz, 0 = unknown).
    nominal_hz: AtomicU32,
    /// The display settings changed: re-read the rate and the output.
    display_changed: AtomicBool,
}

/// One vblank-driven frame, as the UI thread sees it.
#[derive(Clone, Copy, Debug)]
pub struct FrameTick {
    /// Locked time (mono seconds) of the vblank this frame belongs to.
    pub vblank: f64,
    /// Refresh period (seconds).
    pub period: f64,
    /// First frame after the clock was parked.
    pub resumed: bool,
    /// Vblanks since the previous frame was posted (1 = none skipped).
    pub periods: u32,
}

/// Posts `WM_APP_FRAME` on the vertical blank of the island's display while
/// active, and sleeps (zero CPU) while parked. Frame times are phase-locked
/// to the display, so animation steps are exact multiples of its period.
pub struct FrameClock {
    shared: Arc<ClockShared>,
}

impl FrameClock {
    pub fn start(hwnd: SendHwnd, monitor: isize) -> Self {
        let hz = refresh_hz(HMONITOR(monitor as *mut core::ffi::c_void));
        let shared = Arc::new(ClockShared {
            active: Mutex::new(false),
            cv: Condvar::new(),
            pending: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            divisor: AtomicU32::new(1),
            latest: AtomicU64::new(clock::mono().to_bits()),
            period: AtomicU64::new(period_for_hz(hz).to_bits()),
            monitor: AtomicIsize::new(monitor),
            resumed: AtomicBool::new(true),
            periods: AtomicU32::new(1),
            nominal_hz: AtomicU32::new(hz),
            display_changed: AtomicBool::new(false),
        });
        let s = shared.clone();
        std::thread::Builder::new()
            .name("poni-frame-clock".into())
            .spawn(move || clock_thread(s, hwnd))
            .expect("spawn frame clock");
        Self { shared }
    }

    /// Run (true) or park (false) the clock.
    pub fn set_active(&self, on: bool) {
        let mut a = self.shared.active.lock().unwrap();
        if *a != on {
            *a = on;
            self.shared.cv.notify_all();
        }
    }

    /// Post on every `n`-th vblank (1 = every refresh).
    pub fn set_divisor(&self, n: u32) {
        self.shared.divisor.store(n.max(1), Ordering::Relaxed);
    }

    /// Follow the display the island is on, and re-read its refresh rate
    /// (it may have switched between 60 and 144 Hz on the same monitor).
    pub fn set_monitor(&self, hmon: isize) {
        self.shared.monitor.store(hmon, Ordering::Relaxed);
        self.shared.display_changed.store(true, Ordering::Release);
    }

    /// The display mode's refresh rate in Hz (stable, unlike [`Self::period`]).
    pub fn nominal_hz(&self) -> f64 {
        match self.shared.nominal_hz.load(Ordering::Relaxed) {
            hz if hz >= 20 => hz as f64,
            _ => 1.0 / self.period(),
        }
    }

    /// Refresh period of the island's display (seconds).
    pub fn period(&self) -> f64 {
        f64_load(&self.shared.period)
    }

    /// Call when a `WM_APP_FRAME` arrives: the vblank it belongs to.
    pub fn tick(&self) -> FrameTick {
        FrameTick {
            vblank: f64_load(&self.shared.latest),
            period: self.period(),
            resumed: self.shared.resumed.swap(false, Ordering::AcqRel),
            periods: self.shared.periods.load(Ordering::Acquire),
        }
    }

    /// Call when a `WM_APP_FRAME` was handled.
    pub fn frame_handled(&self) {
        self.shared.pending.store(false, Ordering::Release);
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
    }
}

fn clock_thread(s: Arc<ClockShared>, hwnd: SendHwnd) {
    let mut monitor = s.monitor.load(Ordering::Relaxed);
    let hmon = |m: isize| HMONITOR(m as *mut core::ffi::c_void);
    let mut src = Vblank::for_monitor(hmon(monitor));
    let mut timeline = VsyncTimeline::new(f64_load(&s.period));
    let mut hz = s.nominal_hz.load(Ordering::Relaxed);
    // When the display mode was last read (it is re-read every second while
    // running: modes change without telling a background thread).
    let mut checked_at = clock::mono();
    crate::pinfo!("frame clock: {} at {:.1} Hz", src.name(), 1.0 / timeline.period());
    // Vblanks counted since the last posted frame.
    let mut since_post = u32::MAX;
    // Waits that returned at once / failed in a row (a source that doesn't block).
    let mut bad = 0u32;
    // When the clock fell back to the timer (it retries the display now and
    // then), and the timer's next tick.
    let mut timer_since = 0.0f64;
    let mut next_due = 0.0f64;
    let mut parked = true;
    loop {
        {
            let mut active = s.active.lock().unwrap();
            if !*active {
                parked = true;
            }
            while !*active && !s.stop.load(Ordering::Relaxed) {
                active = s.cv.wait(active).unwrap();
            }
        }
        if s.stop.load(Ordering::Relaxed) {
            break;
        }
        let mut recheck = s.display_changed.swap(false, Ordering::AcqRel) || timeline.off_rhythm() >= OFF_RHYTHM_LIMIT || clock::mono() - checked_at > 1.0;
        if parked {
            parked = false;
            recheck = true;
            timeline.reset();
            since_post = u32::MAX;
            s.resumed.store(true, Ordering::Release);
        }
        let m = s.monitor.load(Ordering::Relaxed);
        if recheck || m != monitor {
            checked_at = clock::mono();
            let now_hz = refresh_hz(hmon(m));
            if m != monitor || now_hz != hz {
                monitor = m;
                hz = now_hz;
                s.nominal_hz.store(hz, Ordering::Relaxed);
                src = Vblank::for_monitor(hmon(m));
                bad = 0;
                timeline.set_nominal_period(period_for_hz(hz));
                crate::pinfo!("frame clock: {} at {:.1} Hz", src.name(), 1.0 / timeline.period());
            } else if timeline.off_rhythm() >= OFF_RHYTHM_LIMIT {
                timeline.reset();
            }
        } else if matches!(src, Vblank::Timer) && clock::mono() - timer_since > 5.0 {
            // The display may block properly again (it was off, a mode change).
            src = Vblank::for_monitor(hmon(m));
            bad = 0;
        }

        let period = timeline.period();
        let t0 = clock::mono();
        let ok = src.wait(period, &mut next_due);
        let raw = clock::mono();
        // A wait that returns in well under a period, again and again, isn't
        // waiting for anything: pace with the timer instead of spinning.
        if !ok || raw - t0 < 0.15 * period {
            bad += 1;
            if bad >= 3 && !matches!(src, Vblank::Timer) {
                if timer_since == 0.0 {
                    crate::pwarn!("frame clock: {} isn't blocking ({}); using the timer", src.name(), if ok { "returns at once" } else { "fails" });
                }
                src = Vblank::Timer;
                timer_since = raw;
            }
            if !ok || bad >= 3 {
                continue;
            }
        } else {
            bad = 0;
        }

        let (t, n) = timeline.lock(raw);
        if n == 0 {
            continue;
        }
        f64_store(&s.latest, t);
        f64_store(&s.period, timeline.period());
        since_post = since_post.saturating_add(n);
        if since_post < s.divisor.load(Ordering::Relaxed) {
            continue;
        }
        if !s.pending.swap(true, Ordering::AcqRel) {
            s.periods.store(since_post.min(1000), Ordering::Release);
            since_post = 0;
            unsafe {
                let _ = PostMessageW(hwnd.hwnd(), WM_APP_FRAME, WPARAM(0), LPARAM(0));
            }
        }
    }
}
