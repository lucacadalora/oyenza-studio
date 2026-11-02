//! Add-ons page, the "+" at the end of the tab row: features only the people
//! who switch them on get (`poni_core::addons`). Most download then; Script
//! reader, System, Health, Stocks, Notifications and IELTS are already in
//! the app and only show their pages (System's readings, Health's steps,
//! Stocks' prices and Notifications' listening also run only while they are
//! on; IELTS only while a session runs).
//!
//! One row per add-on: its icon, its name, what it does (or where its
//! download stands, with a bar while it comes in), its size while it is off,
//! ("Built in" for one with nothing to download), and a switch. The whole
//! row toggles, as in Settings: on downloads the add-on and starts it, off
//! stops it and deletes its files
//! (`Command::SaveSettings`; the app passes the change to `services::addons`).
//! One whose files this build can't get yet stays off and says so
//! (`Status::Unpinned`).
//! Both apps list every add-on ([`OFFERED`]) but Screen recorder, which shows
//! once Oyenza Studio Recorder is installed, and Notifications, which only
//! Windows has ([`rows`]). Eight rows (the recorder's with them) are one
//! line each: the name, then the line after it.
//!
//! A Studio Pro add-on's row says Pro where its size was. IELTS and the
//! Downloader are listed only for an account with Pro ([`rows`]); the
//! Downloader's tools come only with it ([`needs_pro`]).

use crate::commands::Command;
use crate::gfx::{TextStyle, Weight};
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
/// and from seven each row is one line (rows under [`TWO_LINES`]).
const ROW_H: f32 = 45.0;
const MAX_H: f32 = 256.0;
const BADGE: f32 = 36.0;
const BADGE_COMPACT: f32 = 30.0;
const TEXT_X: f32 = 48.0;
/// A row's hover wash reaches this far past its contents (as in Settings).
const ROW_BLEED: f32 = 8.0;
const SWITCH_W: f32 = 30.0;
const SWITCH_H: f32 = 18.0;
const BAR_H: f32 = 3.0;
/// Rows shorter than this (seven or more of them: 32 DIPs and less, where two
/// lines would overlap) put the line after the name.
const TWO_LINES: f32 = 33.0;
const BADGE_ONE_LINE: f32 = 24.0;
/// Between the name and the line after it, on a one-line row.
const LINE_GAP: f32 = 10.0;

/// The add-ons this app has, in the page's order: all of them, on Windows
/// and on the Mac (whose Prayer times use macOS's location and sounds).
pub const OFFERED: &[&Addon] = &addons::ALL;
/// The lines name the Mac ("on this Mac") and leave out Windows' reasons.
const MAC: bool = cfg!(target_os = "macos");

pub fn height(st: &AppState) -> f32 {
    let n = rows(st).len();
    ROWS_Y + n as f32 * row_h(n)
}

/// The page's rows: every add-on offered, Screen recorder only once Oyenza
/// Studio Recorder is installed and has spoken to the island
/// (`AppState::recorder_ready`), Notifications only on Windows (a Mac has
/// no way to read other apps' notifications), IELTS only for an account the
/// server says has Studio Pro, Downloader only where it's offered
/// (`AppState::downloader_offered`).
fn rows(st: &AppState) -> Vec<&'static Addon> {
    OFFERED
        .iter()
        .copied()
        .filter(|a| a.id != addons::SCREEN_RECORDER.id || st.recorder_ready())
        .filter(|a| a.id != addons::NOTIFICATIONS.id || cfg!(windows))
        .filter(|a| a.id != addons::DOWNLOADER.id || (st.downloader_offered && st.studio_pro() == Some(true)))
        .filter(|a| a.id != addons::IELTS.id || st.studio_pro() == Some(true))
        .collect()
}

/// The Pro add-on whose switch works only with Studio Pro, since its tab
/// needs it (`AppState::downloader_on`): the Downloader, whose tools download
/// only for Pro. Meeting notes switches on for anyone, and its page says what
/// Pro is.
fn needs_pro(a: &Addon) -> bool {
    a.id == addons::DOWNLOADER.id
}

