//! The island around a MacBook's camera notch.
//!
//! The notch has no pixels: whatever Poni draws in its middle is never seen.
//! So on a screen with a notch each island state picks one of three fits:
//!
//! * **Inside**: the resting island hides inside the notch (nothing shows;
//!   pointing at the notch still lifts and opens it).
//! * **Ears**: a live activity stays in the menu bar's line, its content
//!   left and right of the camera (the timer's ring on the left, its digits
//!   on the right; album art and the equalizer the same way).
//! * **Below**: notices, pop-ups and the open island hang under the notch:
//!   a strip the notch's height on top, the usual content under it.
//!
//! The Pill theme keeps the first two: in the menu bar's line the camera is
//! in the way of a floating pill, and a resting pill below the menu bar would
//! sit on every window's toolbar all day. What hangs below the notch floats
//! there instead ([`floats`]): the capsule or card `pill::TOP_GAP` under the
//! menu bar, every corner rounded, no strip behind the camera. On its way
//! between the two the island's gap moves on its own spring (island.rs,
//! `pill::motion::TOP`) and the outline lets go of the edge smoothly
//! ([`detached_segments`]).
//!
//! Screens without a notch (older Macs, external displays) show the island
//! exactly as Windows does.

use crate::activities::{self, Activity, Mode};
use crate::gfx::{Canvas, TextStyle, Weight};
use crate::icons::Icon;
use crate::island::{Geom, SHOULDER};
use crate::oyen;
use crate::pill;
use crate::state::AppState;
use crate::ui::Ui;
use poni_core::datetime::{fmt_clock, fmt_countdown};
use poni_core::geom::{clamp01, palette, Color, Point, Rect};
use poni_core::pomodoro::Phase;
use poni_core::settings::IdleStyle;
use poni_core::svgpath::Seg;

/// The notch as macOS reports it, in DIPs (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Notch {
    pub w: f32,
    pub h: f32,
}

