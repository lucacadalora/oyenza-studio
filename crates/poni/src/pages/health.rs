//! Health page, the Health add-on's (off until switched on): today's steps
//! against the goal, the week as bars, and where the numbers came from.
//!
//! The numbers come from the person's phone: Oyenza reads daily totals from
//! Apple Health or Health Connect and, with Show on Oyenza Studio turned on
//! there, a signed-in Oyenza Studio receives them (`AppState::health`, in
//! memory only). Without numbers the page says how to get them; numbers more
//! than a day old stay, with a line asking to open Oyenza on the phone.
//! Nothing here animates; the page only redraws when new numbers arrive.

use crate::commands::{Command, OyenzaCmd};
use crate::gfx::{ts, TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::state::{AppState, Notice, NoticeAction, OyenzaLink};
use crate::ui::{wid, PillStyle, Ui, PILL_SMALL};
use poni_core::datetime::{fmt_clock, fmt_date_short, LocalDateTime};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::health::{fmt_distance, fmt_steps, is_stale, HealthSnapshot};

const HEIGHT: f32 = 144.0;
/// The ring on the left, and the gap to the middle column.
const RING_R: f32 = 46.0;
const RING_W: f32 = 10.0;
const RING_BOX: f32 = 2.0 * RING_R + RING_W;
const MID_GAP: f32 = 24.0;
/// Before sign-in the explanation spans the middle and right columns, up to this width.
const SIGNED_OUT_W: f32 = 400.0;
/// The goal line sits at this fraction of the chart height (room for over-goal days).
const GOAL_AT: f32 = 0.72;
const WEEKDAY_LETTER: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];

/// Steps are green, the colour people know from activity rings.
const STEPS: Color = palette::GREEN;

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let right_w = (area.w - 340.0).clamp(110.0, 250.0);
    let right_x = area.right() - right_w;
    let mid_x = area.x + RING_BOX + MID_GAP;
    let mid = Rect::new(mid_x, area.y, (right_x - 24.0 - mid_x).max(0.0), area.h);
    let ring_c = Point::new(area.x + RING_BOX / 2.0, area.y + 11.0 + RING_BOX / 2.0);
    let chart = Rect::new(right_x, area.y + 34.0, right_w, 76.0);

    let Some(h) = st.health.clone() else {
        // No week to chart yet: the explanation takes the middle and right columns.
        draw_ring(ui, ring_c, None);
        let w = (area.right() - mid_x).clamp(0.0, SIGNED_OUT_W);
        draw_not_connected(ui, st, Rect::new(mid_x, area.y, w, area.h), ring_c.y);
        return;
    };
    let today = st.local.day_key();
    let steps = h.steps_on(&today);
    let goal = h.goal();
    draw_ring(ui, ring_c, Some(steps as f32 / goal as f32));
    draw_today(ui, st, &h, steps, goal, mid, area.right());
    draw_week_title(ui, st, &h, Rect::new(right_x, area.y + 4.0, right_w, 18.0));
    draw_chart(ui, st, &h, chart);
}

// ------------------------------------------------------------ ring

/// The day's ring with the steps glyph in the middle. `None` = no numbers yet.
fn draw_ring(ui: &mut Ui, c: Point, progress: Option<f32>) {
    let p = progress.unwrap_or(0.0);
    ui.c.ring(c, RING_R, RING_W, p.min(1.0), palette::TRACK, STEPS);
    if p > 1.0 {
        // Past the goal the ring starts a second, brighter lap.
        ui.c.arc(c, RING_R, 0.0, (p - 1.0).min(1.0), Color::lerp(STEPS, Color::white(1.0), 0.35), RING_W);
    }
    let glyph = if progress.is_some() { STEPS } else { palette::TEXT_FAINT };
    let icon_y = if progress.is_some() { c.y - 14.0 } else { c.y };
    ui.c.icon(Icon::Steps, Rect::from_center(Point::new(c.x, icon_y), 22.0, 22.0), glyph);
    if let Some(p) = progress {
        let pct = format!("{}%", (p * 100.0).round() as u32);
        let st = TextStyle::body().weight(Weight::Semibold).tabular().center();
        ui.c.text(&pct, Rect::new(c.x - RING_R, c.y + 2.0, 2.0 * RING_R, 18.0), &st);
    }
}

