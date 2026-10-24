//! The Glass theme's maths, kept apart from Windows so it's tested here.
//!
//! Liquid glass (Apple, WWDC25 "Meet Liquid Glass") bends light instead of
//! only blurring it: a convex rim magnifies what's under it, the material
//! thickens as the shape grows, small pieces flip between light and dark
//! with what's behind them, and the tint deepens only as far as legibility
//! needs. The renderer (gfx.rs) and its shader (res/glass.hlsl) use the
//! numbers and formulas below; the app feeds them what the screen copy saw.

use crate::geom::lerp;

/// Exponent of the rim profile: the lens moves a point `d` inside the edge
/// by `mag * (1 - d / bevel)^RIM_POWER` towards the middle.
pub const RIM_POWER: f32 = 1.6;
/// `mag * RIM_POWER / bevel` stays below 1, so the sampled position
/// `d + offset(d)` keeps moving inward as `d` grows: the rim magnifies what
/// is under it and never folds it back on itself.
pub const RIM_FOLD_GUARD: f32 = 0.85;
/// Smoked glass (no view of the screen) still lets the screen show through
/// it: this opaque, so ink keeps 4.5:1 over white or black behind.
pub const SMOKED_OPACITY: f32 = 0.82;

/// Heights (DIPs) where the glass is thinnest (a collapsed pill) and thickest
/// (an open page).
const THIN_H: f32 = 37.0;
const THICK_H: f32 = 220.0;

/// How substantial the glass is at one size. Lengths are DIPs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thickness {
    /// 0 for a collapsed pill, 1 for an open page.
    pub t: f32,
    /// Width of the curved rim band.
    pub bevel: f32,
    /// How far the rim's outermost pixel looks inward.
    pub mag: f32,
    /// Shadow reach below the island, and how far it drops. Together they
    /// stay inside the island's hit margin (11 DIPs), the part of the window
    /// each frame redraws around it.
    pub shadow_reach: f32,
    pub shadow_drop: f32,
}

/// Bigger glass is thicker: a wider rim, a stronger bend, a deeper shadow.
pub fn thickness(height: f32) -> Thickness {
    let t = ((height - THIN_H) / (THICK_H - THIN_H)).clamp(0.0, 1.0);
    let bevel = lerp(9.0, 24.0, t);
    Thickness { t, bevel, mag: RIM_FOLD_GUARD * bevel / RIM_POWER, shadow_reach: lerp(5.0, 7.0, t), shadow_drop: lerp(1.5, 3.0, t) }
}

/// How far the lens moves a point `d` inside the edge towards the middle.
pub fn rim_offset(d: f32, bevel: f32, mag: f32) -> f32 {
    if d >= bevel || bevel <= 0.0 {
        return 0.0;
    }
    mag * (1.0 - d.max(0.0) / bevel).powf(RIM_POWER)
}

/// Small glass flips between light and dark with what's behind it, with a
/// dead band and a short hold so a passing bright frame doesn't flicker it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flip {
    pub light: bool,
    /// When the backdrop first crossed the other way (seconds), if it has.
    pending_since: Option<f64>,
}

/// Go light above this backdrop luminance, dark below the other.
pub const FLIP_TO_LIGHT: f32 = 0.62;
pub const FLIP_TO_DARK: f32 = 0.45;
/// The crossing must hold this long (seconds).
pub const FLIP_HOLD: f64 = 0.25;

impl Flip {
    pub fn new(light: bool) -> Self {
        Self { light, pending_since: None }
    }

    /// Feed the backdrop's mean luminance at `now`; true when it flipped.
    pub fn update(&mut self, lum: f32, now: f64) -> bool {
        let wants = if self.light { lum >= FLIP_TO_DARK } else { lum > FLIP_TO_LIGHT };
        if wants == self.light {
            self.pending_since = None;
            return false;
        }
        match self.pending_since {
            None => {
                self.pending_since = Some(now);
                false
            }
            Some(t0) if now - t0 >= FLIP_HOLD => {
                self.light = wants;
                self.pending_since = None;
                true
            }
            Some(_) => false,
        }
    }

    /// A crossing is waiting out its hold (keep drawing so it can finish).
    pub fn pending(&self) -> bool {
        self.pending_since.is_some()
    }