impl Notch {
    /// A 14-inch MacBook Pro's notch at the default scaling (for `--render`).
    pub const MACBOOK: Notch = Notch { w: 185.0, h: 32.0 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Inside,
    Ears,
    Below,
}

/// The narrowest ear a hanging island keeps beside the notch.
const BELOW_EAR: f32 = 24.0;
/// The ears' bottom corner radius.
const EARS_R: f32 = 14.0;
/// The one neutral track under every progress ring (as in activities.rs).
const RING_TRACK: Color = Color::white(0.14);
/// Pill's ears: content keeps this far from the island's outer ends (as the
/// floating pill keeps clear of its round ends) and this far from the camera.
const PILL_END: f32 = 16.0;
const PILL_CAMERA: f32 = 8.0;
/// Pill's resting ears: the time on the left, the battery and its
/// percentage on the right ("100%" and the battery need the most room).
const PILL_EAR_24H: f32 = 88.0;
const PILL_EAR_12H: f32 = 96.0;
const PILL_EAR_TIME_ONLY: f32 = 66.0;
/// The status bar's battery (`pill::status::battery`): its body, how far
/// its nub reaches past it, and the gap to the percentage.
const BATTERY: (f32, f32) = (20.0, 10.0);
const NUB_W: f32 = 0.75 + 1.75;
const PCT_GAP: f32 = 6.0;
/// Pill's secondary text on black.
const PILL_DIM: Color = Color::white(0.55);

pub fn fit(m: &Mode, st: &AppState) -> Fit {
    match m {
        // The status bar look is the Pill theme's; the other themes show the clock for it.
        Mode::Idle if matches!(st.settings.idle_style, IdleStyle::Clock | IdleStyle::Status) => Fit::Ears,
        Mode::Idle => Fit::Inside,
        // The music pill sings the current line (every theme), which needs
        // the width the camera takes: it hangs below the notch while there
        // is a line (Pill floats there).
        Mode::Activity(Activity::Media) if pill::lyric::current(st).is_some() => Fit::Below,
        Mode::Activity(_) => Fit::Ears,
        Mode::Hud(_) | Mode::Notice | Mode::Expanded(_) => Fit::Below,
    }
}

/// The Pill theme floats `m` below the menu bar instead of hanging it from the notch.
pub fn floats(m: &Mode, st: &AppState) -> bool {
    pill::on(st.settings.theme) && fit(m, st) == Fit::Below
}

/// Where the Pill theme's island floats in `m` around notch `n`: `TOP_GAP`
/// under the menu bar for what hangs below the notch, `None` (attached to
/// it, as in the other themes) in the menu bar's line.
pub fn pill_gap(m: &Mode, st: &AppState, n: Notch) -> Option<f32> {
    floats(m, st).then_some(n.h + pill::TOP_GAP)
}

/// How far an island `top` DIPs below the edge has come away from it: 0
/// attached, 1 all the way out (below the menu bar on a notch). The pill
/// under the pointer rises a little into its gap and still counts as out.
pub fn detach(top: f32, notch: Option<Notch>) -> f32 {
    let rise = pill::motion::HOVER_OUTSET - pill::motion::HOVER_SINK;
    let full = notch.map_or(0.0, |n| n.h) + pill::TOP_GAP - rise;
    (top / full.max(1.0)).clamp(0.0, 1.0)
}

/// The black strip a hanging island keeps behind the notch, over its content
/// (none when Pill floats below).
pub fn strip(m: &Mode, st: &AppState, n: Notch) -> f32 {
    if fit(m, st) == Fit::Below && !floats(m, st) {
        n.h
    } else {
        0.0
    }
}

/// Width of each ear in an ears fit.
fn ear(m: &Mode, st: &AppState) -> f32 {
    match m {
        Mode::Idle if pill::on(st.settings.theme) => {
            if st.power.has_battery {
                if st.settings.use_24h {
                    PILL_EAR_24H
                } else {
                    PILL_EAR_12H
                }
            } else if st.settings.use_24h {
                PILL_EAR_TIME_ONLY
            } else {
                PILL_EAR_12H
            }
        }
        Mode::Idle => {
            let clock = if st.settings.use_24h { 64.0 } else { 84.0 };
            if oyen::on(st.settings.theme) {
                clock + 6.0
            } else {
                clock
            }
        }
        Mode::Activity(Activity::Timer) => {
            let long = fmt_countdown(st.pomodoro.remaining(st.now)).len() > 5;
            if long {
                80.0
            } else {
                66.0
            }
        }
        Mode::Activity(Activity::MediaTimer) => 96.0,
        // "10 min" in the right ear (Pill's digits are bigger).
        Mode::Activity(Activity::Prayer) => {
            if pill::on(st.settings.theme) {
                80.0
            } else {
                66.0
            }
        }
        _ => 56.0,
    }
}

/// The island's size for `m` around notch `n`; `base` is its size without one.
pub fn geom(m: &Mode, st: &AppState, n: Notch, base: Geom) -> Geom {
    match fit(m, st) {
        // A hair smaller than the notch on every side, so not a pixel of it
        // shows; a larger corner than the notch's own stays hidden too.
        Fit::Inside => Geom::new(n.w - 2.0, n.h - 1.0, n.h / 2.0),
        Fit::Ears => Geom::new(n.w + 2.0 * ear(m, st), n.h, EARS_R),
        // Pill floats below the menu bar at its own size.
        Fit::Below if floats(m, st) => base,
        Fit::Below => Geom::new(base.w.max(n.w + 2.0 * BELOW_EAR), base.h + n.h, base.r),
    }
}

/// Where the content of `m` draws inside the island's `body`.
pub fn content_body(m: &Mode, st: &AppState, n: Notch, body: Rect) -> Rect {
    let strip = strip(m, st, n);
    Rect::new(body.x, body.y + strip, body.w, (body.h - strip).max(0.0))
}

/// The shoulders grow in as the island comes out of the notch (inside it,
/// flares would poke out beside the camera).
pub fn shoulder(n: Notch, w: f32) -> f32 {
    SHOULDER * clamp01((w - n.w) / 24.0)
}

/// The Pill theme's outline `detach` of the way from hanging off the top
/// edge (0: flat top flush with the edge, concave `shoulder`s, bottom
/// corners `radius`, as `Canvas::notch`) to floating below it (1: every
/// corner `radius`, as `pill::shape::segments`). The shoulders shrink away
/// first, then the top corners round, so the shape never jumps between the
/// two. `body` may sit below the edge (its top is the outline's top).
pub fn detached_segments(body: Rect, radius: f32, shoulder: f32, detach: f32) -> Vec<Seg> {
    let d = clamp01(detach);
    let (x0, y0, x1, y1) = (body.x, body.y, body.right(), body.bottom());
    let rb = radius.min(body.h / 2.0).min(body.w / 2.0).max(0.0);
    let s = (shoulder * (1.0 - 2.0 * d)).clamp(0.0, body.h);
    let rt = rb * (2.0 * d - 1.0).max(0.0);
    let mut segs = Vec::with_capacity(12);
    if s > 0.0 {
        segs.push(Seg::MoveTo(x0 - s, y0));
        segs.push(Seg::ArcTo(s, s, 0.0, false, true, x0, y0 + s));
    } else {
        segs.push(Seg::MoveTo(x0 + rt, y0));
        if rt > 0.0 {
            segs.push(Seg::ArcTo(rt, rt, 0.0, false, false, x0, y0 + rt));
        }
    }
    segs.push(Seg::LineTo(x0, (y1 - rb).max(y0 + s.max(rt))));
    segs.push(Seg::ArcTo(rb, rb, 0.0, false, false, x0 + rb, y1));
    segs.push(Seg::LineTo(x1 - rb, y1));
    segs.push(Seg::ArcTo(rb, rb, 0.0, false, false, x1, (y1 - rb).max(y0 + s.max(rt))));
    if s > 0.0 {
        segs.push(Seg::LineTo(x1, y0 + s));
        segs.push(Seg::ArcTo(s, s, 0.0, false, true, x1 + s, y0));
    } else {
        segs.push(Seg::LineTo(x1, y0 + rt));
        if rt > 0.0 {
            segs.push(Seg::ArcTo(rt, rt, 0.0, false, false, x1 - rt, y0));
        }
    }
    segs.push(Seg::Close);
    segs
}

/// The Pill theme's outline for a mask, `detach` of the way to floating.
pub fn pill_outline(body: Rect, radius: f32, shoulder: f32, detach: f32) -> Vec<Seg> {
    if detach >= 1.0 {
        pill::shape::segments(body, radius)
    } else {
        detached_segments(body, radius, shoulder, detach)
    }
}

/// The Pill theme's island body while it floats or is on its way there:
/// the shared floating body (outline, hairline, shadow) once it is all the
/// way out, the blended outline before that. The shadow fades in over the
/// second half of the way out (it would pop in at the end), and the
/// hairline waits until the shoulders are gone (Classic's never runs along
/// the screen edge either).
pub fn draw_pill_body(c: &mut Canvas, body: Rect, radius: f32, shoulder: f32, detach: f32, fill: Color, border: Option<Color>) {
    if detach >= 1.0 {
        pill::shape::draw_body(c, body, radius, fill, border);
        return;
    }
    let fade = (2.0 * detach - 1.0).clamp(0.0, 1.0) * fill.a;
    if fade > 0.0 && c.on_gpu() {
        let b = pill::shape::snap(body, c.scale());
        let r = radius.min(b.h / 2.0).min(b.w / 2.0).max(0.0);
        c.rrect_shadow(b.offset(0.0, pill::shape::SHADOW_DROP), r, Color::black(fade), &pill::shape::SHADOW_RAMP);
    }
    c.path(&detached_segments(body, radius, shoulder, detach), Some(fill), None);
    if let Some(b) = border.filter(|_| detach >= 0.5) {
        c.path(&detached_segments(body.inset(0.5, 0.5), (radius - 0.5).max(0.0), 0.0, detach), None, Some((b, 1.0)));
    }
}

/// Draw `m`'s ears content. Returns false when the shared activity drawing
/// already fits (album art and equalizer sit at the island's two ends).
pub fn draw_ears(ui: &mut Ui, st: &mut AppState, m: &Mode, body: Rect, n: Notch) -> bool {
    let ear_w = ((body.w - n.w) / 2.0).max(0.0);
    let left = Rect::new(body.x, body.y, ear_w, body.h);
    let right = Rect::new(body.right() - ear_w, body.y, ear_w, body.h);
    if pill::on(st.settings.theme) {
        return draw_pill_ears(ui, st, m, body, left, right);
    }
    match m {
        Mode::Idle => {
            if oyen::on(st.settings.theme) {
                // Oyen rests in the left ear and watches the cursor.
                let look = ui.input.mouse.map(|p| (p.x - body.center().x) / (body.w / 2.0));
                let (frame, moving) = oyen::rest(st.now, st.local.hour < 5, look);
                if moving {
                    ui.request_anim();
                }
                oyen::peek(ui.c, &frame, left.center().x + 4.0, body.bottom() - 5.0, 22.0, body.y + 1.0);
            }
            let time = fmt_clock(&st.local, st.settings.use_24h);
            let ts = TextStyle::body().weight(Weight::Semibold).tabular().center();
            ui.c.text(&time, right.inset(4.0, 0.0), &ts);
            true
        }
        Mode::Activity(Activity::Timer) => {
            let p = &st.pomodoro;
            let col = match p.phase {
                Phase::Focus => palette::ORANGE,
                Phase::ShortBreak | Phase::LongBreak => palette::GREEN,
            };
            let paused = !p.is_running();
            let c = Point::new(left.center().x + 6.0, body.center().y);
            ui.c.ring(c, 9.0, 2.4, p.progress(st.now), RING_TRACK, col);
            let glyph = Rect::from_center(c, 9.0, 9.0);
            if paused {
                ui.c.icon(Icon::Pause, glyph, palette::TEXT_DIM);
            } else if p.phase != Phase::Focus {
                ui.c.icon(Icon::Coffee, glyph, col);
            }
            let text = fmt_countdown(p.remaining(st.now));
            let ts = TextStyle::body().weight(Weight::Semibold).tabular().color(if paused { palette::TEXT_DIM } else { col }).center();
            ui.c.text(&text, Rect::new(right.x, body.y, right.w - 6.0, body.h), &ts);
            true
        }
        // The moon left and the minutes right: the prayer's name would sit
        // under the camera (the open island and the notice at the time say it).
        Mode::Activity(Activity::Prayer) => {
            let Some((_, mins)) = activities::prayer_countdown(st) else { return true };
            let col = st.accent;
            ui.c.icon(Icon::Moon, Rect::from_center(Point::new(left.center().x + 6.0, body.center().y), 16.0, 16.0), col);
            let ts = TextStyle::body().weight(Weight::Semibold).tabular().color(col).center();
            ui.c.text(&format!("{mins} min"), Rect::new(right.x, body.y, right.w - 6.0, body.h), &ts);
            true
        }
        // Art left and bars (or Oyen) right, no words under the camera: the
        // sung line hangs below the notch instead (`fit`).
        Mode::Activity(Activity::Media) => {
            activities::draw_music(ui, st, body, false);
            true
        }
        _ => false,
    }
}

/// The Pill theme in the menu bar's line: its resting pill split by the
/// camera (the time hard left, the battery hard right, as at rest without a
/// notch) and the timer the same way (ring left, digits right). Music falls
/// to the shared drawing: its art and equalizer sit at the two ends.
fn draw_pill_ears(ui: &mut Ui, st: &mut AppState, m: &Mode, body: Rect, left: Rect, right: Rect) -> bool {
    let cy = body.center().y;
    match m {
        Mode::Idle => {
            let time = fmt_clock(&st.local, st.settings.use_24h);
            let ts = TextStyle::headline().tabular().color(Color::white(1.0));
            ui.c.text(&time, Rect::new(left.x + PILL_END, body.y, (left.w - PILL_END - PILL_CAMERA).max(0.0), body.h), &ts);
            if st.power.has_battery {
                pill_battery(ui, st, right.right() - PILL_END, cy);
            }
            true
        }
        // Art left and bars right, nothing under the camera, as the other
        // themes draw music around a notch: Pill's music pill fills its middle
        // with words, which the camera would cut (its sung line floats below
        // the notch instead). Classic's drawing, without the words.
        Mode::Activity(Activity::Media) => {
            activities::draw_music(ui, st, body, false);
            true
        }
        Mode::Activity(Activity::Timer) => {
            let p = &st.pomodoro;
            let col = match p.phase {
                Phase::Focus => st.accent,
                Phase::ShortBreak | Phase::LongBreak => palette::GREEN,
            };
            let paused = !p.is_running();
            // A 14 DIP ring (2.5 stroke) 12 in from the end, the digits 14 in from the other.
            let c = Point::new(left.x + 12.0 + 7.0, cy);
            ui.c.ring(c, 7.0 - 1.25, 2.5, p.progress(st.now), Color::white(0.22), col);
            if paused {
                ui.c.icon(Icon::Pause, Rect::from_center(c, 7.0, 7.0), palette::TEXT_DIM);
            }
            let text = fmt_countdown(p.remaining(st.now));
            let ts = TextStyle::headline().tabular().color(if paused { palette::TEXT_DIM } else { Color::white(1.0) }).right();
            ui.c.text(&text, Rect::new(right.x + PILL_CAMERA, body.y, (right.w - PILL_CAMERA - 14.0).max(0.0), body.h), &ts);
            true
        }
        // Pill's moon at one end and the minutes at the other, as the timer:
        // its line, with the prayer's name, would sit under the camera.
        Mode::Activity(Activity::Prayer) => {
            let Some((_, mins)) = activities::prayer_countdown(st) else { return true };
            pill::live::moon(ui, Point::new(left.x + 12.0 + 7.0, cy), 14.0, st.accent);
            let ts = TextStyle::headline().tabular().color(Color::white(1.0)).right();
            ui.c.text(&format!("{mins} min"), Rect::new(right.x + PILL_CAMERA, body.y, (right.w - PILL_CAMERA - 14.0).max(0.0), body.h), &ts);
            true
        }
        _ => false,
    }
}

/// Snap a DIP coordinate to the device-pixel grid (crisp small outlines).
fn snap(ui: &Ui, v: f32) -> f32 {
    let s = ui.c.scale().max(0.1);
    (v * s).round() / s
}

/// Pill's battery, right-aligned to `right` and centred on `cy`, as the
/// status bar draws it (`pill::status::battery`): the percentage in grey
/// (red when low), then the battery, green (red when low), with a bolt while
/// it charges or is plugged in.
fn pill_battery(ui: &mut Ui, st: &AppState, right: f32, cy: f32) {
    let p = st.power;
    // Plugged in counts as charging, as the status bar and the Charging
    // pop-up have it: a full battery on its charger keeps the bolt.
    let charging = p.charging || p.on_ac;
    let x = snap(ui, right - NUB_W - BATTERY.0);
    let y = snap(ui, cy - BATTERY.1 / 2.0);
    pill::status::battery(ui, Rect::new(x, y, BATTERY.0, BATTERY.1), p.percent, charging);
    let label = format!("{}%", p.percent.min(100));
    let color = if pill::status::low(p.percent, charging) { palette::RED } else { PILL_DIM };
    let ts = TextStyle::body().weight(Weight::Medium).tabular().color(color).right();
    let w = 40.0;
    ui.c.text(&label, Rect::new(x - PCT_GAP - w, cy - 10.0, w, 20.0), &ts);
}
