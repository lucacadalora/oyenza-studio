//! Settings page, behind the gear on the right of the header.
//!
//! The Oyenza account comes first: the Oyenza app's icon, then signing in
//! (the code itself shows on the Oyenza page), who is signed in, and
//! Disconnect. Under a line, what the tray menu also changes: the theme and
//! the idle look as pickers, and six switches. A change saves at once and
//! applies the way the tray's does (`Command::SaveSettings`).

use crate::commands::{Command, OyenzaCmd};
use crate::gfx::{TextStyle, Weight};
use crate::pages::PageKind;
use crate::state::{AppState, OyenzaLink};
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_GAP, PILL_REGULAR, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::oyenza;
use poni_core::settings::{IdleStyle, Theme};

/// Account row, a line, the pickers under their labels, three rows of switches.
/// No taller than the Tasks page (200), so the window keeps its size.
const HEIGHT: f32 = 198.0;
const ACCOUNT_H: f32 = 44.0;
/// The Oyenza app's icon, and the text column right of it.
const ICON: f32 = 36.0;
const TEXT_X: f32 = 48.0;
const RULE_Y: f32 = 54.0;
const LABELS_Y: f32 = 62.0;
const PICKERS_Y: f32 = 80.0;
const SWITCHES_Y: f32 = 114.0;
const ROW_H: f32 = 28.0;
const COL_GAP: f32 = 24.0;
/// A row's hover wash reaches this far past its text and switch.
const ROW_BLEED: f32 = 8.0;
/// Pickers: the selection's inset in the track, and each choice's padding.
const SEG_INSET: f32 = 2.0;
const SEG_PAD: f32 = 11.0;
const SWITCH_W: f32 = 30.0;
const SWITCH_H: f32 = 18.0;
const KNOB_INSET: f32 = 2.0;
/// Oyenza's colour, as on the Oyenza page.
const OYENZA: Color = palette::ORANGE;

const THEMES: [(Theme, &str); 2] = [(Theme::Classic, "Classic"), (Theme::Oyen, "Oyen")];
const IDLE_STYLES: [IdleStyle; 4] = [IdleStyle::Clock, IdleStyle::Eyes, IdleStyle::Minimal, IdleStyle::Hidden];

/// The six switches, as the tray menu names them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Switch {
    Lyrics,
    Pronunciation,
    Sounds,
    Clipboard,
    HideFullscreen,
    Autostart,
}

const COLUMNS: [[(Switch, &str); 3]; 2] = [
    [(Switch::Lyrics, "Synced lyrics"), (Switch::Pronunciation, "Lyrics pronunciation"), (Switch::Sounds, "Sounds")],
    [(Switch::Clipboard, "Clipboard history"), (Switch::HideFullscreen, "Hide in full-screen apps"), (Switch::Autostart, "Start with Windows")],
];

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// While a sign-in code waits, its time left counts down each second.
pub fn next_wake(st: &AppState) -> Option<f64> {
    matches!(st.oyenza.link, OyenzaLink::Connecting { .. }).then(|| st.now.floor() + 1.0)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    draw_account(ui, st, Rect::new(area.x, area.y, area.w, ACCOUNT_H));
    let px = 1.0 / ui.c.scale().max(0.5);
    ui.c.fill_rect(Rect::new(area.x, area.y + RULE_Y, area.w, px), Color::white(0.10));

    let col_w = ((area.w - COL_GAP) / 2.0).max(0.0);
    let cols = [area.x, area.x + col_w + COL_GAP];
    let label_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_FAINT);

    // Pickers.
    ui.c.text("THEME", Rect::new(cols[0], area.y + LABELS_Y, col_w, 14.0), &label_st);
    let names: Vec<&str> = THEMES.iter().map(|t| t.1).collect();
    let sel = THEMES.iter().position(|t| t.0 == st.settings.theme).unwrap_or(0);
    if let Some(i) = picker(ui, "settings.theme", cols[0], area.y + PICKERS_Y, &names, sel) {
        st.settings.theme = THEMES[i].0;
        st.cmd(Command::SaveSettings);
    }
    ui.c.text("IDLE LOOK", Rect::new(cols[1], area.y + LABELS_Y, col_w, 14.0), &label_st);
    // In the Oyen theme the eyes are Oyen's own (as in the tray menu).
    let eyes = if st.settings.theme == Theme::Oyen { "Oyen" } else { "Eyes" };
    let names = ["Clock", eyes, "Minimal", "Hidden"];
    let sel = IDLE_STYLES.iter().position(|s| *s == st.settings.idle_style).unwrap_or(0);
    if let Some(i) = picker(ui, "settings.idle", cols[1], area.y + PICKERS_Y, &names, sel) {
        st.settings.idle_style = IDLE_STYLES[i];
        st.cmd(Command::SaveSettings);
    }

    // Switches, two columns of three.
    let accent = st.accent;
    for (c, column) in COLUMNS.iter().enumerate() {
        for (r, &(sw, label)) in column.iter().enumerate() {
            let row = Rect::new(cols[c], area.y + SWITCHES_Y + r as f32 * ROW_H, col_w, ROW_H);
            let on = switch_on(st, sw);
            let id = wid_n("settings.switch", (c * 3 + r) as u64);
            if switch_row(ui, id, row, label, on, accent) {
                flip(st, sw, on);
            }
        }
    }
}

