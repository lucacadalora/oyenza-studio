//! Oyenza page, always in the tab row with Oyen himself as its tab.
//!
//! Signed out: what signing in brings, with Sign in with Oyenza.
//!
//! Signing in: the 8-letter code to approve in oyenza.app, "Open
//! oyenza.app" (the browser opens oyenza.app/connect with the code filled
//! in), Cancel, and how long the code still works.
//!
//! Signed in, a live view of Oyenza by day: a row to note a spend ("kopi
//! 25rb", Enter saves it straight into Oyenza; Undo for 10 seconds), a strip
//! of days (the five before yesterday by weekday, Yesterday, Today, Tomorrow,
//! Later), and the chosen day's list. A past day or Today lists everything
//! logged in the space, newest first, with the day's money in and out; a row
//! that lands while the page is open glows for a minute. Tomorrow and Later
//! list the subscriptions and card bills due. The list scrolls; it opens on
//! Today. Sums stay "Rp •••" until "Show amounts" (until the island closes),
//! unless "Show amounts on the island" is on: the island may be on a shared
//! screen.
//!
//! Oyen's moods never read anything here.

use crate::commands::{Command, OyenzaCmd};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::state::{AppState, OyenzaLink};
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_GAP, PILL_SMALL};
use poni_core::datetime::fmt_clock;
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::oyenza::{self, Bill, DayPick, Flow, Txn};
use std::cell::Cell;

/// Signed in: input, status line, the day strip, the day's heading, its list, footer.
const HEIGHT: f32 = 262.0;
/// Signing in: title, line, code box, buttons.
const CODE_HEIGHT: f32 = 156.0;
const INPUT_H: f32 = 34.0;
const STATUS_Y: f32 = 34.0;
const STRIP_Y: f32 = 49.0;
const STRIP_H: f32 = 26.0;
const HEAD_Y: f32 = 80.0;
const LIST_Y: f32 = 112.0;
const LIST_BOTTOM: f32 = 230.0;
const FOOTER_Y: f32 = 234.0;
/// A little under a pill, so half a row peeks out under the fade when a day has more.
const ROW_H: f32 = 26.0;
/// The strip's track inset and each day's padding (it narrows when a wide
/// font needs the room).
const SEG_INSET: f32 = 2.0;
const SEG_PAD: f32 = 10.0;
const SEG_PAD_MIN: f32 = 5.0;
/// The fades where the list is cut off, and how far a wheel notch scrolls.
const FADE_H: f32 = 14.0;
const WHEEL_STEP: f32 = ROW_H * 1.5;
/// Disc and text columns, as the Tasks page's checkbox and text.
const MARK_X: f32 = 18.0;
const TEXT_X: f32 = 36.0;
const DISC_R: f32 = 11.0;
/// Rows bleed this far into the page padding for the glow.
const ROW_BLEED: f32 = 8.0;
const ACCENT: Color = palette::ORANGE;
const PLACEHOLDER: &str = "Note a spend, like kopi 25rb · Enter to save";
/// "Live" while the last answer is this fresh (two missed checks).
const LIVE_S: i64 = 2 * oyenza::ACTIVITY_EVERY_S as i64 + 5;

thread_local! {
    /// The list's drawn scroll and when it was drawn (it eases towards `OyenzaState.scroll`).
    static SCROLL_ANIM: Cell<(f32, f64)> = const { Cell::new((0.0, 0.0)) };
}

/// Signed out the page is as tall as the code that follows, so starting to
/// sign in doesn't move the island.
pub fn height(st: &AppState) -> f32 {
    match st.oyenza.link {
        OyenzaLink::Off | OyenzaLink::Connecting { .. } => CODE_HEIGHT,
        OyenzaLink::Connected { .. } => HEIGHT,
    }
}

