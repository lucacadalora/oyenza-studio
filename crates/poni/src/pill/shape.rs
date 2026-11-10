//! The floating outline: every corner rounded (a capsule while the island
//! rests, a card while it's open), its shadow and hairline, and where it
//! takes the mouse. Shared by the Windows and Mac renderers: only
//! `Canvas` calls both of them have.

use crate::gfx::Canvas;
use poni_core::geom::{Color, Point, Rect};
use poni_core::svgpath::Seg;

/// Room the window keeps beside and below the floating body: the shadow
/// reaches 15 DIP beside it and 19 below (it drops [`SHADOW_DROP`]), plus a
/// DIP for snapping. 20 is Classic's room beside the island (shoulders and
/// hit margin), so the window is no wider than Classic's.
pub const MARGIN: f32 = 20.0;

/// The shadow falls this far below the body (DIPs).
pub const SHADOW_DROP: f32 = 4.0;

/// The shadow's opacity by distance from the edge of the body moved down
/// [`SHADOW_DROP`] (DIPs, negative inside). It is the spec's key shadow
/// (black 0.32, 4 down, Gaussian sigma 6) and contact shadow (0.20, 1 down,
/// sigma 1.5) in one ramp: `1 - (1 - 0.32 Q(d / 6)) (1 - 0.20 Q((d + 3) / 1.5))`,
/// Q the Gaussian tail. The contact shadow is exact under the body, where it
/// shows; beside and above it the key shadow is. Stops sit where straight
/// lines between them stay within 0.002 of the curve, and it is cut to 0 at
/// 15 DIP (2.5 sigma, 0.002 left). It starts 6 DIP inside, so it reaches
/// under the body's bottom edge (4 inside the moved body) with room for the
/// edge's anti-aliasing; deeper in, the body covers it.
pub const SHADOW_RAMP: [(f32, f32); 12] = [
    (-6.0, 0.4121),
    (-5.0, 0.3906),
    (-4.25, 0.3641),
    (-3.25, 0.3136),
    (-1.75, 0.2292),
    (-0.75, 0.1869),
    (0.5, 0.1510),
    (3.25, 0.0941),
    (6.0, 0.0508),
    (8.75, 0.0232),
    (12.0, 0.0073),
    (15.0, 0.0),
];

/// The outline as path segments: a rounded rectangle, the radius clamped so
/// a short island is a capsule.
pub fn segments(body: Rect, radius: f32) -> Vec<Seg> {
    let r = radius.min(body.h / 2.0).min(body.w / 2.0).max(0.0);
    let (x0, y0, x1, y1) = (body.x, body.y, body.right(), body.bottom());
    vec![
        Seg::MoveTo(x0 + r, y0),
        Seg::LineTo(x1 - r, y0),
        Seg::ArcTo(r, r, 0.0, false, true, x1, y0 + r),
        Seg::LineTo(x1, y1 - r),
        Seg::ArcTo(r, r, 0.0, false, true, x1 - r, y1),
        Seg::LineTo(x0 + r, y1),
        Seg::ArcTo(r, r, 0.0, false, true, x0, y1 - r),
        Seg::LineTo(x0, y0 + r),
        Seg::ArcTo(r, r, 0.0, false, true, x0 + r, y0),
        Seg::Close,
    ]
}

/// `body` with its edges on whole pixels at `scale` (pixels per DIP), as
/// [`draw_body`] draws it. A mask or a glow that should meet the edge
/// exactly clips to `segments(snap(body, scale), radius)`.
pub fn snap(body: Rect, scale: f32) -> Rect {
    let s = scale.max(0.25);
    let px = |v: f32| (v * s).round() / s;
    let (x0, y0) = (px(body.x), px(body.y));
    Rect::new(x0, y0, (px(body.right()) - x0).max(0.0), (px(body.bottom()) - y0).max(0.0))
}

/// The hairline's width in DIPs: whole pixels, one up to 150 % and two from
/// 175 % (about the spec's 1 DIP without a blurred part pixel).
pub fn hairline(scale: f32) -> f32 {
    let s = scale.max(0.25);
    (s + 0.25).floor().max(1.0) / s
}

