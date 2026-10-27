//! Today page: your days at a glance.
//!
//! Signed in to Oyenza, with Google Calendar or Gmail switched on for it in
//! oyenza.app (`poni_core::agenda`): on the left the date, the apps it reads
//! (one switched off shows dimmed) and five weeks from this Monday, with a dot
//! under each date for what's on it (a calendar event, a flight from Gmail, a
//! bill due); on the right the schedule from now on: the meeting on now (or
//! next, within the hour) with Join, the rest of today, then each date that
//! holds something, to the end of the five weeks. The list scrolls with the
//! wheel or trackpad and the grid rings the date at its top; clicking a date
//! starts the list there, clicking an event opens it in Google Calendar. An
//! app switched off, not connected or not loading says so on today's first
//! lines, with the way to fix it; so do weeks the person hasn't agreed to in
//! oyenza.app yet.
//!
//! Otherwise the same date and weeks (without dots), and beside them one card:
//! sign in (signed out), pick the apps in oyenza.app (none switched on),
//! getting the day, or try again. A server without the day (from before it,
//! or with it switched off) keeps the page's stat tiles (CPU, memory, disk,
//! battery or uptime). On an island narrower than [`COMPACT_BELOW`] the weeks
//! make way. The header leaves the date out on this page, so the page always
//! shows it.
//!
//! Stats are sampled once a second by the app only while the tiles show; the
//! days are asked for by the Oyenza worker only while this page is open. The
//! page itself never asks for continuous frames: "48 min left" and what's
//! still to come move once a minute, and the list jumps to where it scrolls.

use crate::commands::{Command, OyenzaCmd};
use crate::gfx::{Image, TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::state::{AgendaState, AppState, OyenzaLink, PowerState, SysStats};
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_SMALL};
use poni_core::agenda::{self, Answer, Mark, Source, Sources, Weeks};
use poni_core::datetime::{civil_from_days, days_from_civil, fmt_clock, weekday_from_days, LocalDateTime, MONTH_SHORT, WEEKDAY_SHORT};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::sysmon::Module;

const HEIGHT: f32 = 200.0;

/// Below this content width the weeks make way for the list (the island can
/// be made as narrow as 480).
const COMPACT_BELOW: f32 = 480.0;
const CELL_W: f32 = 23.0;
const GRID_W: f32 = 7.0 * CELL_W;
/// A week's row: the date, and its dots under it.
const CELL_H: f32 = 26.0;
/// The initials row, under the weekday and the date.
const GRID_Y: f32 = 48.0;
const INITIALS_H: f32 = 16.0;
const COL_GAP: f32 = 20.0;
const LOGO: f32 = 14.0;
const TILE_GAP: f32 = 8.0;
const TILE_RADIUS: f32 = 16.0;
const RING_R: f32 = 16.0;
const RING_W: f32 = 4.0;
/// Neutral track shared by every progress ring in the app.
const RING_TRACK: Color = Color::white(0.14);
const RING_ICON: f32 = 12.0;
/// A flight from Gmail: its dot.
const GMAIL_RED: Color = Color::hex(0xEA4335);
/// A calendar event: its dot, Google Calendar's blue in every theme (the
/// accent can be orange, as in Oyen, like a bill's dot).
const CALENDAR_BLUE: Color = Color::hex(0x4285F4);

const WEEKDAY_LONG: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTH_LONG: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
/// Monday-first initials.
const INITIALS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

/// Where oyenza.app switches the apps on and agrees to the weeks (Connected
/// apps › Oyenza Studio), opened at Studio's switches.
const APPS_URL: &str = "https://oyenza.app/settings/integrations?show=studio";

/// [`APPS_URL`] naming the app asked for: connected there, it switches on for
/// Studio by itself.
fn apps_url(app: Option<Mark>) -> String {
    match app {
        Some(Mark::Calendar) => format!("{APPS_URL}&app=googlecalendar"),
        Some(Mark::Gmail) => format!("{APPS_URL}&app=gmail"),
        _ => APPS_URL.to_string(),
    }
}
/// The list: a date's heading, a row, the card of the meeting on now, and a
/// line that says what's wrong with the way to fix it.
const HEAD_H: f32 = 22.0;
const ROW_H: f32 = 24.0;
const NOTICE_H: f32 = 32.0;
const FOCUS_H: f32 = 64.0;
const FOCUS_GAP: f32 = 6.0;
const DAY_GAP: f32 = 4.0;
const ROW_INSET: f32 = 8.0;
const MARK: f32 = 16.0;
/// How far one wheel notch scrolls the list.
const WHEEL_STEP: f32 = 2.0 * ROW_H;
const FADE: f32 = 22.0;
const FADE_STEPS: usize = 8;
/// A page drawn again after this long starts at today.
const STALE_S: f64 = 1.0;

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// What the right of the page holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// The stat tiles: signed in to a server without the day.
    Tiles,
    /// The days, from the apps switched on.
    Day,
    Card(Ask),
}

