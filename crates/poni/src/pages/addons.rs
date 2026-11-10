//! Add-ons page, the "+" at the end of the tab row: features only the people
//! who switch them on get (`poni_core::addons`). Most download then; Script
//! reader, System, Health, Stocks, IELTS and Meeting notes are already in
//! the app and only show their pages (System's readings, Health's steps and
//! Stocks' prices also run only while they are on; IELTS only while a
//! session runs, Meeting notes only while it records).
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
//! once Oyenza Studio Recorder is installed ([`rows`]). From seven rows each
//! is one line: the name, then the line after it.
//!
//! A Studio Pro add-on's row says Pro where its size was, for everyone, so
//! people know what Pro brings before they buy it. Without Pro its switch
//! gives way to a +, and a click opens what it does ([`locked`], [`draw_about`]).
//! The Downloader's tools come only with Pro ([`needs_pro`]).

use crate::commands::Command;
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_GAP, PILL_SMALL};
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
/// Where a Pro add-on's page sends people: Pro's features and prices.
const PRICING_URL: &str = "https://studio.oyenza.com/#pricing";

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
/// (`AppState::recorder_ready`), Downloader only where it's offered
/// (`AppState::downloader_offered`).
fn rows(st: &AppState) -> Vec<&'static Addon> {
    OFFERED
        .iter()
        .copied()
        .filter(|a| a.id != addons::SCREEN_RECORDER.id || st.recorder_ready())
        .filter(|a| a.id != addons::DOWNLOADER.id || st.downloader_offered)
        .collect()
}

/// The Pro add-on whose tab goes when Pro does, however it was left
/// (`AppState::downloader_on`): the Downloader, whose tools download only
/// for Pro.
fn needs_pro(a: &Addon) -> bool {
    a.id == addons::DOWNLOADER.id
}

/// A Pro add-on this account can't switch on: its row opens what it does
/// ([`draw_about`]). One left on from before Pro ended still switches off.
fn locked(st: &AppState, a: &Addon) -> bool {
    a.pro && st.studio_pro() != Some(true) && !status(st, a).is_on()
}

/// AI usage or the Downloader is switched on: the island asks the server
/// about Pro as soon as it's signed in (at start, or just connected), so
/// their tabs don't wait for this page to open.
pub fn pro_wanted(st: &AppState) -> bool {
    st.settings.ai_usage_on(true) || (st.downloader_offered && st.settings.addon_on(addons::DOWNLOADER.id))
}

fn row_h(n: usize) -> f32 {
    ((MAX_H - ROWS_Y) / n.max(1) as f32).floor().min(ROW_H)
}

/// Where `a` stands: Screen recorder follows the recorder's install
/// (`Settings::recorder_on`); AI usage comes with Pro until switched off
/// (`Settings::ai_usage_on`); the Downloader is off without Pro, even
/// switched on before; the others, their own switch and download.
fn status(st: &AppState, a: &Addon) -> Status {
    if a.id == addons::SCREEN_RECORDER.id || a.id == addons::AI_USAGE.id {
        let on = if a.id == addons::AI_USAGE.id { st.ai_usage_on() } else { st.record_on() };
        if on {
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
        "caffeine" => Icon::Coffee,
        "notifications" => Icon::Bell,
        "recorder" => Icon::Recorder,
        "downloader" => Icon::Download,
        "ielts" => Icon::Mic,
        "notes" => Icon::Notes,
        "ai" => Icon::Ai,
        _ => Icon::Addons,
    }
}

/// What a Pro add-on does, on the page its row opens.
fn about(a: &Addon) -> &'static str {
    match a.id {
        "ai" => "Each AI service on this computer gets a card: your plan, each limit with when it resets, and what's left. Codex, Claude Code, Cursor, Gemini CLI, GitHub Copilot and more. Sign-ins stay here, and nothing about your usage goes to Oyenza.",
        "notes" => "One click and Oyen listens to your call in Meet, Zoom, Teams or WhatsApp, with no bot joining, then writes up the summary, the decisions and who does what, with the whole transcript. The recording goes to Oyenza's server and is deleted once your notes are saved.",
        "ielts" => "Oyen asks IELTS Speaking questions out loud, listens to your answers and gives you a band. Your answers go to Oyenza's server to be scored and are deleted once that's done. IELTS® is a trademark of the IELTS partners. Not official IELTS material, not endorsed by them.",
        "downloader" => "Paste a link, pick Video or Sound, and watch it come in to Downloads and the Shelf. Only what you have the right to keep.",
        _ => a.blurb,
    }
}

