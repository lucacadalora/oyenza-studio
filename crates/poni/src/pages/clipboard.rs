//! Clipboard page: recent copied text (memory only), newest first.
//!
//! * Click a row to put that text back on the clipboard ("Copied" flash).
//! * Hover a row for pin / remove actions; pinned rows survive "Clear" and
//!   eviction.
//! * "Clear" and a row's remove can be undone for a few seconds.
//! * Mouse wheel scrolls (smoothly) with soft fades at the clipped edges.

use crate::commands::Command;
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, PillStyle, Resp, Ui, PILL_GAP, PILL_REGULAR, PILL_SMALL};
use poni_core::cliphist::{preview, ClipHistory, ClipItem};
use poni_core::geom::{palette, Color, Point, Rect};
use std::cell::{Cell, RefCell};

/// Summary row: item count on the left, Clear / Undo on the right.
const HEAD_H: f32 = PILL_SMALL;
const HEAD_GAP: f32 = 8.0;
const ROW_H: f32 = 32.0;
const ROW_GAP: f32 = 2.0;
const PITCH: f32 = ROW_H + ROW_GAP;
const ROW_RADIUS: f32 = 10.0;
/// Row backgrounds bleed this far into the page padding, so the row text
/// shares the content column's left edge with the summary row.
const ROW_BLEED: f32 = 10.0;
/// Rows visible before the list scrolls.
const MAX_VISIBLE: usize = 5;
/// Tallest page: summary row + five rows (= 204).
const MAX_H: f32 = HEAD_H + HEAD_GAP + MAX_VISIBLE as f32 * PITCH - ROW_GAP;
/// Shortest page (empty / disabled / one to three rows).
const MIN_H: f32 = 150.0;
/// Right-hand zone of a row: age, pin badge, hover actions or "Copied".
const TRAIL_W: f32 = 72.0;
const ACTION: f32 = 26.0;
/// Height of the fade drawn where the list is clipped.
const FADE_H: f32 = 16.0;
/// DIPs scrolled per wheel notch.
const WHEEL_STEP: f32 = PITCH * 1.5;
const FLASH_SECS: f64 = 0.8;
/// How long "Clear" or a removal can be undone.
const UNDO_SECS: f64 = 8.0;
/// Characters scanned / kept for the one-line preview.
const PREVIEW_SCAN: usize = 600;
const PREVIEW_CHARS: usize = 160;

pub fn height(st: &AppState) -> f32 {
    if !st.settings.clipboard_history || st.clip.is_empty() {
        return MIN_H;
    }
    let n = st.clip.len().min(MAX_VISIBLE) as f32;
    (HEAD_H + HEAD_GAP + n * PITCH - ROW_GAP).clamp(MIN_H, MAX_H)
}

/// When the Undo pill expires (the page needs a frame then), if one is up.
pub fn next_wake(st: &AppState) -> Option<f64> {
    UNDO.with(|u| u.borrow().as_ref().map(|u| u.at + UNDO_SECS)).filter(|&t| t > st.now)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if !st.settings.clipboard_history {
        st.ui.clip_flash = None;
        forget_undo();
        draw_disabled(ui, st, area);
        return;
    }

    let undo = undo_live(ui.now);
    let (head, rest) = area.split_top(HEAD_H);
    if !st.clip.is_empty() || undo {
        draw_summary(ui, st, head, undo);
    }
    if st.clip.is_empty() {
        st.ui.clip_scroll = 0.0;
        st.ui.clip_flash = None;
        super::empty_state(ui, area, Icon::Clipboard, "Nothing copied yet", "Text you copy shows up here, kept in memory only");
        return;
    }
    let list = Rect::new(rest.x, rest.y + HEAD_GAP, rest.w, (rest.h - HEAD_GAP).max(0.0));
    draw_list(ui, st, list);
}

// ------------------------------------------------------------------ summary