/// The card beside the weeks.
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
/// look at the next minute while the days show.
pub fn next_wake(st: &AppState) -> Option<f64> {
    (side(st) == Side::Day).then(|| st.now + (60 - st.local.second.min(59)) as f64)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    // Fixed layout height: the island's spring may still be opening.
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    let compact = area.w < COMPACT_BELOW;
    let x = if compact { area.x } else { area.x + GRID_W + COL_GAP };
    let right = Rect::new(x, area.y, (area.right() - x).max(0.0), area.h);
    let left = Rect::new(area.x, area.y, GRID_W, area.h);
    let side = side(st);
    if side == Side::Day {
        draw_days(ui, st, left, right, compact);
        return;
    }
    if !compact {
        draw_weeks_grid(ui, st, left, None, None);
    }
    // The bottom-right tile (or the card) shares the Pill card's corner.
    let card = super::card::pill_card(st, area);
    match side {
        Side::Card(ask) => draw_ask(ui, st, right, card, ask),
        _ => draw_tiles(ui, st, right, card),
    }
}

// ------------------------------------------------------------------ the card

/// The card beside the weeks: the apps' logos, a title, a line, and a button.
/// Heights are measured, so a wider font wraps instead of clipping.
fn draw_ask(ui: &mut Ui, st: &mut AppState, r: Rect, card: Option<(Rect, f32)>, ask: Ask) {
    if r.w < 120.0 {
        return;
    }
    super::card::fill_box(ui, card, r, TILE_RADIUS, palette::SURFACE);
    let inner = r.inset(16.0, 16.0);
    let (title, body) = match ask {
        Ask::SignIn => ("Your weeks, right here", "Your meetings from Google Calendar and flights from Gmail, today and the next five weeks, through your Oyenza account."),
        Ask::Code => ("Your weeks, right here", "Approve the sign-in code in oyenza.app, then your days show here."),
        Ask::NoApps => ("Pick what Today shows", "Switch on Google Calendar or Gmail in oyenza.app, under Connected apps › Oyenza Studio."),
        Ask::Waiting => ("Getting your days…", "From the apps you switched on in oyenza.app."),
        Ask::Failed => ("Couldn't get your days", "Check the connection, then try again."),
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

// ------------------------------------------------------------------ the weeks

/// The date, the apps Today reads (with the days), and five weeks from this
/// Monday: today filled, a dot under each date for what's on it, and the date
/// at the top of the list ringed. With the days, a date from today to the last
/// one the answer speaks for can be clicked: returns the one clicked.
fn draw_weeks_grid(ui: &mut Ui, st: &AppState, r: Rect, days: Option<(&Weeks, Sources)>, ring: Option<i64>) -> Option<i64> {
    let t = st.local;
    if !valid_date(&t) {
        return None;
    }
    let today = days_from_civil(t.year, t.month, t.day);
    let weekday = WEEKDAY_LONG[weekday_from_days(today) as usize % 7].to_uppercase();
    ui.c.text(&weekday, Rect::new(r.x, r.y, r.w, 16.0), &TextStyle::caption().weight(Weight::Bold).color(st.accent));
    let date = format!("{} {}", t.day, MONTH_LONG[(t.month.clamp(1, 12) - 1) as usize]);
    ui.c.text(&date, Rect::new(r.x, r.y + 16.0, r.w, 24.0), &TextStyle::value());
    if let Some((_, apps)) = days {
        // The apps it reads, at the top right; one switched off for Studio, dimmed.
        let mut x = r.right() - LOGO;
        for (m, s) in [(Mark::Gmail, apps.gmail), (Mark::Calendar, apps.googlecalendar)] {
            if let Some(img) = logo(m) {
                let lr = Rect::new(x, r.y + 1.0, LOGO, LOGO);
                ui.c.with_alpha(if s == Source::Off { 0.35 } else { 1.0 }, |c| c.image(&img, lr, 3.5));
            }
            x -= LOGO + 5.0;
        }
    }

    // Every initial in one tone; weekend days are told apart by the dimmer numbers.
    let initials_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_FAINT).center();
    for (col, s) in INITIALS.iter().enumerate() {
        ui.c.text(s, Rect::new(r.x + col as f32 * CELL_W, r.y + GRID_Y, CELL_W, INITIALS_H), &initials_st);
    }
    let top = r.y + GRID_Y + INITIALS_H;
    let start = agenda::grid_start(today);
    let num_st = TextStyle::footnote().tabular().center();
    let step = Color::white(0.2);
    let mut clicked = None;
    for i in 0..agenda::GRID_DAYS {
        let day = start + i;
        let (row, col) = ((i / 7) as f32, (i % 7) as usize);
        let cell = Rect::new(r.x + col as f32 * CELL_W, top + row * CELL_H, CELL_W, CELL_H);
        let num = Rect::new(cell.x, cell.y, cell.w, 20.0);
        let c = Point::new(num.center().x, num.center().y);
        let (_, _, d) = civil_from_days(day);
        let open = days.is_some_and(|(w, _)| day >= today && day <= w.last);
        let hovered = open && {
            let resp = ui.button(wid_n("today.date", i as u64), num);
            if resp.clicked {
                clicked = Some(day);
            }
            resp.hovered
        };
        let label = d.to_string();
        if day == today {
            ui.c.fill_circle(c, 9.5, st.accent);
            ui.c.text(&label, num, &num_st.weight(Weight::Bold).color(ink_on(st.accent)));
        } else {
            if ring == Some(day) {
                ui.c.stroke_circle(c, 9.5, Color::white(0.75), 1.5);
            } else if hovered {
                ui.c.fill_circle(c, 9.5, palette::SURFACE_HOVER);
            }
            let ink = if day < today {
                Color::white(0.32)
            } else if col >= 5 {
                palette::TEXT_DIM
            } else {
                Color::white(0.9)
            };
            let st_num = if d == 1 { num_st.weight(Weight::Bold) } else { num_st };
            ui.c.text(&label, num, &st_num.color(ink));
        }
        // What's on: a dot for each kind, never for days gone by.
        if let Some(k) = days.and_then(|(w, _)| w.kinds.get(&day)).filter(|_| day >= today) {
            let dots: Vec<Color> = [(k.calendar, CALENDAR_BLUE), (k.flight, GMAIL_RED), (k.bill, palette::ORANGE)].into_iter().filter(|(on, _)| *on).map(|(_, c)| c).collect();
            for (j, col) in dots.iter().enumerate() {
                let x = c.x - (dots.len() as f32 - 1.0) * 2.5 + j as f32 * 5.0;
                ui.c.fill_circle(Point::new(x, cell.y + 22.0), 1.5, *col);
            }
        }
        // Where a month starts: a step line between its 1st and the days before it.
        if d == 1 && i > 0 {
            let y = cell.y - 1.5;
            if row > 0.0 {
                ui.c.line(Point::new(cell.x, y), Point::new(r.x + GRID_W, y), step, 1.0);
            }
            if col > 0 {
                ui.c.line(Point::new(cell.x, y), Point::new(cell.x, y + CELL_H), step, 1.0);
                if row < 4.0 {
                    ui.c.line(Point::new(r.x, y + CELL_H), Point::new(cell.x, y + CELL_H), step, 1.0);
                }
            }
        }
    }
    clicked
}

// ------------------------------------------------------------------ the days

/// What a line of the list is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    /// A date's heading ("TODAY · MON 26 OCT").
    Head,
    Focus,
    Notice(Notice),
    /// "Nothing else today", "Nothing planned", "Nothing else in the next five weeks".
    Empty(&'static str),
    /// `Weeks::days[day].rows[row]`.
    Row(usize, usize),
}

/// Something on today's first lines that needs the person, with its button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notice {
    CalendarOff,
    Connect(Mark),
    Failed(Mark),
    /// The person hasn't agreed to the weeks in oyenza.app yet.
    Weeks,
}