/// The island's body: `fill` in the floating outline over its soft shadow,
/// with a `border` hairline all the way round, just inside the edge.
///
/// The outline lands on whole pixels ([`snap`]), so its straight edges and
/// the hairline are crisp at any scale; the hairline keeps the ink of a
/// 1 DIP line however many pixels it is. The shadow fades with `fill` (a
/// near-invisible body casts none) and lies under the opaque body, which
/// covers it exactly: it only shows outside the outline.
///
/// The software renderer draws no shadow. Its layered window takes the
/// mouse wherever a pixel isn't fully transparent, so a shadow there would
/// swallow clicks meant for the app under it (a browser's tabs, beside and
/// below the resting pill). It is also the renderer that pays for every
/// pixel on the CPU. The GPU renderer, WARP included, and the Mac's draw it.
///
/// A look with a frame (Diorama's) has its body drawn as that frame round a
/// pane instead ([`draw_framed_body`]), wherever the island floats.
pub fn draw_body(c: &mut Canvas, body: Rect, radius: f32, fill: Color, border: Option<Color>) {
    if let Some(frame) = c.look().frame {
        draw_framed_body(c, body, radius, fill, border, &frame);
        return;
    }
    let s = c.scale();
    let body = snap(body, s);
    let r = radius.min(body.h / 2.0).min(body.w / 2.0).max(0.0);
    if c.on_gpu() && casts_shadow(fill) {
        c.rrect_shadow(body.offset(0.0, SHADOW_DROP), r, Color::black(fill.a), &SHADOW_RAMP);
    }
    c.path(&segments(body, r), Some(fill), None);
    if let Some(b) = border {
        hairline_round(c, body, r, b);
    }
}

/// A `border` hairline all the way round, just inside `body`'s edge.
fn hairline_round(c: &mut Canvas, body: Rect, r: f32, b: Color) {
    let w = hairline(c.scale());
    let ink = b.with_alpha((b.a / w).min(1.0));
    c.path(&segments(body.inset(w / 2.0, w / 2.0), r - w / 2.0), None, Some((ink, w)));
}

/// The frame's thickness (DIPs) round a body `w` by `h`: the resting
/// picture's bezel (`studio_diorama::BEZEL`) at the resting pill's height,
/// growing with the shorter side to [`FRAME_MAX`] for the open card, so the
/// pill's frame turns into the card's as the island opens.
pub fn frame_width(w: f32, h: f32) -> f32 {
    let t = ((w.min(h) - studio_diorama::H) / (FRAME_GROWN - studio_diorama::H)).clamp(0.0, 1.0);
    studio_diorama::BEZEL + (FRAME_MAX - studio_diorama::BEZEL) * t
}

/// The open card's frame, and the shorter side from which a body has it.
pub const FRAME_MAX: f32 = 5.0;
const FRAME_GROWN: f32 = 150.0;
/// How far the frame's shadow on the pane reaches below its top edge (DIPs),
/// and how dark it starts there.
const PANE_SHADE: f32 = 5.0;
const PANE_SHADE_ALPHA: f32 = 0.16;
/// How far the light takes the frame's face toward its lit colour at the
/// top, and toward its unlit one at the bottom.
const FRAME_LIT: f32 = 0.85;
const FRAME_UNLIT: f32 = 0.7;