/// When the page changes on its own: the code's countdown each second, the
/// Undo lapsing, a short line going, a new row's glow fading.
pub fn next_wake(st: &AppState) -> Option<f64> {
    let o = &st.oyenza;
    let mut next: Option<f64> = None;
    let mut consider = |t: f64| {
        if t > st.now {
            next = Some(next.map_or(t, |n: f64| n.min(t)));
        }
    };
    if matches!(o.link, OyenzaLink::Connecting { .. }) {
        consider(st.now.floor() + 1.0);
    }
    if let Some((_, until)) = &o.saved {
        consider(*until);
    }
    if let Some((_, until)) = &o.flash {
        consider(*until);
    }
    // A glow fades in steps of a second: no frame clock for a minute.
    if let Some(a) = &o.activity {
        if a.days.iter().any(|d| o.seen.glowing_until(d, st.now).is_some()) {
            consider(st.now.floor() + 1.0);
        }
    }
    next
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    match st.oyenza.link.clone() {
        OyenzaLink::Connecting { code, url, until } => draw_code(ui, st, area, &code, &url, until),
        OyenzaLink::Connected { .. } => draw_connected(ui, st, area),
        OyenzaLink::Off => draw_signed_out(ui, st, area),
    }
}

// ---------------------------------------------------------------- signed out

fn draw_signed_out(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let title = "Your money, on the island";
    let body = "Money in, bills due and a quick way to note a spend. Amounts stay hidden until you show them.";
    sign_in_card(ui, st, area, title, body, "oyenza.sign-in");
}

/// The Oyenza app's icon beside what signing in brings, and the button (the
/// IELTS page's too, with its own words). While a code waits, the button
/// shows it on this tab.
pub(crate) fn sign_in_card(ui: &mut Ui, st: &mut AppState, area: Rect, title: &str, body: &str, id: &str) {
    const ICON: f32 = 64.0;
    const GAP: f32 = 20.0;
    let x = area.x + ICON + GAP;
    let w = (area.w - ICON - GAP).clamp(0.0, 420.0);
    let title_st = TextStyle::headline().wrap().top();
    let body_st = TextStyle::footnote().color(palette::TEXT_DIM).wrap().top();
    let (_, title_h) = ui.c.measure(title, &title_st, w);
    let (_, body_h) = ui.c.measure(body, &body_st, w);
    // Text and button as one block, centred on the page like the icon.
    let block = title_h + 3.0 + body_h + 14.0 + PILL_SMALL;
    let mut y = area.y + ((area.h - block) / 2.0).max(0.0);
    crate::oyen::app_icon(ui.c, Point::new(area.x + ICON / 2.0, area.y + area.h / 2.0), ICON);
    ui.c.text(title, Rect::new(x, y, w, title_h + 1.0), &title_st);
    y += title_h + 3.0;
    ui.c.text(body, Rect::new(x, y, w, body_h + 1.0), &body_st);
    y += body_h + 14.0;
    let connecting = matches!(st.oyenza.link, OyenzaLink::Connecting { .. });
    let label = if connecting { "Show the sign-in code" } else { "Sign in with Oyenza" };
    let bw = ui.pill_width(label, false, true).min(w);
    if ui.pill(wid(id), Rect::new(x, y, bw, PILL_SMALL), label, None, PillStyle::Primary(ACCENT)).clicked {
        st.cmd(if connecting { Command::SetPage(crate::pages::PageKind::Oyenza) } else { Command::Oyenza(OyenzaCmd::SignIn) });
    }
}

// ---------------------------------------------------------------- signing in

fn draw_code(ui: &mut Ui, st: &mut AppState, area: Rect, code: &str, url: &str, until: f64) {
    ui.c.text("Sign in to Oyenza", Rect::new(area.x, area.y, area.w, 20.0), &TextStyle::headline());
    ui.c.text(
        if cfg!(windows) {
            "On your phone or this PC, open oyenza.app/connect and enter this code."
        } else {
            "On your phone or this Mac, open oyenza.app/connect and enter this code."
        },
        Rect::new(area.x, area.y + 22.0, area.w, 18.0),
        &TextStyle::footnote().color(palette::TEXT_DIM),
    );
    let code_r = Rect::new(area.x, area.y + 50.0, area.w, 64.0);
    ui.c.fill_rrect(code_r, 14.0, palette::SURFACE);
    ui.c.text(code, code_r, &TextStyle::display().tabular().center());

    let y = area.y + 128.0;
    let open_w = ui.pill_width("Open oyenza.app", true, true);
    let open = Rect::new(area.x, y, open_w, PILL_SMALL);
    if ui.pill(wid("oyenza.open-connect"), open, "Open oyenza.app", Some(Icon::Open), PillStyle::Primary(ACCENT)).clicked {
        st.cmd(Command::OpenUrl(url.to_string()));
    }
    let cancel_w = ui.pill_width("Cancel", false, true);
    let cancel = Rect::new(open.right() + PILL_GAP, y, cancel_w, PILL_SMALL);
    if ui.pill(wid("oyenza.cancel"), cancel, "Cancel", None, PillStyle::Utility).clicked {
        st.cmd(Command::Oyenza(OyenzaCmd::Cancel));
    }
    let left = format!("Code works for {}", oyenza::code_time_left(until - st.now));
    let x = cancel.right() + 12.0;
    ui.c.text(&left, Rect::new(x, y, (area.right() - x).max(0.0), PILL_SMALL), &TextStyle::footnote().color(palette::TEXT_DIM).right());
}

