//! Today page: the day at a glance.
//!
//! Signed in to Oyenza, with Google Calendar or Gmail switched on for it in
//! oyenza.app (`poni_core::agenda`): the date on the left, with the logos of
//! the apps it came from, and on the right the meeting on now (or next, within
//! the hour) with Join for a meeting link, then the rest of the day: all-day
//! events, events and flights by time, and the bills due today.
//!
//! Otherwise the date (weekday, big day number, month/year, ISO week) and a
//! compact Monday-first month calendar, and beside them one card: sign in
//! (signed out), pick the apps in oyenza.app (none switched on), getting the
//! day, or try again. A server without the day (from before it, or with it
//! switched off) keeps the page as it was: a 2×2 grid of stat tiles with rings
//! (CPU, memory, disk, battery or uptime). The header leaves the date out on
//! this page, so the page always shows it (the calendar title carries it when
//! the big date block doesn't fit).
//!
//! Stats are sampled once a second by the app only while the tiles show; the
//! day is asked for by the Oyenza worker only while this page is open. The
//! page itself never asks for continuous frames: the day's "48 min left"
//! moves once a minute.

use crate::commands::{Command, OyenzaCmd};
use crate::gfx::{Image, TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::state::{AgendaState, AppState, OyenzaLink, PowerState, SysStats};
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_SMALL};
use poni_core::agenda::{self, Mark, Source};
use poni_core::datetime::{civil_from_days, days_from_civil, fmt_clock, month_grid, weekday_from_days, LocalDateTime, MONTH_SHORT, WEEKDAY_SHORT};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::sysmon::Module;

const HEIGHT: f32 = 164.0;

/// Below this content width the big date block is dropped (the calendar
/// title then names the day) so the tiles keep their room.
const COMPACT_BELOW: f32 = 560.0;
const DATE_W: f32 = 104.0;
const DATE_GAP: f32 = 12.0;
const CELL_W: f32 = 21.0;
const CELL_H: f32 = 20.0;
const INITIALS_H: f32 = 18.0;
/// Month title above the grid in compact mode.
const TITLE_H: f32 = 22.0;
const BLOCK_GAP: f32 = 18.0;
const TILE_GAP: f32 = 8.0;
const TILE_RADIUS: f32 = 16.0;
const RING_R: f32 = 16.0;
const RING_W: f32 = 4.0;
/// Neutral track shared by every progress ring in the app.
const RING_TRACK: Color = Color::white(0.14);
const RING_ICON: f32 = 12.0;

const WEEKDAY_LONG: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTH_LONG: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
/// Monday-first initials.
const INITIALS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

/// Where oyenza.app switches the apps on (Connected apps › Oyenza Studio).
const APPS_URL: &str = "https://oyenza.app/settings/integrations";
/// The day's rows, and the card of the meeting on now.
const ROW_H: f32 = 23.0;
const FOCUS_H: f32 = 64.0;
const FOCUS_GAP: f32 = 8.0;
const ROW_INSET: f32 = 12.0;
const MARK: f32 = 16.0;

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// What the right of the page holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// The stat tiles: signed in to a server without the day.
    Tiles,
    /// The day, from the apps switched on (the whole page).
    Day,
    Card(Ask),
}

/// The card beside the month.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    SignIn,
    /// Signing in: the code is on the Oyenza tab.
    Code,
    /// Signed in with no app switched on for Today.
    NoApps,
    Waiting,
    Failed,
}

fn side(st: &AppState) -> Side {
    match st.oyenza.link {
        OyenzaLink::Off => Side::Card(Ask::SignIn),
        OyenzaLink::Connecting { .. } => Side::Card(Ask::Code),
        OyenzaLink::Connected { .. } => {
            let o = &st.oyenza;
            match (&o.agenda, o.agenda_state) {
                (_, AgendaState::Unavailable) => Side::Tiles,
                // Yesterday's answer, the page left open past midnight: the new day is on its way.
                (Some(a), _) if a.day == st.day_key() => {
                    if a.apps.any_on() {
                        Side::Day
                    } else {
                        Side::Card(Ask::NoApps)
                    }
                }
                (_, AgendaState::Failed) => Side::Card(Ask::Failed),
                _ => Side::Card(Ask::Waiting),
            }
        }
    }
}