/// The Diorama island's body: the resting pill's raised frame round a pane,
/// grown to `body`. Pill's drop shadow (in `frame.shadow`: black by day,
/// the window's warm glow at night); the frame over the outline, lit at the
/// top, its face at mid-height and turned away from the light at the bottom;
/// the pane, `fill` as the look paints it (its ground), inset by the frame's
/// width, with the frame's soft shadow along its top edge; and a firm
/// `border` line round the outside. `fill`'s opacity is the body's (the Mac
/// fades a Hidden island out this way).
///
/// Vector fills only, nothing kept between frames: the shadow as Pill's,
/// two opaque gradient fills (the frame; the pane with its shadow) and the
/// line. The frame's colours are drawn as they are (a look would repaint
/// the lit frame's white as its ink), the pane and the line as the look
/// paints them.
pub fn draw_framed_body(c: &mut Canvas, body: Rect, radius: f32, fill: Color, border: Option<Color>, frame: &poni_core::look::Frame) {
    let s = c.scale();
    let body = snap(body, s);
    let r = radius.min(body.h / 2.0).min(body.w / 2.0).max(0.0);
    let ground = c.look().paint(fill).with_alpha(1.0);
    let line = border.map(|b| c.look().paint(b));
    let fw = frame_width(body.w, body.h).min(body.h / 2.0).min(body.w / 2.0);
    let inner = body.inset(fw, fw);
    let lit = Color::lerp(frame.face, frame.lit, FRAME_LIT);
    let unlit = Color::lerp(frame.face, frame.unlit, FRAME_UNLIT);
    let shaded = Color::lerp(ground, frame.shade, PANE_SHADE_ALPHA);
    c.unthemed(|c| {
        c.with_alpha(fill.a, |c| {
            if c.on_gpu() && casts_shadow(fill) {
                c.rrect_shadow(body.offset(0.0, SHADOW_DROP), r, frame.shadow, &SHADOW_RAMP);
            }
            c.fill_path_stops(&segments(body, r), body.y, body.bottom(), &[(0.0, lit), (0.5, frame.face), (1.0, unlit)]);
            if inner.w > 0.0 && inner.h > 0.0 {
                let reach = PANE_SHADE.min(inner.h);
                c.fill_path_stops(&segments(inner, (r - fw).max(0.0)), inner.y, inner.y + reach, &[(0.0, shaded), (1.0, ground)]);
            }
            if let Some(b) = line {
                hairline_round(c, body, r, b);
            }
        })
    });
}

/// Whether a body filled with `fill` casts its shadow (at `fill.a` times
/// the ramp): not one that is all but invisible, like a 1/255 body (the
/// Hidden idle style's, which the app no longer draws), whose shadow would
/// round to nothing.
fn casts_shadow(fill: Color) -> bool {
    fill.a * SHADOW_RAMP[0].1 * 255.0 >= 0.5
}

/// Where the resting island takes the mouse: its body and the gap above it,
/// so a pointer thrown at the top edge still lands on it.
pub fn hover_rect(body: Rect) -> Rect {
    Rect::new(body.x, 0.0, body.w, body.bottom())
}

/// How both renderers draw `Canvas::rrect_shadow`: gradient pieces round a
/// rounded rectangle, meeting on whole pixels. Corners spread from the
/// centres of their arcs and sides run straight out from the lines through
/// those centres, all on the same `stops`, so the ramp carries on across
/// every seam.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowPlan {
    /// The ramp as gradient stops: (0..1 along `reach`, opacity).
    pub stops: Vec<(f32, f32)>,
    /// From an arc's centre (or the line through it) to the ramp's end.
    pub reach: f32,
    /// The corner squares, and the centre their radial gradient spreads
    /// from. Corners that share a centre are one piece: a capsule has two
    /// (its round ends), a circle one.
    pub corners: [Option<(Rect, Point)>; 4],
    /// Top, bottom, left and right (none where two corners meet, as at a
    /// capsule's ends): the strip, and where its linear gradient starts (at
    /// stop 0) and ends.
    pub sides: [Option<(Rect, Point, Point)>; 4],
    /// The middle, filled flat at this opacity when the radius is less than
    /// the ramp's depth (the sides then stop at the corners' lines).
    pub middle: Option<(Rect, f32)>,
}