// ---------------------------------------------------------------- signed in

fn draw_connected(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let now = st.now;
    if st.oyenza.saved.as_ref().is_some_and(|(_, until)| now >= *until) && !undo_held(ui) {
        st.oyenza.saved = None;
    }
    if st.oyenza.flash.as_ref().is_some_and(|(_, until)| now >= *until) {
        st.oyenza.flash = None;
    }
    let show = st.settings.oyenza_show_amounts || st.oyenza.reveal;

    // ---- The note row: typing, saving, or what was saved with Undo.
    let row = Rect::new(area.x, area.y, area.w, INPUT_H);
    if let Some((saved, _)) = st.oyenza.saved.clone() {
        draw_saved(ui, st, row, &saved);
    } else if st.oyenza.saving {
        mark(ui, row, Icon::Plus, palette::TEXT_FAINT, false);
        let text = format!("Saving “{}”…", st.oyenza.input.trim());
        ui.c.text(&text, Rect::new(row.x + TEXT_X, row.y, row.w - TEXT_X, row.h), &TextStyle::body().color(palette::TEXT_DIM));
    } else {
        let resp = ui.text_input_row(wid("oyenza.note"), row, &mut st.oyenza.input, PLACEHOLDER, oyenza::NOTE_MAX_CHARS, ACCENT, Some(Icon::Plus));
        if resp.changed {
            st.oyenza.error = None;
        }
        if resp.submitted {
            submit(ui, st);
        }
    }

    // ---- One status line: why a note wasn't saved, or a short confirmation.
    let status = Rect::new(area.x + TEXT_X, area.y + STATUS_Y, area.w - TEXT_X, 14.0);
    if let Some(e) = st.oyenza.error.clone() {
        ui.c.text(&e, status, &TextStyle::footnote().color(ACCENT));
    } else if let Some((f, _)) = st.oyenza.flash.clone() {
        ui.c.text(&f, status, &TextStyle::footnote().color(palette::TEXT_DIM));
    }

    // ---- The strip of days.
    let today = today_of(st);
    if let Some(p) = draw_strip(ui, st, Rect::new(area.x, area.y + STRIP_Y, area.w, STRIP_H), &today) {
        st.oyenza.day = p;
        st.oyenza.scroll = 0.0;
    }
    let pick = st.oyenza.day;

    // ---- The chosen day: its rows, heading and sums.
    let activity = st.oyenza.activity.clone();
    let day = oyenza::pick_date(pick, &today).and_then(|d| activity.as_ref().and_then(|a| a.day(&d).cloned()));
    let bills: Vec<Bill> = oyenza::bills_for(&st.oyenza.bills, &today, pick).into_iter().cloned().collect();
    let mut rows: Vec<Line> = bills.iter().map(|b| Line::Bill(b.clone())).collect();
    if let Some(d) = &day {
        rows.extend(d.rows.iter().map(|t| Line::Txn(t.clone())));
        if d.count > d.rows.len() {
            rows.push(Line::More(d.count - d.rows.len()));
        }
    }
    let count = match pick {
        DayPick::Today | DayPick::Past(_) => day.as_ref().map_or(0, |d| d.count),
        DayPick::Tomorrow | DayPick::Later => bills.len(),
    };
    let head = Rect::new(area.x, area.y + HEAD_Y, area.w, PILL_SMALL);
    let pill_x = if st.settings.oyenza_show_amounts {
        head.right()
    } else {
        let text = if st.oyenza.reveal { "Hide amounts" } else { "Show amounts" };
        let w = ui.pill_width(text, true, true);
        let r = Rect::new(head.right() - w, head.y, w, PILL_SMALL);
        if ui.pill(wid("oyenza.reveal"), r, text, Some(Icon::Eye), PillStyle::Utility).clicked {
            st.oyenza.reveal = !st.oyenza.reveal;
        }
        r.x
    };
    let heading = oyenza::day_heading(pick, &today, count);
    let heading_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_FAINT);
    let room = (pill_x - 12.0 - head.x).max(0.0);
    let (hw, _) = ui.c.measure(&heading, &heading_st, 10_000.0);
    ui.c.text(&heading, Rect::new(head.x, head.y, hw.min(room) + 2.0, head.h), &heading_st);
    let totals = day.as_ref().map(|d| oyenza::day_totals(d, show)).unwrap_or_default();
    let tx = head.x + hw + 12.0;
    if !totals.is_empty() && pill_x - 12.0 - tx > 60.0 {
        ui.c.text(&totals, Rect::new(tx, head.y, pill_x - 12.0 - tx, head.h), &TextStyle::footnote().color(palette::TEXT_DIM));
    }

    // ---- The list.
    let list = Rect::new(area.x, area.y + LIST_Y, area.w, LIST_BOTTOM - LIST_Y);
    if rows.is_empty() {
        let text = match (&activity, pick) {
            (None, DayPick::Today | DayPick::Past(_)) => "Checking Oyenza…",
            _ => oyenza::empty_day_text(pick),
        };
        empty_row(ui, Rect::new(list.x, list.y, list.w, ROW_H), text);
    } else {
        draw_list(ui, st, list, &rows, &today, show);
    }

    // ---- Footer: live while the page keeps asking, and the app itself.
    let footer = Rect::new(area.x, area.y + FOOTER_Y, area.w, PILL_SMALL);
    let offset = oyenza::offset_minutes(&st.local, st.unix);
    let (live, updated) = match st.oyenza.updated_unix {
        Some(t) => {
            let local = oyenza::local_clock(&iso_of(t), offset).unwrap_or(st.local);
            let clock = fmt_clock(&local, st.settings.use_24h);
            let live = st.unix - t <= LIVE_S;
            (live, if live { format!("Live · updated {clock}") } else { format!("Updated {clock}") })
        }
        None => (false, "Checking Oyenza…".into()),
    };
    let open_w = ui.pill_width("Open oyenza.app", true, true);
    let open = Rect::new(footer.right() - open_w, footer.y, open_w, PILL_SMALL);
    let mut text_x = footer.x;
    if live {
        ui.c.fill_circle(Point::new(footer.x + 4.0, footer.center().y), 3.5, palette::GREEN);
        text_x += 14.0;
    }
    ui.c.text(&updated, Rect::new(text_x, footer.y, (open.x - text_x - 12.0).max(0.0), footer.h), &TextStyle::footnote().color(palette::TEXT_FAINT));
    if ui.pill(wid("oyenza.open"), open, "Open oyenza.app", Some(Icon::Open), PillStyle::Utility).clicked {
        st.cmd(Command::OpenUrl(oyenza::WEB.to_string()));
    }
}