    /// Jump straight to the backdrop's side (first frame, theme switched on).
    pub fn settle(&mut self, lum: f32) {
        self.light = lum > (FLIP_TO_LIGHT + FLIP_TO_DARK) / 2.0;
        self.pending_since = None;
    }
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance of an sRGB colour (components 0..=1).
pub fn rel_luminance(c: [f32; 3]) -> f32 {
    0.2126 * to_linear(c[0]) + 0.7152 * to_linear(c[1]) + 0.0722 * to_linear(c[2])
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (la, lb) = (rel_luminance(a), rel_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

/// The least tint, between `min` and `max`, of `toward` mixed into the glass's
/// `material` colour that gives `ink` at least `target` contrast on it: as much
/// of what's behind shows through as legibility allows. `max` when even that
/// isn't enough.
pub fn tint_for_contrast(material: [f32; 3], ink: [f32; 3], toward: [f32; 3], min: f32, max: f32, target: f32) -> f32 {
    let ok = |t: f32| contrast(mix3(material, toward, t), ink) >= target;
    if ok(min) {
        return min;
    }
    if !ok(max) {
        return max;
    }
    let (mut lo, mut hi) = (min, max);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.0;
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// What the glass sees under it: the mean colour (sRGB, 0..=1), its
/// luminance, and how busy it is (mean brightness step between neighbours).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Backdrop {
    pub mean: [f32; 3],
    pub lum: f32,
    pub busy: f32,
}

/// Sample a BGRA picture (`stride` bytes per row) inside `rect` (pixels,
/// left/top inclusive, right/bottom exclusive), every `step` pixels.
pub fn sample_backdrop(px: &[u8], stride: usize, rect: (i32, i32, i32, i32), step: usize) -> Option<Backdrop> {
    if stride < 4 {
        return None;
    }
    // Only what's in the picture (a rectangle can reach past it after the
    // window grew and before the next copy).
    let (w, h) = ((stride / 4) as i32, (px.len() / stride) as i32);
    let (x0, y0, x1, y1) = (rect.0.max(0), rect.1.max(0), rect.2.min(w), rect.3.min(h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let step = step.max(1);
    let at = |x: i32, y: i32| -> Option<[f32; 3]> {
        let o = y as usize * stride + x as usize * 4;
        px.get(o..o + 3).map(|p| [p[2] as f32 / 255.0, p[1] as f32 / 255.0, p[0] as f32 / 255.0])
    };
    let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let (mut sum, mut n, mut steps, mut ns) = ([0.0f32; 3], 0u32, 0.0f32, 0u32);
    let mut y = y0.max(0);
    while y < y1 {
        let mut x = x0.max(0);
        while x < x1 {
            if let Some(c) = at(x, y) {
                for i in 0..3 {
                    sum[i] += c[i];
                }
                n += 1;
                if let Some(r) = at(x + step as i32, y).filter(|_| x + (step as i32) < x1) {
                    steps += (luma(r) - luma(c)).abs();
                    ns += 1;
                }
                if let Some(d) = at(x, y + step as i32).filter(|_| y + (step as i32) < y1) {
                    steps += (luma(d) - luma(c)).abs();
                    ns += 1;
                }
            }
            x += step as i32;
        }
        y += step as i32;
    }
    if n == 0 {
        return None;
    }
    let mean = sum.map(|v| v / n as f32);
    Some(Backdrop { mean, lum: luma(mean), busy: if ns > 0 { steps / ns as f32 } else { 0.0 } })
}

/// The shadow is lighter over a plain backdrop and deeper over a busy one
/// (text, photos), so the glass always lifts off what's behind it.
pub fn shadow_alpha(busy: f32, thick: f32) -> f32 {
    (0.12 + 0.06 * thick + 1.1 * busy).clamp(0.12, 0.34)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rim_magnifies_and_never_folds() {
        for h in [20.0, 37.0, 80.0, 150.0, 220.0, 400.0] {
            let th = thickness(h);
            let mut last = -1.0f32;
            let mut d = 0.0f32;
            while d <= th.bevel + 2.0 {
                let sample = d + rim_offset(d, th.bevel, th.mag);
                assert!(sample > last, "height {h}: sample at depth {d} went back ({sample} <= {last})");
                last = sample;
                d += 0.05;
            }
            // The outermost pixel looks furthest in; nothing past the bevel moves.
            assert!((rim_offset(0.0, th.bevel, th.mag) - th.mag).abs() < 1e-5);
            assert_eq!(rim_offset(th.bevel, th.bevel, th.mag), 0.0);
        }
    }

    #[test]
    fn bigger_glass_is_thicker() {
        let pill = thickness(37.0);
        let page = thickness(260.0);
        assert_eq!(pill.t, 0.0);
        assert_eq!(page.t, 1.0);
        assert!(page.bevel > pill.bevel && page.mag > pill.mag);
        assert!(page.shadow_reach > pill.shadow_reach && page.shadow_drop > pill.shadow_drop);
        let mid = thickness(128.5);
        assert!(mid.bevel > pill.bevel && mid.bevel < page.bevel);
        assert!(page.shadow_reach + page.shadow_drop < 11.0);
    }

    #[test]
    fn flip_waits_out_the_dead_band_and_the_hold() {
        let mut f = Flip::new(false);
        // Bright, but inside the dead band: stays dark.
        assert!(!f.update(0.55, 0.0));
        assert!(!f.update(0.55, 1.0));
        assert!(!f.light);
        // Over the line: flips only after the hold.
        assert!(!f.update(0.8, 2.0));
        assert!(f.pending());
        assert!(!f.update(0.8, 2.1));
        assert!(f.update(0.8, 2.0 + FLIP_HOLD));
        assert!(f.light);
        // A passing dark frame resets the hold instead of flipping back.
        assert!(!f.update(0.2, 3.0));
        assert!(!f.update(0.7, 3.1));
        assert!(!f.update(0.2, 3.2));
        assert!(!f.update(0.2, 3.3));
        assert!(f.light);
        assert!(f.update(0.2, 3.2 + FLIP_HOLD));
        assert!(!f.light);
        f.settle(0.9);
        assert!(f.light);
        f.settle(0.1);
        assert!(!f.light);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast([1.0; 3], [0.0; 3]) - 21.0).abs() < 0.01);
        assert!((contrast([0.5; 3], [0.5; 3]) - 1.0).abs() < 1e-5);
        // #767676 on white is the classic 4.54:1.
        let g = 0x76 as f32 / 255.0;
        assert!((contrast([g; 3], [1.0; 3]) - 4.54).abs() < 0.02);
    }

    #[test]
    fn tint_stops_where_text_becomes_readable() {
        let white = [1.0; 3];
        let black = [0.0; 3];
        let ink_dark = [0x1D as f32 / 255.0, 0x1D as f32 / 255.0, 0x1F as f32 / 255.0];
        // White text over a bright orange page needs a real tint toward black.
        let orange = [0.98, 0.70, 0.45];
        let t = tint_for_contrast(orange, white, black, 0.34, 0.9, 4.5);
        assert!(t > 0.34 && t < 0.9, "{t}");
        assert!(contrast(mix3(orange, black, t), white) >= 4.5);
        // ...and just that much: a little less isn't enough.
        assert!(contrast(mix3(orange, black, t - 0.02), white) < 4.5);
        // Over a dark page the minimum tint already reads.
        assert_eq!(tint_for_contrast([0.10, 0.10, 0.12], white, black, 0.34, 0.9, 4.5), 0.34);
        // Dark ink over a dark page: light glass whitens it enough.
        let t = tint_for_contrast([0.2, 0.2, 0.25], ink_dark, white, 0.42, 0.9, 4.5);
        assert!(contrast(mix3([0.2, 0.2, 0.25], white, t), ink_dark) >= 4.5);
        // An impossible target gives the maximum.
        assert_eq!(tint_for_contrast([0.5; 3], [0.5; 3], black, 0.3, 0.6, 4.5), 0.6);
    }

    #[test]
    fn backdrop_sampling_sees_colour_and_busyness() {
        // 8x4 BGRA: left half pure red, right half white.
        let (w, h) = (8usize, 4usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                let c: [u8; 4] = if x < 4 { [0, 0, 255, 255] } else { [255, 255, 255, 255] };
                px[o..o + 4].copy_from_slice(&c);
            }
        }
        let left = sample_backdrop(&px, w * 4, (0, 0, 4, 4), 1).unwrap();
        assert_eq!(left.mean, [1.0, 0.0, 0.0]);
        assert_eq!(left.busy, 0.0);
        let all = sample_backdrop(&px, w * 4, (0, 0, 8, 4), 1).unwrap();
        assert!((all.mean[0] - 1.0).abs() < 1e-6 && (all.mean[1] - 0.5).abs() < 1e-6);
        assert!(all.busy > 0.0);
        assert!(sample_backdrop(&px, w * 4, (2, 2, 2, 3), 1).is_none());
        // Out-of-range rectangles only read what exists, never the next row.
        assert!(sample_backdrop(&px, w * 4, (-4, -4, 2, 2), 1).is_some());
        let right = sample_backdrop(&px, w * 4, (6, 0, 40, 1), 1).unwrap();
        assert_eq!(right.mean, [1.0, 1.0, 1.0]);
        assert!(sample_backdrop(&px, w * 4, (9, 0, 12, 4), 1).is_none());
    }

    #[test]
    fn busy_backdrops_get_deeper_shadows() {
        assert!(shadow_alpha(0.0, 0.0) < shadow_alpha(0.1, 0.0));
        assert!(shadow_alpha(0.0, 0.0) < shadow_alpha(0.0, 1.0));
        assert_eq!(shadow_alpha(5.0, 1.0), 0.34);
    }
}
