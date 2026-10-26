//! Shelf page: files dropped onto the island, kept as references.
//!
//! Click a tile to open it, right-click to show it in Explorer, press and
//! drag to take it back out. Thumbnails arrive through `st.shelf_thumbs`
//! (requested with `Command::LoadThumb`); the thumbnail worker also reports
//! whether each file still exists, so moved or deleted items are greyed out
//! without touching the disk on the UI thread.
//!
//! Phone drop lives here too: a Phone tile (empty shelf) or pill (filled
//! shelf) opens a panel under the shelf with a QR code. A phone that scans it
//! sends files to the Shelf after a yes on the island, and can save Shelf
//! files (`services::phonedrop`, `poni_core::phonedrop`).

use crate::commands::{Command, PairCmd, PhoneCmd};
use crate::dragdrop;
use crate::gfx::{Image, ImageData, TextStyle, Weight};
use crate::icons::Icon;
use crate::services::imaging::{self, PathKind};
use crate::state::{AppState, PairedPhone, PhoneDrop, PhoneView};
use crate::ui::{wid, wid_n, PillStyle, Ui, WidgetId, PILL_GAP, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::phonedrop::{fmt_bytes, QrMatrix};
use poni_core::shelf::{display_name, extension, ShelfItem, MAX_SHELF_ITEMS};
use std::cell::RefCell;
use std::collections::HashMap;

const PAGE_H: f32 = 150.0;
/// Breathing room below the page tabs (the header already leaves the gap).
const TOP: f32 = 0.0;
/// Summary row: count, hover hint and the Small pills (same height).
const HEAD_H: f32 = PILL_SMALL;
const HEAD_GAP: f32 = 6.0;
const TILE_W: f32 = 84.0;
const TILE_GAP: f32 = 6.0;
const TILE_R: f32 = 14.0;
const THUMB: f32 = imaging::THUMB_DIP;
const THUMB_R: f32 = 12.0;
const FADE_W: f32 = 30.0;
/// Press-and-move distance (DIPs) that turns a press into a drag-out.
const DRAG_START: f32 = 6.0;
const UNDO_SECS: f64 = 8.0;
/// Minimum time between existence re-checks of one file: when the page
/// (re)appears, and when a tile gets hovered.
const RECHECK_SHOWN_SECS: f64 = 15.0;
const RECHECK_HOVER_SECS: f64 = 4.0;
const APPEAR_SECS: f64 = 0.28;
const GLOW_SECS: f32 = 0.14;
/// Drop-zone outline: one solid hairline, the same idle and while dragging.
const ZONE_STROKE: f32 = 1.5;
const DROP_LABEL: &str = "Drop to add to Shelf";
/// The phone panel under the shelf while Phone drop is open.
const PHONE_GAP: f32 = 12.0;
const PHONE_H: f32 = 152.0;
const PHONE_R: f32 = 18.0;
/// White square behind the QR code, and the light margin inside it.
const QR_BOX: f32 = 128.0;
const QR_MARGIN: f32 = 6.0;
/// The Phone tile beside the drop zone on an empty shelf.
const PHONE_TILE_W: f32 = 196.0;
/// Pairing (`services::pairdrop`) is built only in the Windows app so far;
/// elsewhere the phone panel has Done alone.
const PAIRING: bool = cfg!(windows);
/// What the phone panel calls this computer.
const THIS_COMPUTER: &str = if cfg!(windows) { "this PC" } else { "this Mac" };
/// What to allow if the system asks about Phone drop's incoming connections.
const ALLOW_HINT: &str = if cfg!(windows) { "If Windows asks, allow Oyenza Studio on private networks." } else { "If macOS asks, allow incoming connections for Oyenza Studio." };
/// How long a phone result ("3 files from iPhone…") stays in the panel.
pub const PHONE_OUTCOME_S: f64 = 8.0;

pub fn height(st: &AppState) -> f32 {
    if st.phone.is_some() {
        PAGE_H + PHONE_GAP + PHONE_H
    } else {
        PAGE_H
    }
}

/// Transient page state (UI thread only).
#[derive(Default)]
struct Local {
    last_draw: f64,
    anim_t: f64,
    /// Displayed scroll offset, easing toward `st.ui.shelf_scroll`.
    scroll_vis: f32,
    /// 0..1 drop-highlight while files hover over the island.
    glow: f32,
    /// key → time the item first appeared (new tiles fade in).
    seen: HashMap<String, f64>,
    seeded: bool,
    /// key → last thumbnail we drew (bridges re-checks without flicker).
    last_img: HashMap<String, Image>,
    /// key → time of the last existence re-check.
    checked: HashMap<String, f64>,
    hovered: Option<WidgetId>,
    drag_started: bool,
    /// Path being dragged out; re-checked once the drag ends (it may have moved).
    after_drag: Option<String>,
    /// Shelf contents before the last removal, for "Undo".
    undo: Option<(Vec<ShelfItem>, f64)>,
    count: usize,
    /// The phone link's QR code as a bitmap: (link, square and module size in device pixels, image).
    qr_img: Option<(String, u32, u32, Image)>,
}

thread_local! {
    static LOCAL: RefCell<Local> = RefCell::new(Local::default());
}

enum Action {
    PhoneOpen,
    PhoneClose,
    PhoneAnswer(u64, bool),
    Pair(PairCmd),
    Open(String),
    Reveal(String),
    DragOut(String),
    Recheck(String),
    Remove(String),
    Clear,
    Undo,
}

fn key(path: &str) -> String {
    path.to_lowercase()
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    LOCAL.with(|l| {
        if let Ok(mut l) = l.try_borrow_mut() {
            draw_page(ui, st, area, &mut l);
        }
    });
}

fn draw_page(ui: &mut Ui, st: &mut AppState, area: Rect, l: &mut Local) {
    let now = st.now;
    let dt = (now - l.anim_t).clamp(0.0, 0.1) as f32;
    l.anim_t = now;
    let shown = now - l.last_draw > 0.75;
    l.last_draw = now;
    if !ui.input.down {
        l.drag_started = false;
    }

    let items = st.store.shelf.items.clone();
    sync(st, l, &items, now, shown);

    // Drop highlight fades in/out while files hover over the island.
    let target = if st.drag_hover { 1.0 } else { 0.0 };
    let step = dt / GLOW_SECS;
    if shown || (target - l.glow).abs() <= step {
        l.glow = target;
    } else {
        l.glow += step.copysign(target - l.glow);
        ui.request_anim();
    }

    let mut actions = Vec::new();
    // Pill: the boxes that reach the card's bottom corners follow them.
    let card = super::card::pill_card(st, area);
    // With Phone drop open the shelf keeps its height and the panel sits below.
    let (main, phone_r) = match &st.phone {
        Some(_) => (Rect::new(area.x, area.y, area.w, PAGE_H), Some(Rect::new(area.x, area.y + PAGE_H + PHONE_GAP, area.w, PHONE_H))),
        None => (area, None),
    };
    if items.is_empty() {
        draw_empty(ui, st, l, main, card, &mut actions);
    } else {
        draw_items(ui, st, l, main, &items, dt, shown, card, &mut actions);
    }
    if let (Some(r), Some(pd)) = (phone_r, st.phone.clone()) {
        draw_phone(ui, st, l, &pd, r, card, &mut actions);
    }
    apply(st, l, &items, actions);
}

/// Keep thumbnails and local caches in step with the shelf contents.
fn sync(st: &mut AppState, l: &mut Local, items: &[ShelfItem], now: f64, shown: bool) {
    // Every item needs a thumbnail request (this also heals path-casing
    // differences between what was dropped and what the shelf kept).
    for it in items {
        if !st.shelf_thumbs.contains_key(&it.path) {
            st.cmd(Command::LoadThumb(it.path.clone()));
        }
    }
    // Forget state of items that left the shelf (keeping an undo snapshot warm).
    let undo_live = l.undo.as_ref().is_some_and(|(_, t)| now - t < UNDO_SECS);
    if !undo_live {
        l.undo = None;
    }
    let undo_items: &[ShelfItem] = l.undo.as_ref().map(|(v, _)| v.as_slice()).unwrap_or(&[]);
    if st.shelf_thumbs.len() > items.len() + undo_items.len() {
        st.shelf_thumbs.retain(|p, _| items.iter().chain(undo_items).any(|i| &i.path == p));
    }
    let keep: Vec<String> = items.iter().chain(undo_items).map(|i| key(&i.path)).collect();
    if l.seen.len() > keep.len() {
        l.seen.retain(|k, _| keep.contains(k));
    }
    if l.last_img.len() > keep.len() {
        l.last_img.retain(|k, _| keep.contains(k));
    }
    if l.checked.len() > keep.len() {
        l.checked.retain(|k, _| keep.contains(k));
    }

    // New items fade in at the front of the row; the first draw seeds silently.
    if !l.seeded {
        for it in items {
            l.seen.insert(key(&it.path), f64::MIN);
        }
        l.seeded = true;
    } else {
        for it in items {
            l.seen.entry(key(&it.path)).or_insert(now);
        }
    }
    if items.len() > l.count {
        st.ui.shelf_scroll = 0.0;
        l.undo = None;
    }
    l.count = items.len();

    // Files may have been moved or deleted while the page was out of view.
    if shown {
        for it in items {
            recheck(st, l, &it.path, now, RECHECK_SHOWN_SECS);
        }
    }
    if let Some(p) = l.after_drag.clone() {
        if !dragdrop::is_dragging_out() {
            l.after_drag = None;
            recheck(st, l, &p, now, 0.0);
        }
    }
}

/// Ask the worker to look at `path` again (existence + thumbnail) unless it
/// did so within `min_age` seconds. Unchanged files come back from its
/// cache, so this is cheap.
fn recheck(st: &mut AppState, l: &mut Local, path: &str, now: f64, min_age: f64) {
    // Not seen by the worker yet: its first answer is still on the way.
    if imaging::path_kind(path).is_none() {
        return;
    }
    let k = key(path);
    if l.checked.get(&k).is_some_and(|t| now - t < min_age) {
        return;
    }
    if let Some(Some(img)) = st.shelf_thumbs.remove(path) {
        l.last_img.insert(k.clone(), img);
    }
    l.checked.insert(k, now);
    st.cmd(Command::LoadThumb(path.to_string()));
}

fn apply(st: &mut AppState, l: &mut Local, items: &[ShelfItem], actions: Vec<Action>) {
    let now = st.now;
    for a in actions {
        match a {
            Action::PhoneOpen => st.cmd(Command::Phone(PhoneCmd::Open)),
            Action::PhoneClose => st.cmd(Command::Phone(PhoneCmd::Close)),
            Action::PhoneAnswer(id, accept) => st.cmd(Command::Phone(PhoneCmd::Answer { id, accept })),
            Action::Pair(c) => st.cmd(Command::Pair(c)),
            Action::Open(p) => st.cmd(Command::OpenPath(p)),
            Action::Reveal(p) => st.cmd(Command::RevealPath(p)),
            Action::DragOut(p) => {
                l.after_drag = Some(p.clone());
                st.cmd(Command::DragOut(vec![p]));
            }
            Action::Recheck(p) => recheck(st, l, &p, now, 0.0),
            Action::Remove(p) => {
                l.undo = Some((items.to_vec(), now));
                if st.store.shelf.remove(&p) {
                    st.save();
                }
                l.count = st.store.shelf.items.len();
            }
            Action::Clear => {
                l.undo = Some((items.to_vec(), now));
                st.store.shelf.clear();
                st.ui.shelf_scroll = 0.0;
                l.count = 0;
                st.save();
            }
            Action::Undo => {
                if let Some((snapshot, _)) = l.undo.take() {
                    st.store.shelf.items = snapshot;
                    l.count = st.store.shelf.items.len();
                    st.save();
                }
            }
        }
    }
}

// ---------------------------------------------------------------- items

#[allow(clippy::too_many_arguments)]
fn draw_items(
    ui: &mut Ui,
    st: &mut AppState,
    l: &mut Local,
    area: Rect,
    items: &[ShelfItem],
    dt: f32,
    shown: bool,
    card: Option<(Rect, f32)>,
    actions: &mut Vec<Action>,
) {
    let now = st.now;
    let head = Rect::new(area.x, area.y + TOP, area.w, HEAD_H);
    let row_y = head.bottom() + HEAD_GAP;
    let row = Rect::new(area.x, row_y, area.w, (area.bottom() - row_y).max(0.0));
    let n = items.len();

    // Horizontal scrolling (the wheel only gets consumed when there is
    // something to scroll, so it keeps switching pages otherwise).
    let content_w = n as f32 * TILE_W + n.saturating_sub(1) as f32 * TILE_GAP;
    let max_scroll = (content_w - row.w).max(0.0);
    if max_scroll > 0.0 {
        let w = ui.wheel_in(row);
        if w != 0.0 {
            st.ui.shelf_scroll -= w * (TILE_W + TILE_GAP);
        }
    }
    st.ui.shelf_scroll = st.ui.shelf_scroll.clamp(0.0, max_scroll);
    let target = st.ui.shelf_scroll;
    if shown || (l.scroll_vis - target).abs() < 0.3 {
        l.scroll_vis = target;
    } else {
        l.scroll_vis += (target - l.scroll_vis) * (1.0 - (-dt * 16.0).exp());
        ui.request_anim();
    }
    let scroll = l.scroll_vis;
    let glow = l.glow;

    let mut hovered: Option<(WidgetId, usize)> = None;
    let tile_h = (row.h - 4.0).max(0.0);
    ui.c.clip(Rect::new(row.x, row.y - 2.0, row.w, row.h + 4.0));
    for (i, it) in items.iter().enumerate() {
        let x = row.x + i as f32 * (TILE_W + TILE_GAP) - scroll;
        if x >= row.right() || x + TILE_W <= row.x {
            continue;
        }
        let k = key(&it.path);
        let since = now - l.seen.get(&k).copied().unwrap_or(f64::MIN);
        let appear = ease_out((since / APPEAR_SECS).clamp(0.0, 1.0) as f32);
        if appear < 1.0 {
            ui.request_anim();
        }
        let tile = Rect::new(x, row.y + (1.0 - appear) * 8.0, TILE_W, tile_h);
        let kind = imaging::path_kind(&it.path);
        let missing = kind == Some(PathKind::Missing);
        let id = wid_n("shelf.tile", wid(&k));
        let resp = ui.button(id, intersect(tile, row));
        let hot = resp.hovered && glow < 0.5;
        if hot {
            hovered = Some((id, i));
        }

        let img = match st.shelf_thumbs.get(&it.path) {
            Some(Some(img)) => {
                if l.last_img.get(&k).map(|c| c.id) != Some(img.id) {
                    l.last_img.insert(k.clone(), img.clone());
                }
                Some(img.clone())
            }
            _ => l.last_img.get(&k).cloned(),
        };

        let base = ui.c.alpha();
        // Tiles step back while files hover, so the drop zone reads clearly.
        ui.c.set_alpha(base * appear * (1.0 - glow));
        if resp.pressed {
            ui.c.fill_rrect(tile, TILE_R, palette::SURFACE_HOVER);
        } else if hot {
            ui.c.fill_rrect(tile, TILE_R, palette::SURFACE);
        }
        let thumb = Rect::new(tile.x + (TILE_W - THUMB) / 2.0, tile.y + 10.0, THUMB, THUMB);
        let tile_a = ui.c.alpha();
        // Missing files: everything but the caption is greyed out.
        ui.c.set_alpha(tile_a * if missing { 0.38 } else { 1.0 });
        ui.c.fill_rrect(thumb, THUMB_R, palette::SURFACE);
        match &img {
            Some(im) => ui.c.image(im, thumb, THUMB_R),
            None => {
                let folder = kind == Some(PathKind::Folder) || (kind.is_none() && extension(&it.path).is_empty());
                let icon = if folder { Icon::Folder } else { Icon::File };
                ui.c.icon(icon, Rect::from_center(thumb.center(), 24.0, 24.0), palette::TEXT_DIM);
            }
        }
        let name_r = Rect::new(tile.x + 6.0, thumb.bottom() + 8.0, TILE_W - 12.0, 16.0);
        ui.c.text(&display_name(&it.path), name_r, &TextStyle::footnote().weight(Weight::Medium).center());
        ui.c.set_alpha(tile_a);
        let cap_r = Rect::new(name_r.x, name_r.bottom(), name_r.w, 14.0);
        if missing {
            ui.c.text("Missing", cap_r, &TextStyle::caption().weight(Weight::Medium).color(palette::TEXT_DIM).center());
        } else {
            ui.c.text(&caption(&it.path, kind), cap_r, &TextStyle::caption().color(palette::TEXT_FAINT).center());
        }

        // Remove button on hover, tucked onto the thumbnail's corner.
        if hot {
            let c = Point::new(thumb.right() - 2.0, thumb.y + 2.0);
            let cr = ui.button(wid_n("shelf.remove", wid(&k)), intersect(Rect::from_center(c, 22.0, 22.0), row));
            let fill = if cr.pressed {
                Color::white(0.42)
            } else if cr.hovered {
                Color::white(0.34)
            } else {
                Color::white(0.24)
            };
            ui.c.fill_circle(c, 11.0, palette::ISLAND);
            ui.c.fill_circle(c, 9.0, fill);
            ui.c.icon(Icon::Close, Rect::from_center(c, 10.0, 10.0), palette::TEXT);
            if cr.clicked {
                actions.push(Action::Remove(it.path.clone()));
            }
        }
        ui.c.set_alpha(base);

        if resp.clicked {
            actions.push(if missing { Action::Recheck(it.path.clone()) } else { Action::Open(it.path.clone()) });
        }
        if resp.rclicked {
            actions.push(if missing { Action::Recheck(it.path.clone()) } else { Action::Reveal(it.path.clone()) });
        }
        if resp.pressed && !missing && !l.drag_started {
            if let Some((dx, dy)) = resp.drag {
                if dx * dx + dy * dy > DRAG_START * DRAG_START {
                    l.drag_started = true;
                    actions.push(Action::DragOut(it.path.clone()));
                }
            }
        }
    }

    // Soft edges where tiles scroll out of view.
    let fade_h = Rect::new(row.x, row.y - 2.0, FADE_W, row.h + 4.0);
    if scroll > 0.5 {
        fade_edge(ui, fade_h, true);
    }
    if scroll < max_scroll - 0.5 {
        fade_edge(ui, Rect::new(row.right() - FADE_W, fade_h.y, FADE_W, fade_h.h), false);
    }
    ui.c.unclip();

    // Hovering a tile re-checks that file now and then (it may have moved).
    let hovered_id = hovered.map(|(id, _)| id);
    if hovered_id != l.hovered {
        l.hovered = hovered_id;
        if let Some((_, i)) = hovered {
            if let Some(it) = items.get(i) {
                recheck(st, l, &it.path, now, RECHECK_HOVER_SECS);
            }
        }
    }

    if glow > 0.01 {
        draw_drop_overlay(ui, st, Rect::new(row.x, row.y, row.w, tile_h), glow, card);
    }
    draw_header(ui, st, l, head, items, hovered.map(|(_, i)| i), actions);
}

fn draw_header(ui: &mut Ui, st: &AppState, l: &Local, head: Rect, items: &[ShelfItem], hovered: Option<usize>, actions: &mut Vec<Action>) {
    let n = items.len();
    let count = format!("{n} {}", if n == 1 { "item" } else { "items" });
    let cs = TextStyle::body().weight(Weight::Semibold).color(palette::TEXT_DIM);
    let cw = ui.c.measure(&count, &cs, 200.0).0;
    ui.c.text(&count, Rect::new(head.x, head.y, cw + 4.0, head.h), &cs);

    // Small utility pills on the right: Clear, plus Undo for a while after a removal.
    let clear_w = ui.pill_width("Clear", true, true);
    let clear_r = Rect::new(head.right() - clear_w, head.y, clear_w, PILL_SMALL);
    if ui.pill(wid("shelf.clear"), clear_r, "Clear", Some(Icon::Trash), PillStyle::Utility).clicked {
        actions.push(Action::Clear);
    }
    let mut right = clear_r.x;
    if l.undo.as_ref().is_some_and(|(_, t)| st.now - t < UNDO_SECS) {
        let undo_w = ui.pill_width("Undo", true, true);
        let undo_r = Rect::new(clear_r.x - PILL_GAP - undo_w, head.y, undo_w, PILL_SMALL);
        if ui.pill(wid("shelf.undo"), undo_r, "Undo", Some(Icon::Undo), PillStyle::UtilityAccent(st.accent)).clicked {
            actions.push(Action::Undo);
        }
        right = undo_r.x;
    }
    // Phone drop: send from or save to a phone (the panel opens below).
    if st.phone.is_none() {
        let phone_w = ui.pill_width("Phone", true, true);
        let phone_r = Rect::new(right - PILL_GAP - phone_w, head.y, phone_w, PILL_SMALL);
        if ui.pill(wid("shelf.phone"), phone_r, "Phone", Some(Icon::Phone), PillStyle::Utility).clicked {
            actions.push(Action::PhoneOpen);
        }
        right = phone_r.x;
    }

    // No instructions by default: the row only describes the hovered tile
    // (where it lives) or a full shelf.
    let hint = match hovered.and_then(|i| items.get(i)) {
        Some(it) if imaging::path_kind(&it.path) == Some(PathKind::Missing) => {
            Some((format!("Moved or deleted · was in {}", parent_dir(&it.path)), palette::TEXT_DIM))
        }
        Some(it) => Some((parent_dir(&it.path), palette::TEXT_DIM)),
        None if n >= MAX_SHELF_ITEMS => Some(("Full · new files replace the oldest".to_string(), palette::TEXT_FAINT)),
        None => None,
    };
    if let Some((hint, color)) = hint {
        let hx = head.x + cw;
        let hint_r = Rect::new(hx, head.y, (right - 12.0 - hx).max(0.0), head.h);
        ui.c.text(&format!(" · {hint}"), hint_r, &TextStyle::footnote().color(color));
    }
}

/// "PDF", "Folder", "File" — the small line under a tile's name.
fn caption(path: &str, kind: Option<PathKind>) -> String {
    if kind == Some(PathKind::Folder) {
        return "Folder".into();
    }
    let ext = extension(path);
    if !ext.is_empty() {
        ext.to_uppercase()
    } else if kind == Some(PathKind::File) {
        "File".into()
    } else {
        String::new()
    }
}

/// Folder that contains `path` ("C:\a\b.txt" → "C:\a", "C:\b.txt" → "C:\").
fn parent_dir(path: &str) -> String {
    let t = path.trim_end_matches(['\\', '/']);
    match t.rfind(['\\', '/']) {
        Some(i) if i > 0 => {
            let p = &t[..i];
            if p.ends_with(':') {
                format!("{p}\\")
            } else {
                p.to_string()
            }
        }
        _ => t.to_string(),
    }
}

fn draw_drop_overlay(ui: &mut Ui, st: &AppState, zone: Rect, glow: f32, card: Option<(Rect, f32)>) {
    let base = ui.c.alpha();
    ui.c.set_alpha(base * glow);
    drop_zone(ui, card, zone, 16.0, st.accent.with_alpha(0.10), st.accent.with_alpha(0.9));
    // One centred group: Plus + label.
    let ls = TextStyle::body().weight(Weight::Semibold);
    let tw = ui.c.measure(DROP_LABEL, &ls, zone.w).0;
    let (isz, gap) = (16.0, 6.0);
    let c = zone.center();
    let x = c.x - (isz + gap + tw) / 2.0;
    ui.c.icon(Icon::Plus, Rect::new(x, c.y - isz / 2.0, isz, isz), st.accent);
    ui.c.text(DROP_LABEL, Rect::new(x + isz + gap, c.y - 10.0, tw + 2.0, 20.0), &ls);
    ui.c.set_alpha(base);
}

// ---------------------------------------------------------------- empty state

fn draw_empty(ui: &mut Ui, st: &AppState, l: &Local, area: Rect, card: Option<(Rect, f32)>, actions: &mut Vec<Action>) {
    let g = l.glow;
    let accent = st.accent;
    // Same bottom edge as the drop overlay over a filled shelf.
    let full = Rect::new(area.x, area.y + TOP, area.w, (area.h - TOP - 4.0).max(0.0));
    // The Phone tile shares the row until Phone drop is open.
    let zone = if st.phone.is_none() {
        let tile = Rect::new(full.right() - PHONE_TILE_W, full.y, PHONE_TILE_W, full.h);
        if draw_phone_tile(ui, tile, 1.0 - g, card) {
            actions.push(Action::PhoneOpen);
        }
        Rect::new(full.x, full.y, (full.w - PHONE_TILE_W - PILL_GAP).max(0.0), full.h)
    } else {
        full
    };
    let fill = Color::lerp(Color::white(0.03), accent.with_alpha(0.10), g);
    let stroke = Color::lerp(Color::white(0.12), accent.with_alpha(0.9), g);
    drop_zone(ui, card, zone, 18.0, fill, stroke);

    let undo_live = g < 0.5 && l.undo.as_ref().is_some_and(|(_, t)| st.now - t < UNDO_SECS);
    let (icon, title, hint) = if g >= 0.5 {
        (Icon::Plus, DROP_LABEL, "Oyenza Studio keeps a link — the files stay where they are")
    } else if undo_live {
        (Icon::Tray, "Shelf cleared", "")
    } else {
        (Icon::Tray, "Drop files here", "Drag files or folders onto the island to keep them close")
    };
    // Icon, title and hint (or the Undo pill) stacked around the zone centre.
    let c = zone.center();
    let icon_col = Color::lerp(Color::white(0.38), accent, g);
    ui.c.icon(icon, Rect::from_center(Point::new(c.x, c.y - 22.0), 26.0, 26.0), icon_col);
    ui.c.text(title, Rect::new(zone.x, c.y - 4.0, zone.w, 20.0), &TextStyle::headline().center());
    if undo_live {
        let w = ui.pill_width("Undo", true, true);
        let r = Rect::new(c.x - w / 2.0, c.y + 18.0, w, PILL_SMALL);
        if ui.pill(wid("shelf.undo"), r, "Undo", Some(Icon::Undo), PillStyle::UtilityAccent(accent)).clicked {
            actions.push(Action::Undo);
        }
    } else {
        ui.c.text(hint, Rect::new(zone.x + 16.0, c.y + 16.0, zone.w - 32.0, 18.0), &TextStyle::footnote().color(palette::TEXT_DIM).center());
    }
}

// ---------------------------------------------------------------- drawing helpers

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// Fade content into the black island at a row edge. The canvas has no
/// gradients, so nested rectangles build a smooth ramp (nested rather than
/// side by side so no anti-aliased seams show between bands).
fn fade_edge(ui: &mut Ui, r: Rect, from_left: bool) {
    const BANDS: usize = 10;
    let alpha = |j: usize| -> f32 {
        if j >= BANDS {
            return 0.0;
        }
        let t = 1.0 - (j as f32 + 0.5) / BANDS as f32;
        t * t * (3.0 - 2.0 * t)
    };
    for j in (0..BANDS).rev() {
        // Band j is covered by rects j..BANDS; pick each rect's alpha so the
        // composite over band j equals alpha(j).
        let b = 1.0 - (1.0 - alpha(j)) / (1.0 - alpha(j + 1));
        let w = r.w * (j + 1) as f32 / BANDS as f32;
        let band = if from_left { Rect::new(r.x, r.y, w, r.h) } else { Rect::new(r.right() - w, r.y, w, r.h) };
        ui.c.fill_rect(band, Color::black(b.clamp(0.0, 1.0)));
    }
}

/// Drop zone: a fill plus one solid hairline whose outer edge follows the
/// fill's corners exactly (no fill peeking past the stroke). On the Pill card
/// (`card`) its corners in the card's rounded corners follow those.
fn drop_zone(ui: &mut Ui, card: Option<(Rect, f32)>, zone: Rect, radius: f32, fill: Color, stroke: Color) {
    super::card::fill_box(ui, card, zone, radius, fill);
    super::card::stroke_box(ui, card, zone, radius, stroke, ZONE_STROKE);
}

/// When the Undo pill lapses or a phone result leaves the panel (the page
/// needs a frame then), whichever comes first.
pub fn next_wake(st: &AppState) -> Option<f64> {
    let undo = LOCAL.with(|l| l.borrow().undo.as_ref().map(|(_, t)| t + UNDO_SECS));
    let outcome = st.phone.as_ref().and_then(|p| p.outcome.as_ref()).map(|o| o.2).filter(|t| *t > st.now);
    // A pairing code shows its minutes left, then goes away.
    let pairing = match st.phone.as_ref().map(|p| &p.view) {
        Some(PhoneView::Pairing { until, .. }) => Some(if *until > st.now { st.now + (until - st.now).rem_euclid(60.0) + 0.05 } else { st.now }),
        _ => None,
    };
    [undo, outcome, pairing].into_iter().flatten().reduce(f64::min)
}

// ---------------------------------------------------------------- phone drop

/// The Phone tile on an empty shelf; true when clicked.
fn draw_phone_tile(ui: &mut Ui, tile: Rect, alpha: f32, card: Option<(Rect, f32)>) -> bool {
    let resp = ui.button(wid("shelf.phone.tile"), tile);
    let base = ui.c.alpha();
    ui.c.set_alpha(base * alpha);
    let fill = if resp.pressed {
        Color::white(0.09)
    } else if resp.hovered {
        Color::white(0.06)
    } else {
        Color::white(0.03)
    };
    drop_zone(ui, card, tile, 18.0, fill, Color::white(0.12));
    let c = tile.center();
    ui.c.fill_circle(Point::new(c.x, c.y - 24.0), 18.0, Color::white(0.10));
    ui.c.icon(Icon::Phone, Rect::from_center(Point::new(c.x, c.y - 24.0), 20.0, 20.0), palette::TEXT);
    ui.c.text("Phone drop", Rect::new(tile.x, c.y + 2.0, tile.w, 20.0), &TextStyle::headline().center());
    ui.c.text("Send from or save to a phone", Rect::new(tile.x + 10.0, c.y + 22.0, tile.w - 20.0, 18.0), &TextStyle::footnote().color(palette::TEXT_DIM).center());
    ui.c.set_alpha(base);
    resp.clicked
}

/// "IMG_1.HEIC, IMG_2.HEIC and 3 more".
fn names_line(files: &[(String, u64)]) -> String {
    match files {
        [] => String::new(),
        [a] => a.0.clone(),
        [a, b] => format!("{} and {}", a.0, b.0),
        [a, b, rest @ ..] => format!("{}, {} and {} more", a.0, b.0, rest.len()),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The host part of the link ("192.168.1.20"), for the panel's small print.
fn link_host(url: &str) -> &str {
    let rest = url.split_once("//").map_or(url, |(_, r)| r);
    rest.split(['/', ':']).next().unwrap_or("")
}

/// The panel under the shelf while Phone drop is open: a QR code on the
/// left (the same-Wi-Fi link, or a pairing link), what's happening on the right.
fn draw_phone(ui: &mut Ui, st: &AppState, l: &mut Local, pd: &PhoneDrop, r: Rect, card: Option<(Rect, f32)>, actions: &mut Vec<Action>) {
    drop_zone(ui, card, r, PHONE_R, Color::white(0.03), Color::white(0.12));
    let inset = (r.h - QR_BOX) / 2.0;
    let qr_r = Rect::new(r.x + inset, r.y + inset, QR_BOX, QR_BOX);
    // A phone on the same Wi-Fi asking or sending comes first, whatever the view.
    let busy = pd.error.is_some() || pd.ask.is_some() || pd.transfer.is_some();
    let view = if busy { &PhoneView::Lan } else { &pd.view };
    match view {
        PhoneView::Pairing { url, qr: Some(qr), .. } => draw_qr(ui, l, qr, url, qr_r),
        PhoneView::Paired => {
            ui.c.fill_rrect(qr_r, 12.0, Color::white(0.06));
            let c = qr_r.center();
            ui.c.icon(Icon::Phone, Rect::from_center(Point::new(c.x, c.y - 14.0), 26.0, 26.0), palette::TEXT_DIM);
            let n = st.paired.len();
            ui.c.text(&format!("{n} paired"), Rect::new(qr_r.x, c.y + 10.0, qr_r.w, 20.0), &TextStyle::headline().center());
        }
        _ => match &pd.qr {
            Some(qr) => draw_qr(ui, l, qr, &pd.url, qr_r),
            None => {
                ui.c.fill_rrect(qr_r, 12.0, Color::white(0.06));
                let icon = if pd.error.is_some() { Icon::Close } else { Icon::Phone };
                ui.c.icon(icon, Rect::from_center(qr_r.center(), 28.0, 28.0), palette::TEXT_FAINT);
            }
        },
    }

    let x = qr_r.right() + 18.0;
    let w = (r.right() - 16.0 - x).max(0.0);
    let pills_y = r.bottom() - 16.0 - PILL_SMALL;
    let title_r = Rect::new(x, r.y + 18.0, w, 20.0);
    let line = |i: f32| Rect::new(x, r.y + 44.0 + i * 19.0, w, 18.0);
    let dim = TextStyle::footnote().color(palette::TEXT_DIM);
    let small = TextStyle::caption().color(palette::TEXT_FAINT);
    let now = st.now;

    // Pills from the right edge leftwards; returns the clicked index.
    let pills = |ui: &mut Ui, list: &[(&str, Option<Icon>, PillStyle, &str)]| -> Option<usize> {
        let mut right = r.right() - 16.0;
        let mut hit = None;
        for (i, (label, icon, style, id)) in list.iter().enumerate() {
            let pw = ui.pill_width(label, icon.is_some(), true).max(72.0);
            let pr = Rect::new(right - pw, pills_y, pw, PILL_SMALL);
            if ui.pill(wid(id), pr, label, *icon, *style).clicked {
                hit = Some(i);
            }
            right = pr.x - PILL_GAP;
        }
        hit
    };
    // Room left of a row of pills for a caption (so the two never overlap).
    let caption_w = |ui: &mut Ui, labels: &[(&str, bool)]| -> f32 {
        let pills: f32 = labels.iter().map(|(label, icon)| ui.pill_width(label, *icon, true).max(72.0) + PILL_GAP).sum();
        (r.right() - 16.0 - pills - 12.0 - x).max(0.0)
    };

    if let Some(err) = &pd.error {
        ui.c.text("Phone drop couldn't start", title_r, &TextStyle::headline());
        ui.c.text(err, line(0.0), &dim);
        match pills(ui, &[("Close", None, PillStyle::Utility, "shelf.phone.close"), ("Try again", None, PillStyle::Primary(st.accent), "shelf.phone.retry")]) {
            Some(0) => actions.push(Action::PhoneClose),
            Some(1) => actions.push(Action::PhoneOpen),
            _ => {}
        }
        return;
    }

    if let Some(ask) = &pd.ask {
        let n = ask.files.len();
        ui.c.text(&format!("{} wants to send {}", ask.from, plural(n, "file", "files")), title_r, &TextStyle::headline());
        ui.c.text(&names_line(&ask.files), line(0.0), &dim);
        ui.c.text(&fmt_bytes(ask.total), line(1.0), &dim);
        match pills(ui, &[("Accept", Some(Icon::Check), PillStyle::Primary(palette::GREEN), "shelf.phone.accept"), ("Decline", None, PillStyle::Utility, "shelf.phone.decline")]) {
            Some(0) => actions.push(Action::PhoneAnswer(ask.id, true)),
            Some(1) => actions.push(Action::PhoneAnswer(ask.id, false)),
            _ => {}
        }
        return;
    }

    if let Some(t) = &pd.transfer {
        ui.c.text(&format!("Receiving from {}", t.from), title_r, &TextStyle::headline());
        let detail = if t.files > 1 {
            format!("File {} of {} · {} of {}", (t.file + 1).min(t.files), t.files, fmt_bytes(t.done), fmt_bytes(t.total))
        } else {
            format!("{} of {}", fmt_bytes(t.done), fmt_bytes(t.total))
        };
        ui.c.text(&detail, line(0.0), &dim);
        let frac = if t.total > 0 { (t.done as f64 / t.total as f64) as f32 } else { 1.0 };
        ui.progress_bar(wid("shelf.phone.progress"), Rect::new(x, line(1.0).y + 8.0, w, 4.0), frac, palette::GREEN, false);
        if pills(ui, &[("Stop", None, PillStyle::Utility, "shelf.phone.stop")]) == Some(0) {
            actions.push(Action::PhoneClose);
        }
        return;
    }

    match view {
        PhoneView::Pairing { until, .. } => {
            if *until <= now {
                actions.push(Action::Pair(PairCmd::Back));
                return;
            }
            ui.c.text("Scan to pair a phone", title_r, &TextStyle::headline());
            ui.c.text(&format!("Then it can send to {THIS_COMPUTER} from any network."), line(0.0), &dim);
            let second = if st.pair_server == Some(false) {
                format!("Can't reach {} right now. Check the internet.", pairing_host())
            } else {
                let mins = ((until - now) / 60.0).ceil().max(1.0) as u32;
                format!("Use the camera. The code works for {} more {}.", mins, if mins == 1 { "minute" } else { "minutes" })
            };
            ui.c.text(&second, line(1.0), &dim);
            let cw = caption_w(ui, &[("Done", false), ("Back", false)]);
            ui.c.text(&format!("Pairing link · {}", pairing_host()), Rect::new(x, pills_y + 7.0, cw, 16.0), &small);
            match pills(ui, &[("Done", None, PillStyle::Utility, "shelf.pair.done"), ("Back", None, PillStyle::Utility, "shelf.pair.back")]) {
                Some(0) => actions.push(Action::PhoneClose),
                Some(1) => actions.push(Action::Pair(PairCmd::Back)),
                _ => {}
            }
            return;
        }
        PhoneView::Paired => {
            let list = [("Done", None, PillStyle::Utility, "shelf.pair.done"), ("Back", None, PillStyle::Utility, "shelf.pair.back"), ("Pair a phone", Some(Icon::Phone), PillStyle::Utility, "shelf.pair.another")];
            let cw = caption_w(ui, &[("Done", false), ("Back", false), ("Pair a phone", true)]);
            draw_paired_list(ui, st, x, w, r, pills_y, cw, actions);
            match pills(ui, &list) {
                Some(0) => actions.push(Action::PhoneClose),
                Some(1) => actions.push(Action::Pair(PairCmd::Back)),
                Some(2) => actions.push(Action::Pair(PairCmd::Begin)),
                _ => {}
            }
            return;
        }
        PhoneView::Lan => {}
    }

    match pd.outcome.as_ref().filter(|o| o.2 > now) {
        Some((text, good, _)) => {
            let color = if *good { palette::GREEN } else { palette::TEXT };
            let ix = if *good {
                ui.c.icon(Icon::CheckCircle, Rect::new(x, title_r.y + 1.0, 18.0, 18.0), palette::GREEN);
                24.0
            } else {
                0.0
            };
            ui.c.text(text, Rect::new(x + ix, title_r.y, w - ix, title_r.h), &TextStyle::headline().color(color));
            let next = match (&pd.outcome_line, &pd.visitor) {
                (Some(line), _) => line.clone(),
                (None, Some(v)) => format!("{v} can send more from the same page."),
                (None, None) => "Scan the code with your phone's camera.".to_string(),
            };
            ui.c.text(&next, line(0.0), &dim);
        }
        None => match &pd.visitor {
            Some(v) => {
                ui.c.text(&format!("{v} is connected"), title_r, &TextStyle::headline());
                ui.c.text("Pick photos or files on the phone and tap Send.", line(0.0), &dim);
                ui.c.text("Files on the Shelf can be saved from the phone.", line(1.0), &dim);
            }
            None => {
                ui.c.text("Scan with your phone's camera", title_r, &TextStyle::headline());
                ui.c.text(&format!("Your phone needs the same Wi-Fi as {THIS_COMPUTER}."), line(0.0), &dim);
                ui.c.text(ALLOW_HINT, line(1.0), &dim);
            }
        },
    }
    // One pill per meaning: pair the first phone, or see the paired ones
    // (that list has its own Pair a phone).
    let (pair_label, pair_icon, pair_cmd) = if st.paired.is_empty() { ("Pair a phone", Some(Icon::Phone), PairCmd::Begin) } else { ("Paired phones", None, PairCmd::List) };
    let n = if PAIRING { 2 } else { 1 };
    let host = link_host(&pd.url);
    if !host.is_empty() {
        let cw = caption_w(ui, &[("Done", false), (pair_label, pair_icon.is_some())][..n]);
        ui.c.text(&format!("This session's link · {host}"), Rect::new(x, pills_y + 7.0, cw, 16.0), &small);
    }
    match pills(ui, &[("Done", None, PillStyle::Utility, "shelf.phone.done"), (pair_label, pair_icon, PillStyle::Utility, "shelf.pair.open")][..n]) {
        Some(0) => actions.push(Action::PhoneClose),
        Some(1) => actions.push(Action::Pair(pair_cmd)),
        _ => {}
    }
}

/// Where pairing links point (the meeting server's host name).
fn pairing_host() -> String {
    let server = std::env::var("PONI_DROP_SERVER").ok().filter(|s| s.starts_with("http")).unwrap_or_else(|| poni_core::pairdrop::DEFAULT_SERVER.to_string());
    server.split("://").nth(1).unwrap_or(&server).trim_end_matches('/').to_string()
}

/// Most phones the list shows (the panel is 152 DIP tall).
const LIST_ROWS: usize = 3;
const ROW_H: f32 = 30.0;

/// The paired phones, most recently used first, each with Remove.
#[allow(clippy::too_many_arguments)]
fn draw_paired_list(ui: &mut Ui, st: &AppState, x: f32, w: f32, r: Rect, pills_y: f32, caption_w: f32, actions: &mut Vec<Action>) {
    let dim = TextStyle::footnote().color(palette::TEXT_DIM);
    if st.paired.is_empty() {
        ui.c.text("No phone is paired", Rect::new(x, r.y + 18.0, w, 20.0), &TextStyle::headline());
        ui.c.text(&format!("Pair a phone, and it can send to {THIS_COMPUTER} from any network."), Rect::new(x, r.y + 44.0, w, 18.0), &dim);
        return;
    }
    let unix = st.unix;
    for (i, p) in st.paired.iter().take(LIST_ROWS).enumerate() {
        let y = r.y + 12.0 + i as f32 * ROW_H;
        let remove_w = ui.pill_width("Remove", false, true).max(72.0);
        let remove_r = Rect::new(x + w - remove_w, y + (ROW_H - PILL_SMALL) / 2.0 - 2.0, remove_w, PILL_SMALL);
        if ui.pill(wid(&format!("shelf.pair.remove.{}", short(&p.room))), remove_r, "Remove", None, PillStyle::Utility).clicked {
            actions.push(Action::Pair(PairCmd::Remove(p.room.clone())));
        }
        let seen = last_seen(unix, p);
        let sw = ui.c.measure(&seen, &dim, 200.0).0 + 4.0;
        let seen_r = Rect::new(remove_r.x - 12.0 - sw, y + 4.0, sw, 20.0);
        ui.c.text(&seen, seen_r, &dim);
        ui.c.text(&p.name, Rect::new(x, y + 3.0, (seen_r.x - 10.0 - x).max(0.0), 20.0), &TextStyle::body().weight(Weight::Semibold));
    }
    let more = st.paired.len().saturating_sub(LIST_ROWS);
    if more > 0 {
        let small = TextStyle::caption().color(palette::TEXT_FAINT);
        ui.c.text(&format!("+{more} used less recently"), Rect::new(x, pills_y + 7.0, caption_w, 16.0), &small);
    }
}

fn short(room: &str) -> &str {
    &room[..room.len().min(8)]
}

/// "Last seen 5 min ago" and the like.
fn last_seen(unix: i64, p: &PairedPhone) -> String {
    if p.seen <= 0 {
        return "Not used yet".into();
    }
    let s = (unix - p.seen).max(0);
    match s {
        0..=59 => "Seen just now".into(),
        60..=3599 => format!("Seen {} min ago", s / 60),
        3600..=86_399 => format!("Seen {} h ago", s / 3600),
        86_400..=172_799 => "Seen yesterday".into(),
        _ => format!("Seen {} days ago", s / 86_400),
    }
}

/// The QR code on its white square, each module a whole number of device
/// pixels so the edges stay sharp enough to scan from a screen. The square
/// is part of the bitmap: themes repaint drawn greys (a light theme would turn
/// a drawn white square dark), but leave images alone, and a code needs its
/// light border to scan.
fn draw_qr(ui: &mut Ui, l: &mut Local, qr: &QrMatrix, url: &str, r: Rect) {
    let scale = ui.c.scale().max(0.5);
    let box_px = (r.w * scale).round().max(1.0) as u32;
    let n = qr.size.max(1) as f32;
    let module_px = (((r.w - 2.0 * QR_MARGIN) * scale) / n).floor().max(1.0) as u32;
    let img = qr_image(l, qr, url, box_px, module_px);
    // One device pixel per bitmap pixel, on the pixel grid.
    let side = box_px as f32 / scale;
    let snap = |v: f32| (v * scale).round() / scale;
    let dst = Rect::new(snap(r.center().x - side / 2.0), snap(r.center().y - side / 2.0), side, side);
    ui.c.image(&img, dst, 12.0);
}

/// The QR code as one bitmap (a single draw per frame instead of hundreds
/// of rectangles): `box_px` square of white with the modules centred,
/// rebuilt when the link or the scaling changes.
fn qr_image(l: &mut Local, qr: &QrMatrix, url: &str, box_px: u32, module_px: u32) -> Image {
    if let Some((u, b, m, img)) = &l.qr_img {
        if u == url && *b == box_px && *m == module_px {
            return img.clone();
        }
    }
    let n = qr.size as u32;
    let side = (n * module_px).min(box_px);
    let off = (box_px - side) / 2;
    // Premultiplied BGRA: opaque white, then black modules.
    let mut px = vec![255u8; (box_px * box_px * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            if !qr.get(x as usize, y as usize) {
                continue;
            }
            for dy in 0..module_px {
                let py = off + y * module_px + dy;
                if py >= box_px {
                    continue;
                }
                for dx in 0..module_px {
                    let pxx = off + x * module_px + dx;
                    if pxx >= box_px {
                        continue;
                    }
                    let o = ((py * box_px + pxx) * 4) as usize;
                    px[o..o + 3].fill(0);
                }
            }
        }
    }
    let img = ImageData::new(box_px, box_px, px);
    l.qr_img = Some((url.to_string(), box_px, module_px, img.clone()));
    img
}