/// Where a switch reads from. Start with Windows is read off the UI thread
/// when the page opens; until then it shows off, dimmed.
fn switch_on(st: &AppState, sw: Switch) -> Option<bool> {
    let s = &st.settings;
    match sw {
        Switch::Lyrics => Some(s.lyrics),
        Switch::Pronunciation => Some(s.lyrics_pronunciation),
        Switch::Sounds => Some(s.sounds),
        Switch::Clipboard => Some(s.clipboard_history),
        Switch::HideFullscreen => Some(s.hide_in_fullscreen),
        Switch::Autostart => st.ui.autostart,
    }
}

fn flip(st: &mut AppState, sw: Switch, on: Option<bool>) {
    let s = &mut st.settings;
    match sw {
        Switch::Lyrics => s.lyrics = !s.lyrics,
        Switch::Pronunciation => s.lyrics_pronunciation = !s.lyrics_pronunciation,
        Switch::Sounds => s.sounds = !s.sounds,
        // Off forgets the history, pinned items included (as from the tray).
        Switch::Clipboard => s.clipboard_history = !s.clipboard_history,
        Switch::HideFullscreen => s.hide_in_fullscreen = !s.hide_in_fullscreen,
        Switch::Autostart => {
            if let Some(on) = on {
                st.cmd(Command::SetAutostart(!on));
            }
            return;
        }
    }
    st.cmd(Command::SaveSettings);
}

// ---------------------------------------------------------------- account

fn draw_account(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let cy = r.center().y;
    crate::oyen::app_icon(ui.c, Point::new(r.x + ICON / 2.0, cy), ICON);
    match st.oyenza.link.clone() {
        OyenzaLink::Off => {
            let label = "Sign in with Oyenza";
            let w = ui.pill_width(label, false, false);
            let b = Rect::new(r.right() - w, cy - PILL_REGULAR / 2.0, w, PILL_REGULAR);
            if ui.pill(wid("settings.sign-in"), b, label, None, PillStyle::Primary(OYENZA)).clicked {
                st.cmd(Command::Oyenza(OyenzaCmd::SignIn));
            }
            account_text(ui, r, b.x, "Oyenza", "Sign in to see money in and bills due here.");
        }
        OyenzaLink::Connecting { code, until, .. } => {
            let cancel_w = ui.pill_width("Cancel", false, true);
            let cancel = Rect::new(r.right() - cancel_w, cy - PILL_SMALL / 2.0, cancel_w, PILL_SMALL);
            if ui.pill(wid("settings.cancel"), cancel, "Cancel", None, PillStyle::Utility).clicked {
                st.cmd(Command::Oyenza(OyenzaCmd::Cancel));
            }
            let show_w = ui.pill_width("Show code", false, true);
            let show = Rect::new(cancel.x - PILL_GAP - show_w, cancel.y, show_w, PILL_SMALL);
            if ui.pill(wid("settings.show-code"), show, "Show code", None, PillStyle::Primary(OYENZA)).clicked {
                st.cmd(Command::SetPage(PageKind::Oyenza));
            }
            let line = format!("Code {code} · works for {}", oyenza::code_time_left(until - st.now));
            account_text(ui, r, show.x, "Signing in to Oyenza", &line);
        }
        OyenzaLink::Connected { account } => {
            let dis_w = ui.pill_width("Disconnect", false, true);
            let dis = Rect::new(r.right() - dis_w, cy - PILL_SMALL / 2.0, dis_w, PILL_SMALL);
            if ui.pill(wid("settings.disconnect"), dis, "Disconnect", None, PillStyle::Utility).clicked {
                st.cmd(Command::Oyenza(OyenzaCmd::Disconnect));
            }
            // Sums and senders straight away on the island (off: they wait for Show).
            let label = "Show amounts";
            let lw = ui.c.measure(label, &TextStyle::body(), 300.0).0.ceil();
            let row = Rect::new(dis.x - 2.0 * ROW_BLEED - lw - 10.0 - SWITCH_W, cy - ROW_H / 2.0, lw + 10.0 + SWITCH_W, ROW_H);
            let on = st.settings.oyenza_show_amounts;
            if switch_row(ui, wid("settings.amounts"), row, label, Some(on), st.accent) {
                st.settings.oyenza_show_amounts = !on;
                st.cmd(Command::SaveSettings);
            }
            account_text(ui, r, row.x - ROW_BLEED, "Oyenza", &format!("Signed in as {account}"));
        }
    }
}