/// The stat tiles show (the app samples the stats only meanwhile).
pub fn shows_tiles(st: &AppState) -> bool {
    side(st) == Side::Tiles
}

/// The day's "48 min left" and what's still to come move with the clock: a
/// look at the next minute while the day shows.
pub fn next_wake(st: &AppState) -> Option<f64> {
    (side(st) == Side::Day).then(|| st.now + (60 - st.local.second.min(59)) as f64)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    // Fixed layout height: the island's spring may still be opening.
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    let side = side(st);
    if side == Side::Day {
        draw_day(ui, st, area);
        return;
    }
    let compact = area.w < COMPACT_BELOW;
    let cal_w = CELL_W * 7.0;
    // The date block and the calendar share one top edge, so the weekday
    // name sits on the same line as the M–S initials.
    let cal_top = area.y + ((area.h - calendar_block_h(&st.local, compact)) / 2.0).max(0.0);
    let mut x = area.x;
    if !compact {
        draw_date(ui, st, Rect::new(x, area.y, DATE_W, area.h), cal_top);
        x += DATE_W + DATE_GAP;
    }
    draw_calendar(ui, st, Rect::new(x, area.y, cal_w, area.h), cal_top, compact);
    x += cal_w + BLOCK_GAP;
    let right = Rect::new(x, area.y, (area.right() - x).max(0.0), area.h);
    // The bottom-right tile (or the card) shares the Pill card's corner.
    let card = super::card::pill_card(st, area);
    match side {
        Side::Card(ask) => draw_ask(ui, st, right, card, ask),
        _ => draw_tiles(ui, st, right, card),
    }
}

// ------------------------------------------------------------------ the card

/// The card beside the month: the apps' logos, a title, a line, and a button.
/// Heights are measured, so a wider font wraps instead of clipping.
fn draw_ask(ui: &mut Ui, st: &mut AppState, r: Rect, card: Option<(Rect, f32)>, ask: Ask) {
    if r.w < 120.0 {
        return;
    }
    super::card::fill_box(ui, card, r, TILE_RADIUS, palette::SURFACE);
    let inner = r.inset(16.0, 16.0);
    let (title, body) = match ask {
        Ask::SignIn => ("Your day, right here", "Your meetings from Google Calendar and flights from Gmail, through your Oyenza account."),
        Ask::Code => ("Your day, right here", "Approve the sign-in code in oyenza.app, then your day shows here."),
        Ask::NoApps => ("Pick what Today shows", "Switch on Google Calendar or Gmail in oyenza.app, under Connected apps › Oyenza Studio."),
        Ask::Waiting => ("Getting your day…", "From the apps you switched on in oyenza.app."),
        Ask::Failed => ("Couldn't get your day", "Check the connection, then try again."),
    };
    let mut y = inner.y;
    if matches!(ask, Ask::SignIn | Ask::Code | Ask::NoApps) {
        let mut x = inner.x;
        for m in [Mark::Calendar, Mark::Gmail] {
            if let Some(img) = logo(m) {
                ui.c.image(&img, Rect::new(x, y, 28.0, 28.0), 7.0);
            }
            x += 34.0;
        }
        y += 28.0 + 10.0;
    }
    let title_st = TextStyle::headline().wrap().top();
    let body_st = TextStyle::footnote().color(palette::TEXT_DIM).wrap().top();
    let (_, title_h) = ui.c.measure(title, &title_st, inner.w);
    let (_, body_h) = ui.c.measure(body, &body_st, inner.w);
    ui.c.text(title, Rect::new(inner.x, y, inner.w, title_h + 1.0), &title_st);
    y += title_h + 3.0;
    ui.c.text(body, Rect::new(inner.x, y, inner.w, body_h + 1.0), &body_st);
    y += body_h + 10.0;
    let button = match ask {
        Ask::SignIn => Some(("Sign in with Oyenza", None, PillStyle::Primary(palette::ORANGE))),
        Ask::Code => Some(("Show the sign-in code", None, PillStyle::Primary(palette::ORANGE))),
        Ask::NoApps => Some(("Open oyenza.app", Some(Icon::Open), PillStyle::Primary(palette::ORANGE))),
        Ask::Failed => Some(("Try again", None, PillStyle::Utility)),
        Ask::Waiting => None,
    };
    let Some((label, icon, style)) = button else { return };
    // On the card's bottom edge, unless the text runs that far.
    let by = (inner.bottom() - PILL_SMALL).max(y);
    let w = ui.pill_width(label, icon.is_some(), true).min(inner.w);
    if !ui.pill(wid("today.ask"), Rect::new(inner.x, by, w, PILL_SMALL), label, icon, style).clicked {
        return;
    }
    match ask {
        Ask::SignIn => {
            st.oyenza.from_today = true;
            st.cmd(Command::Oyenza(OyenzaCmd::SignIn));
        }
        Ask::Code => st.cmd(Command::SetPage(PageKind::Oyenza)),
        Ask::NoApps => st.cmd(Command::OpenUrl(APPS_URL.into())),
        Ask::Failed => st.cmd(Command::Oyenza(OyenzaCmd::RefreshDay)),
        Ask::Waiting => {}
    }
}

