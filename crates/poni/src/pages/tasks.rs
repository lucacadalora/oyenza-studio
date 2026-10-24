//! Tasks page: a quick to-do list.
//!
//! The input is the list's first row (a plus in the checkbox column; Enter
//! adds and keeps focus for rapid entry, Esc leaves the field), the tasks
//! below with circle checkboxes, hover-only delete and smooth wheel scrolling
//! with fading edges, and a footer with today's progress and "Clear done".
//! Delete and "Clear done" can be undone for a few seconds.
//!
//! Rows glide to their new place when a task is checked off (done tasks sink
//! below the open ones), but only once the pointer leaves the list (or after
//! a moment), so the next row never slides under the cursor. New rows fade
//! in and the checkbox pops. All of it is event-driven: nothing animates
//! while idle.

use crate::gfx::TextStyle;
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, PillStyle, Ui, WidgetId, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::todo::{Task, MAX_TASKS, MAX_TASK_LEN};
use std::cell::Cell;
use std::collections::HashMap;

const HEIGHT: f32 = 200.0;
const INPUT_H: f32 = 34.0;
const ROW_H: f32 = 31.0;
/// List viewport, relative to the page top: exactly four rows.
const LIST_TOP: f32 = 44.0;
const LIST_H: f32 = 4.0 * ROW_H;
/// The footer holds a Small capsule, so it is exactly that tall.
const FOOTER_H: f32 = PILL_SMALL;
/// Checkbox / ring / plus column centre and the text column, from the row's left edge.
const MARK_X: f32 = 18.0;
const TEXT_X: f32 = 36.0;
/// The neutral track under every progress ring.
const RING_TRACK: Color = Color::white(0.14);
/// Height of the fade at a scrollable edge.
const FADE: f32 = 14.0;
const FADE_STEPS: usize = 7;
/// Scroll distance per wheel notch.
const WHEEL_STEP: f32 = ROW_H * 1.5;
/// Exponential smoothing rate (1/s) for scrolling and row movement.
const GLIDE_RATE: f32 = 16.0;
const POP_S: f64 = 0.32;
const APPEAR_S: f64 = 0.24;
/// After a toggle the rows keep their places while the pointer stays over the list, at most this long.
const FREEZE_S: f64 = 2.0;
/// How long "Undo" is offered after a delete or "Clear done".
const UNDO_SECS: f64 = 8.0;
/// When the page hasn't been drawn for this long, jump to the current state.
const STALE_S: f64 = 1.0;

#[derive(Default)]
struct ListAnim {
    last_draw: f64,
    /// Displayed scroll offset (eases toward `st.ui.task_scroll`).
    scroll: f32,
    /// Displayed y of each row inside the list content, by task id.
    rows: HashMap<u64, f32>,
    /// Tasks added (or restored) from this page → when (for a fade-in).
    appeared: HashMap<u64, f64>,
    /// Last checkbox toggled from this page (pop + strike-through sweep).
    toggled: Option<(u64, f64)>,
    /// Display order (task ids) frozen at the last toggle, and when.
    frozen: Option<(Vec<u64>, f64)>,
    /// Tasks removed by delete / "Clear done" and when, for "Undo".
    undo: Option<(Vec<Task>, f64)>,
}

thread_local! {
    static ANIM: Cell<ListAnim> = Cell::new(ListAnim::default());
}

fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Move `from` toward `to` with frame-rate independent smoothing; snaps when close.
fn glide(from: f32, to: f32, dt: f32) -> (f32, bool) {
    let v = from + (to - from) * (1.0 - (-GLIDE_RATE * dt).exp());
    if (to - v).abs() < 0.3 {
        (to, false)
    } else {
        (v, true)
    }
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Reorder `items` to follow a frozen display order; ids it doesn't know
/// (just added) keep their relative order after the known ones.
fn apply_frozen<T>(items: &mut [(u64, T)], order: &[u64]) {
    let slot: HashMap<u64, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    items.sort_by_key(|(id, _)| slot.get(id).copied().unwrap_or(usize::MAX));
}

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let now = st.now;
    let mut anim = ANIM.with(Cell::take);
    let stale = now - anim.last_draw > STALE_S;
    if stale {
        anim.rows.clear();
        anim.appeared.clear();
        anim.toggled = None;
        anim.frozen = None;
    }
    // "Undo" lapses after UNDO_SECS, but never under a press that is about to click it.
    let undo_id = wid("tasks.undo");
    let undo_held = ui.input.pressed_id == Some(undo_id) || ui.input.clicked.is_some_and(|(id, _)| id == undo_id);
    if anim.undo.as_ref().is_some_and(|(_, t0)| now - t0 >= UNDO_SECS) && !undo_held {
        anim.undo = None;
    }
    let dt = if stale { 0.0 } else { (now - anim.last_draw).clamp(0.0, 0.05) as f32 };
    let mut moving = false;
    let day_start = st.day_start_unix();
    if st.store.tasks.tasks.len() >= MAX_TASKS {
        make_room(st, day_start);
    }
    let full = st.store.tasks.tasks.len() >= MAX_TASKS;

    // ---- Input: the list's first row, sharing its checkbox and text columns.
    let input_r = Rect::new(area.x, area.y, area.w, INPUT_H);
    let placeholder = if full { "Task list is full" } else { "Add a task, Enter to save" };
    let accent = st.accent;
    let resp = ui.text_input_row(wid("tasks.input"), input_r, &mut st.ui.task_input, placeholder, MAX_TASK_LEN, accent, Some(Icon::Plus));
    if resp.submitted {
        submit(ui, st, &mut anim, day_start);
    }

    // ---- List.
    let view = Rect::new(area.x, area.y + LIST_TOP, area.w, LIST_H);
    let footer_r = Rect::new(area.x, area.y + HEIGHT - FOOTER_H, area.w, FOOTER_H);
    let mut items: Vec<(u64, (bool, String))> = st.store.tasks.display_order(day_start).into_iter().map(|t| (t.id, (t.done, t.text.clone()))).collect();
    // Keep the order of the last toggle while the pointer is still over the list.
    let over_list = ui.hovered(view);
    if !anim.frozen.as_ref().is_some_and(|(_, t0)| over_list && now - t0 < FREEZE_S) {
        anim.frozen = None;
    }
    if let Some((order, _)) = &anim.frozen {
        apply_frozen(&mut items, order);
    }

    if items.is_empty() {
        let hint_area = Rect::new(area.x, view.y, area.w, HEIGHT - LIST_TOP);
        super::empty_state(ui, hint_area, Icon::ListChecks, "Nothing on your list", "Done tasks clear at the end of the day");
        st.ui.task_scroll = 0.0;
        anim.scroll = 0.0;
        anim.rows.clear();
        if anim.undo.is_some() {
            undo_pill(ui, st, &mut anim, undo_id, footer_r);
        }
    } else {
        let max_scroll = (items.len() as f32 * ROW_H - LIST_H).max(0.0);
        if max_scroll > 0.0 {
            let notches = ui.wheel_in(view);
            st.ui.task_scroll -= notches * WHEEL_STEP;
        }
        st.ui.task_scroll = st.ui.task_scroll.clamp(0.0, max_scroll);
        let (scroll, m) = if stale { (st.ui.task_scroll, false) } else { glide(anim.scroll, st.ui.task_scroll, dt) };
        anim.scroll = scroll.clamp(0.0, max_scroll);
        moving |= m;
        let scrolling = m;

        let mut toggle = None;
        let mut delete = None;
        let mut rows = HashMap::with_capacity(items.len());
        ui.c.clip(view);
        for (i, (id, (done, text))) in items.iter().enumerate() {
            let target = i as f32 * ROW_H;
            let (y, m) = match anim.rows.get(id) {
                Some(&y0) => glide(y0, target, dt),
                None => (target, false),
            };
            moving |= m;
            rows.insert(*id, y);
            let appear = anim.appeared.get(id).map(|&t0| ((now - t0) / APPEAR_S).clamp(0.0, 1.0) as f32).unwrap_or(1.0);
            if appear < 1.0 {
                moving = true;
            }
            let row = Rect::new(area.x, view.y + y - anim.scroll + 6.0 * (1.0 - ease_out_cubic(appear)), area.w, ROW_H);
            let Some(hit) = intersect(row, view) else { continue };
            let pop = match anim.toggled {
                Some((tid, t0)) if tid == *id => {
                    let t = ((now - t0) / POP_S).clamp(0.0, 1.0) as f32;
                    if t < 1.0 {
                        moving = true;
                    }
                    Some(t)
                }
                _ => None,
            };
            let prev_alpha = ui.c.alpha();
            ui.c.set_alpha(prev_alpha * appear);
            // A row on the move glides between pixels; at rest it sits on whole pixels.
            let snap_was = ui.c.set_text_snap(!(scrolling || m || appear < 1.0));
            let action = draw_row(ui, row, hit, view, *id, *done, text, pop);
            ui.c.set_text_snap(snap_was);
            ui.c.set_alpha(prev_alpha);
            match action {
                RowAction::Toggle => toggle = Some(*id),
                RowAction::Delete => delete = Some(*id),
                RowAction::None => {}
            }
        }
        // Fade content out at edges that can scroll further.
        if anim.scroll > 0.5 {
            fade(ui, Rect::new(view.x, view.y, view.w, FADE), true);
        }
        if anim.scroll < max_scroll - 0.5 {
            fade(ui, Rect::new(view.x, view.bottom() - FADE, view.w, FADE), false);
        }
        ui.c.unclip();
        anim.rows = rows;

        if let Some(id) = toggle {
            if st.store.tasks.toggle(id, st.unix) {
                st.save();
                anim.toggled = Some((id, now));
                // Freeze what is on screen now; the rows re-sort once the pointer leaves.
                anim.frozen = Some((items.iter().map(|(id, _)| *id).collect(), now));
            }
        }
        if let Some(id) = delete {
            if let Some(task) = st.store.tasks.tasks.iter().find(|t| t.id == id).cloned() {
                st.store.tasks.remove(id);
                st.save();
                offer_undo(&mut anim, vec![task], now);
                anim.rows.remove(&id);
                anim.appeared.remove(&id);
            }
        }

        draw_footer(ui, st, footer_r, day_start, full, &mut anim, undo_id);
    }

    anim.appeared.retain(|_, t0| now - *t0 < APPEAR_S);
    if anim.toggled.is_some_and(|(_, t0)| now - t0 >= POP_S) {
        anim.toggled = None;
    }
    if moving {
        ui.request_anim();
    }
    anim.last_draw = now;
    ANIM.with(|c| c.set(anim));
}