/// The pieces of `Canvas::rrect_shadow(r, radius, _, ramp)` at `scale`, or
/// `None` when there's nothing to draw.
pub fn shadow_plan(r: Rect, radius: f32, ramp: &[(f32, f32)], scale: f32) -> Option<ShadowPlan> {
    let (&(first, _), &(last, _)) = (ramp.first()?, ramp.last()?);
    if ramp.len() < 2 || last <= first || last <= 0.0 || r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let s = scale.max(0.25);
    let px = |v: f32| (v * s).round() / s;
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    // Renderers keep gradients by their stops: radii in half DIPs keep that
    // to a few dozen while the island morphs (the ramp moves under 0.25 DIP).
    let rq = (rad * 2.0).round() / 2.0;
    let reach = rq + last;
    let depth = (-first).max(0.0);
    let mut stops = Vec::with_capacity(ramp.len() + 1);
    if first < -rq {
        stops.push((0.0, ramp_at(ramp, -rq)));
    }
    stops.extend(ramp.iter().filter(|&&(d, _)| d >= -rq).map(|&(d, a)| ((d + rq) / reach, a)));

    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let (cl, cr, ct, cb) = (x0 + rad, x1 - rad, y0 + rad, y1 - rad);
    // The seams between pieces, on whole pixels: two anti-aliased edges
    // meeting between pixels would let a faint line of the screen through.
    let (bl, bt) = (px(cl), px(ct));
    let (br, bb) = (px(cr).max(bl), px(cb).max(bt));
    let (ol, or, ot, ob) = (x0 - last, x1 + last, y0 - last, y1 + last);
    let (wide, tall) = (br > bl, bb > bt);
    let (mx, my) = ((cl + cr) / 2.0, (ct + cb) / 2.0);
    let corners = match (wide, tall) {
        (true, true) => [
            Some((Rect::new(ol, ot, bl - ol, bt - ot), Point::new(cl, ct))),
            Some((Rect::new(br, ot, or - br, bt - ot), Point::new(cr, ct))),
            Some((Rect::new(ol, bb, bl - ol, ob - bb), Point::new(cl, cb))),
            Some((Rect::new(br, bb, or - br, ob - bb), Point::new(cr, cb))),
        ],
        (true, false) => [
            Some((Rect::new(ol, ot, bl - ol, ob - ot), Point::new(cl, my))),
            Some((Rect::new(br, ot, or - br, ob - ot), Point::new(cr, my))),
            None,
            None,
        ],
        (false, true) => [
            Some((Rect::new(ol, ot, or - ol, bt - ot), Point::new(mx, ct))),
            Some((Rect::new(ol, bb, or - ol, ob - bb), Point::new(mx, cb))),
            None,
            None,
        ],
        (false, false) => [Some((Rect::new(ol, ot, or - ol, ob - ot), Point::new(mx, my))), None, None, None],
    };
    // Sides reach `depth` inside the edge, or the corners' lines if nearer.
    let (it, ib) = ((y0 + depth).min(bt), (y1 - depth).max(bb));
    let (il, ir) = ((x0 + depth).min(bl), (x1 - depth).max(br));
    let sides = [
        wide.then(|| (Rect::new(bl, ot, br - bl, it - ot), Point::new(cl, ct), Point::new(cl, ct - reach))),
        wide.then(|| (Rect::new(bl, ib, br - bl, ob - ib), Point::new(cl, cb), Point::new(cl, cb + reach))),
        tall.then(|| (Rect::new(ol, bt, il - ol, bb - bt), Point::new(cl, ct), Point::new(cl - reach, ct))),
        tall.then(|| (Rect::new(ir, bt, or - ir, bb - bt), Point::new(cr, ct), Point::new(cr + reach, ct))),
    ];
    let middle = (depth >= rad && wide && tall).then(|| (Rect::new(bl, bt, br - bl, bb - bt), stops[0].1));
    Some(ShadowPlan { stops, reach, corners, sides, middle })
}