// ------------------------------------------------------------------ the day

/// The date and where the day came from on the left; the meeting on now and
/// the rest of the day on the right.
fn draw_day(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let Some(answer) = st.oyenza.agenda.clone() else { return };
    let left = Rect::new(area.x, area.y, DATE_W, area.h);
    draw_date(ui, st, left, area.y + 4.0);
    // The apps it came from, then when (or what went wrong).
    let mut x = left.x;
    for (m, s) in [(Mark::Calendar, answer.apps.googlecalendar), (Mark::Gmail, answer.apps.gmail)] {
        if s == Source::Off {
            continue;
        }
        if let Some(img) = logo(m) {
            ui.c.image(&img, Rect::new(x, area.y + 126.0, MARK, MARK), 4.0);
        }
        x += MARK + 5.0;
    }
    let (status, warn) = day_status(st, &answer.apps);
    let status_st = TextStyle::caption().color(if warn { palette::ORANGE } else { palette::TEXT_FAINT });
    ui.c.text(&status, Rect::new(left.x, area.y + 146.0, DATE_W + DATE_GAP, 16.0), &status_st);

    let x0 = area.x + DATE_W + DATE_GAP;
    let r = Rect::new(x0, area.y, (area.right() - x0).max(0.0), area.h);
    let show = st.settings.oyenza_show_amounts || st.oyenza.reveal;
    let now_min = st.local.hour as i32 * 60 + st.local.minute as i32;
    let view = agenda::view(&answer, &st.oyenza.bills, &st.oyenza.bills_today, &st.day_key(), now_min, show);
    if view.is_empty() {
        let title_st = TextStyle::headline().center();
        let body_st = TextStyle::footnote().color(palette::TEXT_DIM).center();
        let cy = r.center().y;
        ui.c.text("Nothing else today", Rect::new(r.x, cy - 20.0, r.w, 20.0), &title_st);
        ui.c.text("Your calendar is clear from here.", Rect::new(r.x, cy + 2.0, r.w, 18.0), &body_st);
        return;
    }
    let mut y = r.y;
    if let Some(f) = &view.focus {
        draw_focus(ui, st, Rect::new(r.x, y, r.w, FOCUS_H), f);
        y += FOCUS_H + FOCUS_GAP;
    }
    let fits = ((r.bottom() - y) / ROW_H).floor().max(0.0) as usize;
    let more = view.rows.len().saturating_sub(fits);
    // One row gives way to "+N more today" when they don't all fit.
    let shown = if more > 0 { fits.saturating_sub(1) } else { view.rows.len() };
    for row in view.rows.iter().take(shown) {
        draw_row(ui, Rect::new(r.x, y, r.w, ROW_H), row);
        y += ROW_H;
    }
    if more > 0 && fits > 0 {
        let rest = view.rows.len() - shown;
        let line = format!("+{rest} more today, in oyenza.app and your calendar");
        ui.c.text(&line, Rect::new(r.x + ROW_INSET, y, r.w - 2.0 * ROW_INSET, ROW_H), &TextStyle::footnote().color(palette::TEXT_FAINT));
    }
}

/// "Updated 10:10", or what went wrong with an app switched on (in orange).
fn day_status(st: &AppState, apps: &agenda::Sources) -> (String, bool) {
    let said = |s: Source, name: &str| match s {
        Source::Failed | Source::Other => Some(format!("{name} didn't load")),
        Source::NotConnected => Some(format!("Connect {name}")),
        _ => None,
    };
    if let Some(line) = said(apps.googlecalendar, "Calendar").or_else(|| said(apps.gmail, "Gmail")) {
        return (line, true);
    }
    if st.oyenza.agenda_state == AgendaState::Failed {
        return ("Offline".into(), true);
    }
    (st.oyenza.agenda_unix.map(|at| super::health::updated_text(st, at)).unwrap_or_default(), false)
}