/// Add the typed task. Keeps keyboard focus (rapid entry) and scrolls the new
/// row into view. A full list keeps the text so nothing typed is lost.
fn submit(ui: &mut Ui, st: &mut AppState, anim: &mut ListAnim, day_start: i64) {
    if st.ui.task_input.trim().is_empty() {
        st.ui.task_input.clear();
        ui.input.caret = 0;
        return;
    }
    let Some(id) = st.store.tasks.add(&st.ui.task_input, st.unix) else { return };
    st.ui.task_input.clear();
    ui.input.caret = 0;
    st.save();
    anim.appeared.insert(id, st.now);
    // The new row goes to its sorted place, so stop holding the old order.
    anim.frozen = None;
    if let Some(idx) = st.store.tasks.display_order(day_start).iter().position(|t| t.id == id) {
        let y = idx as f32 * ROW_H;
        if y + ROW_H > st.ui.task_scroll + LIST_H {
            st.ui.task_scroll = y + ROW_H - LIST_H;
        } else if y < st.ui.task_scroll {
            st.ui.task_scroll = y;
        }
    }
}

/// Remember removed tasks for "Undo". Removals made while "Undo" is still
/// showing join the same snapshot, so one click brings all of them back.
fn offer_undo(anim: &mut ListAnim, removed: Vec<Task>, now: f64) {
    if removed.is_empty() {
        return;
    }
    match anim.undo.as_mut() {
        Some((tasks, t0)) => {
            tasks.extend(removed);
            *t0 = now;
        }
        None => anim.undo = Some((removed, now)),
    }
}

/// Put the snapshot back (open tasks first should the list have filled up since).
fn restore(st: &mut AppState, anim: &mut ListAnim) {
    let Some((mut tasks, _)) = anim.undo.take() else { return };
    tasks.sort_by_key(|t| (t.done, std::cmp::Reverse(t.done_unix)));
    let mut restored = false;
    for t in tasks {
        if st.store.tasks.tasks.len() >= MAX_TASKS {
            break;
        }
        if st.store.tasks.tasks.iter().any(|x| x.id == t.id) {
            continue;
        }
        anim.appeared.insert(t.id, st.now);
        st.store.tasks.tasks.push(t);
        restored = true;
    }
    if restored {
        st.save();
    }
    anim.frozen = None;
}

