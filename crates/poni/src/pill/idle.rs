//! The resting pill (`Mode::Idle`): the time on the left, the battery on
//! the right (its percentage and a drawn battery, a bolt inside while it
//! charges); on a PC without a battery, the time alone in the middle.
//! `IdleStyle` still picks what rests there (Clock, Minimal, Eyes, Hidden,
//! and Status, which [`super::status`] draws). `None` / `false` = Classic's
//! layout. [`battery`] is the drawn battery for the other Pill surfaces.

use crate::gfx::{Canvas, TextStyle, Weight};
use crate::island::Geom;
use crate::state::AppState;
use crate::ui::Ui;
use poni_core::datetime::fmt_clock;
use poni_core::geom::{palette, Color, Rect};
use poni_core::settings::IdleStyle;
use poni_core::svgpath::Seg;

/// Every resting pill is this tall, and a capsule (radius = height / 2).
const H: f32 = 32.0;
/// Content keeps this far in from each round end (the radius).
const SIDE: f32 = 16.0;
/// The time on its own, centred (a PC without a battery): Classic's clock
/// widths, so "23:59" and "12:59 PM" keep air on both sides.
const CLOCK_W: f32 = 84.0;
const CLOCK_12H_W: f32 = 104.0;
/// The time with the battery: the time's slot at the left end ("23:59" or
/// "12:59 PM", 15 semibold), the battery's at the right ("100%", the gap,
/// the glyph), and the gap between them. 152 and 180 wide.
const TIME_SLOT: f32 = 40.0;
const TIME_12H_SLOT: f32 = 68.0;
const PCT_SLOT: f32 = 35.0;
const PCT_GAP: f32 = 6.0;
const MID_GAP: f32 = 16.0;
const PCT_INK: Color = Color::white(0.55);
/// Minimal: a small blank capsule, nothing in it but something to hover.
const MINIMAL_W: f32 = 96.0;
const MINIMAL_H: f32 = 24.0;
/// Eyes: Classic's two eyes, centred in a clock-sized capsule.
const EYES_W: f32 = 84.0;

/// The battery glyph at its own size (DIPs): a 20 x 10 body, then the nub
/// 0.75 away and 1.75 wide. [`battery`] scales it with its rect's height.
pub const BATTERY_W: f32 = 22.5;
pub const BATTERY_H: f32 = 10.0;
const OUTLINE_INK: Color = Color::white(0.42);