/// The meeting on now (or next): when, how long, its title, where, and Join.
fn draw_focus(ui: &mut Ui, st: &mut AppState, r: Rect, f: &agenda::Focus) {
    ui.c.fill_rrect(r, 14.0, palette::SURFACE);
    let x = r.x + ROW_INSET;
    let join_w = f.join.as_ref().map(|_| ui.pill_width("Join", true, true)).unwrap_or(0.0);
    let text_right = r.right() - ROW_INSET - if join_w > 0.0 { join_w + 10.0 } else { 0.0 };
    let w = (text_right - x).max(0.0);
    let tag = if f.now { "NOW" } else { "NEXT" };
    let tag_st = TextStyle::caption().weight(Weight::Bold).color(st.accent);
    let tag_w = ui.c.measure(tag, &tag_st, w).0.ceil();
    ui.c.text(tag, Rect::new(x, r.y + 8.0, tag_w + 1.0, 15.0), &tag_st);
    let line = format!(" · {} · {}", f.when, f.left);
    ui.c.text(&line, Rect::new(x + tag_w, r.y + 8.0, (w - tag_w).max(0.0), 15.0), &TextStyle::caption().tabular().color(palette::TEXT_DIM));
    ui.c.text(&f.title, Rect::new(x, r.y + 23.0, w, 21.0), &TextStyle::headline());
    if let Some(place) = &f.place {
        if let Some(img) = logo(Mark::Calendar) {
            ui.c.image(&img, Rect::new(x, r.y + 47.0, 14.0, 14.0), 3.5);
        }
        ui.c.text(place, Rect::new(x + 19.0, r.y + 44.0, (w - 19.0).max(0.0), 20.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    }
    if let Some(link) = &f.join {
        let b = Rect::new(r.right() - ROW_INSET - join_w, r.center().y - PILL_SMALL / 2.0, join_w, PILL_SMALL);
        if ui.pill(wid("today.join"), b, "Join", Some(Icon::Open), PillStyle::Primary(st.accent)).clicked {
            st.cmd(Command::OpenUrl(link.clone()));
        }
    }
}

/// One row: when, the app it came from, what, and a detail on the right.
fn draw_row(ui: &mut Ui, r: Rect, row: &agenda::Row) {
    let x = r.x + ROW_INSET;
    ui.c.text(&row.when, Rect::new(x, r.y, 44.0, r.h), &TextStyle::footnote().tabular().color(palette::TEXT_DIM));
    let mark = Rect::new(x + 42.0, r.center().y - MARK / 2.0, MARK, MARK);
    match row.mark {
        Mark::Bill => {
            ui.c.fill_circle(mark.center(), MARK / 2.0, palette::ORANGE.with_alpha(0.2));
            ui.c.icon(Icon::Card, Rect::from_center(mark.center(), 11.0, 11.0), palette::ORANGE);
        }
        m => {
            if let Some(img) = logo(m) {
                ui.c.image(&img, mark, 4.0);
            }
        }
    }
    let detail_st = TextStyle::footnote().tabular().color(palette::TEXT_DIM).right();
    let right = r.right() - ROW_INSET;
    let detail_w = ui.c.measure(&row.detail, &detail_st, r.w * 0.4).0.ceil().min(r.w * 0.4);
    let title_x = mark.right() + 8.0;
    let title_w = (right - detail_w - 10.0 - title_x).max(0.0);
    ui.c.text(&row.title, Rect::new(title_x, r.y, title_w, r.h), &TextStyle::body());
    if !row.detail.is_empty() {
        ui.c.text(&row.detail, Rect::new(right - detail_w, r.y, detail_w, r.h), &detail_st);
    }
}

// ------------------------------------------------------------------ logos

/// Google Calendar's and Gmail's own icons, the ones oyenza.app shows (64 px,
/// about 5.6 KB together), decoded the first time a page draws one.
const CALENDAR_PNG: &[u8] = include_bytes!("../../res/apps/googlecalendar.png");
const GMAIL_PNG: &[u8] = include_bytes!("../../res/apps/gmail.png");

thread_local! {
    static LOGOS: std::cell::RefCell<Option<[Option<Image>; 2]>> = const { std::cell::RefCell::new(None) };
}

fn logo(m: Mark) -> Option<Image> {
    let i = match m {
        Mark::Calendar => 0,
        Mark::Gmail => 1,
        Mark::Bill => return None,
    };
    LOGOS.with(|l| l.borrow_mut().get_or_insert_with(|| [decode(CALENDAR_PNG), decode(GMAIL_PNG)])[i].clone())
}

#[cfg(windows)]
fn decode(png: &[u8]) -> Option<Image> {
    crate::services::imaging::decode_bytes(png, 64).ok()
}

#[cfg(target_os = "macos")]
fn decode(png: &[u8]) -> Option<Image> {
    crate::services::thumbs::decode(png, 64)
}


// ------------------------------------------------------------------ date

/// `top` = the calendar block's top edge. The weekday line uses the same
/// 16-high band and 11 DIP caps as the initials row, so both share a baseline.
fn draw_date(ui: &mut Ui, st: &AppState, r: Rect, top: f32) {
    let t = st.local;
    if !valid_date(&t) {
        return;
    }
    let wd = t.weekday() as usize % 7;
    let y = top;
    ui.c.text(&WEEKDAY_LONG[wd].to_uppercase(), Rect::new(r.x, y, r.w, INITIALS_H - 2.0), &TextStyle::caption().weight(Weight::Bold).color(st.accent));
    ui.c.text(&t.day.to_string(), Rect::new(r.x - 2.0, y + 14.0, r.w + 2.0, 58.0), &TextStyle::hero().weight(Weight::Semibold));
    ui.c.text(
        &format!("{} {}", MONTH_LONG[(t.month - 1) as usize], t.year),
        Rect::new(r.x, y + 74.0, r.w, 18.0),
        &TextStyle::body().color(palette::TEXT_DIM),
    );
    ui.c.text(&format!("Week {}", iso_week(&t)), Rect::new(r.x, y + 92.0, r.w, 16.0), &TextStyle::footnote().color(palette::TEXT_FAINT));
}

// ------------------------------------------------------------------ calendar

/// Rows with at least one day in them (4–6).
fn week_rows(t: &LocalDateTime) -> usize {
    month_grid(t.year, t.month).iter().filter(|w| w.iter().any(|d| d.is_some())).count().max(1)
}

/// Height of the calendar block: optional title, initials, week rows.
fn calendar_block_h(t: &LocalDateTime, with_title: bool) -> f32 {
    let rows = if valid_date(t) { week_rows(t) } else { 6 };
    (if with_title { TITLE_H } else { 0.0 }) + INITIALS_H + rows as f32 * CELL_H
}

/// `top` = the block's top edge (shared with the date block).
fn draw_calendar(ui: &mut Ui, st: &AppState, r: Rect, top: f32, with_title: bool) {
    let t = st.local;
    if !valid_date(&t) {
        return;
    }
    let grid = month_grid(t.year, t.month);
    let rows = week_rows(&t);
    let mut y = top;

    if with_title {
        // Without the big date block the title names the day as well.
        let st_title = TextStyle::body().weight(Weight::Semibold);
        let tr = Rect::new(r.x + 4.0, y, r.w - 4.0, TITLE_H - 4.0);
        let day = format!("{} {} {}", WEEKDAY_SHORT[t.weekday() as usize % 7], t.day, MONTH_LONG[(t.month - 1) as usize]);
        let full = format!("{day} {}", t.year);
        let title = if ui.c.measure(&full, &st_title, 10_000.0).0 <= tr.w { full } else { day };
        ui.c.text(&title, tr, &st_title);
        y += TITLE_H;
    }

    // Every initial in one tone; weekend days are told apart by the dimmer numbers.
    let initials_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_FAINT).center();
    for (col, s) in INITIALS.iter().enumerate() {
        ui.c.text(s, Rect::new(r.x + col as f32 * CELL_W, y, CELL_W, INITIALS_H - 2.0), &initials_st);
    }
    y += INITIALS_H;

    let day_st = TextStyle::footnote().tabular().center();
    let today_ink = ink_on(st.accent);
    for week in grid.iter().take(rows) {
        for (col, day) in week.iter().enumerate() {
            let Some(d) = *day else { continue };
            let cell = Rect::new(r.x + col as f32 * CELL_W, y, CELL_W, CELL_H);
            let label = d.to_string();
            if d == t.day {
                ui.c.fill_circle(cell.center(), 9.5, st.accent);
                ui.c.text(&label, cell, &day_st.weight(Weight::Bold).color(today_ink));
            } else {
                let c = if col >= 5 { palette::TEXT_DIM } else { Color::white(0.9) };
                ui.c.text(&label, cell, &day_st.color(c));
            }
        }
        y += CELL_H;
    }
}

// ------------------------------------------------------------------ tiles

struct Tile {
    icon: Icon,
    label: &'static str,
    /// Ring fill fraction; `None` = track only (no meaningful fraction).
    frac: Option<f32>,
    color: Color,
    value: String,
    /// Preferred detail line, and a shorter fallback when it doesn't fit.
    detail: String,
    detail_short: String,
}

fn draw_tiles(ui: &mut Ui, st: &mut AppState, r: Rect, card: Option<(Rect, f32)>) {
    if r.w < 120.0 {
        return;
    }
    let s = &st.stats;
    let battery = st.power.has_battery;
    let tiles = [cpu_tile(s), memory_tile(s), disk_tile(s), if battery { battery_tile(&st.power) } else { uptime_tile(st) }];
    // With System on, each ring opens its module on the System page (uptime
    // lives under CPU there); without it the tiles are only to read.
    let modules = [Module::Cpu, Module::Memory, Module::Drives, if battery { Module::Battery } else { Module::Cpu }];
    let system = st.system_on();
    let rows = r.rows(2, TILE_GAP);
    for (i, tile) in tiles.iter().enumerate() {
        let row = rows[i / 2];
        let cell = row.columns(2, TILE_GAP)[i % 2];
        let resp = system.then(|| ui.button(wid_n("today-tile", i as u64), cell));
        draw_tile(ui, cell, tile, resp.is_some_and(|r| r.hovered), card);
        if resp.is_some_and(|r| r.clicked) {
            st.cmd(Command::OpenSystem(modules[i]));
        }
    }
}

fn draw_tile(ui: &mut Ui, r: Rect, t: &Tile, hovered: bool, card: Option<(Rect, f32)>) {
    super::card::fill_box(ui, card, r, TILE_RADIUS, if hovered { palette::SURFACE_HOVER } else { palette::SURFACE });
    let cy = r.center().y;
    let c = Point::new(r.x + 12.0 + RING_R, cy);
    match t.frac {
        Some(f) => ui.c.ring(c, RING_R, RING_W, f.clamp(0.0, 1.0), RING_TRACK, t.color),
        None => ui.c.stroke_circle(c, RING_R, RING_TRACK, RING_W),
    }
    ui.c.icon(t.icon, Rect::from_center(c, RING_ICON, RING_ICON), Color::white(0.72));

    // Text sits on the tile's fill, so the faintest tone here is TEXT_DIM.
    let tx = c.x + RING_R + 11.0;
    let tw = (r.right() - 10.0 - tx).max(0.0);
    ui.c.text(t.label, Rect::new(tx, cy - 26.0, tw, 15.0), &TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_DIM));
    let value_col = if t.value == PLACEHOLDER { palette::TEXT_DIM } else { palette::TEXT };
    ui.c.text(&t.value, Rect::new(tx, cy - 12.0, tw, 24.0), &TextStyle::value().tabular().color(value_col));
    if !t.detail.is_empty() {
        let st = TextStyle::caption().color(palette::TEXT_DIM);
        let fits = ui.c.measure(&t.detail, &st, 10_000.0).0 <= tw;
        let detail = if fits || t.detail_short.is_empty() { &t.detail } else { &t.detail_short };
        ui.c.text(detail, Rect::new(tx, cy + 11.0, tw, 15.0), &st);
    }
}