/// A line placed in the list: its top (from the list's top), height, and date.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placed {
    y: f32,
    h: f32,
    day: i64,
    line: Line,
}

/// What today's first lines say about the apps.
fn notices(a: &Answer) -> Vec<Notice> {
    let mut out = Vec::new();
    match a.apps.googlecalendar {
        Source::Off => out.push(Notice::CalendarOff),
        Source::NotConnected => out.push(Notice::Connect(Mark::Calendar)),
        Source::Failed | Source::Other => out.push(Notice::Failed(Mark::Calendar)),
        Source::On => {}
    }
    // Gmail switched off is a choice: its logo dims, nothing more.
    match a.apps.gmail {
        Source::NotConnected => out.push(Notice::Connect(Mark::Gmail)),
        Source::Failed | Source::Other => out.push(Notice::Failed(Mark::Gmail)),
        Source::On | Source::Off => {}
    }
    out
}

/// The list, line by line: each date's heading, then (today) what needs the
/// person, the meeting on now and what's still to come, then each later date.
fn lay_out(w: &Weeks, a: &Answer) -> Vec<Placed> {
    let mut out = Vec::new();
    let mut y = 0.0;
    let mut put = |y: &mut f32, h: f32, day: i64, line: Line| {
        out.push(Placed { y: *y, h, day, line });
        *y += h;
    };
    let calendar_on = a.apps.googlecalendar == Source::On;
    for (i, d) in w.days.iter().enumerate() {
        put(&mut y, HEAD_H, d.day, Line::Head);
        if d.day == w.today {
            for n in notices(a) {
                put(&mut y, NOTICE_H, d.day, Line::Notice(n));
            }
            if w.focus.is_some() {
                put(&mut y, FOCUS_H + FOCUS_GAP, d.day, Line::Focus);
            } else if d.rows.is_empty() && calendar_on {
                put(&mut y, ROW_H, d.day, Line::Empty(if w.today_had() { "Nothing else today" } else { "Nothing today" }));
            }
        } else if d.rows.is_empty() {
            put(&mut y, ROW_H, d.day, Line::Empty("Nothing planned"));
        }
        for j in 0..d.rows.len() {
            put(&mut y, ROW_H, d.day, Line::Row(i, j));
        }
        y += DAY_GAP;
    }
    if a.weeks_to_allow() {
        put(&mut y, NOTICE_H, w.last, Line::Notice(Notice::Weeks));
    } else if w.days.len() == 1 && w.last > w.today && calendar_on {
        put(&mut y, ROW_H, w.last, Line::Empty("Nothing else in the next five weeks"));
    }
    out
}

