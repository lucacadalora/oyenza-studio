//! Apple Music for Windows' own transport controls, pressed through UI
//! Automation (the interface screen readers use).
//!
//! Its media session declines seeks from outside and usually reports Previous
//! as off, so the island's progress bar and back button did nothing for it.
//! Its window does both and names them for accessibility tools: the progress
//! slider (`LCDScrubber`; `Scrubber` in the Mini Player) takes a new position
//! through its RangeValue pattern, and the back button
//! (`TransportControl_SkipBack`) presses through Invoke, which restarts the
//! song or goes back one, as clicking it does. Nothing here moves the mouse
//! or focuses, restores or raises the window, and it works while the window
//! is minimized. The ids are Apple's own, not a promised interface: when they
//! aren't found the media worker falls back to the media session.
//!
//! Runs on the media worker (an MTA thread). UI Automation gives up on any
//! call Apple Music doesn't answer within [`TRANSACTION_MS`], so a hung player
//! can't stall now playing for long.

use poni_core::nowplaying;
use std::time::{Duration, Instant};
use windows::core::{Interface, PWSTR, VARIANT};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, TRUE};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationInvokePattern, IUIAutomationLegacyIAccessiblePattern, IUIAutomationRangeValuePattern, TreeScope_Descendants,
    UIA_AutomationIdPropertyId, UIA_ControlTypePropertyId, UIA_InvokePatternId, UIA_LegacyIAccessiblePatternId,
    UIA_RangeValuePatternId, UIA_SliderControlTypeId, UIA_E_TIMEOUT, UIA_PATTERN_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible};

const EXE: &str = "applemusic.exe";
/// The class of a WinUI 3 window (Apple Music's main window and Mini Player).
const WINDOW_CLASS: &str = "WinUIDesktopWin32WindowClass";
/// UI Automation gives up on Apple Music after these (milliseconds).
const CONNECT_MS: u32 = 1000;
const TRANSACTION_MS: u32 = 1500;
/// After a look-ahead found nothing, the next one waits this long (each is a
/// walk through Apple Music's window).
const MISS_PAUSE: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Slider,
    Back,
}

impl Control {
    /// Automation ids, best first: the main window's, then the Mini Player's.
    fn ids(self) -> &'static [&'static str] {
        match self {
            Control::Slider => &["LCDScrubber", "Scrubber"],
            Control::Back => &["TransportControl_SkipBack"],
        }
    }

    fn name(self) -> &'static str {
        match self {
            Control::Slider => "progress slider",
            Control::Back => "back button",
        }
    }
}

#[derive(Debug)]
pub enum UiErr {
    /// Apple Music has no window open.
    NoWindow,
    /// Its window doesn't have the control (another layout, an update renamed
    /// it, or the window sits on another desktop).
    NotFound(&'static str),
    /// The control is there but greyed out or read-only.
    Unavailable(&'static str),
    Os(windows::core::Error),
}

impl From<windows::core::Error> for UiErr {
    fn from(e: windows::core::Error) -> Self {
        UiErr::Os(e)
    }
}

impl std::fmt::Display for UiErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiErr::NoWindow => f.write_str("no Apple Music window"),
            UiErr::NotFound(c) => write!(f, "its {c} wasn't found"),
            UiErr::Unavailable(c) => write!(f, "its {c} is unavailable"),
            UiErr::Os(e) => write!(f, "{e}"),
        }
    }
}

/// Apple Music's controls, looked up on first use and remembered while they
/// still answer (a restart or a rebuilt window makes them stale; they are
/// then looked up again).
#[derive(Default)]
pub struct AppleMusicUi {
    uia: Option<IUIAutomation>,
    slider: Option<IUIAutomationElement>,
    back: Option<IUIAutomationElement>,
    /// When a look-ahead last found nothing.
    missed_at: Option<Instant>,
}

impl AppleMusicUi {
    /// Move Apple Music's own progress slider to `fraction` (0..=1) of the song.
    pub fn seek(&mut self, fraction: f64) -> Result<(), UiErr> {
        self.with(Control::Slider, |el| unsafe {
            if !el.CurrentIsEnabled()?.as_bool() {
                return Err(UiErr::Unavailable(Control::Slider.name()));
            }
            let range: IUIAutomationRangeValuePattern = pattern(el, UIA_RangeValuePatternId, Control::Slider)?;
            if range.CurrentIsReadOnly()?.as_bool() {
                return Err(UiErr::Unavailable(Control::Slider.name()));
            }
            let value = nowplaying::slider_value(fraction, range.CurrentMinimum()?, range.CurrentMaximum()?)
                .ok_or(UiErr::Unavailable(Control::Slider.name()))?;
            range.SetValue(value)?;
            Ok(())
        })
    }