/// The ramp's opacity at distance `d`, straight between its stops.
fn ramp_at(ramp: &[(f32, f32)], d: f32) -> f32 {
    let Some(i) = ramp.iter().position(|&(x, _)| x >= d) else { return ramp.last().map_or(0.0, |s| s.1) };
    if i == 0 {
        return ramp[0].1;
    }
    let ((xa, ya), (xb, yb)) = (ramp[i - 1], ramp[i]);
    ya + (yb - ya) * (d - xa) / (xb - xa).max(1e-6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::svgpath::arc_to_cubics;

    const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, w, h)
    }

    /// Bodies as the island draws them (at rest and mid-morph), with the
    /// radius asked for.
    fn bodies() -> Vec<(Rect, f32)> {
        vec![
            (rect(500.0 - 42.0, 8.0, 84.0, 32.0), 16.0),
            (rect(500.0 - 60.0, 8.0, 120.0, 32.0), 40.0),
            (rect(500.0 - 76.0, 8.0, 152.0, 32.0), 16.0),
            (rect(500.0 - 170.0, 8.0, 340.0, 48.0), 24.0),
            (rect(500.0 - 310.0, 8.0, 620.0, 216.0), 40.0),
            (rect(173.37, 8.0, 401.9, 97.3), 27.6),
            (rect(10.0, 8.0, 30.0, 90.0), 40.0),
            (rect(10.0, 8.0, 60.0, 30.0), 0.0),
        ]
    }

    /// The outline's points, arcs flattened, and the radius it was drawn with.
    fn trace(segs: &[Seg]) -> Vec<(f32, f32)> {
        let mut pts = Vec::new();
        let mut cur = (0.0, 0.0);
        for s in segs {
            match *s {
                Seg::MoveTo(x, y) | Seg::LineTo(x, y) => {
                    cur = (x, y);
                    pts.push(cur);
                }
                Seg::ArcTo(rx, ry, rot, large, sweep, x, y) => {
                    for c in arc_to_cubics(cur.0, cur.1, rx, ry, rot, large, sweep, x, y) {
                        for i in 1..=16 {
                            let t = i as f32 / 16.0;
                            let u = 1.0 - t;
                            let (a, b, c2, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                            pts.push((a * cur.0 + b * c[0] + c2 * c[2] + d * c[4], a * cur.1 + b * c[1] + c2 * c[3] + d * c[5]));
                        }
                        cur = (c[4], c[5]);
                    }
                    cur = (x, y);
                }
                Seg::CubicTo(..) | Seg::QuadTo(..) => panic!("the outline has no free curves"),
                Seg::Close => {}
            }
        }
        pts
    }

    /// Signed distance from the rounded rectangle's edge (negative inside).
    fn sdf(r: Rect, radius: f32, p: (f32, f32)) -> f32 {
        let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        let c = r.center();
        let qx = (p.0 - c.x).abs() - (r.w / 2.0 - rad);
        let qy = (p.1 - c.y).abs() - (r.h / 2.0 - rad);
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad
    }

    #[test]
    fn outline_is_one_closed_figure() {
        for (b, r) in bodies() {
            let segs = segments(b, r);
            let Seg::MoveTo(sx, sy) = segs[0] else { panic!("starts with a move") };
            assert!(matches!(segs.last(), Some(Seg::Close)), "{b:?} ends closed");
            assert_eq!(segs.iter().filter(|s| matches!(s, Seg::MoveTo(..))).count(), 1, "{b:?} is one figure");
            let Some(Seg::ArcTo(.., ex, ey)) = segs.get(segs.len() - 2) else { panic!("the last corner closes it") };
            assert!((ex - sx).abs() < 1e-4 && (ey - sy).abs() < 1e-4, "{b:?} ends where it starts");
        }
    }

    #[test]
    fn outline_stays_on_the_body_edge() {
        for (b, r) in bodies() {
            // Cubic arcs stray from a true circle by up to 0.03 % of the radius.
            let tol = 3e-4 * r.min(b.h / 2.0).min(b.w / 2.0).max(0.0) + 2e-3;
            for p in trace(&segments(b, r)) {
                assert!(p.0 >= b.x - 1e-3 && p.0 <= b.right() + 1e-3 && p.1 >= b.y - 1e-3 && p.1 <= b.bottom() + 1e-3, "{b:?} r {r}: {p:?} outside");
                assert!(sdf(b, r, p).abs() < tol, "{b:?} r {r}: {p:?} off the edge by {}", sdf(b, r, p));
            }
        }
    }

    #[test]
    fn short_island_is_a_capsule() {
        let b = rect(458.0, 8.0, 84.0, 32.0);
        for asked in [16.0, 20.0, 40.0, 1000.0] {
            let segs = segments(b, asked);
            assert!(segs.iter().all(|s| !matches!(s, Seg::ArcTo(rx, ..) if (*rx - 16.0).abs() > 1e-4)), "radius {asked} clamps to h / 2");
            // The ends are half circles: the straight sides have no length.
            let Seg::LineTo(_, a) = segs[3] else { panic!() };
            let Seg::ArcTo(.., b1) = segs[2] else { panic!() };
            assert!((a - b1).abs() < 1e-4, "no straight end");
            for p in trace(&segs) {
                let cx = if p.0 < 500.0 { 474.0 } else { 526.0 };
                if (p.0 - cx).abs() > 1e-3 && (p.0 < 474.0 || p.0 > 526.0) {
                    assert!(((p.0 - cx).hypot(p.1 - 24.0) - 16.0).abs() < 0.01, "{p:?} on the end's circle");
                }
            }
        }
        // Narrow and tall: half the width.
        let tall = segments(rect(0.0, 8.0, 30.0, 90.0), 40.0);
        assert!(matches!(tall[2], Seg::ArcTo(rx, ..) if (rx - 15.0).abs() < 1e-4));
        // The open card keeps the radius it asks for; a negative one is square.
        assert!(matches!(segments(rect(190.0, 8.0, 620.0, 216.0), 40.0)[2], Seg::ArcTo(rx, ..) if (rx - 40.0).abs() < 1e-4));
        assert!(matches!(segments(rect(0.0, 8.0, 60.0, 30.0), -3.0)[2], Seg::ArcTo(rx, ..) if rx == 0.0));
    }

    #[test]
    fn snapped_edges_land_on_whole_pixels() {
        for s in SCALES {
            for (b, _) in bodies() {
                let n = snap(b, s);
                for (v, was) in [(n.x, b.x), (n.y, b.y), (n.right(), b.right()), (n.bottom(), b.bottom())] {
                    assert!(((v * s) - (v * s).round()).abs() < 1e-3, "{s}: {v} is a whole pixel");
                    assert!(((v - was) * s).abs() <= 0.5 + 1e-3, "{s}: moved under half a pixel");
                }
            }
            let h = hairline(s);
            assert!(((h * s) - (h * s).round()).abs() < 1e-4 && h * s >= 1.0, "{s}: hairline is whole pixels");
            assert!((h - 1.0).abs() <= 0.34, "{s}: hairline about a DIP ({h})");
        }
    }

    /// Gaussian tail, Abramowitz and Stegun 7.1.26 (error under 1.5e-7).
    fn q(x: f64) -> f64 {
        let z = x / std::f64::consts::SQRT_2;
        let t = 1.0 / (1.0 + 0.327_591_1 * z.abs());
        let poly = t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
        let erfc = poly * (-z * z).exp();
        if z >= 0.0 {
            erfc / 2.0
        } else {
            1.0 - erfc / 2.0
        }
    }

    fn spec_shadow(d: f32) -> f32 {
        let d = d as f64;
        (1.0 - (1.0 - 0.32 * q(d / 6.0)) * (1.0 - 0.20 * q((d + (SHADOW_DROP as f64 - 1.0)) / 1.5))) as f32
    }

    #[test]
    fn shadow_ramp_is_the_spec_shadow() {
        let (first, last) = (SHADOW_RAMP[0].0, SHADOW_RAMP[SHADOW_RAMP.len() - 1].0);
        assert_eq!(SHADOW_RAMP[SHADOW_RAMP.len() - 1].1, 0.0, "fades out completely");
        assert!(SHADOW_RAMP.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 > w[1].1), "ordered, fading outwards");
        let mut d = first;
        while d < last - 3.0 {
            assert!((ramp_at(&SHADOW_RAMP, d) - spec_shadow(d)).abs() < 0.0025, "at {d}: {} vs {}", ramp_at(&SHADOW_RAMP, d), spec_shadow(d));
            d += 0.05;
        }
        // Reaches under the body's bottom edge with a pixel to spare, and
        // stays inside the window's room beside and below the body.
        assert!(-first >= SHADOW_DROP + 1.0);
        assert!(last <= MARGIN - 0.5 && SHADOW_DROP + last <= MARGIN - 0.5);
    }

    /// The opacity the plan paints at `p`, and how many pieces cover it.
    fn painted(plan: &ShadowPlan, p: (f32, f32)) -> (f32, usize) {
        let inside = |r: &Rect| p.0 >= r.x && p.0 < r.right() && p.1 >= r.y && p.1 < r.bottom();
        let along = |t: f32| {
            let t = t.clamp(0.0, 1.0);
            let st = &plan.stops;
            match st.iter().position(|s| s.0 >= t) {
                None => st[st.len() - 1].1,
                Some(0) => st[0].1,
                Some(i) => st[i - 1].1 + (st[i].1 - st[i - 1].1) * (t - st[i - 1].0) / (st[i].0 - st[i - 1].0).max(1e-6),
            }
        };
        let mut hits = Vec::new();
        for (sq, o) in plan.corners.iter().flatten() {
            if inside(sq) {
                hits.push(along((p.0 - o.x).hypot(p.1 - o.y) / plan.reach));
            }
        }
        for (strip, a, b) in plan.sides.iter().flatten() {
            if inside(strip) {
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                hits.push(along(((p.0 - a.x) * dx + (p.1 - a.y) * dy) / (dx * dx + dy * dy)));
            }
        }
        if let Some((m, a)) = &plan.middle {
            if inside(m) {
                hits.push(*a);
            }
        }
        (hits.first().copied().unwrap_or(0.0), hits.len())
    }

    #[test]
    fn shadow_pieces_paint_the_ramp_without_seams() {
        let ramp = SHADOW_RAMP;
        let (depth, last) = (-ramp[0].0, ramp[ramp.len() - 1].0);
        for s in SCALES {
            for (b, r) in bodies() {
                let b = snap(b, s).offset(0.0, SHADOW_DROP);
                let plan = shadow_plan(b, r, &ramp, s).expect("a plan");
                assert!(plan.stops.windows(2).all(|w| w[0].0 <= w[1].0) && plan.stops.iter().all(|st| (0.0..=1.0).contains(&st.0)));
                // Every pixel centre in the band is painted once, at the
                // ramp's opacity for its distance from the edge.
                let (x0, x1) = (((b.x - last) * s).floor() as i32, ((b.right() + last) * s).ceil() as i32);
                let (y0, y1) = (((b.y - last) * s).floor() as i32, ((b.bottom() + last) * s).ceil() as i32);
                for py in y0..y1 {
                    for pxl in x0..x1 {
                        let p = ((pxl as f32 + 0.5) / s, (py as f32 + 0.5) / s);
                        let d = sdf(b, r, p);
                        if d <= -depth + 0.6 || d >= last - 0.05 {
                            continue;
                        }
                        let (a, n) = painted(&plan, p);
                        assert_eq!(n, 1, "{s}: {b:?} r {r}: {p:?} (d {d}) painted {n} times");
                        // The flat middle (a radius under the ramp's depth) is
                        // an approximation; elsewhere the radius in half DIPs
                        // moves the ramp under 0.25 DIP, and it is never
                        // steeper than 0.06 a DIP.
                        let flat = plan.middle.is_some_and(|(m, _)| p.0 >= m.x && p.0 < m.right() && p.1 >= m.y && p.1 < m.bottom());
                        assert!(flat || (a - ramp_at(&ramp, d)).abs() < 0.016, "{s}: {b:?} r {r}: {p:?} (d {d}) {a} vs {}", ramp_at(&ramp, d));
                    }
                }
                // Seams on whole pixels, and nothing past the ramp's reach.
                for (sq, _) in plan.corners.iter().flatten() {
                    for v in [sq.x, sq.right(), sq.y, sq.bottom()] {
                        let edge = [b.x - last, b.right() + last, b.y - last, b.bottom() + last].iter().any(|e| (e - v).abs() < 1e-4);
                        assert!(edge || ((v * s) - (v * s).round()).abs() < 1e-3, "{s}: seam {v} off the pixel grid");
                    }
                    assert!(sq.x >= b.x - last - 1e-3 && sq.right() <= b.right() + last + 1e-3 && sq.bottom() <= b.bottom() + last + 1e-3);
                }
            }
        }
    }

    #[test]
    fn shadow_stays_in_the_windows_room() {
        for s in SCALES {
            for (b, r) in bodies() {
                let body = snap(b, s);
                let plan = shadow_plan(body.offset(0.0, SHADOW_DROP), r, &SHADOW_RAMP, s).unwrap();
                let room = Rect::new(body.x - MARGIN, 0.0, body.w + 2.0 * MARGIN, body.bottom() + MARGIN);
                let pieces = plan.corners.iter().flatten().map(|c| c.0).chain(plan.sides.iter().flatten().map(|p| p.0));
                for p in pieces {
                    assert!(p.x >= room.x && p.right() <= room.right() && p.bottom() <= room.bottom(), "{s}: {p:?} outside {room:?}");
                }
            }
        }
    }

    #[test]
    fn a_capsule_takes_four_fills_and_a_card_eight() {
        let count = |p: &ShadowPlan| (p.corners.iter().flatten().count(), p.sides.iter().flatten().count());
        for s in SCALES {
            assert_eq!(count(&shadow_plan(snap(rect(458.0, 12.0, 84.0, 32.0), s), 16.0, &SHADOW_RAMP, s).unwrap()), (2, 2), "{s}: capsule");
            assert_eq!(count(&shadow_plan(snap(rect(190.0, 12.0, 620.0, 216.0), s), 40.0, &SHADOW_RAMP, s).unwrap()), (4, 4), "{s}: card");
            assert_eq!(count(&shadow_plan(snap(rect(10.0, 12.0, 30.0, 90.0), s), 40.0, &SHADOW_RAMP, s).unwrap()), (2, 2), "{s}: upright");
            assert_eq!(count(&shadow_plan(snap(rect(10.0, 12.0, 40.0, 40.0), s), 20.0, &SHADOW_RAMP, s).unwrap()), (1, 0), "{s}: circle");
        }
    }

    #[test]
    fn the_frame_grows_from_the_pills_bezel_to_the_cards() {
        // The resting pill's own frame at its height (under the pointer too,
        // where the picture covers the body), the open card's at its size.
        assert_eq!(frame_width(100.0, studio_diorama::H), studio_diorama::BEZEL);
        assert_eq!(frame_width(124.0, 30.0), studio_diorama::BEZEL);
        assert_eq!(frame_width(620.0, 216.0), FRAME_MAX);
        // On the way between them it grows without a step.
        let mut last = 0.0;
        for h in (30..=240).map(|h| h as f32) {
            let w = frame_width(620.0, h);
            assert!(w >= last && w - last < 0.05, "{h}: {w} after {last}");
            last = w;
        }
    }

    #[test]
    fn only_a_visible_body_casts_a_shadow() {
        assert!(casts_shadow(Color::black(1.0)));
        assert!(casts_shadow(Color::black(0.5)));
        assert!(!casts_shadow(Color::black(1.0 / 255.0)), "the Hidden idle style casts none");
        assert!(!casts_shadow(Color::black(0.0)));
    }

    #[test]
    fn small_radius_fills_the_middle() {
        let plan = shadow_plan(rect(10.0, 10.0, 100.0, 60.0), 2.0, &SHADOW_RAMP, 1.0).unwrap();
        let (m, a) = plan.middle.expect("a middle under the ramp's depth");
        assert!(m.w > 0.0 && m.h > 0.0 && (a - ramp_at(&SHADOW_RAMP, -2.0)).abs() < 1e-3);
        assert!(shadow_plan(rect(10.0, 10.0, 100.0, 60.0), 16.0, &SHADOW_RAMP, 1.0).unwrap().middle.is_none());
        assert!(shadow_plan(rect(10.0, 10.0, 0.0, 60.0), 16.0, &SHADOW_RAMP, 1.0).is_none());
        assert!(shadow_plan(rect(10.0, 10.0, 50.0, 60.0), 16.0, &SHADOW_RAMP[..1], 1.0).is_none());
    }
}