const PLACEHOLDER: &str = "–";

/// Ring colour for a "how full" fraction: white, then orange, then red.
fn load_color(f: f32) -> Color {
    if f >= 0.9 {
        palette::RED
    } else if f >= 0.75 {
        palette::ORANGE
    } else {
        Color::white(0.92)
    }
}

fn placeholder(icon: Icon, label: &'static str) -> Tile {
    Tile { icon, label, frac: None, color: Color::white(0.92), value: PLACEHOLDER.into(), detail: String::new(), detail_short: String::new() }
}

fn cpu_tile(s: &SysStats) -> Tile {
    if !s.valid || !s.cpu.is_finite() {
        return placeholder(Icon::Cpu, "CPU");
    }
    let f = s.cpu.clamp(0.0, 1.0);
    let detail = match f {
        f if f < 0.10 => "Idle",
        f if f < 0.35 => "Light load",
        f if f < 0.70 => "Moderate",
        f if f < 0.90 => "Busy",
        _ => "Very busy",
    };
    Tile { icon: Icon::Cpu, label: "CPU", frac: Some(f), color: load_color(f), value: pct(f), detail: detail.into(), detail_short: String::new() }
}

fn memory_tile(s: &SysStats) -> Tile {
    if !s.valid || s.mem_total == 0 {
        return placeholder(Icon::Memory, "Memory");
    }
    let f = (s.mem_used as f64 / s.mem_total as f64).clamp(0.0, 1.0) as f32;
    let detail = format!("{} / {}", gb_one_decimal(s.mem_used), fmt_bytes(s.mem_total));
    let short = gb_one_decimal(s.mem_used) + " GB";
    Tile { icon: Icon::Memory, label: "Memory", frac: Some(f), color: load_color(f), value: pct(f), detail, detail_short: short }
}