    /// Press Apple Music's own back button.
    pub fn skip_back(&mut self) -> Result<(), UiErr> {
        self.with(Control::Back, |el| unsafe {
            if !el.CurrentIsEnabled()?.as_bool() {
                return Err(UiErr::Unavailable(Control::Back.name()));
            }
            match pattern::<IUIAutomationInvokePattern>(el, UIA_InvokePatternId, Control::Back) {
                Ok(button) => button.Invoke()?,
                // Not exposed as a plain button: its default action is the press.
                Err(_) => pattern::<IUIAutomationLegacyIAccessiblePattern>(el, UIA_LegacyIAccessiblePatternId, Control::Back)?.DoDefaultAction()?,
            }
            Ok(())
        })
    }

    /// Look the progress slider up while a drag on the island's bar is still
    /// going, so the seek at its end needn't wait for the search. A slider
    /// that still answers is kept; after a miss, no new search for a while.
    pub fn prepare_seek(&mut self) {
        if self.slider.as_ref().is_some_and(|el| unsafe { el.CurrentProcessId() }.is_ok()) {
            return;
        }
        if self.missed_at.is_some_and(|t| t.elapsed() < MISS_PAUSE) {
            return;
        }
        let t = Instant::now();
        self.slider = None;
        match self.find(Control::Slider) {
            Ok(el) => {
                self.slider = Some(el);
                self.missed_at = None;
                crate::pinfo!("media: found Apple Music's own progress slider in {} ms", t.elapsed().as_millis());
            }
            Err(e) => {
                self.missed_at = Some(Instant::now());
                crate::pinfo!("media: Apple Music's progress slider: {e}");
            }
        }
    }

    /// Run `act` on the control: the remembered one first; looked up afresh
    /// when that went stale or is greyed out (another Apple Music window may
    /// have a live one).
    fn with(&mut self, c: Control, act: impl Fn(&IUIAutomationElement) -> Result<(), UiErr>) -> Result<(), UiErr> {
        if let Some(el) = self.slot(c).clone() {
            match act(&el) {
                Ok(()) => return Ok(()),
                // Apple Music isn't answering: looking again would only wait again.
                Err(UiErr::Os(e)) if e.code().0 as u32 == UIA_E_TIMEOUT => return Err(UiErr::Os(e)),
                Err(_) => *self.slot(c) = None,
            }
        }
        let el = self.find(c)?;
        *self.slot(c) = Some(el.clone());
        act(&el)
    }

    fn slot(&mut self, c: Control) -> &mut Option<IUIAutomationElement> {
        match c {
            Control::Slider => &mut self.slider,
            Control::Back => &mut self.back,
        }
    }

    fn automation(&mut self) -> windows::core::Result<IUIAutomation> {
        if let Some(a) = &self.uia {
            return Ok(a.clone());
        }
        let a: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
            .or_else(|_| unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) })?;
        if let Ok(a2) = a.cast::<IUIAutomation2>() {
            unsafe {
                let _ = a2.SetConnectionTimeout(CONNECT_MS);
                let _ = a2.SetTransactionTimeout(TRANSACTION_MS);
            }
        }
        self.uia = Some(a.clone());
        Ok(a)
    }

    /// The control in Apple Music's windows: each id in turn, in every window.
    fn find(&mut self, c: Control) -> Result<IUIAutomationElement, UiErr> {
        let windows = apple_music_windows();
        if windows.is_empty() {
            return Err(UiErr::NoWindow);
        }
        let uia = self.automation()?;
        let roots: Vec<IUIAutomationElement> = windows.iter().filter_map(|&w| unsafe { uia.ElementFromHandle(w) }.ok()).collect();
        let mut greyed = None;
        for id in c.ids() {
            let cond = condition(&uia, c, id)?;
            for root in &roots {
                match unsafe { root.FindFirst(TreeScope_Descendants, &cond) } {
                    Ok(el) => {
                        if unsafe { el.CurrentIsEnabled() }.is_ok_and(|e| e.as_bool()) {
                            return Ok(el);
                        }
                        greyed.get_or_insert(el);
                    }
                    Err(e) if e.code().0 as u32 == UIA_E_TIMEOUT => return Err(UiErr::Os(e)),
                    // windows-rs reports "nothing found" (a null element) as an error with code S_OK.
                    Err(_) => {}
                }
            }
        }
        greyed.ok_or(UiErr::NotFound(c.name()))
    }
}

/// The control's `id` pattern; a control without it is unavailable (windows-rs
/// reports the missing pattern as an error with code S_OK).
fn pattern<T: Interface>(el: &IUIAutomationElement, id: UIA_PATTERN_ID, c: Control) -> Result<T, UiErr> {
    match unsafe { el.GetCurrentPatternAs::<T>(id) } {
        Ok(p) => Ok(p),
        Err(e) if e.code().is_ok() => Err(UiErr::Unavailable(c.name())),
        Err(e) => Err(UiErr::Os(e)),
    }
}