/// Done tasks from earlier days are never shown (`display_order` lists only
/// today's), yet they count toward `MAX_TASKS`. Once the list is full, drop
/// those hidden ones so the user isn't blocked by tasks they cannot see.
fn make_room(st: &mut AppState, day_start: i64) {
    let before = st.store.tasks.tasks.len();
    st.store.tasks.tasks.retain(|t| !t.done || t.done_unix.is_some_and(|d| d >= day_start));
    let removed = before - st.store.tasks.tasks.len();
    if removed > 0 {
        crate::pinfo!("tasks: list was full, removed {removed} done tasks from earlier days");
        st.save();
    }
}

enum RowAction {
    None,
    Toggle,
    Delete,
}

/// One task row. `hit` is the part of `row` inside the list viewport `view`.
#[allow(clippy::too_many_arguments)]
fn draw_row(ui: &mut Ui, row: Rect, hit: Rect, view: Rect, id: u64, done: bool, text: &str, pop: Option<f32>) -> RowAction {
    let mut action = RowAction::None;
    let resp = ui.button(wid_n("tasks.row", id), hit);
    if resp.pressed {
        ui.c.fill_rrect(row, 9.0, palette::SURFACE_HOVER);
    } else if resp.hovered {
        ui.c.fill_rrect(row, 9.0, palette::SURFACE);
    }
    if resp.clicked {
        action = RowAction::Toggle;
    }
    let cy = row.center().y;

    // Checkbox (pops when toggled).
    let scale = pop.map(|t| 1.0 + 0.22 * (t * std::f32::consts::PI).sin()).unwrap_or(1.0);
    let c = Point::new(row.x + MARK_X, cy);
    let size = 18.0 * scale;
    let icon_r = Rect::from_center(c, size, size);
    if done {
        ui.c.fill_circle(c, size * 9.0 / 24.0, palette::GREEN.with_alpha(0.16));
        ui.c.icon(Icon::CheckCircle, icon_r, palette::GREEN);
    } else {
        ui.c.icon(Icon::Circle, icon_r, if resp.hovered { palette::TEXT_DIM } else { palette::TEXT_FAINT });
    }

    // Text, struck through when done (the line sweeps in right after checking).
    let tx = row.x + TEXT_X;
    let tw = (row.w - TEXT_X - 40.0).max(0.0);
    let style = TextStyle::body().color(if done { palette::TEXT_FAINT } else { palette::TEXT });
    ui.c.text(text, Rect::new(tx, row.y, tw, row.h), &style);
    if done && tw > 0.0 {
        let w = ui.c.measure(text, &style, tw).0.min(tw);
        let sweep = pop.map(ease_out_cubic).unwrap_or(1.0);
        if sweep > 0.0 {
            let y = cy + 1.0;
            ui.c.line(Point::new(tx - 1.0, y), Point::new(tx - 1.0 + (w + 2.0) * sweep, y), palette::TEXT_FAINT, 1.2);
        }
    }

    // Delete, only while hovering a fully visible row.
    let del = Rect::new(row.right() - 32.0, cy - 13.0, 26.0, 26.0);
    if resp.hovered && del.y >= view.y && del.bottom() <= view.bottom() {
        let color = if ui.hovered(del) { palette::RED } else { palette::TEXT_DIM };
        if ui.icon_button(wid_n("tasks.del", id), del, Icon::Trash, color, 14.0).clicked {
            action = RowAction::Delete;
        }
    }
    action
}

/// Darken `r` toward the island colour; `top` = strongest at the top edge.
/// Glass has no one island colour to fade into, so the list just ends there.
fn fade(ui: &mut Ui, r: Rect, top: bool) {
    if ui.c.look().material == poni_core::look::Material::Glass {
        return;
    }
    let step = r.h / FADE_STEPS as f32;
    for k in 0..FADE_STEPS {
        // k = 0 is the strip at the edge.
        let a = 0.92 * (1.0 - (k as f32 + 0.5) / FADE_STEPS as f32).powf(1.4);
        let y = if top { r.y + k as f32 * step } else { r.bottom() - (k as f32 + 1.0) * step };
        ui.c.fill_rect(Rect::new(r.x, y, r.w, step), palette::ISLAND.with_alpha(a));
    }
}

/// A Small Utility capsule with an accent "Undo" label, right-aligned in
/// `row`. Returns its left edge.
fn undo_pill(ui: &mut Ui, st: &mut AppState, anim: &mut ListAnim, id: WidgetId, row: Rect) -> f32 {
    let accent = st.accent;
    let w = ui.pill_width("Undo", true, true);
    let r = Rect::new(row.right() - w, row.y, w, row.h);
    let clicked = ui.pill(id, r, "Undo", Some(Icon::Undo), PillStyle::UtilityAccent(accent)).clicked;
    if clicked {
        restore(st, anim);
    }
    r.x
}

