//! The Settings window on Windows: a borderless layered window that shows
//! the Settings content (`settings_window::View`) with a renderer of its own
//! (`Gfx`, always the software one: it draws only when something in it
//! changes), so its rounded card and soft shadow sit on the desktop like the
//! island does. It exists only while it is open: closing it drops the
//! window, its renderer and its pixels, so nothing of it stays at rest. It
//! is light or dark as Windows' apps are.

use crate::gfx::Gfx;
use crate::settings_window::{Frame, Key, Painted, Section, View, CARD_H, CARD_W};
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
    /// Windows asks for less motion (Settings › Accessibility › Animation
    /// effects off): everything lands at once.
    reduce: bool,
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
        Ok(SettingsWindow { hwnd, gfx, view: View::new(section), at: (x, y), dpi, card_h, tracking: false, light, reduce: animations_off() })
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

    /// Windows' apps switched between light and dark, or animation effects
    /// on or off (or may have): read them again.
    pub fn refresh_mode(&mut self) {
        self.set_light(crate::services::apps_use_light_theme());
        self.reduce = animations_off();
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
    /// dark as Windows' apps are. `only`: a frame of a glide, which redraws
    /// and shows only that part (DIPs). `capture`: keep this frame's pixels
    /// for the self-test.
    pub fn paint(&mut self, st: &mut AppState, now: f64, only: Option<Rect>, capture: bool) -> Painted {
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
        let frame = Frame { card: self.card(), own_chrome: true, top: 0.0, still: capture || self.reduce };
        let part = only.filter(|_| !capture && !self.gfx.draws_all()).map(|r| crate::gfx::px_rect(r, self.scale(), 1, w, h));
        // A glide's frame draws over exactly the pixels it cleared: whole
        // pixels, one more round what moves.
        let s = self.scale();
        let clip = part.map(|p| Rect::new(p.left as f32 / s, p.top as f32 / s, (p.right - p.left) as f32 / s, (p.bottom - p.top) as f32 / s));
        let out = match self.gfx.begin(part.unwrap_or(full)) {
            Ok(mut canvas) => {
                if let Some(r) = clip {
                    canvas.clip(r);
                }
                let out = self.view.draw(&mut canvas, st, now, frame, light);
                if clip.is_some() {
                    canvas.unclip();
                }
                Some(out)
            }
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

/// Windows' Animation effects are off (Settings › Accessibility › Visual effects).
fn animations_off() -> bool {
    let mut on = windows::Win32::Foundation::BOOL(1);
    let read = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut core::ffi::c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    read.is_ok() && !on.as_bool()
}

// ---------------------------------------------------------------- self-test

/// What filming the window found (self-test).
pub struct Film {
    /// Frames filmed, and the window's size (pixels).
    pub frames: usize,
    pub size: (i32, i32),
    /// How long each glide's frame drawn in part took (ms, sorted).
    pub partial_ms: Vec<f64>,
    /// Frames where the window drawn in parts showed something else than
    /// the one drawn whole, and the worst of them: its frame, how many
    /// pixels differ, and where (pixels).
    pub differing: Vec<usize>,
    pub worst: Option<(usize, usize, RECT)>,
    /// The widgets the person's part couldn't find.
    pub missing: Vec<&'static str>,
}

/// What the pointer does on a frame of the film.
#[derive(Clone, Copy)]
enum Cue {
    Move(Point),
    Down(Point),
    Up(Point),
    /// Onto the rows, and the wheel turns: notches (+ = up).
    Wheel(Point, f32),
}

/// Self-test: the window as a person uses it, on a held clock at `hz`. The
/// pointer comes onto Sounds and flips it, picks Island in the sidebar, and
/// turns the wheel a notch down over its rows. Two windows play it at once:
/// one draws a glide's frames as the app does (only what moves), the other
/// whole. After every frame both are compared as they show on screen (a
/// frame drawn in part shows only that part: the rest of the window stays as
/// it was), and `on_frame` gets the first one's (premultiplied BGRA) and the
/// part it drew (pixels; None = all of it). They open on a short screen
/// (the card at its shortest), so Island's rows scroll.
pub fn film(st: &mut AppState, work: RECT, dpi: u32, hz: f64, mut on_frame: impl FnMut(usize, i32, i32, &[u8], Option<RECT>)) -> Result<Film> {
    let scale = dpi as f32 / 96.0;
    let short = RECT { bottom: work.top + ((CARD_H_MIN + 2.0 * MARGIN + 16.0) * scale).ceil() as i32, ..work };
    let mut parts = SettingsWindow::new(short, dpi, Section::General)?;
    let mut whole = SettingsWindow::new(short, dpi, Section::General)?;
    for w in [&mut parts, &mut whole] {
        // The motion itself is filmed, whatever Windows' animation effects say.
        w.reduce = false;
        w.light = false;
    }
    let sounds = crate::settings_window::row_id(Section::General, st, Key::Sounds);
    let island = crate::settings_window::nav_id(st, Section::Island);
    // The person's part, in 144ths of a second from the start.
    let at = |k: f64| (k * hz / 144.0).round() as usize;
    let (w_px, h_px) = surface_px(scale, parts.card_h);
    let mut film = Film { frames: at(216.0), size: (w_px, h_px), partial_ms: Vec::new(), differing: Vec::new(), worst: None, missing: Vec::new() };
    let t0 = crate::clock::mono() + 1.0;
    let (mut shown_a, mut shown_b) = (Vec::new(), Vec::new());
    for f in 0..film.frames {
        let now = t0 + f as f64 / hz;
        crate::clock::hold(Some(now));
        st.now = now;
        let mut find = |id: Option<u64>, what: &'static str| {
            let c = id.and_then(|id| parts.view.hit_rect(id)).map(|r| r.center());
            if c.is_none() && !film.missing.contains(&what) {
                film.missing.push(what);
            }
            c
        };
        let cue = match f {
            _ if f == at(3.0) => find(sounds, "Sounds").map(Cue::Move),
            _ if f == at(20.0) => find(sounds, "Sounds").map(Cue::Down),
            _ if f == at(24.0) => find(sounds, "Sounds").map(Cue::Up),
            _ if f == at(80.0) => find(island, "Island").map(Cue::Move),
            _ if f == at(84.0) => find(island, "Island").map(Cue::Down),
            _ if f == at(88.0) => find(island, "Island").map(Cue::Up),
            _ if f == at(150.0) => {
                let card = parts.card();
                Some(Cue::Wheel(Point::new(card.right() - 160.0, card.center().y), -1.0))
            }
            _ => None,
        };
        for v in [&mut parts.view, &mut whole.view] {
            match cue {
                Some(Cue::Move(p)) => {
                    v.hover(p);
                }
                Some(Cue::Down(p)) => v.press(p),
                Some(Cue::Up(p)) => v.release(p),
                Some(Cue::Wheel(p, n)) => {
                    v.hover(p);
                    v.wheel(p, n);
                }
                None => {}
            }
        }
        // Both windows take the same click: the second starts from the
        // settings the first had before it, and makes the same change.
        let before = st.settings.clone();
        let a = film_frame(&mut parts, st, now, cue.is_some(), true);
        st.settings = before;
        let b = film_frame(&mut whole, st, now, cue.is_some(), false);
        let mut drew = None;
        if let Some((px, ms, part)) = a {
            match part {
                // Only that part goes to the screen.
                Some(r) if shown_a.len() == px.len() => {
                    film.partial_ms.push(ms);
                    let row = |y: i32| ((y * w_px + r.left) * 4) as usize..((y * w_px + r.right) * 4) as usize;
                    for y in r.top..r.bottom {
                        shown_a[row(y)].copy_from_slice(&px[row(y)]);
                    }
                    drew = Some(r);
                }
                _ => shown_a = px,
            }
        }
        if let Some((px, _, _)) = b {
            shown_b = px;
        }
        if let Some((n, r)) = differ(&shown_a, &shown_b, w_px) {
            film.differing.push(f);
            if film.worst.is_none_or(|(_, m, _)| n > m) {
                film.worst = Some((f, n, r));
            }
        }
        on_frame(f, w_px, h_px, &shown_a, drew);
    }
    crate::clock::hold(None);
    film.partial_ms.sort_by(f64::total_cmp);
    Ok(film)
}

/// A frame of `w` as the app draws it: all of it after input or a change
/// (again while a click's change shows), only what moves on a glide's frame
/// (`partial`; otherwise all of it), nothing when nothing moves. Its pixels
/// when it drew, how long it took (ms), and the part it drew (pixels, as
/// `paint` picks it; None = all of it).
fn film_frame(w: &mut SettingsWindow, st: &mut AppState, now: f64, input: bool, partial: bool) -> Option<(Vec<u8>, f64, Option<RECT>)> {
    let changed = input || w.view.dirty;
    if !changed && !w.view.moving {
        return None;
    }
    let only = if changed || !partial { None } else { w.view.next_clip() };
    let (wp, hp) = surface_px(w.scale(), w.card_h);
    let part = only.filter(|_| !w.gfx.draws_all()).map(|r| crate::gfx::px_rect(r, w.scale(), 1, wp, hp));
    let t = std::time::Instant::now();
    for _ in 0..4 {
        w.gfx.request_capture(RECT { left: 0, top: 0, right: wp, bottom: hp });
        let out = w.paint(st, now, only, false);
        if only.is_some() || !(out.again || w.view.dirty) {
            break;
        }
    }
    Some((w.gfx.take_capture(), t.elapsed().as_secs_f64() * 1000.0, part))
}

/// How many pixels of `a` and `b` (BGRA, `w` a row) differ by more than a
/// hair, and where they are. None when the two match.
fn differ(a: &[u8], b: &[u8], w: i32) -> Option<(usize, RECT)> {
    if a.len() != b.len() {
        return Some((a.len().max(b.len()) / 4, RECT::default()));
    }
    let mut n = 0;
    let mut r = RECT { left: i32::MAX, top: i32::MAX, right: 0, bottom: 0 };
    for (i, (p, q)) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0).enumerate() {
        if p.iter().zip(q).any(|(x, y)| x.abs_diff(*y) > 2) {
            n += 1;
            let (x, y) = (i as i32 % w, i as i32 / w);
            r = RECT { left: r.left.min(x), top: r.top.min(y), right: r.right.max(x + 1), bottom: r.bottom.max(y + 1) };
        }
    }
    (n > 0).then_some((n, r))
}