/// "12 items · 2 pinned" and a Small utility pill: Clear, or Undo right after
/// something was removed.
fn draw_summary(ui: &mut Ui, st: &mut AppState, r: Rect, undo: bool) {
    let scale = ui.c.scale().max(0.5);
    let now = ui.now;
    let n = st.clip.len();
    let pinned = st.clip.iter().filter(|it| it.pinned).count();
    let pill_y = snap(r.y + (r.h - PILL_SMALL) / 2.0, scale);
    let mut text_right = r.right();

    if undo {
        let w = snap(ui.pill_width("Undo", true, true), scale);
        let pr = Rect::new(snap(r.right() - w, scale), pill_y, w, PILL_SMALL);
        if ui.pill(wid("clip.undo"), pr, "Undo", Some(Icon::Undo), PillStyle::UtilityAccent(st.accent)).clicked {
            if let Some(u) = take_undo() {
                restore(&mut st.clip, &u);
            }
            st.ui.clip_flash = None;
        }
        text_right = pr.x - PILL_GAP;
    } else if pinned < n {
        // Only when something unpinned is left to clear.
        let w = snap(ui.pill_width("Clear", true, true), scale);
        let pr = Rect::new(snap(r.right() - w, scale), pill_y, w, PILL_SMALL);
        if ui.pill(wid("clip.clear"), pr, "Clear", Some(Icon::Trash), PillStyle::Utility).clicked {
            let before: Vec<ClipItem> = st.clip.iter().cloned().collect();
            st.clip.clear();
            let removed = before.iter().filter(|it| !it.pinned).cloned().collect();
            remember(before, removed, now);
            st.ui.clip_scroll = 0.0;
            st.ui.clip_flash = None;
        }
        text_right = pr.x - PILL_GAP;
    }

    let n = st.clip.len();
    if n == 0 {
        return;
    }
    let pinned = st.clip.iter().filter(|it| it.pinned).count();
    let mut count = format!("{n} {}", if n == 1 { "item" } else { "items" });
    if pinned > 0 {
        count.push_str(&format!(" \u{b7} {pinned} pinned"));
    }
    let count_st = TextStyle::body().weight(Weight::Semibold).color(palette::TEXT_DIM);
    ui.c.text(&count, Rect::new(r.x, r.y, (text_right - r.x).max(0.0), r.h), &count_st);
}

// ------------------------------------------------------------------ list

enum RowAction {
    Copy(usize),
    TogglePin(usize),
    Remove(usize),
}

fn draw_list(ui: &mut Ui, st: &mut AppState, list: Rect) {
    let n = st.clip.len();
    let content_h = n as f32 * PITCH - ROW_GAP;
    let max_scroll = (content_h - list.h).max(0.0);

    // Only claim the wheel when there is something to scroll; otherwise the
    // app uses it to switch pages. The whole page body (summary included)
    // scrolls; the tab header above it still switches pages.
    if max_scroll > 0.0 {
        let page = Rect::new(list.x - ROW_BLEED, list.y - HEAD_H - HEAD_GAP, list.w + 2.0 * ROW_BLEED, list.h + HEAD_H + HEAD_GAP);
        let w = ui.wheel_in(page);
        if w != 0.0 {
            st.ui.clip_scroll -= w * WHEEL_STEP;
        }
    }
    st.ui.clip_scroll = st.ui.clip_scroll.clamp(0.0, max_scroll);
    let scroll = smooth_scroll(ui, st.ui.clip_scroll, max_scroll);

    // Copied flash: active for FLASH_SECS, then cleared (and the animation stops).
    let now = ui.now;
    let flash = st.ui.clip_flash.filter(|&(i, t0)| i < n && now >= t0 && now - t0 < FLASH_SECS);
    st.ui.clip_flash = flash;
    if flash.is_some() {
        ui.request_anim();
    }

    let first = (scroll / PITCH).floor().max(0.0) as usize;
    let last = (((scroll + list.h) / PITCH).ceil().max(0.0) as usize).min(n);
    let mut action: Option<RowAction> = None;

    // The clip and the fades include the rows' bleed into the padding.
    let wide = Rect::new(list.x - ROW_BLEED, list.y, list.w + 2.0 * ROW_BLEED, list.h);
    ui.c.clip(wide);
    // While the list glides its rows move between pixels; at rest they sit on whole pixels.
    let snap_was = ui.c.set_text_snap(scroll == st.ui.clip_scroll);
    for i in first..last {
        let Some(item) = st.clip.get(i) else { break };
        let row = Rect::new(list.x, list.y + i as f32 * PITCH - scroll, list.w, ROW_H);
        let flash_t = match flash {
            Some((fi, t0)) if fi == i => Some((now - t0) as f32),
            _ => None,
        };
        if let Some(a) = draw_row(ui, st.accent, st.unix, i, item, row, list, flash_t) {
            action = Some(a);
        }
    }
    ui.c.set_text_snap(snap_was);
    // Fades where rows are cut off, stronger the more is hidden.
    fade_edge(ui, Rect::new(wide.x, wide.y, wide.w, FADE_H), true, (scroll / FADE_H).min(1.0));
    fade_edge(ui, Rect::new(wide.x, wide.bottom() - FADE_H, wide.w, FADE_H), false, ((max_scroll - scroll) / FADE_H).min(1.0));
    ui.c.unclip();

    if n <= 2 {
        draw_hint(ui, Rect::new(list.x, list.y + content_h, list.w, list.bottom() - list.y - content_h));
    }

    match action {
        Some(RowAction::Copy(i)) => {
            if let Some(it) = st.clip.get(i) {
                let text = it.text.clone();
                st.cmd(Command::CopyText(text));
                st.ui.clip_flash = Some((i, st.now));
                ui.request_anim();
            }
        }
        Some(RowAction::TogglePin(i)) => st.clip.toggle_pin(i),
        Some(RowAction::Remove(i)) => {
            let before: Vec<ClipItem> = st.clip.iter().cloned().collect();
            if let Some(removed) = st.clip.remove(i) {
                remember(before, vec![removed], now);
                st.ui.clip_flash = match st.ui.clip_flash {
                    Some((fi, _)) if fi == i => None,
                    Some((fi, t)) if fi > i => Some((fi - 1, t)),
                    f => f,
                };
            }
        }
        None => {}
    }
}