/// Automation id `id`, and for the slider also the slider control type (the
/// Mini Player's "Scrubber" is a common name).
fn condition(uia: &IUIAutomation, c: Control, id: &str) -> windows::core::Result<IUIAutomationCondition> {
    unsafe {
        let by_id = uia.CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(id))?;
        match c {
            Control::Slider => {
                let slider = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_SliderControlTypeId.0))?;
                uia.CreateAndCondition(&by_id, &slider)
            }
            Control::Back => Ok(by_id),
        }
    }
}

/// Apple Music's top-level windows: its WinUI windows (the main window, the
/// Mini Player), visible ones first; any of its windows when it has none (its
/// hidden helper windows, such as the keyboard's, hold no controls).
fn apple_music_windows() -> Vec<HWND> {
    struct Found {
        /// Process id → whether it is AppleMusic.exe.
        pids: Vec<(u32, bool)>,
        out: Vec<(bool, bool, HWND)>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lp: LPARAM) -> BOOL {
        let found = &mut *(lp.0 as *mut Found);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return TRUE;
        }
        let apple = match found.pids.iter().find(|(p, _)| *p == pid) {
            Some(&(_, a)) => a,
            None => {
                let a = process_is_apple_music(pid);
                found.pids.push((pid, a));
                a
            }
        };
        if apple {
            found.out.push((class_name(hwnd) == WINDOW_CLASS, IsWindowVisible(hwnd).as_bool(), hwnd));
        }
        TRUE
    }
    let mut found = Found { pids: Vec::new(), out: Vec::new() };
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut Found as isize)) };
    if found.out.iter().any(|&(winui, _, _)| winui) {
        found.out.retain(|&(winui, _, _)| winui);
    }
    found.out.sort_by_key(|&(_, visible, _)| !visible);
    found.out.into_iter().map(|(_, _, h)| h).collect()
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) }.max(0) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

fn process_is_apple_music(pid: u32) -> bool {
    let Ok(h) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return false };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.is_ok();
    unsafe {
        let _ = CloseHandle(h);
    }
    ok && String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]).rsplit(['\\', '/']).next().is_some_and(|f| f.eq_ignore_ascii_case(EXE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    #[test]
    fn without_apple_music_says_so() {
        if !apple_music_windows().is_empty() {
            return; // Apple Music is open on this PC.
        }
        let mut ui = AppleMusicUi::default();
        assert!(matches!(ui.seek(0.5), Err(UiErr::NoWindow)));
        assert!(matches!(ui.skip_back(), Err(UiErr::NoWindow)));
    }

    /// CI runs `tools/fake-apple-music.cs` as AppleMusic.exe: a window with a
    /// slider and a back button under Apple Music's automation ids, logging
    /// what happens to them to `PONI_FAKE_AM_LOG`.
    #[test]
    #[ignore = "needs the stand-in Apple Music window (tools/fake-apple-music.cs, run by CI)"]
    fn presses_apple_musics_own_controls() {
        let log = std::env::var("PONI_FAKE_AM_LOG").expect("PONI_FAKE_AM_LOG names the stand-in's log");
        wait_for(&log, "ready");
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let mut ui = AppleMusicUi::default();
        let before = private_bytes();
        // A drag on the island's bar starts: the slider is looked up meanwhile.
        let t = Instant::now();
        ui.prepare_seek();
        eprintln!("look-ahead (finds the slider): {} ms", t.elapsed().as_millis());
        eprintln!("private memory added by UI Automation: {:.1} MB", (private_bytes() - before) as f64 / 1048576.0);
        assert!(ui.slider.is_some(), "the look-ahead found no slider");
        let t = Instant::now();
        ui.seek(0.5).expect("seek after the look-ahead");
        eprintln!("seek after the look-ahead: {} ms", t.elapsed().as_millis());
        wait_for(&log, "slider 120");
        ui.seek(0.25).expect("second seek");
        wait_for(&log, "slider 60");
        let t = Instant::now();
        ui.skip_back().expect("back button");
        eprintln!("first back press (finds the button): {} ms", t.elapsed().as_millis());
        wait_for(&log, "back");
        // The slider counts seconds; the far end is the song's end.
        ui.seek(1.0).expect("seek to the end");
        wait_for(&log, "slider 240");
        // A remembered slider that went stale is looked up again.
        ui.slider = Some(ui.back.clone().expect("the back button was remembered"));
        ui.seek(0.75).expect("seek with a stale slider");
        wait_for(&log, "slider 180");
    }

    fn private_bytes() -> i64 {
        use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut c = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb) }.is_ok();
        if ok {
            c.PrivateUsage as i64
        } else {
            0
        }
    }

    fn wait_for(log: &str, line: &str) {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            let text = std::fs::read_to_string(log).unwrap_or_default();
            if text.lines().any(|l| l.trim() == line) {
                return;
            }
            assert!(Instant::now() < until, "{line:?} never reached the stand-in's log:\n{text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
