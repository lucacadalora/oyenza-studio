//! Add-ons page, the "+" at the end of the tab row: features that download
//! only for the people who switch them on (`poni_core::addons`).
//!
//! One row per add-on: its icon, its name, what it does (or where its
//! download stands, with a bar while it comes in), its size while it is off,
//! and a switch. The whole row toggles, as in Settings: on downloads the
//! add-on and starts it, off stops it and deletes its files
//! (`Command::SaveSettings`; the app passes the change to `services::addons`).
//! The Mac app lists only the add-ons it has ([`OFFERED`]).

use crate::commands::Command;
use crate::gfx::TextStyle;
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid_n, Ui};
use poni_core::addons::{self, Addon, Status};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::phonedrop::fmt_bytes;

const INTRO_H: f32 = 18.0;
const ROWS_Y: f32 = 28.0;
const ROW_H: f32 = 52.0;
const BADGE: f32 = 36.0;
const TEXT_X: f32 = 48.0;
/// A row's hover wash reaches this far past its contents (as in Settings).
const ROW_BLEED: f32 = 8.0;
const SWITCH_W: f32 = 30.0;
const SWITCH_H: f32 = 18.0;
const BAR_H: f32 = 3.0;

/// The add-ons this app has, in the page's order: all of them on Windows;
/// Japanese readings on the Mac (Prayer times needs Windows' location and
/// sound services).
pub const OFFERED: &[&Addon] = if cfg!(windows) { &addons::ALL } else { &[&addons::JAPANESE_READINGS] };
/// The lines name the Mac ("on this Mac") and leave out Windows' reasons.
const MAC: bool = cfg!(target_os = "macos");

pub fn height(_st: &AppState) -> f32 {
    ROWS_Y + OFFERED.len() as f32 * ROW_H
}

/// Each add-on's icon on its badge.
fn icon(a: &Addon) -> Icon {
    match a.id {
        "sholat" => Icon::Moon,
        "ja" => Icon::Lyrics,
        _ => Icon::Addons,
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let intro = "Each one downloads only when you switch it on. Switching it off deletes its files.";
    ui.c.text(intro, Rect::new(area.x, area.y, area.w, INTRO_H), &TextStyle::footnote().color(palette::TEXT_DIM));
    // The last row's hover wash meets the Pill card's rounded corners.
    let card = super::clipboard::pill_card(st, area);
    for (i, a) in OFFERED.iter().enumerate() {
        let row = Rect::new(area.x, area.y + ROWS_Y + i as f32 * ROW_H, area.w, ROW_H);
        if addon_row(ui, st, row, i, a, card) {
            let on = st.settings.addon_on(a.id);
            st.settings.set_addon(a.id, !on);
            st.cmd(Command::SaveSettings);
        }
    }
}

/// Returns true when the row was clicked. `card`: the Pill card (`clipboard::pill_card`).
fn addon_row(ui: &mut Ui, st: &AppState, r: Rect, i: usize, a: &Addon, card: Option<(Rect, f32)>) -> bool {
    let status = st.addon(a.id);
    let accent = st.accent;
    let hit = Rect::new(r.x - ROW_BLEED, r.y + 2.0, r.w + 2.0 * ROW_BLEED, r.h - 4.0);
    let resp = ui.button(wid_n("addons.row", i as u64), hit);
    if resp.hovered || resp.pressed {
        super::clipboard::fill_box(ui, card, hit, 10.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
    }
    let cy = r.center().y;
    ui.c.fill_circle(Point::new(r.x + BADGE / 2.0, cy), BADGE / 2.0, accent.with_alpha(0.18));
    ui.c.icon(icon(a), Rect::from_center(Point::new(r.x + BADGE / 2.0, cy), 18.0, 18.0), accent);

    let sw = Rect::new(r.right() - SWITCH_W, cy - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
    crate::pages::settings::draw_switch(ui.c, sw, status.is_on(), accent);
    // The size, while there is a choice to make.
    let mut text_right = sw.x - 16.0;
    if status == Status::Off {
        let size = fmt_bytes(a.size());
        let style = TextStyle::footnote().color(palette::TEXT_DIM).right();
        let w = ui.c.measure(&size, &style, 200.0).0.ceil();
        ui.c.text(&size, Rect::new(sw.x - 10.0 - w, r.y, w, r.h), &style);
        text_right = sw.x - 10.0 - w - 16.0;
    }

    let x = r.x + TEXT_X;
    let w = (text_right - x).max(0.0);
    ui.c.text(a.title, Rect::new(x, r.y + 6.0, w, 20.0), &TextStyle::headline());
    let line_color = if status == Status::Failed { palette::ORANGE } else { palette::TEXT_DIM };
    ui.c.text(&status.line(a, MAC), Rect::new(x, r.y + 27.0, w, 17.0), &TextStyle::footnote().color(line_color));
    if let Some(p) = status.progress() {
        let bar = Rect::new(x, r.y + 46.0, w, BAR_H);
        ui.c.fill_rrect(bar, BAR_H / 2.0, palette::TRACK);
        if p > 0.0 {
            ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * p).max(BAR_H), BAR_H), BAR_H / 2.0, accent);
        }
    }
    resp.clicked
}