/// A Pro add-on's page: its name, what it does and where to see Pro.
fn draw_about(ui: &mut Ui, st: &mut AppState, area: Rect, a: &Addon) {
    let accent = st.accent;
    let c = Point::new(area.x + BADGE / 2.0, area.y + 4.0 + BADGE / 2.0);
    ui.c.fill_circle(c, BADGE / 2.0, accent.with_alpha(0.18));
    ui.c.icon(icon(a), Rect::from_center(c, 18.0, 18.0), accent);
    let x = area.x + TEXT_X;
    let w = (area.w - TEXT_X).clamp(0.0, 460.0);
    let title_w = ui.c.measure(a.title, &TextStyle::headline(), w.max(1.0)).0.ceil();
    ui.c.text(a.title, Rect::new(x, area.y + 3.0, title_w + 1.0, 20.0), &TextStyle::headline());
    let tag = Rect::new(x + title_w + 8.0, area.y + 4.0, 36.0, 18.0);
    ui.c.fill_rrect(tag, 9.0, crate::oyen::ORANGE.with_alpha(0.2));
    ui.c.text("PRO", tag, &TextStyle::caption().weight(Weight::Bold).center().color(crate::oyen::ORANGE));
    ui.c.text(a.blurb, Rect::new(x, area.y + 23.0, w, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    let para = TextStyle::footnote().wrap().top();
    let body = about(a);
    let h = ui.c.measure(body, &para, w).1;
    ui.c.text(body, Rect::new(x, area.y + 50.0, w, h + 1.0), &para);
    let y = area.y + 50.0 + h + 8.0;
    ui.c.text("Studio Pro is in early access and can't be bought yet.", Rect::new(x, y, w, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    let y = y + 25.0;
    let see = "See Studio Pro";
    let see_w = ui.pill_width(see, false, true);
    if ui.pill(wid("addons.about.see"), Rect::new(x, y, see_w, PILL_SMALL), see, None, PillStyle::Primary(accent)).clicked {
        st.cmd(Command::OpenUrl(PRICING_URL.to_string()));
    }
    let back = "Back";
    let back_w = ui.pill_width(back, false, true);
    if ui.pill(wid("addons.about.back"), Rect::new(x + see_w + PILL_GAP, y, back_w, PILL_SMALL), back, None, PillStyle::Neutral).clicked {
        st.ui.addon_about = None;
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if let Some(a) = st.ui.addon_about.and_then(Addon::find).filter(|a| locked(st, a)) {
        draw_about(ui, st, area, a);
        return;
    }
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
            if locked(st, a) {
                st.ui.addon_about = Some(a.id);
            } else if !on && !a.pinned() {
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

    // A switch, or the + of a Pro add-on that opens what it does.
    let edge = if locked(st, a) {
        ui.c.icon(Icon::Plus, Rect::from_center(Point::new(r.right() - 8.0, cy), 14.0, 14.0), palette::TEXT_DIM);
        r.right() - 16.0
    } else {
        let sw = Rect::new(r.right() - SWITCH_W, cy - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
        crate::pages::settings::draw_switch(ui.c, sw, status.is_on(), accent);
        sw.x
    };
    // The size, while there is a choice to make.
    let mut text_right = edge - 16.0;
    if status == Status::Off {
        let size = if a.pro { "Pro".to_string() } else if a.downloads() { fmt_bytes(a.size()) } else { "Built in".to_string() };
        let style = if a.pro { TextStyle::footnote().weight(Weight::Semibold).color(crate::oyen::ORANGE).right() } else { TextStyle::footnote().color(palette::TEXT_DIM).right() };
        let w = ui.c.measure(&size, &style, 200.0).0.ceil();
        ui.c.text(&size, Rect::new(edge - 10.0 - w, r.y, w, r.h), &style);
        text_right = edge - 10.0 - w - 16.0;
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
        // Downloader is offered: the recorder and the Downloader join.
        st.recorder = true;
        st.store.recorder_island = true;
        st.downloader_offered = true;
        st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
        st.oyenza.pro = Some(true);
        assert_eq!(rows(&st).len(), without + 2);
        assert!(height(&st) <= MAX_H, "the window never grows for this page");
        let h = row_h(rows(&st).len());
        assert!(h < TWO_LINES, "that many rows are a line each");
        assert!(BADGE_ONE_LINE.min(h - 2.0) + 2.0 <= h, "{h} DIPs: the badges keep apart");
    }

    #[test]
    fn pro_add_ons_show_for_everyone_and_say_what_they_do_without_pro() {
        use poni_core::addons::{AI_USAGE, DOWNLOADER, IELTS, MEETING_NOTES, STOCKS};
        let connected = || OyenzaLink::Connected { account: "a\u{2022}\u{2022}\u{2022}@example.com".into() };
        let listed = |st: &AppState, id: &str| rows(st).iter().any(|a| a.id == id);
        let mut st = AppState::new(Settings::default(), Store::default());
        st.downloader_offered = true;
        // The Downloader switched on before, its tools in place.
        st.settings.set_addon(DOWNLOADER.id, true);
        st.addons.insert(DOWNLOADER.id, Status::Ready);
        for (link, pro) in [(OyenzaLink::Off, None), (connected(), None), (connected(), Some(false))] {
            st.oyenza.link = link;
            st.oyenza.pro = pro;
            for a in [&AI_USAGE, &MEETING_NOTES, &IELTS, &DOWNLOADER] {
                assert!(listed(&st, a.id), "{} listed without Pro ({pro:?})", a.id);
                assert_eq!(status(&st, a), Status::Off, "{} stays off", a.id);
                assert!(locked(&st, a), "{}: its row opens what it does ({pro:?})", a.id);
                assert_ne!(about(a), a.blurb, "{} says more than its row", a.id);
            }
            assert!(!st.ai_usage_on());
        }
        // With Pro they switch: AI usage is on already, until switched off.
        st.oyenza.link = connected();
        st.oyenza.pro = Some(true);
        assert_eq!(status(&st, &DOWNLOADER), Status::Ready);
        assert_eq!(status(&st, &AI_USAGE), Status::Ready);
        assert!(st.ai_usage_on() && !locked(&st, &AI_USAGE) && !locked(&st, &IELTS));
        st.settings.set_addon(AI_USAGE.id, false);
        assert_eq!(status(&st, &AI_USAGE), Status::Off);
        assert!(!st.ai_usage_on() && !locked(&st, &AI_USAGE));
        // One left on when Pro ended still switches off.
        st.settings.set_addon(IELTS.id, true);
        st.addons.insert(IELTS.id, Status::Ready);
        st.oyenza.pro = Some(false);
        assert!(!locked(&st, &IELTS));
        // Not where it isn't offered (the Store build), Pro or not.
        st.downloader_offered = false;
        assert!(!listed(&st, DOWNLOADER.id));
        // A free add-on: listed and on without Pro.
        st.oyenza = Default::default();
        st.settings.set_addon(STOCKS.id, true);
        st.addons.insert(STOCKS.id, Status::Ready);
        assert_eq!(status(&st, &STOCKS), Status::Ready);
        assert!(listed(&st, STOCKS.id) && !locked(&st, &STOCKS));
    }

    #[test]
    fn the_island_asks_about_pro_at_sign_in_for_what_needs_it() {
        use poni_core::addons::{AI_USAGE, DOWNLOADER};
        let mut st = AppState::new(Settings::default(), Store::default());
        assert!(pro_wanted(&st), "AI usage's tab waits on it");
        st.settings.set_addon(AI_USAGE.id, false);
        assert!(!pro_wanted(&st), "nothing to ask about at sign-in");
        st.downloader_offered = true;
        st.settings.set_addon(DOWNLOADER.id, true);
        assert!(pro_wanted(&st));
        st.downloader_offered = false;
        assert!(!pro_wanted(&st), "not offered here");
    }
}
