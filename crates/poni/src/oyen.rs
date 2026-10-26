//! The Oyen theme: Oyen, the Oyenza cat, living in the island.
//!
//! Oyen is drawn from the Oyenza app's own built face ([`poni_core::oyen`]),
//! never from a picture, so he is the same character, with the same moods, as
//! in the app. He peeks over a line near the island's bottom edge the way the
//! Oyenza Studio logo does, his chin hidden behind it. (The logo's cream paws are
//! left out: at island size they read as teeth, and Oyen has no mouth.)
//!
//! Moods only ever follow what the island is doing (resting, music, the
//! cursor, the night), never an amount of money.

use crate::gfx::Canvas;
use poni_core::geom::{Color, Point, Rect};
use poni_core::oyen::{self, Affine, Element, Frame, Input, Mood, Paint, Path, Place};
use poni_core::settings::Theme;
use poni_core::svgpath::{self, Seg};
use std::f32::consts::TAU;
use std::f64::consts::PI;
use std::sync::OnceLock;

/// Oyen's orange: the theme's accent.
pub const ORANGE: Color = Color::hex(oyen::ORANGE);
const FOLD: Color = Color::hex(oyen::FOLD_ORANGE);
const INK: Color = Color::hex(oyen::INK);
const PINK: Color = Color::hex(oyen::PINK);
const GREEN: Color = Color::hex(oyen::GREEN);
/// The Oyenza app icon's ground.
const ICON_TILE: Color = Color::hex(0x171717);
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

