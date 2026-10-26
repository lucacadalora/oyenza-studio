//! The island around a MacBook's camera notch.
//!
//! The notch has no pixels: whatever Poni draws in its middle is never seen.
//! So on a screen with a notch each island state picks one of three fits:
//!
//! * **Inside**: the resting island hides inside the notch (nothing shows;
//!   pointing at the notch still lifts and opens it).
//! * **Ears**: a live activity stays in the menu bar's line, its content
//!   left and right of the camera (the timer's ring on the left, its digits
//!   on the right; a recording's dot and time, album art and the equalizer
//!   the same way).
//! * **Below**: notices, pop-ups and the open island hang under the notch:
//!   a strip the notch's height on top, the usual content under it.
//!
//! The Eyes look rests in the ears too, and plays hide and seek with the
//! camera ([`draw_peek`]): the eyes (Oyen, in his theme) peek out beside it
//! on the side the pointer is on and look at the pointer wherever it is on
//! the screen; when it crosses over they duck behind the camera and come out
//! on the other side. Minimal and Hidden rest inside the notch.
//!
//! The Pill theme is the pill or the notch, never both ([`pill_fit`]): at
//! rest it goes back into the notch (the menu bar beside it already shows
//! the time and the battery), and whatever shows (music, from the first note
//! to the end of a pause, timers, pop-ups, the open card) floats as the pill
//! `pill::TOP_GAP` under the menu bar ([`floats`]), every corner rounded, no
//! strip behind the camera and no ears beside it. Only the Eyes look still
//! peeks out beside the camera. On its way between the two the island's gap
//! moves on its own spring (island.rs, `pill::motion::TOP`) and the outline
//! lets go of the edge smoothly ([`detached_segments`]).
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
use poni_core::peek;
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
/// The Eyes look's ears: room for the eyes (or Oyen) to peek out on either
/// side of the camera, this far from its edge.
const PEEK_EAR: f32 = 46.0;
const PEEK_GAP: f32 = 6.0;
/// The pair of eyes from one's outer edge to the other's (activities.rs
/// draws them 6.5 DIPs wide, 22 apart).
const EYES_W: f32 = 22.0 + 6.5;
/// How far the eyes turn toward the pointer (DIPs, across and down).
const LOOK: (f32, f32) = (2.5, 2.0);
/// Oyen's head in an ear, as beside the clock.
const PEEK_OYEN_W: f32 = 22.0;
/// Oyen watches the pointer this long after it moves, then rests again (he
/// has his blinks and ear flicks to do).
const OYEN_WATCH: f64 = 2.5;

pub fn fit(m: &Mode, st: &AppState) -> Fit {
    if pill::on(st.settings.theme) {
        return pill_fit(m, st);
    }
    match m {
        // The status bar look is the Pill theme's; the other themes show the
        // clock for it. The eyes peek out beside the camera (`draw_peek`).
        Mode::Idle if matches!(st.settings.idle_style, IdleStyle::Clock | IdleStyle::Status | IdleStyle::Eyes) => Fit::Ears,
        // Windows only (`AppState::aside`): a Mac never steps aside.
        Mode::Idle | Mode::Aside => Fit::Inside,
        // The music pill sings the current line (every theme), which needs
        // the width the camera takes: it hangs below the notch while there
        // is a line.
        Mode::Activity(Activity::Media) if pill::lyric::current(st).is_some() => Fit::Below,
        Mode::Activity(_) => Fit::Ears,
        Mode::Hud(_) | Mode::Notice | Mode::Expanded(_) => Fit::Below,
    }
}