fn heading_y(lines: &[Placed], day: i64) -> Option<f32> {
    lines.iter().find(|l| l.line == Line::Head && l.day == day).map(|l| l.y)
}

/// As far as the list scrolls: to its end, or further for a clicked date, so
/// its heading can reach the top.
fn max_scroll(lines: &[Placed], pick: Option<i64>) -> f32 {
    let end = lines.last().map_or(0.0, |l| l.y + l.h);
    let to_pick = pick.and_then(|p| heading_y(lines, p)).unwrap_or(0.0);
    (end - HEIGHT).max(to_pick).max(0.0)
}

/// The date whose lines are at the top of the list.
fn top_day(lines: &[Placed], scroll: f32) -> Option<i64> {
    lines.iter().rev().find(|l| l.line == Line::Head && l.y <= scroll + 0.5).map(|l| l.day)
}

/// The weeks on the left, the list on the right.
fn draw_days(ui: &mut Ui, st: &mut AppState, left: Rect, list: Rect, compact: bool) {
    let Some(answer) = st.oyenza.agenda.clone() else { return };
    // Opened again after a while: from today (a jump just asked for, by a click or a self-test, goes ahead).
    if !st.ui.today_jump && st.now - st.ui.today_drawn > STALE_S {
        st.ui.today_scroll = 0.0;
        st.ui.today_pick = None;
        st.ui.today_jump = false;
    }
    st.ui.today_drawn = st.now;
    let show = st.settings.oyenza_show_amounts || st.oyenza.reveal;
    let now_min = st.local.hour as i32 * 60 + st.local.minute as i32;
    let w = agenda::weeks(&answer, &st.oyenza.bills, &st.oyenza.bills_today, &st.day_key(), now_min, show, st.ui.today_pick);
    let lines = lay_out(&w, &answer);
    let max = max_scroll(&lines, st.ui.today_pick);
    if std::mem::take(&mut st.ui.today_jump) {
        st.ui.today_scroll = st.ui.today_pick.and_then(|p| heading_y(&lines, p)).unwrap_or(0.0);
    }
    let notches = ui.wheel_in(list);
    st.ui.today_scroll = (st.ui.today_scroll - notches * WHEEL_STEP).clamp(0.0, max);

    if !compact {
        let ring = top_day(&lines, st.ui.today_scroll).filter(|d| *d != w.today);
        if let Some(day) = draw_weeks_grid(ui, st, left, Some((&w, answer.apps)), ring) {
            // Today goes back to the top; another date starts the list (from the next frame, which the click asks for).
            st.ui.today_pick = (day != w.today).then_some(day);
            st.ui.today_jump = true;
        }
    }
    draw_list(ui, st, list, &w, &lines, max);
}