/// The day the page counts from: the server's, else the PC's.
fn today_of(st: &AppState) -> String {
    let o = &st.oyenza;
    match &o.activity {
        Some(a) if !a.today.is_empty() => a.today.clone(),
        _ if !o.bills_today.is_empty() => o.bills_today.clone(),
        _ => st.day_key(),
    }
}

/// The strip: the days side by side in one track, the chosen one lifted (as
/// the Settings page's pickers), a dot on Today while something new glows
/// there. A wide font first narrows the padding, then lets the oldest days
/// go. Returns the day clicked, if any.
fn draw_strip(ui: &mut Ui, st: &AppState, r: Rect, today: &str) -> Option<DayPick> {
    let text_st = TextStyle::footnote().weight(Weight::Semibold);
    let mut days = oyenza::strip(today);
    let widths = |ui: &mut Ui, days: &[(DayPick, String)], pad: f32| days.iter().map(|(_, l)| (ui.c.measure(l, &text_st, 300.0).0 + 2.0 * pad).ceil()).collect::<Vec<f32>>();
    let mut pad = SEG_PAD;
    let mut w = widths(ui, &days, pad);
    while w.iter().sum::<f32>() + 2.0 * SEG_INSET > r.w && pad > SEG_PAD_MIN {
        pad -= 1.0;
        w = widths(ui, &days, pad);
    }
    while w.iter().sum::<f32>() + 2.0 * SEG_INSET > r.w && days.len() > 5 {
        days.remove(0);
        w.remove(0);
    }
    let inner_h = r.h - 2.0 * SEG_INSET;
    let track_w = w.iter().sum::<f32>() + 2.0 * SEG_INSET;
    ui.c.fill_rrect(Rect::new(r.x, r.y, track_w, r.h), r.h / 2.0, palette::SURFACE);
    let today_glows = st.oyenza.day != DayPick::Today
        && st.oyenza.activity.as_ref().and_then(|a| a.day(today)).is_some_and(|d| st.oyenza.seen.glowing_until(d, st.now).is_some());
    let mut x = r.x + SEG_INSET;
    let mut picked = None;
    for (i, ((pick, label), cw)) in days.iter().zip(&w).enumerate() {
        let cell = Rect::new(x, r.y + SEG_INSET, *cw, inner_h);
        x += cw;
        let color = if *pick == st.oyenza.day {
            ui.c.fill_rrect(cell, inner_h / 2.0, Color::white(0.16));
            palette::TEXT
        } else {
            let resp = ui.button(wid_n("oyenza.day", i as u64), Rect::new(cell.x, r.y, cell.w, r.h));
            if resp.clicked {
                picked = Some(*pick);
            }
            if resp.pressed || resp.hovered {
                ui.c.fill_rrect(cell, inner_h / 2.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
                palette::TEXT
            } else {
                palette::TEXT_DIM
            }
        };
        ui.c.text(label, cell, &text_st.color(color).center());
        if *pick == DayPick::Today && today_glows {
            ui.c.fill_circle(Point::new(cell.right() - 6.0, cell.y + 5.0), 3.0, palette::GREEN);
        }
    }
    picked
}

/// One line of a day's list.
#[derive(Clone)]
enum Line {
    Bill(Bill),
    Txn(Txn),
    /// The server sent the day's newest rows only: how many more there are.
    More(usize),
}

fn draw_list(ui: &mut Ui, st: &mut AppState, list: Rect, rows: &[Line], today: &str, show: bool) {
    let content_h = rows.len() as f32 * ROW_H;
    let max_scroll = (content_h - list.h).max(0.0);
    // Only claim the wheel when there is something to scroll; otherwise the
    // app uses it to switch pages.
    if max_scroll > 0.0 {
        let w = ui.wheel_in(list);
        if w != 0.0 {
            st.oyenza.scroll -= w * WHEEL_STEP;
        }
    }
    st.oyenza.scroll = st.oyenza.scroll.clamp(0.0, max_scroll);
    let scroll = smooth_scroll(ui, st.oyenza.scroll, max_scroll);
    let offset = oyenza::offset_minutes(&st.local, st.unix);
    let wide = Rect::new(list.x - ROW_BLEED, list.y, list.w + 2.0 * ROW_BLEED, list.h);
    ui.c.clip(wide);
    // While the list glides its rows move between pixels; at rest they sit on whole pixels.
    let snap_was = ui.c.set_text_snap(scroll == st.oyenza.scroll);
    let first = (scroll / ROW_H).floor().max(0.0) as usize;
    let last = (((scroll + list.h) / ROW_H).ceil().max(0.0) as usize).min(rows.len());
    for (i, line) in rows.iter().enumerate().take(last).skip(first) {
        let r = Rect::new(list.x, list.y + i as f32 * ROW_H - scroll, list.w, ROW_H);
        match line {
            Line::Bill(b) => bill_row(ui, r, b, today, st.oyenza.day, show),
            Line::Txn(t) => {
                let glow = st.oyenza.seen.glow(&t.id, st.now);
                if glow > 0.0 {
                    ui.c.fill_rrect(Rect::new(r.x - ROW_BLEED, r.y + 1.0, r.w + 2.0 * ROW_BLEED, r.h - 2.0), 10.0, palette::GREEN.with_alpha(0.14 * glow));
                }
                let time = oyenza::local_clock(&t.at, offset).map(|l| fmt_clock(&l, st.settings.use_24h)).unwrap_or_default();
                txn_row(ui, r, t, &time, show);
            }
            Line::More(n) => empty_row(ui, r, &format!("and {n} more in oyenza.app")),
        }
    }
    ui.c.set_text_snap(snap_was);
    fade_edge(ui, Rect::new(wide.x, wide.y, wide.w, FADE_H), true, (scroll / FADE_H).min(1.0));
    fade_edge(ui, Rect::new(wide.x, wide.bottom() - FADE_H, wide.w, FADE_H), false, ((max_scroll - scroll) / FADE_H).min(1.0));
    ui.c.unclip();
}

fn submit(ui: &mut Ui, st: &mut AppState) {
    let text = st.oyenza.input.trim().to_string();
    if text.is_empty() {
        return;
    }
    if !oyenza::has_amount(&text) {
        // Nothing to send: the server would say the same.
        st.oyenza.error = Some(oyenza::note_error_text("no_amount").into());
        return;
    }
    st.oyenza.input = text.clone();
    st.oyenza.saving = true;
    st.oyenza.error = None;
    st.oyenza.flash = None;
    ui.blur();
    st.cmd(Command::Oyenza(OyenzaCmd::Note(text)));
}

/// "Undo" lapses, but never under a press that is about to click it.
fn undo_held(ui: &Ui) -> bool {
    let id = wid("oyenza.undo");
    ui.input.pressed_id == Some(id) || ui.input.clicked.is_some_and(|(c, _)| c == id)
}

fn draw_saved(ui: &mut Ui, st: &mut AppState, row: Rect, saved: &[oyenza::Saved]) {
    mark(ui, row, Icon::Check, palette::GREEN, true);
    let w = ui.pill_width("Undo", true, true);
    let undo = Rect::new(row.right() - w, row.center().y - PILL_SMALL / 2.0, w, PILL_SMALL);
    let x = row.x + TEXT_X;
    let head_st = TextStyle::body().weight(Weight::Semibold);
    let (hw, _) = ui.c.measure("Saved", &head_st, 200.0);
    ui.c.text("Saved", Rect::new(x, row.y, hw + 2.0, row.h), &head_st);
    let line_x = x + hw + 8.0;
    ui.c.text(&oyenza::saved_line(saved), Rect::new(line_x, row.y, (undo.x - 12.0 - line_x).max(0.0), row.h), &TextStyle::body().color(palette::TEXT_DIM));
    if ui.pill(wid("oyenza.undo"), undo, "Undo", Some(Icon::Undo), PillStyle::UtilityAccent(ACCENT)).clicked {
        for s in saved {
            st.cmd(Command::Oyenza(OyenzaCmd::Undo(s.id.clone())));
        }
        st.oyenza.saved = None;
        st.oyenza.flash = Some(("Taking it back…".into(), st.now + 4.0));
    }
}

/// A disc in the list's mark column (or the plain icon of the input row).
fn mark(ui: &mut Ui, row: Rect, icon: Icon, color: Color, disc: bool) {
    let c = Point::new(row.x + MARK_X, row.center().y);
    if disc {
        ui.c.fill_circle(c, DISC_R, color.with_alpha(0.22));
        ui.c.icon(icon, Rect::from_center(c, 13.0, 13.0), color);
    } else {
        ui.c.icon(icon, Rect::from_center(c, 16.0, 16.0), color);
    }
}

fn empty_row(ui: &mut Ui, r: Rect, text: &str) {
    ui.c.text(text, Rect::new(r.x + TEXT_X, r.y, r.w - TEXT_X, r.h), &TextStyle::footnote().color(palette::TEXT_FAINT));
}

/// One list row: a disc, a title with a dim detail, and the sum.
struct Row<'a> {
    color: Color,
    icon: Icon,
    title: &'a str,
    detail: &'a str,
    /// The detail's last part (the time): kept when the rest doesn't fit.
    detail_tail: &'a str,
    sum: &'a str,
    sum_color: Option<Color>,
    sum_dim: bool,
}