// ------------------------------------------------------------ middle column

/// The middle column; `right` is the page's right edge, which the bottom line
/// may run to (below the chart's weekday letters).
fn draw_today(ui: &mut Ui, st: &AppState, h: &HealthSnapshot, steps: u32, goal: u32, r: Rect, right: f32) {
    // "6,420  / 8,000 steps", sharing a baseline like the other pages' amounts.
    let big = fmt_steps(steps);
    let big_st = TextStyle::display().tabular().top();
    let small_st = TextStyle::new(ts::HEADLINE).color(palette::TEXT_DIM).top();
    let y = r.y + 8.0;
    let (bw, bh) = ui.c.measure(&big, &big_st, r.w);
    let (_, sh) = ui.c.measure("steps", &small_st, r.w);
    ui.c.text(&big, Rect::new(r.x, y, bw + 2.0, bh), &big_st);
    let sy = y + (bh - sh) * 0.81;
    let goal_text = format!("/ {} steps", fmt_steps(goal));
    ui.c.text(&goal_text, Rect::new(r.x + bw + 6.0, sy, (r.w - bw - 6.0).max(0.0), sh), &small_st);

    let strong = TextStyle::footnote().weight(Weight::Semibold);
    let dim = TextStyle::footnote().color(palette::TEXT_DIM);
    let line = Rect::new(r.x, y + 42.0, r.w, 18.0);
    if steps >= goal {
        let ir = Rect::new(line.x, line.center().y - 6.5, 13.0, 13.0);
        ui.c.icon(Icon::Check, ir, STEPS);
        text_run(ui, line.x + 18.0, line, "Goal reached", &strong.color(STEPS));
    } else {
        let x = text_run(ui, line.x, line, &fmt_steps(goal - steps), &strong);
        text_run(ui, x, line, " to go", &dim);
    }

    // Distance and active energy, when the phone has them.
    let today = h.day(&st.local.day_key());
    let mut extra = Vec::new();
    if let Some(m) = today.and_then(|d| d.distance_m) {
        extra.push(fmt_distance(m));
    }
    if let Some(k) = today.and_then(|d| d.active_kcal) {
        extra.push(format!("{} kcal", fmt_steps(k)));
    }
    if !extra.is_empty() {
        ui.c.text(&extra.join(" · "), Rect::new(r.x, y + 64.0, r.w, 18.0), &dim);
    }

    let faint = TextStyle::caption().color(palette::TEXT_FAINT);
    if is_stale(h.updated_at, st.unix) {
        // Not updated for a day: the phone sends only while Oyenza is open on it.
        ui.c.text(&updated_text(st, h.updated_at), Rect::new(r.x, y + 92.0, r.w, 15.0), &faint);
        let hint = TextStyle::caption().color(palette::TEXT_DIM);
        ui.c.text("Open Oyenza on your phone to update", Rect::new(r.x, y + 107.0, (right - r.x).max(r.w), 15.0), &hint);
    } else {
        ui.c.text(&format!("From {} via Oyenza", h.source.name()), Rect::new(r.x, y + 92.0, r.w, 15.0), &faint);
        ui.c.text(&updated_text(st, h.updated_at), Rect::new(r.x, y + 107.0, r.w, 15.0), &faint);
    }
}

/// "Updated 08:12" today, "Updated yesterday", or "Updated 21 Oct" (the Today page's day too).
pub(crate) fn updated_text(st: &AppState, at: i64) -> String {
    let local = st.local;
    let since_midnight = local.hour as i64 * 3600 + local.minute as i64 * 60 + local.second as i64;
    let ago = (st.unix - at).max(0);
    if ago <= since_midnight {
        let secs = since_midnight - ago;
        let t = LocalDateTime { hour: (secs / 3600) as u8, minute: ((secs / 60) % 60) as u8, second: 0, ..local };
        return format!("Updated {}", fmt_clock(&t, st.settings.use_24h));
    }
    let days = (ago - since_midnight - 1) / 86_400 + 1;
    if days == 1 {
        "Updated yesterday".to_string()
    } else {
        format!("Updated {}", fmt_date_short(&local.add_days(-days)))
    }
}