/// Is a rest move under way at `now`, or within `margin` seconds of one? The
/// self-test waits for a still Oyen before it takes a picture.
pub fn mid_rest_move(now: f64, margin: f64) -> bool {
    let phase = now.rem_euclid(REST_CYCLE);
    phase < BLINK_WINDOW + margin || phase > REST_CYCLE - margin
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

/// Bopping to the music while it plays: eyes shut in a smile, bouncing on the
/// beat and leaning a little to each side. Resting, eyes open, once it pauses
/// (the lean straightens out first).
pub fn music(now: f64, playing: bool, paused_at: Option<f64>) -> Frame {
    let (mut f, lean) = if playing {
        (oyen::frame(Mood::Happy, now.rem_euclid(Mood::Happy.loop_secs()), Input::default(), SMALL_EYES), lean(now))
    } else {
        let left = paused_at.map_or(0.0, |p| 1.0 - smooth(((now - p) / SETTLE) as f32) as f64);
        (oyen::frame(Mood::Oyen, 0.0, Input::default(), SMALL_EYES), paused_at.map_or(0.0, lean) * left)
    };
    if lean != 0.0 {
        f.head = f.head.then(Affine::rotation_about(lean, oyen::Point::new(0.0, oyen::CHIN)));
    }
    f
}

/// Does the media pill still move on its own: music playing, notes still in
/// the air, or Oyen straightening up after a pause?
pub fn music_moving(now: f64, playing: bool, paused_at: Option<f64>, notes: &Notes) -> bool {
    playing || notes.flying() || paused_at.is_some_and(|p| now < p + SETTLE)
}

/// His lean at `t` (degrees, clockwise): right, then left, over two beats,
/// upright on every landing.
fn lean(t: f64) -> f64 {
    LEAN_DEG * (PI * t / BEAT).sin()
}

// ---------------------------------------------------------------- notes

/// Oyen's beat while music plays: the happy mood bounces twice a loop, so his
/// head lands every 0.6 s (100 bpm). A note leaves on each landing.
pub const BEAT: f64 = 0.6;
/// How long a note floats, from leaving Oyen to gone.
pub const NOTE_LIFE: f64 = 1.9;
/// A note's pop as it leaves.
const POP: f64 = 0.22;
/// A gap this long between frames means the pill was away (paused, a page
/// open): the notes start afresh on the next beat instead of catching up.
const GAP: f64 = 0.25;
/// How far Oyen leans at the top of a bounce (degrees), and how long he takes
/// to straighten up when the music pauses.
const LEAN_DEG: f64 = 5.0;
const SETTLE: f64 = 0.3;
/// Where a note leaves (DIPs above his line) and how far it rises.
const NOTE_START: f32 = 5.5;
const NOTE_RISE: f32 = 16.5;

/// The notes in the air over the media pill: one leaves on each beat while
/// the music plays, floats up for [`NOTE_LIFE`] and is gone. A pause stops new
/// ones; those in the air finish their way up.
#[derive(Clone, Debug, Default)]
pub struct Notes {
    /// Beats (counted from mono time 0) whose notes are in the air.
    born: Vec<i64>,
    /// The beat at the last step, and when that step was.
    beat: i64,
    seen: f64,
}

impl Notes {
    /// Bring the notes up to `now`: a beat that passes while the music plays
    /// sends one off; notes past their life drop out.
    pub fn step(&mut self, now: f64, playing: bool) {
        let beat = (now / BEAT).floor() as i64;
        let fresh = now - self.seen > GAP || now < self.seen;
        if playing && !fresh && beat > self.beat {
            self.born.push(beat);
        }
        self.beat = beat;
        self.seen = now;
        self.born.retain(|&b| now - b as f64 * BEAT < NOTE_LIFE);
    }

    pub fn flying(&self) -> bool {
        !self.born.is_empty()
    }
}

/// One note's way up: where it leaves (DIPs right of Oyen's middle), how far
/// it drifts sideways while rising, a pair (♫) or one (♪), deep orange or
/// Oyen's own (a little smaller), and where its sway starts (in turns).
struct Flight {
    x: f32,
    drift: f32,
    pair: bool,
    deep: bool,
    phase: f32,
}

/// Eight beats of notes, then the same again. Most leave on his left, near
/// him or further out; two go up the narrow side on his right, far enough
/// apart that they never meet there.
const FLIGHTS: [Flight; 8] = [
    Flight { x: -16.5, drift: -7.0, pair: false, deep: true, phase: 0.0 },
    Flight { x: 15.0, drift: 2.5, pair: true, deep: false, phase: 0.35 },
    Flight { x: -26.0, drift: -6.0, pair: true, deep: false, phase: 0.6 },
    Flight { x: -17.0, drift: -6.5, pair: true, deep: true, phase: 0.8 },
    Flight { x: -25.5, drift: -7.0, pair: false, deep: false, phase: 0.25 },
    Flight { x: 16.0, drift: 3.0, pair: false, deep: true, phase: 0.15 },
    Flight { x: -17.5, drift: -6.0, pair: false, deep: true, phase: 0.45 },
    Flight { x: -27.0, drift: -6.5, pair: true, deep: false, phase: 0.7 },
];

/// ♪ and ♫ (original drawings), about 10 units tall around (0, 0): tilted
/// heads, stems, a flag or a beam. Every part runs clockwise, so where parts
/// overlap they fill as one shape.
const ONE: &str = "M-3.761 3.787A2.3 1.7 -20 0 1 0.561 2.213A2.3 1.7 -20 0 1 -3.761 3.787Z\
    M-0.612 -5.2H0.638V2.65H-0.612Z\
    M0.638 -5.2C2.9 -4.4 4 -2.8 3.3 -0.4C3 -1.9 2 -2.7 0.638 -2.9Z";
const TWO: &str = "M-5.261 4.387A2.3 1.7 -20 0 1 -0.939 2.813A2.3 1.7 -20 0 1 -5.261 4.387Z\
    M0.939 3.387A2.3 1.7 -20 0 1 5.261 1.813A2.3 1.7 -20 0 1 0.939 3.387Z\
    M-2.112 -4.4H-0.862V3.25H-2.112Z\
    M4.088 -5.4H5.338V2.25H4.088Z\
    M-2.112 -4.4L5.338 -5.4V-2.9L-2.112 -1.9Z";

fn glyph(pair: bool) -> &'static [Seg] {
    static GLYPHS: OnceLock<[Vec<Seg>; 2]> = OnceLock::new();
    let g = GLYPHS.get_or_init(|| [ONE, TWO].map(|d| svgpath::parse(d).unwrap_or_default()));
    &g[pair as usize]
}