fn draw_list(ui: &mut Ui, st: &mut AppState, view: Rect, w: &Weeks, lines: &[Placed], max: f32) {
    let scroll = st.ui.today_scroll;
    ui.c.clip(view);
    for (i, l) in lines.iter().enumerate() {
        let r = Rect::new(view.x, view.y + l.y - scroll, view.w, l.h);
        if r.bottom() <= view.y || r.y >= view.bottom() {
            continue;
        }
        // A button only where the line shows; one cut by an edge keeps its look but takes no clicks.
        let whole = r.y >= view.y - 0.5 && r.bottom() <= view.bottom() + 0.5;
        let hit = intersect(r, view);
        match l.line {
            Line::Head => draw_head(ui, st, r, l.day, w.today),
            Line::Focus => {
                if let Some(f) = &w.focus {
                    let card = Rect::new(r.x, r.y, r.w, FOCUS_H);
                    draw_focus(ui, st, card, f, intersect(card, view), whole);
                }
            }
            Line::Notice(n) => draw_notice(ui, st, r, n, i, whole),
            Line::Empty(text) => ui.c.text(text, Rect::new(r.x + ROW_INSET, r.y, r.w - 2.0 * ROW_INSET, r.h), &TextStyle::body().color(palette::TEXT_FAINT)),
            Line::Row(d, j) => {
                let row = &w.days[d].rows[j];
                let link = row.link.clone().filter(|l| agenda::open_ok(l));
                let hovered = match (&link, hit) {
                    (Some(_), Some(h)) => {
                        let resp = ui.button(wid_n("today.row", (d as u64) << 16 | j as u64), h);
                        if resp.clicked {
                            st.cmd(Command::OpenUrl(link.clone().unwrap_or_default()));
                        }
                        resp.hovered
                    }
                    _ => false,
                };
                draw_row(ui, r, row, hovered);
            }
        }
    }
    // Fade the list out at an edge that scrolls further.
    if scroll > 0.5 {
        fade(ui, Rect::new(view.x, view.y, view.w, 14.0), true);
    }
    if scroll < max - 0.5 && lines.last().is_some_and(|l| l.y + l.h - scroll > view.h) {
        fade(ui, Rect::new(view.x, view.bottom() - FADE, view.w, FADE), false);
    }
    ui.c.unclip();
}

/// "TODAY · MON 26 OCT", with when it was read (or "Offline") on the right.
fn draw_head(ui: &mut Ui, st: &AppState, r: Rect, day: i64, today: i64) {
    let text_r = Rect::new(r.x + ROW_INSET, r.y + 4.0, r.w - 2.0 * ROW_INSET, r.h - 4.0);
    let color = if day == today { st.accent } else { palette::TEXT_DIM };
    ui.c.text(&agenda::day_label(day, today).to_uppercase(), text_r, &TextStyle::caption().weight(Weight::Bold).color(color));
    if day == today {
        let (status, warn) = day_status(st);
        if !status.is_empty() {
            ui.c.text(&status, text_r, &TextStyle::caption().color(if warn { palette::ORANGE } else { palette::TEXT_FAINT }).right());
        }
    }
}

/// "Updated 10:10", or "Offline" (in orange) when the last ask failed.
fn day_status(st: &AppState) -> (String, bool) {
    if st.oyenza.agenda_state == AgendaState::Failed {
        return ("Offline".into(), true);
    }
    (st.oyenza.agenda_unix.map(|at| super::health::updated_text(st, at)).unwrap_or_default(), false)
}