/// It is switched on where it's offered: the island asks the server about
/// Pro as soon as it's signed in (at start, or just connected), so its tab
/// doesn't wait for this page to open.
pub fn pro_wanted(st: &AppState) -> bool {
    st.downloader_offered && st.settings.addon_on(addons::DOWNLOADER.id)
}

fn row_h(n: usize) -> f32 {
    ((MAX_H - ROWS_Y) / n.max(1) as f32).floor().min(ROW_H)
}

/// Where `a` stands: Screen recorder follows the recorder's install
/// (`Settings::recorder_on`); the Downloader is off without Pro, even
/// switched on before; the others, their own switch and download.
fn status(st: &AppState, a: &Addon) -> Status {
    if a.id == addons::SCREEN_RECORDER.id {
        if st.record_on() {
            Status::Ready
        } else {
            Status::Off
        }
    } else if needs_pro(a) && st.studio_pro() != Some(true) {
        Status::Off
    } else {
        st.addon(a.id)
    }
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
        "notifications" => Icon::Bell,
        "recorder" => Icon::Recorder,
        "downloader" => Icon::Download,
        "ielts" => Icon::Mic,
        "notes" => Icon::Notes,
        _ => Icon::Addons,
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let intro = "Each one stays off until you switch it on, and switching off deletes any download.";
    ui.c.text(intro, Rect::new(area.x, area.y, area.w, INTRO_H), &TextStyle::footnote().color(palette::TEXT_DIM));
    // The last row's hover wash meets the Pill card's rounded corners.
    let card = super::card::pill_card(st, area);
    let rows = rows(st);
    let h = row_h(rows.len());
    for (i, a) in rows.iter().enumerate() {
        let row = Rect::new(area.x, area.y + ROWS_Y + i as f32 * h, area.w, h);
        if addon_row(ui, st, row, i, a, card) {
            let on = status(st, a).is_on();
            if !on && !a.pinned() {
                // This build can't get its files: it stays off, and its line says why.
                st.addons.insert(a.id, Status::Unpinned);
            } else {
                st.settings.set_addon(a.id, !on);
                st.cmd(Command::SaveSettings);
            }
        }
    }
}

