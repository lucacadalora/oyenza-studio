//! Script page: a teleprompter under the webcam.
//!
//! Paste a script, or drop a .txt file on the island while this page shows,
//! and read it right under the camera: the line to say bright, the next ones
//! dim, the way the Music page shows lyrics. The reader moves a line at a
//! time at the pace in words a minute (`poni_core::script`). Between lines
//! nothing redraws: the app's wake timer brings the next line, which glides
//! up in about a third of a second.
//!
//! While a reading is under way the island stays open. Click the script to
//! steer it from the keyboard (Space pauses, ←/→ go back or on a line, ↑/↓
//! change the pace, Esc lets go), or press Alt+Shift+S in any app on Windows.
//! The Windows island is left out of screen shares and recordings while this
//! page shows (`window::exclude_from_capture`).
//!
//! Once Oyenza Studio Recorder is installed, Record (by Paste) has it record
//! the island's screen; pausing and stopping stay on the recorder's own bar.

use crate::commands::Command;
use crate::gfx::{ts, TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::media::{follow, settle, step_dt};
use crate::state::AppState;
use crate::ui::{wid, KeyInput, PillStyle, Ui, PILL_GAP, PILL_REGULAR};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::script::{self as model, Line, Reader, MAX_WPM, MIN_WPM, WPM_STEP};
use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Same as Tasks, the tallest page, so this one never makes the window grow.
const HEIGHT: f32 = 200.0;
/// The script's type size (every line: the one to say is only brighter, so
/// nothing re-flows as the reading moves).
const READ_SIZE: f32 = 20.0;
const LEADING: f32 = 1.25;
const ROW_GAP: f32 = 4.0;
/// The line to say sits one row down, so the line before it shows above.
const ANCHOR_ROWS: f32 = 1.0;
const EDGE_FADE: f32 = 18.0;
/// Glide between lines (critically damped, as the lyrics).
const OMEGA: f32 = 12.0;
/// The controls under the script.
const BAR_GAP: f32 = 10.0;
const READ_MIN_W: f32 = 108.0;
const ICON_BTN: f32 = 30.0;
const PACE_LABEL_W: f32 = 64.0;
/// Record's dot and ring, a little bigger than the bar's other icons.
const RECORD_ICON: f32 = 18.0;
/// A message from the app ("Nothing to paste") shows this long.
const NOTE_S: f64 = 4.0;
/// Wheel notches per line.
const WHEEL_LINE: f32 = 1.0;
const VIEW_ID: &str = "script.view";

fn read_style() -> TextStyle {
    TextStyle::new(READ_SIZE).weight(Weight::Semibold).line_spacing(LEADING).top()
}

fn row_h() -> f32 {
    READ_SIZE * LEADING + ROW_GAP
}

// ---------------------------------------------------------------- lines and glide

/// The script cut into lines at the page's width, and the glide between them.
#[derive(Default)]
struct Cache {
    /// What `lines` were cut from: text hash, width and DPI scale.
    key: (u64, u32, u32),
    lines: Vec<Line>,
    /// Word → width at [`READ_SIZE`] (DIPs), for re-cutting at another width.
    widths: HashMap<String, f32>,
    /// Shown position in lines (follows the reader's line).
    pos: f32,
    vel: f32,
    last_t: Option<f64>,
}

thread_local! {
    // Pages only ever run on the UI thread.
    static CACHE: Cell<Option<Box<Cache>>> = const { Cell::new(None) };
}

fn take_cache() -> Box<Cache> {
    CACHE.try_with(|c| c.take()).ok().flatten().unwrap_or_default()
}

fn put_cache(c: Box<Cache>) {
    let _ = CACHE.try_with(|cell| cell.set(Some(c)));
}

fn text_hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl Cache {
    fn update(&mut self, ui: &mut Ui, text: &str, width: f32) {
        let key = (text_hash(text), width.to_bits(), ui.c.scale().to_bits());
        if key == self.key && !(self.lines.is_empty() && !text.is_empty()) {
            return;
        }
        if key.2 != self.key.2 {
            self.widths.clear();
        }
        self.key = key;
        let st = read_style();
        let widths = &mut self.widths;
        let c = &mut *ui.c;
        self.lines = model::wrap(text, width, |w| *widths.entry(w.to_string()).or_insert_with(|| c.measure(w, &st, f32::MAX).0));
    }
}

/// The lines the page cut last (empty until it is drawn).
fn with_lines<R>(f: impl FnOnce(&[Line]) -> R) -> R {
    let c = take_cache();
    let r = f(&c.lines);
    put_cache(c);
    r
}

// ---------------------------------------------------------------- app hooks

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// When the next line is due while a reading plays, or a message ends (the
/// app's wake timer).
pub fn next_wake(st: &AppState) -> Option<f64> {
    let wpm = st.store.script.pace();
    let line = with_lines(|lines| st.script.next_change(lines, wpm));
    let note = st.script_note.map(|(_, until)| until).filter(|&until| until > st.now);
    match (line, note) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Pause or resume (the Alt+Shift+S shortcut).
pub fn toggle(st: &mut AppState) {
    let now = st.now;
    with_lines(|lines| st.script.toggle(now, lines));
}

/// A new script: from the top, paused, saved.
pub fn set_text(st: &mut AppState, text: String) {
    st.store.script.text = text;
    st.script = Reader::default();
    st.cmd(Command::SaveStore);
}

/// Show a short message under the script.
pub fn note(st: &mut AppState, text: &'static str) {
    st.script_note = Some((text, st.now + NOTE_S));
}

/// Files dropped while this page shows: the first text file becomes the
/// script. True when one did (the drop is then not for the Shelf).
pub fn take_drop(st: &mut AppState, paths: &[String]) -> bool {
    let Some(path) = paths.iter().find(|p| model::is_text_file(p)) else { return false };
    let read = std::fs::metadata(path).ok().filter(|m| m.len() <= model::MAX_FILE_BYTES).and_then(|_| std::fs::read(path).ok());
    match read.as_deref().and_then(model::decode) {
        Some(text) => set_text(st, text),
        None => note(st, "No text in that file"),
    }
    true
}

// ---------------------------------------------------------------- drawing

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let mut cache = take_cache();
    if st.store.script.is_empty() {
        draw_empty(ui, st, area);
        cache.last_t = None;
    } else {
        draw_reading(ui, st, area, &mut cache);
    }
    if st.drag_hover {
        draw_drop_hint(ui, area);
    }
    put_cache(cache);
}

fn draw_empty(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let top = Rect::new(area.x, area.y, area.w, area.h - 52.0);
    super::empty_state(ui, top, Icon::Script, "Paste a script to read", "Copy it anywhere, then press Paste. Or drop a .txt file here.");
    let label = "Paste";
    let w = ui.pill_width(label, true, false);
    let r = Rect::new(area.center().x - w / 2.0, top.bottom() - 6.0, w, PILL_REGULAR);
    if ui.pill(wid("script.paste.empty"), r, label, Some(Icon::Clipboard), PillStyle::Primary(st.accent)).clicked {
        st.cmd(Command::ScriptPaste);
    }
    if let Some(text) = live_note(ui, st) {
        let y = r.bottom() + 4.0;
        ui.c.text(text, Rect::new(area.x, y, area.w, 16.0), &TextStyle::footnote().color(palette::TEXT_DIM).center());
    }
}

fn draw_reading(ui: &mut Ui, st: &mut AppState, area: Rect, cache: &mut Cache) {
    let now = ui.now;
    let wpm = st.store.script.pace();
    let bar_y = area.bottom() - PILL_REGULAR;
    let view = Rect::new(area.x, area.y, area.w, bar_y - BAR_GAP - area.y);
    cache.update(ui, &st.store.script.text, view.w);
    let lines = std::mem::take(&mut cache.lines);
    if st.script.tick(now, &lines, wpm) {
        ui.request_repaint();
    }

    // Click the script to steer it from the keyboard; a click on another
    // line also goes there.
    let resp = ui.interact(wid(VIEW_ID), view, crate::ui::CursorKind::Hand);
    let cur = st.script.line(&lines);
    let keys = ui.key_focus(wid(VIEW_ID), resp.clicked);
    let focused = ui.input.focused == Some(wid(VIEW_ID));
    let row = row_h();
    let anchor_y = view.y + ANCHOR_ROWS * row;
    let hovered_row = ui.input.mouse.filter(|_| resp.hovered).map(|m| (cache.pos + (m.y - anchor_y) / row).floor() as isize);
    if resp.clicked {
        if let Some(r) = hovered_row.filter(|&r| r >= 0 && (r as usize) < lines.len() && r as usize != cur) {
            st.script.seek(r as usize, &lines, now);
        }
    }
    for k in keys {
        match k {
            KeyInput::Char(' ') | KeyInput::Enter => st.script.toggle(now, &lines),
            KeyInput::Left => st.script.step(-1, &lines, now),
            KeyInput::Right => st.script.step(1, &lines, now),
            KeyInput::Up => set_pace(st, &lines, wpm + WPM_STEP),
            KeyInput::Down => set_pace(st, &lines, wpm.saturating_sub(WPM_STEP)),
            KeyInput::Home => st.script.restart(now),
            KeyInput::End => st.script.seek(usize::MAX, &lines, now),
            _ => {}
        }
    }
    let notches = ui.wheel_in(view);
    if notches != 0.0 {
        st.script.step(-(notches / WHEEL_LINE).round() as isize, &lines, now);
    }

    // Glide to the line to say.
    let cur = st.script.line(&lines);
    let target = cur as f32;
    let gap = cache.last_t.map_or(f64::INFINITY, |t| now - t);
    cache.last_t = Some(now);
    if !(0.0..=0.5).contains(&gap) || (cache.pos - target).abs() > 3.0 {
        // Resumed after a while or a long jump: land without a long slide.
        cache.pos = target;
        cache.vel = 0.0;
    }
    let dt = step_dt(gap.clamp(0.0, 0.25) as f32, cache.vel, cache.pos, target);
    follow(&mut cache.pos, &mut cache.vel, target, OMEGA, dt);
    let settled = settle(&mut cache.pos, &mut cache.vel, target, 0.004, 0.02);
    if !settled {
        ui.request_anim();
    }

    // The lines around it, the one to say bright, faded at the view's edges.
    let scale = ui.c.scale().max(0.5);
    let style = read_style();
    let first = (cache.pos.floor() as isize - ANCHOR_ROWS as isize - 1).max(0) as usize;
    let rows_shown = (view.h / row).ceil() as usize + 3;
    ui.c.clip(view);
    let snap_was = ui.c.set_text_snap(settled);
    for i in first..(first + rows_shown).min(lines.len()) {
        let mut y = anchor_y + (i as f32 - cache.pos) * row;
        if settled {
            y = super::snap(y, scale);
        }
        if y > view.bottom() || y + row < view.y {
            continue;
        }
        let cy = y + READ_SIZE * LEADING / 2.0;
        let fade = smoothstep((cy - view.y).min(view.bottom() - cy) / EDGE_FADE);
        let d = i as f32 - cache.pos;
        // The line to say 1.0; the next ones step down so the eye finds the
        // line after; the ones already said dimmer still.
        let base = if d >= 0.0 { 1.0 - 0.45 * d.min(1.0) - 0.12 * (d - 1.0).clamp(0.0, 2.0) } else { 1.0 - 0.72 * (-d).min(1.0) };
        let mut alpha = base.clamp(0.0, 1.0) * fade;
        if hovered_row == Some(i as isize) && i != cur && fade > 0.4 {
            let pill = Rect::new(view.x - 6.0, y - 2.0, view.w + 12.0, READ_SIZE * LEADING + 4.0);
            ui.c.fill_rrect(pill, 8.0, Color::white(0.07 * fade));
            alpha = alpha.max(0.7 * fade);
        }
        if alpha <= 0.01 {
            continue;
        }
        ui.c.text(&lines[i].text, Rect::new(view.x, y, view.w, READ_SIZE * LEADING + 2.0), &style.clone().color(Color::white(alpha)));
    }
    ui.c.set_text_snap(snap_was);
    ui.c.unclip();
    if focused {
        ui.c.stroke_rrect(view.inset(-4.0, -2.0), 10.0, Color::white(0.18), 1.0);
    }

    draw_bar(ui, st, area, bar_y, &lines, focused);
    cache.lines = lines;
}

fn set_pace(st: &mut AppState, lines: &[Line], wpm: u32) {
    let (old, new) = (st.store.script.pace(), wpm.clamp(MIN_WPM, MAX_WPM));
    if new != old {
        st.script.repace(st.now, lines, old, new);
        st.store.script.wpm = new;
        st.cmd(Command::SaveStore);
    }
}

/// Read/Pause, back to the start, the pace, then what's going on; Record
/// (with the recorder installed), Paste and Clear on the right.
fn draw_bar(ui: &mut Ui, st: &mut AppState, area: Rect, y: f32, lines: &[Line], focused: bool) {
    let now = ui.now;
    let wpm = st.store.script.pace();
    let r = &st.script;
    let (label, icon) = if r.playing() {
        ("Pause", Icon::Pause)
    } else if r.done {
        ("Read again", Icon::Reset)
    } else if r.active {
        ("Resume", Icon::Play)
    } else {
        ("Read", Icon::Play)
    };
    let w = ui.pill_width(label, true, false).max(READ_MIN_W);
    let read_r = Rect::new(area.x, y, w, PILL_REGULAR);
    if ui.pill(wid("script.read"), read_r, label, Some(icon), PillStyle::Primary(st.accent)).clicked {
        st.script.toggle(now, lines);
    }
    let mut x = read_r.right() + PILL_GAP;
    let cy = y + PILL_REGULAR / 2.0;
    let btn = |x: f32| Rect::new(x, cy - ICON_BTN / 2.0, ICON_BTN, ICON_BTN);
    if st.script.active {
        if ui.icon_button(wid("script.stop"), btn(x), Icon::Stop, palette::TEXT_DIM, 15.0).clicked {
            st.script.stop(now);
        }
    } else if ui.icon_button(wid("script.restart"), btn(x), Icon::Reset, palette::TEXT_DIM, 15.0).clicked {
        st.script.restart(now);
    }
    x += ICON_BTN + PILL_GAP;

    // Pace: − 140 wpm +
    if ui.icon_button(wid("script.slower"), btn(x), Icon::Minus, palette::TEXT_DIM, 14.0).clicked {
        set_pace(st, lines, wpm.saturating_sub(WPM_STEP));
    }
    x += ICON_BTN;
    let pace = format!("{wpm} wpm");
    ui.c.text(&pace, Rect::new(x, y, PACE_LABEL_W, PILL_REGULAR), &TextStyle::body().weight(Weight::Semibold).tabular().center());
    x += PACE_LABEL_W;
    if ui.icon_button(wid("script.faster"), btn(x), Icon::Plus, palette::TEXT_DIM, 14.0).clicked {
        set_pace(st, lines, wpm + WPM_STEP);
    }
    x += ICON_BTN + PILL_GAP + 4.0;

    // Right: Clear, then Paste (a new script replaces this one), then Record.
    let clear_r = btn(area.right() - ICON_BTN);
    if ui.icon_button(wid("script.clear"), clear_r, Icon::Trash, palette::TEXT_DIM, 14.0).clicked {
        set_text(st, String::new());
    }
    let paste = "Paste";
    let pw = ui.pill_width(paste, true, false);
    let paste_r = Rect::new(clear_r.x - PILL_GAP - pw, y, pw, PILL_REGULAR);
    if ui.pill(wid("script.paste"), paste_r, paste, Some(Icon::Clipboard), PillStyle::Utility).clicked {
        st.cmd(Command::ScriptPaste);
    }

    let mut right = paste_r.x;
    let mut hint = None;
    if st.recorder {
        let record_r = btn(paste_r.x - PILL_GAP - ICON_BTN);
        let resp = ui.icon_button(wid("script.record"), record_r, Icon::Record, palette::RED, RECORD_ICON);
        ui.c.icon(Icon::Circle, Rect::from_center(record_r.center(), RECORD_ICON, RECORD_ICON), palette::TEXT_DIM);
        if resp.clicked {
            st.cmd(Command::RecordScreen);
            note(st, "Opening the recorder");
        }
        // The island has no tooltips: the status line says what the dot does.
        if resp.hovered {
            hint = Some("Record this screen");
        }
        right = record_r.x;
    }

    // What's going on, in the room left between the two groups.
    let room = Rect::new(x, y, (right - PILL_GAP - x).max(0.0), PILL_REGULAR);
    let style = TextStyle::new(ts::FOOTNOTE).color(palette::TEXT_DIM);
    let status = match hint {
        Some(text) if live_note(ui, st).is_none() => text.into(),
        _ => status_line(ui, st, now, lines, wpm, focused, room.w, &style),
    };
    if room.w > 40.0 {
        ui.c.clip(room);
        ui.c.text(&status, room, &style);
        ui.c.unclip();
    }
}

/// The time left while reading; how to steer while the keyboard is on the
/// script; otherwise whether screen shares can see it, in fewer words when
/// that doesn't fit in `room` (Record takes some of it).
#[allow(clippy::too_many_arguments)]
fn status_line(ui: &mut Ui, st: &AppState, now: f64, lines: &[Line], wpm: u32, focused: bool, room: f32, style: &TextStyle) -> String {
    if let Some(text) = live_note(ui, st) {
        return text.into();
    }
    if focused {
        return "Space pauses · ↑↓ pace".into();
    }
    if st.script.playing() {
        let left = st.script.remaining(now, lines, wpm);
        return if left < 60.0 { "Under a minute left".into() } else { format!("About {} min left", (left / 60.0).round() as u32) };
    }
    let (full, short) = match st.script_hidden {
        Some(true) => ("Hidden from screen shares", "Hidden from shares"),
        _ if cfg!(target_os = "macos") => ("Shows in full-screen shares", "Shows in shares"),
        _ => ("Shows in screen shares", "Shows in shares"),
    };
    if ui.c.measure(full, style, f32::MAX).0 <= room { full } else { short }.into()
}

/// A message from the app while it lasts ([`next_wake`] brings the frame
/// that takes it away).
fn live_note(ui: &Ui, st: &AppState) -> Option<&'static str> {
    let (text, until) = st.script_note?;
    (ui.now < until).then_some(text)
}

fn draw_drop_hint(ui: &mut Ui, area: Rect) {
    let r = area.inset(2.0, 2.0);
    ui.c.fill_rrect(r, 12.0, Color::rgba(0.0, 0.0, 0.0, 0.72));
    ui.c.stroke_rrect(r, 12.0, Color::white(0.35), 1.5);
    let c = r.center();
    ui.c.icon(Icon::File, Rect::from_center(Point::new(c.x, c.y - 14.0), 24.0, 24.0), Color::white(0.8));
    ui.c.text("Drop a .txt file to read it", Rect::new(r.x, c.y + 6.0, r.w, 20.0), &TextStyle::headline().center());
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