/// Title, explanation and a button (or the "Off in Oyenza" tag), stacked and
/// centred on `center_y` (the ring's centre). Heights are measured, so a wider
/// font wraps instead of clipping. Signed out, the button signs in to Oyenza
/// right here (while a code waits, it shows the code). Signed in already
/// (`services::oyenza`) with no steps, the switch is off on the phone (or its
/// Oyenza can't send them yet): the text says where to turn it on.
fn draw_not_connected(ui: &mut Ui, st: &mut AppState, r: Rect, center_y: f32) {
    let signed_in = matches!(st.oyenza.link, OyenzaLink::Connected { .. });
    let title = "Your steps, from your phone";
    let body = if signed_in {
        "Open Oyenza on your iPhone or Android and, under your steps in Health (Sehat), turn on Show on Oyenza Studio. Your daily steps then show here."
    } else {
        "Oyenza on your iPhone or Android reads them from Apple Health or Health Connect. Sign in to Oyenza to see them here."
    };
    let title_st = TextStyle::headline().wrap().top();
    let body_st = TextStyle::footnote().color(palette::TEXT_DIM).wrap().top();
    let (_, title_h) = ui.c.measure(title, &title_st, r.w);
    let (_, body_h) = ui.c.measure(body, &body_st, r.w);
    let (gap, tag_gap) = (6.0, 12.0);
    let tag_h = if signed_in { 24.0 } else { PILL_SMALL };
    let total = title_h + gap + body_h + tag_gap + tag_h;
    let y = (center_y - total / 2.0).clamp(r.y, (r.bottom() - total).max(r.y));
    ui.c.text(title, Rect::new(r.x, y, r.w, title_h + 1.0), &title_st);
    let body_y = y + title_h + gap;
    ui.c.text(body, Rect::new(r.x, body_y, r.w, body_h + 1.0), &body_st);
    let tag_y = body_y + body_h + tag_gap;
    match st.oyenza.link {
        OyenzaLink::Off => {
            let label = "Sign in with Oyenza";
            let w = ui.pill_width(label, false, true).min(r.w);
            if ui.pill(wid("health.sign-in"), Rect::new(r.x, tag_y, w, PILL_SMALL), label, None, PillStyle::Primary(palette::ORANGE)).clicked {
                st.cmd(Command::Oyenza(OyenzaCmd::SignIn));
            }
        }
        OyenzaLink::Connecting { .. } => {
            let label = "Show the sign-in code";
            let w = ui.pill_width(label, false, true).min(r.w);
            if ui.pill(wid("health.show-code"), Rect::new(r.x, tag_y, w, PILL_SMALL), label, None, PillStyle::Primary(palette::ORANGE)).clicked {
                st.cmd(Command::SetPage(PageKind::Oyenza));
            }
        }
        // Not a button: the switch is on the phone.
        OyenzaLink::Connected { .. } => {
            let label = "Off in Oyenza";
            let tag_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_DIM);
            let (tw, _) = ui.c.measure(label, &tag_st, r.w);
            let tag = Rect::new(r.x, tag_y, (tw.ceil() + 20.0).min(r.w), tag_h);
            ui.c.fill_rrect(tag, tag.h / 2.0, palette::SURFACE);
            ui.c.text(label, tag.inset(10.0, 0.0), &tag_st.center());
        }
    }
}

/// Draw `s` at `x` inside `line`; returns the x after it.
fn text_run(ui: &mut Ui, x: f32, line: Rect, s: &str, st: &TextStyle) -> f32 {
    let avail = (line.right() - x).max(0.0);
    if avail <= 0.0 {
        return x;
    }
    let w = ui.c.measure(s, st, avail).0.min(avail);
    ui.c.text(s, Rect::new(x, line.y, w + 1.0, line.h), st);
    x + w
}

// ------------------------------------------------------------ week

