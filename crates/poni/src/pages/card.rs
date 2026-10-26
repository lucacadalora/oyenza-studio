//! Boxes near the open card's bottom corners.
//!
//! Shared by the pages that put a box or a row's background near the open
//! card's bottom corners (Shelf, Today, Add-ons, Prayer, Settings). The Pill
//! card's corners are rounder than Classic's, so a box that keeps its Classic
//! radius there pinches the band between it and the card's edge.

use crate::state::AppState;
use crate::ui::Ui;
use poni_core::geom::{Color, Rect};
use poni_core::svgpath::Seg;

/// Pill: the open card (`area`, the page's content rect, grown back by the
/// padding and the header) and its corner radius, read from the island's own
/// geometry so the pages follow whatever radius the card is given. `None` in
/// the other themes: their pages draw exactly as before.
pub(crate) fn pill_card(st: &AppState, area: Rect) -> Option<(Rect, f32)> {
    if !crate::pill::on(st.settings.theme) {
        return None;
    }
    let r = crate::activities::geom(&crate::activities::Mode::Expanded(st.page), st).r;
    let (pad, head) = (super::PAD, super::HEADER_H);
    Some((Rect::new(area.x - pad, area.y - head, area.w + 2.0 * pad, area.h + head + pad), r))
}

/// The corner radii (top-left, top-right, bottom-right, bottom-left) for a box
/// `r` with corner `radius` inside `card` (corner radius `card_r`). A bottom
/// corner that sits in the card's rounded corner takes the card's radius less
/// its larger distance to the card's edges, so the band between the two keeps
/// about its width round the curve (concentric corners); never sharper than
/// `radius`, and no rounder than the box leaves room for. The top corners sit
/// under the header, where the card's sides are straight.
pub(crate) fn pill_corners(card: Rect, card_r: f32, r: Rect, radius: f32) -> [f32; 4] {
    let below = card.bottom() - r.bottom();
    let cap = (r.h - radius).min(r.w / 2.0).max(radius);
    let corner = |side: f32| if side < card_r && below < card_r { (card_r - side.max(below)).clamp(radius, cap) } else { radius };
    [radius, radius, corner(card.right() - r.right()), corner(r.x - card.x)]
}

/// Fill `r` with corner `radius`; on the Pill card (`card` from [`pill_card`])
/// with the corners [`pill_corners`] gives it.
pub(crate) fn fill_box(ui: &mut Ui, card: Option<(Rect, f32)>, r: Rect, radius: f32, fill: Color) {
    match card.map(|(c, cr)| pill_corners(c, cr, r, radius)).filter(|k| *k != [radius; 4]) {
        Some(k) => ui.c.path(&rrect_segments(r, k), Some(fill), None),
        None => ui.c.fill_rrect(r, radius, fill),
    }
}

/// [`fill_box`]'s outline: a `width` line just inside `r`'s edge.
pub(crate) fn stroke_box(ui: &mut Ui, card: Option<(Rect, f32)>, r: Rect, radius: f32, color: Color, width: f32) {
    let half = width / 2.0;
    match card.map(|(c, cr)| pill_corners(c, cr, r, radius)).filter(|k| *k != [radius; 4]) {
        Some(k) => ui.c.path(&rrect_segments(r.inset(half, half), k.map(|x| (x - half).max(0.0))), None, Some((color, width))),
        None => ui.c.stroke_rrect(r, (radius - half).max(0.0), color, width),
    }
}

/// A rounded rectangle with its own radius at each corner, as path segments.
fn rrect_segments(r: Rect, [tl, tr, br, bl]: [f32; 4]) -> Vec<Seg> {
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    vec![
        Seg::MoveTo(x0 + tl, y0),
        Seg::LineTo(x1 - tr, y0),
        Seg::ArcTo(tr, tr, 0.0, false, true, x1, y0 + tr),
        Seg::LineTo(x1, y1 - br),
        Seg::ArcTo(br, br, 0.0, false, true, x1 - br, y1),
        Seg::LineTo(x0 + bl, y1),
        Seg::ArcTo(bl, bl, 0.0, false, true, x0, y1 - bl),
        Seg::LineTo(x0, y0 + tl),
        Seg::ArcTo(tl, tl, 0.0, false, true, x0 + tl, y0),
        Seg::Close,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list row as the pages draw them: 32 tall with a 10 radius.
    const ROW_H: f32 = 32.0;
    const ROW_RADIUS: f32 = 10.0;
    const PITCH: f32 = ROW_H + 2.0;

    /// The Pill card the pages see at the default width: 620 wide, r 40.
    fn card(h: f32) -> Rect {
        Rect::new(0.0, 0.0, 620.0, h)
    }

    /// Closest approach (DIPs) of the box `r` with corner radii `k` to the
    /// edge of the card `c` (radius `cr`), over its two bottom corners.
    fn bottom_gap(c: Rect, cr: f32, r: Rect, k: [f32; 4]) -> f32 {
        // Distance from `p` to the card's outline, positive inside.
        let inside = |x: f32, y: f32| {
            let (cx, cy) = (x.clamp(c.x + cr, c.right() - cr), y.clamp(c.y + cr, c.bottom() - cr));
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            if d > 0.0 { cr - d } else { (x - c.x).min(c.right() - x).min(y - c.y).min(c.bottom() - y) }
        };
        let mut gap = f32::MAX;
        for i in 0..=90 {
            let a = (i as f32).to_radians();
            let (br, bl) = (k[2], k[3]);
            gap = gap.min(inside(r.right() - br + br * a.cos(), r.bottom() - br + br * a.sin()));
            gap = gap.min(inside(r.x + bl - bl * a.cos(), r.bottom() - bl + bl * a.sin()));
        }
        gap
    }

    #[test]
    fn pill_boxes_follow_the_card_corners() {
        let c = card(272.0);
        // A list's bottom row: bleeds to 8 from the sides, 18 above the bottom.
        let row = Rect::new(8.0, c.bottom() - 18.0 - ROW_H, 604.0, ROW_H);
        let k = pill_corners(c, 40.0, row, ROW_RADIUS);
        assert_eq!(k, [ROW_RADIUS, ROW_RADIUS, 22.0, 22.0], "bottom corners as round as the row leaves room for");
        assert!(bottom_gap(c, 40.0, row, k) >= 7.9, "keeps its 8 DIP side gap round the corner");
        assert!(bottom_gap(c, 40.0, row, [ROW_RADIUS; 4]) < 5.5, "Classic's radius pinches it");
        // A row higher up, or any box on Classic's 30 corner that is already fine, keeps its radius.
        let up = Rect::new(8.0, row.y - 2.0 * PITCH, 604.0, ROW_H);
        assert_eq!(pill_corners(c, 40.0, up, ROW_RADIUS), [ROW_RADIUS; 4]);
        // Today's bottom-right tile (18 in from the side and the bottom): concentric.
        let tile = Rect::new(456.0, c.bottom() - 18.0 - 78.0, 146.0, 78.0);
        let k = pill_corners(c, 40.0, tile, 16.0);
        assert_eq!(k, [16.0, 16.0, 22.0, 16.0]);
        assert!(bottom_gap(c, 40.0, tile, k) >= 17.9);
        assert_eq!(pill_corners(c, 30.0, tile, 16.0), [16.0; 4], "never sharper than its own radius");
        // Shelf's drop zone sits 22 above the bottom: its 18 already keeps the band even.
        let zone = Rect::new(18.0, 50.0, 380.0, c.bottom() - 22.0 - 50.0);
        assert_eq!(pill_corners(c, 40.0, zone, 18.0), [18.0; 4]);
    }
}