/// Title and a dim detail on the left, the sum on the right. A long title
/// gives way first: the detail keeps its room, or at least its time.
fn row(ui: &mut Ui, r: Rect, row: Row) {
    let Row { color, icon, title, detail, detail_tail, sum, sum_color, sum_dim } = row;
    mark(ui, r, icon, color, true);
    let sum_st = if sum_dim {
        TextStyle::footnote().color(palette::TEXT_DIM).right()
    } else {
        let st = TextStyle::body().weight(Weight::Semibold).tabular().right();
        match sum_color {
            Some(c) => st.color(c),
            None => st,
        }
    };
    let (sw, _) = ui.c.measure(sum, &sum_st, 300.0);
    let sum_r = Rect::new(r.right() - sw - 2.0, r.y, sw + 2.0, r.h);
    ui.c.text(sum, sum_r, &sum_st);
    let x = r.x + TEXT_X;
    let avail = (sum_r.x - 12.0 - x).max(0.0);
    let title_st = TextStyle::body();
    let detail_st = TextStyle::footnote().color(palette::TEXT_DIM);
    let (tw, _) = ui.c.measure(title, &title_st, 10_000.0);
    let (dw, _) = ui.c.measure(detail, &detail_st, 10_000.0);
    let (tailw, _) = ui.c.measure(detail_tail, &detail_st, 10_000.0);
    // The detail, or just its time, keeps its room unless the title is short.
    let (shown_detail, detail_w) = if detail.is_empty() {
        ("", 0.0)
    } else if tw + 8.0 + dw <= avail || dw <= avail * 0.45 {
        (detail, dw)
    } else {
        (detail_tail, tailw)
    };
    let title_w = if shown_detail.is_empty() { avail } else { (avail - detail_w - 8.0).max(avail * 0.3) };
    ui.c.text(title, Rect::new(x, r.y, tw.min(title_w) + 2.0, r.h), &title_st);
    if !shown_detail.is_empty() {
        let dx = x + tw.min(title_w) + 8.0;
        ui.c.text(shown_detail, Rect::new(dx, r.y, (sum_r.x - 12.0 - dx).max(0.0), r.h), &detail_st);
    }
}