fn disk_tile(s: &SysStats) -> Tile {
    if !s.valid || s.disk_total == 0 {
        return placeholder(Icon::Disk, "Disk");
    }
    let free = s.disk_free.min(s.disk_total);
    let used = 1.0 - (free as f64 / s.disk_total as f64) as f32;
    let detail = format!("free of {}", fmt_bytes(s.disk_total));
    Tile { icon: Icon::Disk, label: "Disk", frac: Some(used), color: load_color(used), value: fmt_bytes(free), detail, detail_short: "free".into() }
}

fn battery_tile(p: &PowerState) -> Tile {
    let percent = p.percent.min(100);
    let f = percent as f32 / 100.0;
    let (detail, color) = if p.charging {
        ("Charging", palette::GREEN)
    } else if p.on_ac {
        (if percent >= 99 { "Fully charged" } else { "Plugged in" }, palette::GREEN)
    } else if percent <= 20 {
        ("Low battery", palette::RED)
    } else if p.saver {
        ("Battery saver", palette::YELLOW)
    } else {
        ("On battery", Color::white(0.92))
    };
    let short = if p.charging || p.on_ac { "Plugged in" } else { "On battery" };
    Tile {
        icon: if p.charging { Icon::Bolt } else { Icon::Battery },
        label: "Battery",
        frac: Some(f),
        color,
        value: format!("{percent}%"),
        detail: detail.into(),
        detail_short: short.into(),
    }
}