/// Returns true when the row was clicked. `card`: the Pill card (`card::pill_card`).
fn addon_row(ui: &mut Ui, st: &AppState, r: Rect, i: usize, a: &Addon, card: Option<(Rect, f32)>) -> bool {
    let status = status(st, a);
    let accent = st.accent;
    let hit = Rect::new(r.x - ROW_BLEED, r.y + 2.0, r.w + 2.0 * ROW_BLEED, r.h - 4.0);
    let resp = ui.button(wid_n("addons.row", i as u64), hit);
    if resp.hovered || resp.pressed {
        super::card::fill_box(ui, card, hit, 10.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
    }
    let cy = r.center().y;
    // Six rows sit closer: a smaller badge, the text centred in the row;
    // eight are a line each.
    let one_line = r.h < TWO_LINES;
    let (badge, dy) = if one_line {
        // Eleven rows are 20 DIPs: the badge keeps a DIP clear above and below.
        (BADGE_ONE_LINE.min(r.h - 2.0), 0.0)
    } else if r.h < ROW_H {
        (BADGE_COMPACT, ((r.h - ROW_H) / 2.0).round())
    } else {
        (BADGE, 0.0)
    };
    ui.c.fill_circle(Point::new(r.x + BADGE / 2.0, cy), badge / 2.0, accent.with_alpha(0.18));
    let glyph = if one_line { 14.0f32.min(r.h - 8.0) } else { 18.0 };
    ui.c.icon(icon(a), Rect::from_center(Point::new(r.x + BADGE / 2.0, cy), glyph, glyph), accent);

    let sw = Rect::new(r.right() - SWITCH_W, cy - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
    crate::pages::settings::draw_switch(ui.c, sw, status.is_on(), accent);
    // The size, while there is a choice to make.
    let mut text_right = sw.x - 16.0;
    if status == Status::Off {
        let size = if a.pro { "Pro".to_string() } else if a.downloads() { fmt_bytes(a.size()) } else { "Built in".to_string() };
        let style = if a.pro { TextStyle::footnote().weight(Weight::Semibold).color(crate::oyen::ORANGE).right() } else { TextStyle::footnote().color(palette::TEXT_DIM).right() };
        let w = ui.c.measure(&size, &style, 200.0).0.ceil();
        ui.c.text(&size, Rect::new(sw.x - 10.0 - w, r.y, w, r.h), &style);
        text_right = sw.x - 10.0 - w - 16.0;
    }

    let x = r.x + TEXT_X;
    let w = (text_right - x).max(0.0);
    let line_color = if status == Status::Failed { palette::ORANGE } else { palette::TEXT_DIM };
    // Screen recorder came on with the recorder, not by a click here: say so.
    let line = if a.id == addons::SCREEN_RECORDER.id && status == Status::Ready {
        "On, since Oyenza Studio Recorder is installed".to_string()
    } else {
        status.line(a, MAC)
    };
    let bar_y = if one_line {
        // The name, then the line after it while there is room.
        let title_w = ui.c.measure(a.title, &TextStyle::headline(), w.max(1.0)).0.ceil().min(w);
        ui.c.text(a.title, Rect::new(x, r.y, title_w + 1.0, r.h), &TextStyle::headline());
        let lx = x + title_w + LINE_GAP;
        if text_right > lx {
            ui.c.text(&line, Rect::new(lx, r.y, text_right - lx, r.h), &TextStyle::footnote().color(line_color));
        }
        r.bottom() - BAR_H - 1.0
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

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::settings::Settings;
    use poni_core::store::Store;
    use crate::state::OyenzaLink;

    #[test]
    fn every_row_offered_fits_the_page() {
        let mut st = AppState::new(Settings::default(), Store::default());
        let without = rows(&st).len();
        // Oyenza Studio Recorder installed, and signed in with Pro where the
        // Downloader is offered: the recorder, IELTS and the Downloader join.
        st.recorder = true;
        st.store.recorder_island = true;
        st.downloader_offered = true;
        st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
        st.oyenza.pro = Some(true);
        assert_eq!(rows(&st).len(), without + 3);
        assert!(height(&st) <= MAX_H, "the window never grows for this page");
        let h = row_h(rows(&st).len());
        assert!(h < TWO_LINES, "that many rows are a line each");
        assert!(BADGE_ONE_LINE.min(h - 2.0) + 2.0 <= h, "{h} DIPs: the badges keep apart");
    }

    #[test]
    fn the_downloader_is_listed_and_switches_only_with_studio_pro() {
        use poni_core::addons::{DOWNLOADER, NOTIFICATIONS};
        let mut st = AppState::new(Settings::default(), Store::default());
        st.downloader_offered = true;
        assert!(!pro_wanted(&st), "nothing to ask about at sign-in");
        // The Downloader switched on before, its tools in place.
        st.settings.set_addon(DOWNLOADER.id, true);
        st.addons.insert(DOWNLOADER.id, Status::Ready);
        assert!(pro_wanted(&st), "asked at sign-in, though not listed yet");
        let listed = |st: &AppState| rows(st).iter().any(|a| a.id == DOWNLOADER.id);
        for (link, pro) in [(OyenzaLink::Off, None), (OyenzaLink::Connected { account: "a•••@example.com".into() }, None), (OyenzaLink::Connected { account: "a•••@example.com".into() }, Some(false))] {
            st.oyenza.link = link;
            st.oyenza.pro = pro;
            assert!(!listed(&st), "not listed without Pro ({pro:?})");
            assert_eq!(status(&st, &DOWNLOADER), Status::Off, "stays off");
        }
        st.oyenza.pro = Some(true);
        assert!(listed(&st));
        assert_eq!(status(&st, &DOWNLOADER), Status::Ready);
        // Not where it isn't offered (the Store build, the Mac), Pro or not.
        st.downloader_offered = false;
        assert!(!listed(&st) && !pro_wanted(&st));
        // Notifications is free (Luca, 28 Oct): listed and on without Pro.
        st.oyenza = Default::default();
        st.settings.set_addon(NOTIFICATIONS.id, true);
        st.addons.insert(NOTIFICATIONS.id, Status::Ready);
        assert_eq!(status(&st, &NOTIFICATIONS), Status::Ready);
        assert_eq!(rows(&st).iter().any(|a| a.id == NOTIFICATIONS.id), cfg!(windows));
    }
}