fn draw_week_title(ui: &mut Ui, st: &AppState, h: &HealthSnapshot, r: Rect) {
    let total: u32 = h.week(&st.local).iter().filter_map(|(_, s)| *s).sum();
    let label = TextStyle::caption().color(palette::TEXT_FAINT);
    ui.c.text("This week", r, &label);
    ui.c.text(&fmt_steps(total), r, &TextStyle::caption().weight(Weight::Semibold).tabular().right());
}

/// Seven bars ending today, with the goal as a dashed line.
fn draw_chart(ui: &mut Ui, st: &AppState, h: &HealthSnapshot, r: Rect) {
    let label_w = 30.0;
    let bars = Rect::new(r.x + 2.0, r.y, (r.w - label_w - 2.0).max(0.0), r.h - 18.0);
    let goal_y = bars.bottom() - bars.h * GOAL_AT;

    let (dash, gap) = (2.5f32, 4.5f32);
    let mut x = bars.x;
    while x < bars.right() {
        let x2 = (x + dash).min(bars.right());
        ui.c.line(Point::new(x, goal_y), Point::new(x2, goal_y), Color::white(0.22), 1.0);
        x += dash + gap;
    }
    ui.c.text(&fmt_short(h.goal()), Rect::new(bars.right() + 5.0, goal_y - 7.0, label_w - 5.0, 14.0), &TextStyle::caption().color(palette::TEXT_FAINT));

    let week = h.week(&st.local).map(|(d, s)| (d, s.unwrap_or(0)));
    let goal = h.goal() as f32;
    let cols = bars.columns(week.len(), 0.0);
    let bar_w = cols.first().map(|c| (c.w * 0.4).clamp(6.0, 12.0)).unwrap_or(10.0);
    for (i, ((day, steps), col)) in week.iter().zip(cols.iter()).enumerate() {
        let today = i + 1 == week.len();
        let frac = (*steps as f32 / goal * GOAL_AT).clamp(0.0, 1.0);
        let bh = if *steps > 0 { (frac * bars.h).max(bar_w) } else { bar_w };
        let br = Rect::new(col.center().x - bar_w / 2.0, bars.bottom() - bh, bar_w, bh);
        let hovered = ui.hovered(Rect::new(col.x, r.y - 14.0, col.w, r.h + 14.0));
        let base = if *steps == 0 {
            palette::TRACK
        } else if today {
            STEPS
        } else if *steps as f32 >= goal {
            STEPS.mul_alpha(0.5)
        } else {
            Color::white(0.26)
        };
        let c = if hovered && !today { base.with_alpha((base.a * 1.6).min(1.0)) } else { base };
        ui.c.fill_rrect(br, bar_w / 2.0, c);

        let letter_st = if today {
            TextStyle::caption().weight(Weight::Semibold).color(STEPS).center()
        } else {
            TextStyle::caption().color(if hovered { palette::TEXT_DIM } else { palette::TEXT_FAINT }).center()
        };
        let wd = day.weekday() as usize % 7;
        ui.c.text(WEEKDAY_LETTER[wd], Rect::new(col.x, bars.bottom() + 4.0, col.w, 14.0), &letter_st);

        if hovered {
            let text = if *steps == 0 { "—".to_string() } else { fmt_steps(*steps) };
            let tr = Rect::new(col.center().x - 32.0, (br.y - 16.0).max(r.y - 14.0), 64.0, 14.0);
            ui.c.text(&text, tr, &TextStyle::caption().weight(Weight::Semibold).tabular().center());
        }
    }
}

/// "8k", "7.5k", "950" for the goal label.
fn fmt_short(n: u32) -> String {
    if n < 1000 {
        n.to_string()
    } else if n % 1000 == 0 {
        format!("{}k", n / 1000)
    } else {
        format!("{:.1}k", n as f32 / 1000.0)
    }
}

/// The island's notice for the moment new numbers from the phone put today
/// over the step goal (see `poni_core::health::goal_reached`).
pub fn goal_notice(steps: u32, until: f64) -> Notice {
    Notice {
        icon: Icon::Steps,
        color: palette::GREEN,
        title: format!("{} steps today", fmt_steps(steps)),
        title_dim: String::new(),
        subtitle: "Daily step goal reached".into(),
        until,
        action: Some(NoticeAction::OpenPage(PageKind::Health)),
        inline: None,
        reveal: None,
    }
}