/// The Pill theme around a notch: at rest the real notch alone (the eyes
/// still peek out beside it), everything else the floating pill. The music
/// pill floats for as long as it shows, so it never jumps up into the
/// menu bar's line when a song has no line to sing (the intro, a break, a
/// pause, a song without synced lyrics) and back down at the next one.
fn pill_fit(m: &Mode, st: &AppState) -> Fit {
    match m {
        Mode::Idle if st.settings.idle_style == IdleStyle::Eyes => Fit::Ears,
        Mode::Idle | Mode::Aside => Fit::Inside,
        Mode::Activity(_) | Mode::Hud(_) | Mode::Notice | Mode::Expanded(_) => Fit::Below,
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
        Mode::Idle if st.settings.idle_style == IdleStyle::Eyes => PEEK_EAR,
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
        // "12:34" in the right ear, as the timer's; "1:02:03" past an hour.
        Mode::Activity(Activity::Recording) => {
            if st.rec_elapsed() >= 3600.0 {
                80.0
            } else {
                66.0
            }
        }
        // "10 min" in the right ear.
        Mode::Activity(Activity::Prayer) => 66.0,
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
    if *m == Mode::Idle && st.settings.idle_style == IdleStyle::Eyes {
        draw_peek(ui, st, body, n);
        return true;
    }
    let ear_w = ((body.w - n.w) / 2.0).max(0.0);
    let left = Rect::new(body.x, body.y, ear_w, body.h);
    let right = Rect::new(body.right() - ear_w, body.y, ear_w, body.h);
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
        // A screen recording: its dot left of the camera, its time right.
        Mode::Activity(Activity::Recording) => {
            activities::recording_mark(ui, st, Point::new(left.center().x + 6.0, body.center().y));
            let (text, ts) = activities::recording_time(st);
            ui.c.text(&text, Rect::new(right.x, body.y, right.w - 6.0, body.h), &ts.center());
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

/// The Eyes look around a notch: the eyes (Oyen, in his theme) peek out
/// beside the camera on the pointer's side and look at it, wherever it is
/// on the screen. When it crosses over, they duck behind the camera, which
/// hides their way across, and come out on the other side. The same in
/// every theme: Pill and Glass rest in the menu bar's line in black here.
fn draw_peek(ui: &mut Ui, st: &mut AppState, body: Rect, n: Notch) {
    let now = st.now;
    let (spot, moving) = peek_spot(st, n, now);
    if moving {
        ui.request_anim();
    }
    let look = peek_look(st, spot, now);
    let x = body.center().x + spot.x;
    if oyen::on(st.settings.theme) {
        let (frame, moving) = oyen::rest(now, st.local.hour < 5, look.map(|l| l.0));
        if moving {
            ui.request_anim();
        }
        oyen::peek(ui.c, &frame, x, body.bottom() - 5.0, PEEK_OYEN_W, body.y + 1.0);
    } else {
        let (gx, gy) = look.unwrap_or((0.0, 0.0));
        activities::draw_eyes(ui, st, x, body.center().y + gy * LOOK.1, gx * LOOK.0);
    }
}

/// Where the peeking eyes (Oyen, in his theme) are at `now`, from the
/// island's top centre, and whether they are on their way across.
fn peek_spot(st: &AppState, n: Notch, now: f64) -> (Point, bool) {
    let w = if oyen::on(st.settings.theme) { PEEK_OYEN_W } else { EYES_W };
    let (along, moving) = st.ui.peek.along(now);
    (Point::new(along * (n.w / 2.0 + PEEK_GAP + w / 2.0), n.h / 2.0), moving)
}

/// Where the eyes at `spot` look at `now` (-1 to 1 each way): at the
/// pointer, wherever it is. Oyen only watches it for a moment after it
/// moves, and not while he sleeps.
fn peek_look(st: &AppState, spot: Point, now: f64) -> Option<(f32, f32)> {
    let p = &st.ui.peek;
    if oyen::on(st.settings.theme) && (st.local.hour < 5 || !p.moved_within(now, OYEN_WATCH)) {
        return None;
    }
    p.pointer.map(|at| peek::gaze(spot, at))
}

/// What the peeking eyes show at `now` for the pointer they have: their
/// side and where they look. The app redraws them when it changes.
pub fn peek_key(st: &AppState, n: Notch, now: f64) -> (bool, Option<(f32, f32)>) {
    let (spot, _) = peek_spot(st, n, now);
    (st.ui.peek.left, peek_look(st, spot, now))
}