/// The notes in the air, around Oyen at (`cx`, `bar_y`) (his place in the
/// pill), inside `clip`. `left` dims the ones on his left, where a new
/// track's title passes.
pub fn notes(c: &mut Canvas, notes: &Notes, now: f64, cx: f32, bar_y: f32, clip: Rect, left: f32) {
    if !notes.flying() {
        return;
    }
    c.clip(clip);
    for &b in &notes.born {
        let f = &FLIGHTS[b.rem_euclid(FLIGHTS.len() as i64) as usize];
        let age = now - b as f64 * BEAT;
        let u = (age / NOTE_LIFE).clamp(0.0, 1.0) as f32;
        let alpha = ((age / 0.08).min(1.0) as f32) * (1.0 - smooth((u - 0.55) / 0.45)) * if f.x < 0.0 { left } else { 1.0 };
        if alpha <= 0.004 {
            continue;
        }
        // Off quickly, slowing as it climbs; a little sway and spin all the way.
        let rise = 1.0 - (1.0 - u).powf(1.6);
        let turn = TAU * (u * 1.1 + f.phase);
        let at = Point::new(cx + f.x + f.drift * rise + 1.4 * turn.sin(), bar_y - NOTE_START - NOTE_RISE * rise);
        let spin = f.x.signum() * 9.0 + 12.0 * (0.85 * turn + 0.6).sin();
        let size = if f.deep { 0.8 } else { 0.72 } * (0.4 + 0.6 * back_out((age / POP) as f32)) * (1.0 - 0.15 * u);
        let color = if f.deep { FOLD } else { ORANGE };
        let segs = place(glyph(f.pair), at, spin, size);
        c.with_alpha(alpha, |c| c.unthemed(|c| c.path(&segs, Some(color), None)));
    }
    c.unclip();
}

/// `segs` turned `deg` degrees clockwise, scaled by `k` and moved to `at`.
fn place(segs: &[Seg], at: Point, deg: f32, k: f32) -> Vec<Seg> {
    let (sin, cos) = deg.to_radians().sin_cos();
    let map = |x: f32, y: f32| (at.x + k * (x * cos - y * sin), at.y + k * (x * sin + y * cos));
    segs.iter()
        .map(|s| match *s {
            Seg::MoveTo(x, y) => {
                let (x, y) = map(x, y);
                Seg::MoveTo(x, y)
            }
            Seg::LineTo(x, y) => {
                let (x, y) = map(x, y);
                Seg::LineTo(x, y)
            }
            Seg::QuadTo(x1, y1, x, y) => {
                let ((x1, y1), (x, y)) = (map(x1, y1), map(x, y));
                Seg::QuadTo(x1, y1, x, y)
            }
            Seg::CubicTo(x1, y1, x2, y2, x, y) => {
                let ((x1, y1), (x2, y2), (x, y)) = (map(x1, y1), map(x2, y2), map(x, y));
                Seg::CubicTo(x1, y1, x2, y2, x, y)
            }
            Seg::ArcTo(rx, ry, rot, large, sweep, x, y) => {
                let (x, y) = map(x, y);
                Seg::ArcTo(rx * k, ry * k, rot + deg, large, sweep, x, y)
            }
            Seg::Close => Seg::Close,
        })
        .collect()
}

/// Smoothstep on 0..1 (clamped).
fn smooth(x: f32) -> f32 {
    let v = x.clamp(0.0, 1.0);
    v * v * (3.0 - 2.0 * v)
}

/// 0 to 1 with a small overshoot on the way (clamped).
fn back_out(x: f32) -> f32 {
    const S: f32 = 1.70158;
    let v = x.clamp(0.0, 1.0) - 1.0;
    1.0 + v * v * ((S + 1.0) * v + S)
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

// ---------------------------------------------------------------- the Oyenza tab

/// How long Oyen takes to rise into the Oyenza tab as the island opens.
const TAB_RISE: f64 = 0.45;
/// His arrival, rise and wave: the Hello mood's wave is over by 1.2 s (its
/// last 0.4 s only hold still), so the frames stop there.
const TAB_ARRIVE: f64 = 1.2;

/// A move Oyen makes in the Oyenza tab, by himself, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabMove {
    /// The island opened: he rises into his tab, waving (the Hello mood).
    Arrive,
    /// His tab was picked: a happy hop.
    Picked,
}