/// One history row: `row` is the text column; its background bleeds
/// `ROW_BLEED` into the padding on both sides. `flash_t` = seconds since this
/// row was copied (while flashing).
#[allow(clippy::too_many_arguments)]
fn draw_row(ui: &mut Ui, accent: Color, unix: i64, i: usize, item: &ClipItem, row: Rect, list: Rect, flash_t: Option<f32>) -> Option<RowAction> {
    let key = item_key(item);
    let scale = ui.c.scale().max(0.5);
    let (bx0, bx1) = (snap(row.x - ROW_BLEED, scale), snap(row.right() + ROW_BLEED, scale));
    let bg_r = Rect::new(bx0, row.y, bx1 - bx0, row.h);
    // Hit-test only the visible part so rows hidden under the fades/summary
    // can't be clicked.
    let top = bg_r.y.max(list.y);
    let bottom = bg_r.bottom().min(list.bottom());
    let vis = Rect::new(bg_r.x, top, bg_r.w, (bottom - top).max(0.0));
    let resp = if vis.h >= 8.0 { ui.button(wid_n("clip.row", key), vis) } else { Resp::default() };
    let mut action = if resp.clicked { Some(RowAction::Copy(i)) } else { None };

    let bg = if resp.pressed {
        Color::white(0.13)
    } else if resp.hovered {
        palette::SURFACE
    } else {
        Color::white(0.0)
    };
    if bg.a > 0.0 {
        ui.c.fill_rrect(bg_r, ROW_RADIUS, bg);
    }
    // Flash strength: full for most of the flash, then a quick fade-out.
    let flash_a = flash_t.map(|t| {
        let fade_from = FLASH_SECS as f32 - 0.25;
        if t <= fade_from {
            1.0
        } else {
            (1.0 - (t - fade_from) / 0.25).clamp(0.0, 1.0)
        }
    });
    if let Some(a) = flash_a {
        ui.c.fill_rrect(bg_r, ROW_RADIUS, palette::GREEN.with_alpha(0.16 * a));
    }

    // Preview text, on the content column's left edge.
    let text_r = Rect::new(row.x, row.y, (row.w - TRAIL_W - 8.0).max(0.0), row.h);
    ui.c.text(&row_preview(&item.text), text_r, &TextStyle::body());

    // Trailing zone, ending on the content column's right edge.
    let cy = row.center().y;
    let trail_right = row.right();
    if let Some(a) = flash_a {
        ui.c.with_alpha(a, |c| {
            let st = TextStyle::body().weight(Weight::Semibold).color(palette::GREEN).right();
            let label_w = c.measure("Copied", &st, TRAIL_W).0 + 2.0;
            c.text("Copied", Rect::new(trail_right - label_w, row.y, label_w, row.h), &st);
            c.icon(Icon::Check, Rect::new(trail_right - label_w - 18.0, cy - 7.0, 14.0, 14.0), palette::GREEN);
        });
    } else if resp.hovered && row.y >= list.y - 0.5 && row.bottom() <= list.bottom() + 0.5 {
        // Hover actions, registered after the row so they win the hit test.
        let del = Rect::new(bg_r.right() - 4.0 - ACTION, cy - ACTION / 2.0, ACTION, ACTION);
        let pin = Rect::new(del.x - 2.0 - ACTION, del.y, ACTION, ACTION);
        let (pin_icon, pin_col) = if item.pinned { (Icon::PinFill, accent) } else { (Icon::Pin, palette::TEXT_DIM) };
        if ui.icon_button(wid_n("clip.pin", key), pin, pin_icon, pin_col, 14.0).clicked {
            action = Some(RowAction::TogglePin(i));
        }
        if ui.icon_button(wid_n("clip.del", key), del, Icon::Close, palette::TEXT_DIM, 13.0).clicked {
            action = Some(RowAction::Remove(i));
        }
    } else {
        let age = fmt_age(unix.saturating_sub(item.at_unix));
        let age_st = TextStyle::footnote().color(palette::TEXT_FAINT).tabular().right();
        let age_w = ui.c.measure(&age, &age_st, TRAIL_W).0;
        ui.c.text(&age, Rect::new(trail_right - age_w - 2.0, row.y, age_w + 2.0, row.h), &age_st);
        if item.pinned {
            ui.c.icon(Icon::PinFill, Rect::new(trail_right - age_w - 6.0 - 13.0, cy - 6.5, 13.0, 13.0), accent);
        }
    }
    action
}