pub fn geom(st: &AppState) -> Option<Geom> {
    if st.settings.idle_style == IdleStyle::Status {
        if let Some(g) = super::status::geom(st) {
            return Some(g);
        }
    }
    match st.settings.idle_style {
        // Where the status bar leaves the resting pill to us, it is the clock.
        IdleStyle::Clock | IdleStyle::Status => Some(Geom::new(clock_width(st), H, H / 2.0)),
        IdleStyle::Minimal => Some(Geom::new(MINIMAL_W, MINIMAL_H, MINIMAL_H / 2.0)),
        IdleStyle::Eyes => Some(Geom::new(EYES_W, H, H / 2.0)),
        // Classic's invisible strip: it only catches the pointer at the edge.
        IdleStyle::Hidden => None,
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, body: Rect) -> bool {
    if st.settings.idle_style == IdleStyle::Status && super::status::draw(ui, st, body) {
        return true;
    }
    match st.settings.idle_style {
        IdleStyle::Clock | IdleStyle::Status => {
            draw_clock(ui, st, body);
            true
        }
        IdleStyle::Minimal => true,
        // Classic draws the eyes (centred, blinking on its own wake) and the
        // invisible strip.
        IdleStyle::Eyes | IdleStyle::Hidden => false,
    }
}

/// The next time the resting pill changes by itself (beyond the minute
/// tick the app already wakes for).
pub fn next_wake(_st: &AppState) -> Option<f64> {
    None
}

/// The battery the resting pill shows: its level and whether it charges.
/// `None` on a PC without one.
fn battery_state(st: &AppState) -> Option<(u8, bool)> {
    let p = &st.power;
    // Plugged in counts as charging, as the Charging pop-up has it: a full
    // battery on its charger keeps the bolt.
    p.has_battery.then_some((p.percent.min(100), p.charging || p.on_ac))
}

fn clock_width(st: &AppState) -> f32 {
    let h24 = st.settings.use_24h;
    if battery_state(st).is_none() {
        return if h24 { CLOCK_W } else { CLOCK_12H_W };
    }
    let time = if h24 { TIME_SLOT } else { TIME_12H_SLOT };
    (SIDE + time + MID_GAP + PCT_SLOT + PCT_GAP + BATTERY_W + SIDE).round()
}

/// "13:27" at the left end, "99%" and the battery at the right one (ref2).
/// Without a battery the time sits alone in the middle.
fn draw_clock(ui: &mut Ui, st: &AppState, body: Rect) {
    let time = fmt_clock(&st.local, st.settings.use_24h);
    let ts = TextStyle::headline().tabular().color(Color::white(1.0));
    let Some((percent, charging)) = battery_state(st) else {
        ui.c.text(&time, body, &ts.center());
        return;
    };
    let (tw, _) = ui.c.measure(&time, &ts, 400.0);
    ui.c.text(&time, Rect::new(body.x + SIDE, body.y, tw + 2.0, body.h), &ts);

    let cy = body.center().y;
    let glyph = Rect::new(body.right() - SIDE - BATTERY_W, cy - BATTERY_H / 2.0, BATTERY_W, BATTERY_H);
    battery(ui, glyph, percent, charging);
    let pct = format!("{percent}%");
    let pct_ts = TextStyle::body().weight(Weight::Medium).tabular().color(PCT_INK);
    let (pw, _) = ui.c.measure(&pct, &pct_ts, 200.0);
    let x = glyph.x - PCT_GAP - pw;
    ui.c.text(&pct, Rect::new(x, body.y, pw + 2.0, body.h), &pct_ts);
}

/// A drawn battery in `r`, the nub on its right (natural size
/// [`BATTERY_W`] x [`BATTERY_H`]; it scales with `r.h`): a grey outline,
/// the level filled in green (red at 20 % or below unless it charges) and,
/// while it charges, a white bolt cut into the fill. Outline and nub sit on
/// whole device pixels so they stay crisp at 100 %.
pub fn battery(ui: &mut Ui, r: Rect, percent: u8, charging: bool) {
    battery_glyph(ui.c, r, percent, charging);
}

/// [`battery`] for code that draws on the canvas without a `Ui`.
pub fn battery_glyph(c: &mut Canvas, r: Rect, percent: u8, charging: bool) {
    let s = r.h / BATTERY_H;
    let px = c.scale().max(0.1);
    let snap = |v: f32| (v * px).round() / px;
    // Whole pixels, at least one; a half rounds down so 1.25 stays slim at 200 %.
    let whole = |v: f32| ((v * px - 0.01).round().max(1.0)) / px;
    let (nub_gap, nub_w) = (whole(0.75 * s), whole(1.75 * s));
    let (x0, y0) = (snap(r.x), snap(r.y));
    let (x1, y1) = (snap(r.right() - nub_gap - nub_w), snap(r.bottom()));
    let body = Rect::new(x0, y0, x1 - x0, y1 - y0);

    // The stroke's radius is its centre line's: the outer corner stays at 3.
    let line = whole(1.25 * s);
    c.stroke_rrect(body, 3.0 * s - line / 2.0, OUTLINE_INK, line);
    let nub_h = whole(4.0 * s);
    let nub_y = snap(body.center().y - nub_h / 2.0);
    c.fill_rrect(Rect::new(x1 + nub_gap, nub_y, nub_w, nub_h), 0.75 * s, OUTLINE_INK);

    let inset = whole(2.0 * s).max(line + 1.0 / px);
    let inner = body.inset(inset, inset);
    let level = f32::from(percent.min(100)) / 100.0;
    if level > 0.0 && inner.w > 0.0 && inner.h > 0.0 {
        let color = if !charging && percent <= 20 { palette::RED } else { palette::GREEN };
        let w = (inner.w * level).max(2.0 * s).min(inner.w);
        c.fill_rrect(Rect::new(inner.x, inner.y, w, inner.h), (1.5 * s).min(w / 2.0), color);
    }
    if charging {
        // The bolt (6 x 8, Icon::Bolt's shape) with a 1 DIP edge of the
        // island's own black cut around it, so it reads on the fill.
        let mid = body.center();
        let pts = [(0.6, -4.0), (-3.0, 0.674), (-0.343, 0.674), (-0.9, 4.0), (3.0, -0.674), (0.3, -0.674)];
        let mut segs: Vec<Seg> = Vec::with_capacity(pts.len() + 1);
        for (i, (dx, dy)) in pts.iter().enumerate() {
            let (x, y) = (mid.x + dx * s, mid.y + dy * s);
            segs.push(if i == 0 { Seg::MoveTo(x, y) } else { Seg::LineTo(x, y) });
        }
        segs.push(Seg::Close);
        c.path(&segs, Some(palette::ISLAND), Some((palette::ISLAND, 2.0 * s)));
        c.path(&segs, Some(Color::white(1.0)), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PowerState;
    use poni_core::settings::{Settings, Theme};
    use poni_core::store::Store;

    fn state(battery: bool, use_24h: bool) -> AppState {
        let mut st = AppState::new(Settings::default(), Store::default());
        st.settings.theme = Theme::Pill;
        st.settings.use_24h = use_24h;
        st.power = PowerState { has_battery: battery, percent: 99, charging: false, on_ac: !battery, saver: false };
        st
    }

    #[test]
    fn the_resting_pill_is_a_capsule_sized_to_the_time_and_the_battery() {
        let size = |battery, use_24h| geom(&state(battery, use_24h));
        assert_eq!(size(true, true), Some(Geom::new(152.0, 32.0, 16.0)));
        assert_eq!(size(true, false), Some(Geom::new(180.0, 32.0, 16.0)));
        assert_eq!(size(false, true), Some(Geom::new(84.0, 32.0, 16.0)));
        assert_eq!(size(false, false), Some(Geom::new(104.0, 32.0, 16.0)));
    }

    #[test]
    fn plugged_in_charges_and_the_other_looks_keep_their_shapes() {
        let mut st = state(true, true);
        assert_eq!(battery_state(&st), Some((99, false)));
        // Full on the charger: Windows clears the charging flag, the bolt stays.
        st.power.on_ac = true;
        assert_eq!(battery_state(&st), Some((99, true)));
        assert_eq!(battery_state(&state(false, true)), None);
        st.settings.idle_style = IdleStyle::Minimal;
        assert_eq!(geom(&st), Some(Geom::new(96.0, 24.0, 12.0)));
        st.settings.idle_style = IdleStyle::Eyes;
        assert_eq!(geom(&st), Some(Geom::new(84.0, 32.0, 16.0)));
        // Hidden stays Classic's invisible strip.
        st.settings.idle_style = IdleStyle::Hidden;
        assert_eq!(geom(&st), None);
    }
}