/// A line that says what's wrong, with the way to fix it on its right.
fn draw_notice(ui: &mut Ui, st: &mut AppState, r: Rect, n: Notice, i: usize, whole: bool) {
    let open = |app: Option<Mark>| Command::OpenUrl(apps_url(app));
    let (mark, text, label, icon, style, cmd) = match n {
        Notice::CalendarOff => (Some(Mark::Calendar), "Google Calendar is off for Studio", "Turn on", Some(Icon::Open), PillStyle::Primary(palette::ORANGE), open(Some(Mark::Calendar))),
        Notice::Connect(Mark::Calendar) => (Some(Mark::Calendar), "Connect Google Calendar in oyenza.app", "Connect", Some(Icon::Open), PillStyle::Primary(palette::ORANGE), open(Some(Mark::Calendar))),
        Notice::Connect(m) => (Some(m), "Connect Gmail in oyenza.app", "Connect", Some(Icon::Open), PillStyle::Primary(palette::ORANGE), open(Some(m))),
        Notice::Failed(m) => {
            let text = if m == Mark::Calendar { "Google Calendar didn't load" } else { "Gmail didn't load" };
            (Some(m), text, "Try again", None, PillStyle::Utility, Command::Oyenza(OyenzaCmd::RefreshDay))
        }
        Notice::Weeks => (None, "Your next five weeks can show here too", "Allow", Some(Icon::Open), PillStyle::Primary(palette::ORANGE), open(None)),
    };
    let mut x = r.x + ROW_INSET;
    if let Some(img) = mark.and_then(logo) {
        let off = n == Notice::CalendarOff;
        let lr = Rect::new(x, r.center().y - MARK / 2.0, MARK, MARK);
        ui.c.with_alpha(if off { 0.35 } else { 1.0 }, |c| c.image(&img, lr, 4.0));
        x += MARK + 8.0;
    }
    let pill_w = ui.pill_width(label, icon.is_some(), true);
    let pill = Rect::new(r.right() - ROW_INSET - pill_w, r.center().y - PILL_SMALL / 2.0, pill_w, PILL_SMALL);
    ui.c.text(text, Rect::new(x, r.y, (pill.x - 10.0 - x).max(0.0), r.h), &TextStyle::body());
    if whole && ui.pill(wid_n("today.notice", i as u64), pill, label, icon, style).clicked {
        st.cmd(cmd);
    }
}

/// The meeting on now (or next): when, how long, its title, where, and Join.
/// The card itself opens the event in Google Calendar.
fn draw_focus(ui: &mut Ui, st: &mut AppState, r: Rect, f: &agenda::Focus, hit: Option<Rect>, whole: bool) {
    let link = f.link.clone().filter(|l| agenda::open_ok(l));
    let mut hovered = false;
    if let (Some(link), Some(h)) = (&link, hit) {
        let resp = ui.button(wid("today.focus"), h);
        if resp.clicked {
            st.cmd(Command::OpenUrl(link.clone()));
        }
        hovered = resp.hovered;
    }
    ui.c.fill_rrect(r, 14.0, if hovered { palette::SURFACE_HOVER } else { palette::SURFACE });
    let x = r.x + 12.0;
    let join_w = f.join.as_ref().map(|_| ui.pill_width("Join", true, true)).unwrap_or(0.0);
    let text_right = r.right() - 12.0 - if join_w > 0.0 { join_w + 10.0 } else { 0.0 };
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
    if let Some(join) = &f.join {
        let b = Rect::new(r.right() - 12.0 - join_w, r.center().y - PILL_SMALL / 2.0, join_w, PILL_SMALL);
        if whole && ui.pill(wid("today.join"), b, "Join", Some(Icon::Open), PillStyle::Primary(st.accent)).clicked {
            st.cmd(Command::OpenUrl(join.clone()));
        }
    }
}

/// One row: when, the app it came from, what, and a detail on the right.
fn draw_row(ui: &mut Ui, r: Rect, row: &agenda::Row, hovered: bool) {
    if hovered {
        ui.c.fill_rrect(r, 8.0, palette::SURFACE_HOVER);
    }
    let x = r.x + ROW_INSET;
    ui.c.text(&row.when, Rect::new(x, r.y, 46.0, r.h), &TextStyle::footnote().tabular().color(palette::TEXT_DIM));
    let mark = Rect::new(x + 46.0, r.center().y - MARK / 2.0, MARK, MARK);
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

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// The island's black over the list's edge (none on Glass, which has no
/// solid ground to fade into).
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

    #[test]
    fn studio_links_name_the_app_asked_for() {
        assert_eq!(apps_url(Some(Mark::Calendar)), "https://oyenza.app/settings/integrations?show=studio&app=googlecalendar");
        assert_eq!(apps_url(Some(Mark::Gmail)), "https://oyenza.app/settings/integrations?show=studio&app=gmail");
        assert_eq!(apps_url(None), APPS_URL);
    }
}