/// Faint usage hint in the empty space below a short list.
fn draw_hint(ui: &mut Ui, r: Rect) {
    if r.h < 36.0 {
        return;
    }
    ui.c.text(
        "Click a row to copy it again",
        Rect::new(r.x, r.y + (r.h - 20.0) / 2.0, r.w, 20.0),
        &TextStyle::footnote().color(palette::TEXT_FAINT).center(),
    );
}

// ------------------------------------------------------------------ disabled

fn draw_disabled(ui: &mut Ui, st: &mut AppState, area: Rect) {
    // Icon, title, hint and a pill as one vertically centred group.
    let group_h = 26.0 + 10.0 + 20.0 + 2.0 + 18.0 + 14.0 + PILL_REGULAR;
    let mut y = area.y + ((area.h - group_h) / 2.0).max(0.0);
    let cx = area.center().x;
    ui.c.icon(Icon::Clipboard, Rect::from_center(Point::new(cx, y + 13.0), 26.0, 26.0), Color::white(0.35));
    y += 36.0;
    ui.c.text("Clipboard history is off", Rect::new(area.x, y, area.w, 20.0), &TextStyle::headline().center());
    y += 22.0;
    ui.c.text(
        "Keep your last text copies here. Nothing is written to disk.",
        Rect::new(area.x, y, area.w, 18.0),
        &TextStyle::footnote().color(palette::TEXT_DIM).center(),
    );
    y += 32.0;
    let scale = ui.c.scale().max(0.5);
    let w = snap(ui.pill_width("Turn on", false, false), scale);
    let pill = Rect::new(snap(cx - w / 2.0, scale), snap(y, scale), w, PILL_REGULAR);
    if ui.pill(wid("clip.enable"), pill, "Turn on", None, PillStyle::Primary(st.accent)).clicked {
        st.settings.clipboard_history = true;
        st.cmd(Command::SaveSettings);
    }
}

// ------------------------------------------------------------------ undo

/// What the last Clear / remove took away, for a few seconds.
struct Undo {
    /// The whole list before the action (gives the original order).
    before: Vec<ClipItem>,
    removed: Vec<ClipItem>,
    at: f64,
}

thread_local! {
    // Pages only ever run on the UI thread. Memory only, like the history.
    static UNDO: RefCell<Option<Undo>> = const { RefCell::new(None) };
}