fn uptime_tile(st: &AppState) -> Tile {
    let s = &st.stats;
    if !s.valid {
        return placeholder(Icon::Clock, "Uptime");
    }
    let (detail, detail_short) = boot_label(&st.local, s.uptime_s, st.settings.use_24h).unwrap_or_default();
    Tile { icon: Icon::Clock, label: "Uptime", frac: None, color: Color::white(0.92), value: fmt_uptime(s.uptime_s), detail, detail_short }
}

// ------------------------------------------------------------------ formatting

fn pct(f: f32) -> String {
    format!("{}%", (f * 100.0).round().clamp(0.0, 100.0) as i32)
}

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// "6.1" (GiB, one decimal).
fn gb_one_decimal(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / GIB)
}

/// Compact size with unit: "640 MB", "8.4 GB", "212 GB", "1.8 TB"
/// (binary units, as Explorer shows them).
fn fmt_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / GIB;
    if gb >= 1000.0 {
        let tb = gb / 1024.0;
        if tb >= 10.0 {
            format!("{tb:.0} TB")
        } else {
            format!("{tb:.1} TB")
        }
    } else if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// "3d 4h", "5h 12m", "42m".
fn fmt_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs / 3600) % 24, (secs / 60) % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// When the PC booted: ("Since 09:14", "Since today"), ("Since Mon 09:14",
/// "Since Mon") or ("Since 3 Oct", same) — full text plus a narrow fallback.
fn boot_label(now: &LocalDateTime, uptime_s: u64, use_24h: bool) -> Option<(String, String)> {
    if !valid_date(now) {
        return None;
    }
    let today = days_from_civil(now.year, now.month, now.day);
    let now_s = today * 86_400 + now.hour as i64 * 3600 + now.minute as i64 * 60 + now.second as i64;
    let boot_s = now_s - i64::try_from(uptime_s).ok()?;
    let boot_day = boot_s.div_euclid(86_400);
    let sod = boot_s.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(boot_day);
    let boot = LocalDateTime { year, month, day, hour: (sod / 3600) as u8, minute: ((sod / 60) % 60) as u8, second: (sod % 60) as u8 };
    let clock = fmt_clock(&boot, use_24h);
    let ago = today - boot_day;
    Some(if ago <= 0 {
        (format!("Since {clock}"), "Since today".to_string())
    } else if ago < 7 {
        let wd = WEEKDAY_SHORT[weekday_from_days(boot_day) as usize % 7];
        (format!("Since {wd} {clock}"), format!("Since {wd}"))
    } else {
        let s = format!("Since {} {}", day, MONTH_SHORT[(month.clamp(1, 12) - 1) as usize]);
        (s.clone(), s)
    })
}