fn txn_row(ui: &mut Ui, r: Rect, t: &Txn, time: &str, show: bool) {
    let (icon, color, sum_color) = match t.flow() {
        Flow::In => (Icon::MoneyIn, palette::GREEN, Some(palette::GREEN)),
        Flow::Out => (Icon::MoneyOut, palette::BLUE, None),
        Flow::Move => (Icon::Transfer, palette::CYAN, Some(palette::TEXT_DIM)),
    };
    let title = oyenza::txn_title(t);
    let detail = oyenza::txn_detail(t, time);
    let sum = oyenza::signed_sum(t, show);
    row(ui, r, Row { color, icon, title: &title, detail: &detail, detail_tail: time, sum: &sum, sum_color, sum_dim: false });
}

fn bill_row(ui: &mut Ui, r: Rect, b: &Bill, today: &str, pick: DayPick, show: bool) {
    let kind = if b.is_card() { "Card bill due" } else { "Subscription" };
    let detail = match pick {
        DayPick::Today => format!("{kind} · today"),
        DayPick::Later => format!("{kind} · {}", oyenza::due_label(&b.due, today)),
        _ => kind.to_string(),
    };
    let (icon, sum, dim) = match (&b.text, b.is_card()) {
        (_, true) => (Icon::Card, "card bill".to_string(), true),
        (Some(t), false) => (Icon::Calendar, if show { t.clone() } else { oyenza::masked(t) }, false),
        (None, false) => (Icon::Calendar, String::new(), true),
    };
    row(ui, r, Row { color: palette::ORANGE, icon, title: &b.name, detail: &detail, detail_tail: "", sum: &sum, sum_color: None, sum_dim: dim });
}