/// The account's name over one dim line, between Oyen and `right` (where
/// the row's buttons start).
fn account_text(ui: &mut Ui, r: Rect, right: f32, title: &str, line: &str) {
    let x = r.x + TEXT_X;
    let w = (right - 16.0 - x).max(0.0);
    let cy = r.center().y;
    ui.c.text(title, Rect::new(x, cy - 19.0, w, 20.0), &TextStyle::headline());
    ui.c.text(line, Rect::new(x, cy + 1.0, w, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
}

// ---------------------------------------------------------------- widgets

/// Choices side by side in a track, the chosen one lifted (the Focus page's
/// picker, a size smaller). Returns the choice clicked, if any.
fn picker(ui: &mut Ui, id: &str, x: f32, y: f32, names: &[&str], sel: usize) -> Option<usize> {
    let st = TextStyle::footnote().weight(Weight::Semibold);
    let inner_h = PILL_SMALL - 2.0 * SEG_INSET;
    let mut cx = x + SEG_INSET;
    let cells: Vec<Rect> = names
        .iter()
        .map(|n| {
            let w = (2.0 * SEG_PAD + ui.c.measure(n, &st, 300.0).0).ceil();
            let cell = Rect::new(cx, y + SEG_INSET, w, inner_h);
            cx += w;
            cell
        })
        .collect();
    ui.c.fill_rrect(Rect::new(x, y, cx + SEG_INSET - x, PILL_SMALL), PILL_SMALL / 2.0, palette::SURFACE);
    let mut picked = None;
    for (i, (name, cell)) in names.iter().zip(&cells).enumerate() {
        let color = if i == sel {
            ui.c.fill_rrect(*cell, inner_h / 2.0, Color::white(0.16));
            palette::TEXT
        } else {
            let resp = ui.button(wid_n(id, i as u64), Rect::new(cell.x, y, cell.w, PILL_SMALL));
            if resp.clicked {
                picked = Some(i);
            }
            if resp.pressed || resp.hovered {
                ui.c.fill_rrect(*cell, inner_h / 2.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
                palette::TEXT
            } else {
                palette::TEXT_DIM
            }
        };
        ui.c.text(name, *cell, &st.color(color).center());
    }
    picked
}

/// A label with a switch at the row's right end; the whole row toggles.
/// `None` = not known yet: drawn off and dimmed, and it doesn't click.
/// Returns true when clicked.
fn switch_row(ui: &mut Ui, id: u64, r: Rect, label: &str, on: Option<bool>, accent: Color) -> bool {
    let hit = Rect::new(r.x - ROW_BLEED, r.y, r.w + 2.0 * ROW_BLEED, r.h);
    let resp = if on.is_some() { ui.button(id, hit) } else { Default::default() };
    if resp.hovered || resp.pressed {
        ui.c.fill_rrect(hit.inset(0.0, 1.0), 8.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
    }
    let sw = Rect::new(r.right() - SWITCH_W, r.center().y - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
    ui.c.text(label, Rect::new(r.x, r.y, (sw.x - 10.0 - r.x).max(0.0), r.h), &TextStyle::body());
    let alpha = if on.is_some() { 1.0 } else { 0.5 };
    ui.c.with_alpha(alpha, |c| draw_switch(c, sw, on == Some(true), accent));
    resp.clicked
}

fn draw_switch(c: &mut crate::gfx::Canvas, r: Rect, on: bool, accent: Color) {
    c.fill_rrect(r, r.h / 2.0, if on { accent } else { palette::TRACK });
    let d = r.h - 2.0 * KNOB_INSET;
    let x = if on { r.right() - KNOB_INSET - d / 2.0 } else { r.x + KNOB_INSET + d / 2.0 };
    c.fill_circle(Point::new(x, r.center().y), d / 2.0, palette::ON_ACCENT);
}