/// Oyen as the Oyenza tab's icon: the Oyenza mark, his whole head `w` DIPs
/// wide, centred on `center` and kept inside `clip` (the tab). `moved`: his
/// last move of his own and when it started; after it he rests (a blink, an
/// ear flick now and then) and, with the pointer over the tab row, watches it
/// (`look`, -1 left .. 1 right of him). At night he sleeps. Returns whether he
/// is moving by himself (the caller keeps frames coming until he stops).
#[allow(clippy::too_many_arguments)]
pub fn tab(c: &mut Canvas, center: Point, w: f32, clip: Rect, now: f64, moved: Option<(TabMove, f64)>, night: bool, look: Option<f32>) -> bool {
    // At night he rises asleep, without the wave.
    let arrive = if night { TAB_RISE } else { TAB_ARRIVE };
    let (frame, rise, moving) = match moved.map(|(m, at)| (m, now - at)) {
        Some((TabMove::Arrive, s)) if s < arrive => {
            let rise = 1.0 - back_out((s / TAB_RISE) as f32);
            let frame = if night { oyen::frame(Mood::Sleep, 0.6, Input::default(), SMALL_EYES) } else { oyen::frame(Mood::Hello, s, Input::default(), SMALL_EYES) };
            (frame, rise, true)
        }
        Some((TabMove::Picked, s)) if s < Mood::Happy.loop_secs() && !night => (oyen::frame(Mood::Happy, s, Input::default(), SMALL_EYES), 0.0, true),
        _ => {
            let (frame, moving) = rest(now, night, look);
            (frame, 0.0, moving)
        }
    };
    let k = w / MARK_W;
    c.clip(clip);
    draw(c, &frame, Point::new(center.x, center.y + 12.0 * k + rise * clip.h), k);
    c.unclip();
    moving
}

/// Oyen whole and still, as on the Oyenza mark: his head `w` DIPs wide, head
/// and ears centred on `center`. The mark fits a 108-unit square whose
/// middle is 12 units above its origin.
fn badge(c: &mut Canvas, center: Point, w: f32) {
    let k = w / MARK_W;
    let eyes = if w * c.scale() <= 32.0 { SMALL_EYES } else { 1.0 };
    let frame = oyen::frame(Mood::Oyen, 0.0, Input::default(), eyes);
    draw(c, &frame, Point::new(center.x, center.y + 12.0 * k), k);
}

/// The Oyenza app's icon, `size` DIPs square and centred on `center`: Oyen on
/// the icon's near-black tile (catat assets/images/icon.png: his head is
/// 61.7 % of the tile, his middle 1.3 % below its centre), with the rounded
/// corners phones give it. Its own colours in every theme.
pub fn app_icon(c: &mut Canvas, center: Point, size: f32) {
    let tile = Rect::new(center.x - size / 2.0, center.y - size / 2.0, size, size);
    c.unthemed(|c| c.fill_rrect(tile, size * 0.2237, ICON_TILE));
    badge(c, Point::new(center.x, center.y + 0.013 * size), 0.617 * size);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Step `notes` at 60 frames a second from `from` to `to`.
    fn run(notes: &mut Notes, from: f64, to: f64, playing: bool) {
        let mut t = from;
        while t < to {
            notes.step(t, playing);
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn a_note_leaves_on_each_beat_while_music_plays() {
        let mut n = Notes::default();
        // Beat 16 began at 9.6 s, before the first frame: it sends nothing.
        run(&mut n, 10.0, 12.0, true);
        assert_eq!(n.born, vec![17, 18, 19]);
    }

    #[test]
    fn a_pause_sends_none_and_those_in_the_air_finish() {
        let mut n = Notes::default();
        run(&mut n, 10.0, 12.0, true);
        run(&mut n, 12.0, 13.0, false);
        assert_eq!(n.born, vec![19]);
        assert!(music_moving(13.0, false, Some(12.0), &n));
        run(&mut n, 13.0, 13.5, false);
        assert!(!n.flying());
        assert!(!music_moving(13.5, false, Some(12.0), &n));
    }

    #[test]
    fn coming_back_after_a_gap_starts_afresh() {
        let mut n = Notes::default();
        run(&mut n, 10.0, 11.0, true);
        // The pill was away for a while: no catching up on missed beats.
        n.step(12.1, true);
        assert_eq!(n.born, vec![18]);
        run(&mut n, 12.1 + 1.0 / 60.0, 12.7, true);
        assert_eq!(n.born, vec![18, 21]);
    }

    #[test]
    fn oyen_lands_upright_and_straightens_after_a_pause() {
        let lean = |f: &Frame| f.head.b.atan2(f.head.a).to_degrees();
        assert!(lean(&music(6.0, true, None)).abs() < 1e-6);
        assert!((lean(&music(6.3, true, None)) - LEAN_DEG).abs() < 0.2);
        assert!(lean(&music(6.4, false, Some(6.3))) > 1.0);
        assert!(lean(&music(6.3 + SETTLE, false, Some(6.3))).abs() < 1e-6);
    }

    #[test]
    fn the_note_drawings_parse() {
        assert!(glyph(false).len() > 8);
        assert!(glyph(true).len() > 12);
    }
}