/// The list eases to where the wheel sent it.
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

/// The island's ground over `r`, from solid at the outer edge to clear: rows
/// fade out where the list is cut off.
fn fade_edge(ui: &mut Ui, r: Rect, outer_is_top: bool, strength: f32) {
    if strength <= 0.01 || r.h <= 0.0 {
        return;
    }
    const STEPS: usize = 8;
    const EDGE: f32 = 0.94;
    let target = |j: usize| EDGE * strength * (STEPS - j) as f32 / STEPS as f32;
    for k in (0..STEPS).rev() {
        let below = if k + 1 < STEPS { target(k + 1) } else { 0.0 };
        let a = 1.0 - (1.0 - target(k)) / (1.0 - below);
        let depth = r.h * (k + 1) as f32 / STEPS as f32;
        let band = if outer_is_top { Rect::new(r.x, r.y, r.w, depth) } else { Rect::new(r.x, r.bottom() - depth, r.w, depth) };
        ui.c.fill_rect(band, Color::black(a));
    }
}

/// A Unix time as the server writes instants, for [`oyenza::local_clock`].
fn iso_of(unix: i64) -> String {
    let (y, m, d) = poni_core::datetime::civil_from_days(unix.div_euclid(86_400));
    let s = unix.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3600, s % 3600 / 60, s % 60)
}
