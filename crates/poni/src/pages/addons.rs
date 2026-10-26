//! Add-ons page, the "+" at the end of the tab row: features only the people
//! who switch them on get (`poni_core::addons`). Most download then; Script
//! reader, System, Health, Stocks and IELTS are already in the app and only
//! show their pages (System's readings, Health's steps and Stocks' prices
//! also run only while they are on; IELTS only while a session runs).
//!
//! One row per add-on: its icon, its name, what it does (or where its
//! download stands, with a bar while it comes in), its size while it is off,
//! ("Built in" for one with nothing to download), and a switch. The whole
//! row toggles, as in Settings: on downloads the add-on and starts it, off
//! stops it and deletes its files
//! (`Command::SaveSettings`; the app passes the change to `services::addons`).
//! Both apps list every add-on ([`OFFERED`]).

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
/// Five rows fit the island's height (the System page's 256 DIPs), so the
/// window never grows for this page; a sixth packs them closer ([`row_h`]),
/// and from a seventh each row is one line (rows under [`TWO_LINES`]).
const ROW_H: f32 = 45.0;
const MAX_H: f32 = 256.0;
const TWO_LINES: f32 = 36.0;
const BADGE: f32 = 36.0;
const BADGE_COMPACT: f32 = 30.0;
const BADGE_ONE_LINE: f32 = 26.0;
const TEXT_X: f32 = 48.0;
/// A row's hover wash reaches this far past its contents (as in Settings).
const ROW_BLEED: f32 = 8.0;
const SWITCH_W: f32 = 30.0;
const SWITCH_H: f32 = 18.0;
const BAR_H: f32 = 3.0;

/// The add-ons this app has, in the page's order: all of them, on Windows
/// and on the Mac (whose Prayer times use macOS's location and sounds).
pub const OFFERED: &[&Addon] = &addons::ALL;
/// The lines name the Mac ("on this Mac") and leave out Windows' reasons.
const MAC: bool = cfg!(target_os = "macos");

pub fn height(_st: &AppState) -> f32 {
    let n = OFFERED.len();
    ROWS_Y + n as f32 * row_h(n)
}

fn row_h(n: usize) -> f32 {
    ((MAX_H - ROWS_Y) / n.max(1) as f32).floor().min(ROW_H)
}

/// Each add-on's icon on its badge.
fn icon(a: &Addon) -> Icon {
    match a.id {
        "sholat" => Icon::Moon,
        "ja" => Icon::Lyrics,
        "script" => Icon::Script,
        "system" => Icon::Gauge,
        "health" => Icon::Steps,
        "stocks" => Icon::Stocks,
        "ielts" => Icon::Mic,
        _ => Icon::Addons,
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let intro = "Each one stays off until you switch it on, and switching off deletes any download.";
    ui.c.text(intro, Rect::new(area.x, area.y, area.w, INTRO_H), &TextStyle::footnote().color(palette::TEXT_DIM));
    // The last row's hover wash meets the Pill card's rounded corners.
    let card = super::card::pill_card(st, area);
    let h = row_h(OFFERED.len());
    for (i, a) in OFFERED.iter().enumerate() {
        let row = Rect::new(area.x, area.y + ROWS_Y + i as f32 * h, area.w, h);
        if addon_row(ui, st, row, i, a, card) {
            let on = st.settings.addon_on(a.id);
            st.settings.set_addon(a.id, !on);
            st.cmd(Command::SaveSettings);
        }
    }
}

/// Returns true when the row was clicked. `card`: the Pill card (`card::pill_card`).
fn addon_row(ui: &mut Ui, st: &AppState, r: Rect, i: usize, a: &Addon, card: Option<(Rect, f32)>) -> bool {
    let status = st.addon(a.id);
    let accent = st.accent;
    let hit = Rect::new(r.x - ROW_BLEED, r.y + 2.0, r.w + 2.0 * ROW_BLEED, r.h - 4.0);
    let resp = ui.button(wid_n("addons.row", i as u64), hit);
    if resp.hovered || resp.pressed {
        super::card::fill_box(ui, card, hit, 10.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
    }
    let cy = r.center().y;
    // Six rows sit closer: a smaller badge, the text centred in the row; from
    // seven, one line each.
    let one_line = r.h < TWO_LINES;
    let (badge, dy) = match (one_line, r.h < ROW_H) {
        (true, _) => (BADGE_ONE_LINE, 0.0),
        (false, true) => (BADGE_COMPACT, ((r.h - ROW_H) / 2.0).round()),
        (false, false) => (BADGE, 0.0),
    };
    let glyph = if one_line { 16.0 } else { 18.0 };
    ui.c.fill_circle(Point::new(r.x + BADGE / 2.0, cy), badge / 2.0, accent.with_alpha(0.18));
    ui.c.icon(icon(a), Rect::from_center(Point::new(r.x + BADGE / 2.0, cy), glyph, glyph), accent);

    let sw = Rect::new(r.right() - SWITCH_W, cy - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
    crate::pages::settings::draw_switch(ui.c, sw, status.is_on(), accent);
    // The size, while there is a choice to make.
    let mut text_right = sw.x - 16.0;
    if status == Status::Off {
        let size = if a.downloads() { fmt_bytes(a.size()) } else { "Built in".to_string() };
        let style = TextStyle::footnote().color(palette::TEXT_DIM).right();
        let w = ui.c.measure(&size, &style, 200.0).0.ceil();
        ui.c.text(&size, Rect::new(sw.x - 10.0 - w, r.y, w, r.h), &style);
        text_right = sw.x - 10.0 - w - 16.0;
    }

    let x = r.x + TEXT_X;
    let w = (text_right - x).max(0.0);
    let line_color = if status == Status::Failed { palette::ORANGE } else { palette::TEXT_DIM };
    let line = status.line(a, MAC);
    let bar_y = if one_line {
        // The name, then what it does (or where its download stands), cut to fit.
        let name = TextStyle::headline();
        let nw = ui.c.measure(a.title, &name, w).0.ceil().min(w);
        ui.c.text(a.title, Rect::new(x, r.y, nw + 1.0, r.h), &name);
        let lx = x + nw + 10.0;
        ui.c.text(&line, Rect::new(lx, r.y, (x + w - lx).max(0.0), r.h), &TextStyle::footnote().color(line_color));
        r.bottom() - BAR_H - 2.0
    } else {
        ui.c.text(a.title, Rect::new(x, r.y + 3.0 + dy, w, 20.0), &TextStyle::headline());
        ui.c.text(&line, Rect::new(x, r.y + 22.0 + dy, w, 17.0), &TextStyle::footnote().color(line_color));
        r.y + 40.0 + dy
    };
    if let Some(p) = status.progress() {
        let bar = Rect::new(x, bar_y, w, BAR_H);
        ui.c.fill_rrect(bar, BAR_H / 2.0, palette::TRACK);
        if p > 0.0 {
            ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * p).max(BAR_H), BAR_H), BAR_H / 2.0, accent);
        }
    }
    resp.clicked
}
