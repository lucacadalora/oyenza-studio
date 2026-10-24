//! The Oyen theme: Oyen, the Oyenza cat, living in the island.
//!
//! Oyen is drawn from the Oyenza app's own built face ([`poni_core::oyen`]),
//! never from a picture, so he is the same character, with the same moods, as
//! in the app. He peeks over a line near the island's bottom edge the way the
//! Oyen Poni logo does, his chin hidden behind it. (The logo's cream paws are
//! left out: at island size they read as teeth, and Oyen has no mouth.)
//!
//! Moods only ever follow what the island is doing (resting, music, the
//! cursor, the night), never an amount of money.

use crate::gfx::Canvas;
use poni_core::geom::{Color, Point, Rect};
use poni_core::oyen::{self, Element, Frame, Input, Mood, Paint, Path, Place};
use poni_core::settings::Theme;
use poni_core::svgpath::Seg;

/// Oyen's orange: the theme's accent.
pub const ORANGE: Color = Color::hex(oyen::ORANGE);
const FOLD: Color = Color::hex(oyen::FOLD_ORANGE);
const INK: Color = Color::hex(oyen::INK);
const PINK: Color = Color::hex(oyen::PINK);
const GREEN: Color = Color::hex(oyen::GREEN);
/// Shapes that float around the head (thinking dots, z's) are ink on a light
/// page; on the black island they need to be light.
const FLOAT: Color = Color::white(0.85);
const FLOAT_ON_LIGHT: f32 = 0.85;

/// The mark's head is 104 units wide. The logo's island bar starts at y 23,
/// just under the eyes.
const MARK_W: f32 = 104.0;
const BAR_TOP: f32 = 23.0;
/// The mark's small cut grows the eyes for sizes of 32 px and below.
const SMALL_EYES: f64 = 1.19;

/// How long one rest cycle lasts: a blink at its start, an ear flick every
/// third cycle. Between moves Oyen is a still frame, so the app sleeps.
const REST_CYCLE: f64 = 6.5;
const BLINK_WINDOW: f64 = 0.4;

pub fn on(theme: Theme) -> bool {
    theme == Theme::Oyen
}

/// Is a rest move (blink or ear flick) under way at `now`?
fn rest_moving(now: f64) -> bool {
    now.rem_euclid(REST_CYCLE) < BLINK_WINDOW
}

/// When the next rest move starts.
pub fn next_rest_move(now: f64) -> f64 {
    now - now.rem_euclid(REST_CYCLE) + REST_CYCLE
}

/// What Oyen does while the island rests. `look` is the cursor's horizontal
/// place over the island (-1 left .. 1 right) while it hovers: his eyes
/// follow it, glancing down towards it. Returns the frame and whether it
/// moves on its own (the caller then keeps frames coming).
pub fn rest(now: f64, night: bool, look: Option<f32>) -> (Frame, bool) {
    if night {
        // Asleep, drawn still: no breathing frames at night.
        return (oyen::frame(Mood::Sleep, 0.6, Input::default(), SMALL_EYES), false);
    }
    if let Some(x) = look {
        let input = Input { pencil: x.clamp(-1.0, 1.0) as f64, ..Input::default() };
        return (oyen::frame(Mood::Nulis, 0.0, input, SMALL_EYES), false);
    }
    let phase = now.rem_euclid(REST_CYCLE);
    if !rest_moving(now) {
        return (oyen::frame(Mood::Oyen, 0.0, Input::default(), SMALL_EYES), false);
    }
    // The idle mood blinks at 1.7 s and flicks the folded ear at 0.6 s.
    let flick = (now / REST_CYCLE).floor() as i64 % 3 == 2;
    let t = if flick { 0.55 + phase } else { 1.6 + phase };
    (oyen::frame(Mood::Oyen, t, Input::default(), SMALL_EYES), true)
}

/// Bobbing to the music while it plays; resting, eyes open, while paused.
pub fn music(now: f64, playing: bool) -> Frame {
    if playing {
        oyen::frame(Mood::Happy, now.rem_euclid(Mood::Happy.loop_secs()), Input::default(), SMALL_EYES)
    } else {
        oyen::frame(Mood::Oyen, 0.0, Input::default(), SMALL_EYES)
    }
}

/// Oyen peeking over the line at `bar_y`, his head `w` DIPs wide and centred
/// on `cx`. Only what is above the line shows (nothing above `top` either).
pub fn peek(c: &mut Canvas, frame: &Frame, cx: f32, bar_y: f32, w: f32, top: f32) {
    let k = w / MARK_W;
    let origin = Point::new(cx, bar_y - BAR_TOP * k);
    c.clip(Rect::new(cx - w, top, 2.0 * w, (bar_y - top).max(0.0)));
    draw(c, frame, origin, k);
    c.unclip();
}

/// One frame of the face, the mark's (0, 0) at `origin`, `k` DIPs per unit.
/// Same layering as the app: a raised ear tip behind the head, a folded one
/// on it, then eyes and extras. Oyen keeps his own colours in every theme.
fn draw(c: &mut Canvas, f: &Frame, origin: Point, k: f32) {
    let look = c.look();
    let float = if look.light { look.ink.with_alpha(FLOAT_ON_LIGHT) } else { FLOAT };
    c.unthemed(|c| draw_face(c, f, origin, k, float));
}

fn draw_face(c: &mut Canvas, f: &Frame, origin: Point, k: f32, float: Color) {
    let h = f.head;
    let map = |p: oyen::Point| {
        let q = h.apply(p);
        Point::new(origin.x + q.x as f32 * k, origin.y + q.y as f32 * k)
    };
    // Strokes scale with the head (it squashes a few percent when bouncing).
    let stroke_k = k * ((h.a * h.d - h.b * h.c).abs().sqrt() as f32);
    let tip = if f.tip.faces_up { ORANGE } else { FOLD };
    if f.tip.behind_head {
        c.path(&segs(&f.tip.path, &map), Some(tip), None);
    }
    c.path(&segs(oyen::head_path(), &map), Some(ORANGE), None);
    if !f.tip.behind_head {
        c.path(&segs(&f.tip.path, &map), Some(tip), None);
    }
    for s in f.eyes.iter().chain(&f.extras) {
        let color = match s.paint {
            Paint::Ink if s.place == Place::Eye => INK,
            Paint::Ink => float,
            Paint::Pink => PINK,
            Paint::Green => GREEN,
            Paint::White => Color::white(1.0),
        };
        let path = segs(&s.path, &map);
        c.with_alpha(s.opacity.clamp(0.0, 1.0) as f32, |c| match s.stroke {
            Some(w) => c.path(&path, None, Some((color, w as f32 * stroke_k))),
            None => c.path(&path, Some(color), None),
        });
    }
}

fn segs(p: &Path, map: &impl Fn(oyen::Point) -> Point) -> Vec<Seg> {
    p.elements
        .iter()
        .map(|e| match *e {
            Element::Move(a) => {
                let a = map(a);
                Seg::MoveTo(a.x, a.y)
            }
            Element::Line(a) => {
                let a = map(a);
                Seg::LineTo(a.x, a.y)
            }
            Element::Quad(c, a) => {
                let (c, a) = (map(c), map(a));
                Seg::QuadTo(c.x, c.y, a.x, a.y)
            }
            Element::Cubic(c1, c2, a) => {
                let (c1, c2, a) = (map(c1), map(c2), map(a));
                Seg::CubicTo(c1.x, c1.y, c2.x, c2.y, a.x, a.y)
            }
            Element::Close => Seg::Close,
        })
        .collect()
}
