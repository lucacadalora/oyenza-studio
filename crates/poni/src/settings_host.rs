//! The Settings window on Windows: a borderless layered window that shows
//! the Settings content (`settings_window::View`) with a renderer of its own
//! (`Gfx`, always the software one: it draws only when something in it
//! changes), so its rounded card and soft shadow sit on the desktop like the
//! island does. It exists only while it is open: closing it drops the
//! window, its renderer and its pixels, so nothing of it stays at rest. It
//! is light or dark as Windows' apps are.

use crate::gfx::Gfx;
use crate::settings_window::{Frame, Painted, Section, View, CARD_H, CARD_W};
use crate::state::AppState;
use poni_core::geom::{Point, Rect};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: PCWSTR = w!("Studio.Settings");

/// Room around the card for its shadow (DIPs).
const MARGIN: f32 = 24.0;
/// On a short screen the card is less tall, down to this, and its content scrolls.
const CARD_H_MIN: f32 = 440.0;

/// The open Settings window.
pub struct SettingsWindow {
    pub hwnd: HWND,
    gfx: Gfx,
    pub view: View,
    /// Window position (screen pixels) and the DPI it draws at.
    pub at: (i32, i32),
    dpi: u32,
    /// The card's height (DIPs), from the screen it opened on.
    card_h: f32,
    /// The pointer is over the window's client part (TrackMouseEvent armed).
    pub tracking: bool,
    /// Light or dark, as Windows' apps were when it opened or last changed.
    light: bool,
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Register the window class (with the app's icon, for the taskbar).
pub fn register(wndproc: crate::window::WndProc) -> Result<()> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let icon = LoadIconW(hinst, PCWSTR(1 as _)).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hIcon: icon,
            hIconSm: icon,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_win32());
        }
    }
    Ok(())
}

/// The window's size in pixels at `scale`, for a card `card_h` tall.
fn surface_px(scale: f32, card_h: f32) -> (i32, i32) {
    (((CARD_W + 2.0 * MARGIN) * scale).ceil() as i32, ((card_h + 2.0 * MARGIN) * scale).ceil() as i32)
}

/// The card inside the window, in DIPs.
pub fn card_rect(card_h: f32) -> Rect {
    Rect::new(MARGIN, MARGIN, CARD_W, card_h)
}

/// How tall the card can be on a work area `work_h` pixels tall.
fn card_h_for(work_h: i32, scale: f32) -> f32 {
    (work_h as f32 / scale - 2.0 * MARGIN - 16.0).clamp(CARD_H_MIN, CARD_H).floor()
}

impl SettingsWindow {
    /// Make the window, centred in `work` (a monitor's work area, pixels)
    /// at `dpi`. It shows with its first frame.
    pub fn new(work: RECT, dpi: u32, section: Section) -> Result<Self> {
        let scale = dpi as f32 / 96.0;
        let card_h = card_h_for(work.bottom - work.top, scale);
        let (w, h) = surface_px(scale, card_h);
        let x = work.left + ((work.right - work.left) - w) / 2;
        let y = work.top + (((work.bottom - work.top) - h) / 2).max(0);
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_APPWINDOW,
                CLASS,
                w!("Oyenza Studio Settings"),
                WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX,
                x,
                y,
                w,
                h,
                None,
                None,
                GetModuleHandleW(None)?,
                None,
            )?
        };
        let gfx = match Gfx::new(dpi as f32, hwnd) {
            Ok(g) => g.software_only(),
            Err(e) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return Err(e);
            }
        };
        let light = crate::services::apps_use_light_theme();
        Ok(SettingsWindow { hwnd, gfx, view: View::new(section), at: (x, y), dpi, card_h, tracking: false, light })
    }

    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        if dpi != self.dpi {
            self.dpi = dpi;
            self.gfx.set_dpi(dpi as f32);
            self.view.dirty = true;
        }
    }

    /// Windows' apps switched between light and dark (or may have): read it again.
    pub fn refresh_mode(&mut self) {
        self.set_light(crate::services::apps_use_light_theme());
    }

    /// Draw light or dark from now on (the self-test picks).
    pub fn set_light(&mut self, light: bool) {
        if light != self.light {
            self.light = light;
            self.view.dirty = true;
        }
    }

    pub fn mouse_dip(&self, x: i32, y: i32) -> Point {
        let s = self.scale();
        Point::new(x as f32 / s, y as f32 / s)
    }

    /// The card inside the window, in DIPs.
    pub fn card(&self) -> Rect {
        card_rect(self.card_h)
    }

    /// Where the window can be dragged from: inside the card, where no
    /// widget is.
    pub fn drags_at(&self, p: Point) -> bool {
        self.card().contains(p) && self.view.hit_at(p).is_none()
    }

    /// Draw the window for `st` (and show it, the first time), light or
    /// dark as Windows' apps are. `capture`: keep this frame's pixels for
    /// the self-test.
    pub fn paint(&mut self, st: &mut AppState, now: f64, capture: bool) -> Painted {
        let (w, h) = surface_px(self.scale(), self.card_h);
        if let Err(e) = self.gfx.set_surface(w, h) {
            crate::pwarn!("settings window: surface failed ({e})");
            return Painted::default();
        }
        let light = self.light;
        self.gfx.set_look(poni_core::look::glass(light));
        let full = RECT { left: 0, top: 0, right: w, bottom: h };
        if capture {
            self.gfx.request_capture(full);
        }
        let frame = Frame { card: self.card(), own_chrome: true, top: 0.0 };
        let out = match self.gfx.begin(full) {
            Ok(mut canvas) => Some(self.view.draw(&mut canvas, st, now, frame, light)),
            Err(e) => {
                crate::pwarn!("settings window: begin frame failed ({e})");
                None
            }
        };
        if out.is_some() {
            if let Err(e) = self.gfx.end() {
                crate::pwarn!("settings window: EndDraw failed ({e})");
            }
        }
        if let Err(e) = self.gfx.present(self.at.0, self.at.1) {
            crate::pwarn!("settings window: present failed ({e})");
        }
        unsafe {
            if !IsWindowVisible(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
        out.unwrap_or_default()
    }

    /// The last frame's pixels (premultiplied BGRA) and its size.
    pub fn take_capture(&mut self) -> (i32, i32, Vec<u8>) {
        let (w, h) = surface_px(self.scale(), self.card_h);
        (w, h, self.gfx.take_capture())
    }

    /// Bring the window forward (it was open already).
    pub fn raise(&self) {
        unsafe {
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
    }
}