/// "1/3 done today" with a small ring, then "Clear done" when anything is
/// done, or "Undo" for a few seconds after a removal.
fn draw_footer(ui: &mut Ui, st: &mut AppState, r: Rect, day_start: i64, full: bool, anim: &mut ListAnim, undo_id: WidgetId) {
    let (done, total) = st.store.tasks.today_progress(day_start);
    let cy = r.center().y;
    let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
    ui.c.ring(Point::new(r.x + MARK_X, cy), 6.0, 2.2, frac, RING_TRACK, palette::GREEN);

    let show_clear = done > 0;
    let mut text_right = r.right();
    if anim.undo.is_some() {
        text_right = undo_pill(ui, st, anim, undo_id, r) - 12.0;
    } else if show_clear {
        let w = ui.pill_width("Clear done", false, true);
        let pr = Rect::new(r.right() - w, r.y, w, r.h);
        text_right = pr.x - 12.0;
        if ui.pill(wid("tasks.clear"), pr, "Clear done", None, PillStyle::Utility).clicked {
            let removed: Vec<Task> = st.store.tasks.tasks.iter().filter(|t| t.done).cloned().collect();
            if st.store.tasks.clear_done() > 0 {
                // The scroll offset is clamped to the shorter list on the next frame.
                st.save();
                offer_undo(anim, removed, st.now);
                anim.toggled = None;
                anim.frozen = None;
            }
        }
    }

    let tx = r.x + TEXT_X;
    let text_r = Rect::new(tx, r.y, (text_right - tx).max(0.0), r.h);
    let (text, color) = if full && show_clear {
        ("List is full — clear done tasks to add more".to_string(), palette::ORANGE)
    } else if full {
        (format!("List is full ({MAX_TASKS} tasks)"), palette::ORANGE)
    } else if total > 0 && done == total {
        ("All done for today".to_string(), palette::GREEN)
    } else {
        (format!("{done}/{total} done today"), palette::TEXT_DIM)
    };
    ui.c.text(&text, text_r, &TextStyle::footnote().color(color));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_intersection() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert_eq!(intersect(a, Rect::new(5.0, 5.0, 10.0, 10.0)), Some(Rect::new(5.0, 5.0, 5.0, 5.0)));
        assert_eq!(intersect(a, Rect::new(10.0, 0.0, 5.0, 5.0)), None);
    }

    #[test]
    fn glide_converges_and_snaps() {
        let (mut v, mut moving) = (0.0f32, true);
        let mut frames = 0;
        while moving && frames < 200 {
            (v, moving) = glide(v, 100.0, 1.0 / 60.0);
            frames += 1;
        }
        assert_eq!(v, 100.0);
        assert!(frames < 40, "took {frames} frames");
        assert_eq!(glide(5.0, 5.0, 0.0), (5.0, false));
    }

    #[test]
    fn frozen_order_holds_and_appends_newcomers() {
        // After toggling 2 (which would sink below 3), the frozen order keeps it in place.
        let mut items = vec![(1u64, ()), (3, ()), (4, ()), (2, ())];
        apply_frozen(&mut items, &[1, 2, 3]);
        let ids: Vec<u64> = items.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn undo_snapshots_accumulate() {
        let task = |id: u64| Task { id, text: format!("t{id}"), done: false, created_unix: 0, done_unix: None };
        let mut anim = ListAnim::default();
        offer_undo(&mut anim, vec![task(1)], 1.0);
        offer_undo(&mut anim, vec![task(2)], 3.0);
        offer_undo(&mut anim, Vec::new(), 4.0);
        let (tasks, t0) = anim.undo.expect("snapshot");
        assert_eq!(tasks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(t0, 3.0);
    }
}

/// When the page next changes on its own (Undo lapsing, frozen order ending).
pub fn next_wake(_st: &AppState) -> Option<f64> {
    ANIM.with(|c| {
        let a = c.take();
        let undo = a.undo.as_ref().map(|(_, t0)| t0 + UNDO_SECS);
        let frozen = a.frozen.as_ref().map(|(_, t0)| t0 + FREEZE_S);
        c.set(a);
        match (undo, frozen) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        }
    })
}