/// ISO-8601 week number (weeks start Monday; week 1 holds the first Thursday).
fn iso_week(t: &LocalDateTime) -> i64 {
    let z = days_from_civil(t.year, t.month, t.day);
    let mon0 = (weekday_from_days(z) as i64 + 6) % 7; // 0 = Monday
    let thursday = z - mon0 + 3;
    let (ty, _, _) = civil_from_days(thursday);
    (thursday - days_from_civil(ty, 1, 1)) / 7 + 1
}

/// The app fills `local` from GetLocalTime; guard against the zeroed default.
fn valid_date(t: &LocalDateTime) -> bool {
    (1..=12).contains(&t.month) && t.day >= 1
}

/// Text colour that stays legible on an accent fill.
fn ink_on(fill: Color) -> Color {
    if fill.luminance() > 0.6 {
        palette::ON_ACCENT_DARK
    } else {
        palette::ON_ACCENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> LocalDateTime {
        LocalDateTime { year, month, day, hour, minute, second: 0 }
    }

    #[test]
    fn iso_weeks() {
        assert_eq!(iso_week(&dt(2026, 10, 24, 0, 0)), 43);
        // 1 Jan 2027 is a Friday: still week 53 of 2026.
        assert_eq!(iso_week(&dt(2027, 1, 1, 0, 0)), 53);
        assert_eq!(iso_week(&dt(2027, 1, 4, 0, 0)), 1);
        // 29 Dec 2025 (Monday) already belongs to week 1 of 2026.
        assert_eq!(iso_week(&dt(2025, 12, 29, 0, 0)), 1);
    }

    #[test]
    fn sizes_and_uptime() {
        assert_eq!(fmt_bytes(212 * 1024 * 1024 * 1024), "212 GB");
        assert_eq!(fmt_bytes((8.4 * GIB) as u64), "8.4 GB");
        assert_eq!(fmt_bytes(640 * 1024 * 1024), "640 MB");
        assert_eq!(fmt_bytes((1.8 * 1024.0 * GIB) as u64), "1.8 TB");
        assert_eq!(gb_one_decimal((6.1 * GIB) as u64), "6.1");
        assert_eq!(fmt_bytes((15.8 * GIB) as u64), "16 GB");
        assert_eq!(fmt_uptime(3 * 86_400 + 4 * 3600 + 59), "3d 4h");
        assert_eq!(fmt_uptime(5 * 3600 + 12 * 60), "5h 12m");
        assert_eq!(fmt_uptime(42 * 60 + 5), "42m");
        assert_eq!(pct(0.234), "23%");
    }

    #[test]
    fn boot_labels() {
        let now = dt(2026, 10, 24, 10, 30); // Saturday
        let (full, short) = boot_label(&now, 3600, true).unwrap();
        assert_eq!((full.as_str(), short.as_str()), ("Since 09:30", "Since today"));
        let (full, short) = boot_label(&now, 5 * 86_400, true).unwrap();
        assert_eq!((full.as_str(), short.as_str()), ("Since Mon 10:30", "Since Mon"));
        let (full, _) = boot_label(&now, 30 * 86_400, true).unwrap();
        assert_eq!(full, "Since 24 Sep");
        assert!(boot_label(&LocalDateTime::default(), 10, true).is_none());
    }
}