fn remember(before: Vec<ClipItem>, removed: Vec<ClipItem>, now: f64) {
    if removed.is_empty() {
        return;
    }
    UNDO.with(|u| *u.borrow_mut() = Some(Undo { before, removed, at: now }));
}

/// Whether an Undo is still on offer; drops an expired snapshot.
fn undo_live(now: f64) -> bool {
    UNDO.with(|u| {
        let mut u = u.borrow_mut();
        let live = u.as_ref().is_some_and(|x| now >= x.at && now - x.at < UNDO_SECS);
        if !live {
            *u = None;
        }
        live
    })
}

fn take_undo() -> Option<Undo> {
    UNDO.with(|u| u.borrow_mut().take())
}

fn forget_undo() {
    let _ = UNDO.try_with(|u| u.borrow_mut().take());
}

/// Put the removed items back where they were. Anything copied since stays
/// on top; a removed text that has been copied again is not duplicated.
fn restore(clip: &mut ClipHistory, u: &Undo) {
    let current: Vec<ClipItem> = clip.iter().cloned().collect();
    let same = |a: &ClipItem, b: &ClipItem| a.text == b.text && a.at_unix == b.at_unix;
    let mut out: Vec<ClipItem> = current.iter().filter(|c| !u.before.iter().any(|b| same(b, c))).cloned().collect();
    for b in &u.before {
        if let Some(c) = current.iter().find(|c| same(c, b)) {
            out.push(c.clone());
        } else if u.removed.iter().any(|r| same(r, b)) && !current.iter().any(|c| c.text == b.text) {
            out.push(b.clone());
        }
    }
    // Rebuild oldest first (push adds to the front), keeping each item's time and pin.
    while clip.remove(0).is_some() {}
    for it in out.iter().rev() {
        if clip.push(&it.text, it.at_unix) && it.pinned {
            clip.toggle_pin(0);
        }
    }
}

// ------------------------------------------------------------------ helpers

thread_local! {
    /// Displayed (animated) scroll offset and the time it was last drawn.
    static SCROLL_ANIM: Cell<(f32, f64)> = const { Cell::new((0.0, f64::NEG_INFINITY)) };
}

/// Ease the drawn scroll offset towards `target`; frames are requested only
/// while it is still moving.
fn smooth_scroll(ui: &mut Ui, target: f32, max_scroll: f32) -> f32 {
    let now = ui.now;
    SCROLL_ANIM.with(|cell| {
        let (prev, last) = cell.get();
        // After an idle gap (the frame clock was parked) step as if one frame passed.
        let dt = if now > last { (now - last).min(1.0 / 30.0) as f32 } else { 0.0 };
        let mut pos = prev.clamp(0.0, max_scroll);
        pos += (target - pos) * (1.0 - (-dt * 18.0).exp());
        if (target - pos).abs() < 0.5 {
            pos = target;
        } else {
            ui.request_anim();
        }
        cell.set((pos, now));
        pos
    })
}

/// Black gradient over `r` (the island is pure black, so this reads as content
/// fading out). Opaque-ish at the outer edge, transparent at the inner one.
fn fade_edge(ui: &mut Ui, r: Rect, outer_is_top: bool, strength: f32) {
    if strength <= 0.01 || r.h <= 0.0 {
        return;
    }
    const STEPS: usize = 8;
    const EDGE: f32 = 0.94;
    // Nested bands that all touch the outer edge (no seams at fractional
    // scales). Band k covers the outer (k+1)/STEPS of the fade; per-band alphas
    // make the composite fall off linearly towards the inner edge.
    let target = |j: usize| EDGE * strength * (STEPS - j) as f32 / STEPS as f32;
    for k in (0..STEPS).rev() {
        let below = if k + 1 < STEPS { target(k + 1) } else { 0.0 };
        let a = 1.0 - (1.0 - target(k)) / (1.0 - below);
        let depth = r.h * (k + 1) as f32 / STEPS as f32;
        let band = if outer_is_top { Rect::new(r.x, r.y, r.w, depth) } else { Rect::new(r.x, r.bottom() - depth, r.w, depth) };
        ui.c.fill_rect(band, Color::black(a));
    }
}

fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// One-line preview; only the head of very long copies is scanned.
fn row_preview(text: &str) -> String {
    let head = match text.char_indices().nth(PREVIEW_SCAN) {
        Some((b, _)) => &text[..b],
        None => text,
    };
    let mut p = preview(head, PREVIEW_CHARS);
    if head.len() < text.len() {
        if p.is_empty() {
            p = preview(text, PREVIEW_CHARS);
        } else if !p.ends_with('…') {
            p.push('…');
        }
    }
    p.retain(|c| !c.is_control());
    p
}

/// Stable per-item id component (content head + length + capture time), so a
/// press that straddles a list change can't hit a different row.
fn item_key(it: &ClipItem) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in it.text.as_bytes().iter().take(256) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ (it.text.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (it.at_unix as u64).rotate_left(29)
}

/// "now", "2m", "1h", "3d".
fn fmt_age(secs: i64) -> String {
    match secs {
        s if s < 60 => "now".to_string(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(fmt_age(-5), "now");
        assert_eq!(fmt_age(59), "now");
        assert_eq!(fmt_age(150), "2m");
        assert_eq!(fmt_age(3600), "1h");
        assert_eq!(fmt_age(3 * 86_400 + 10), "3d");
    }

    #[test]
    fn previews() {
        assert_eq!(row_preview("  first line\n\tsecond  "), "first line second");
        let long = "word ".repeat(1000);
        let p = row_preview(&long);
        assert!(p.ends_with('…') && p.chars().count() <= PREVIEW_CHARS);
        let late = format!("{}tail", " ".repeat(PREVIEW_SCAN + 10));
        assert_eq!(row_preview(&late), "tail");
        assert_eq!(row_preview("a\u{0}b"), "ab");
    }

    #[test]
    fn page_heights() {
        assert_eq!(MAX_H, 204.0);
        assert!(MIN_H <= MAX_H);
    }

    fn texts(c: &ClipHistory) -> Vec<(String, bool)> {
        c.iter().map(|it| (it.text.clone(), it.pinned)).collect()
    }

    #[test]
    fn undo_restores_order_pins_and_newer_copies() {
        let mut c = ClipHistory::new(30);
        for (i, t) in ["d", "c", "b", "a"].iter().enumerate() {
            c.push(t, 100 + i as i64);
        }
        c.toggle_pin(1); // "b"
        let before: Vec<ClipItem> = c.iter().cloned().collect();
        c.clear();
        let removed = before.iter().filter(|it| !it.pinned).cloned().collect();
        let u = Undo { before, removed, at: 0.0 };
        // Copied after the clear: "e" is new, "c" was cleared and copied again.
        c.push("e", 200);
        c.push("c", 201);
        restore(&mut c, &u);
        let want: Vec<(String, bool)> =
            [("c", false), ("e", false), ("a", false), ("b", true), ("d", false)].iter().map(|(t, p)| (t.to_string(), *p)).collect();
        assert_eq!(texts(&c), want);
    }

    #[test]
    fn undo_single_remove_goes_back_in_place() {
        let mut c = ClipHistory::new(30);
        for (i, t) in ["c", "b", "a"].iter().enumerate() {
            c.push(t, 10 + i as i64);
        }
        let before: Vec<ClipItem> = c.iter().cloned().collect();
        let removed = c.remove(1).unwrap();
        restore(&mut c, &Undo { before, removed: vec![removed], at: 0.0 });
        assert_eq!(texts(&c).into_iter().map(|(t, _)| t).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn undo_respects_capacity_and_keeps_pins() {
        let mut c = ClipHistory::new(3);
        for (i, t) in ["c", "b", "a"].iter().enumerate() {
            c.push(t, 10 + i as i64);
        }
        c.toggle_pin(2); // "c" (oldest) pinned
        let before: Vec<ClipItem> = c.iter().cloned().collect();
        c.clear();
        let removed = before.iter().filter(|it| !it.pinned).cloned().collect();
        let u = Undo { before, removed, at: 0.0 };
        c.push("x", 50);
        c.push("y", 51);
        restore(&mut c, &u);
        // Cap 3: the newer copies and the pinned item stay; the oldest unpinned go.
        assert_eq!(texts(&c), vec![("y".to_string(), false), ("x".to_string(), false), ("c".to_string(), true)]);
    }
}
