//! Geometry and colour primitives in device-independent pixels (DIPs).

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Point) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_center(c: Point, w: f32, h: f32) -> Self {
        Self::new(c.x - w / 2.0, c.y - h / 2.0, w, h)
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }
    /// Shrink by `dx` on the left/right and `dy` on the top/bottom.
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    /// Split off `w` from the left; returns (left, rest).
    pub fn split_left(&self, w: f32) -> (Rect, Rect) {
        let w = w.min(self.w).max(0.0);
        (Rect::new(self.x, self.y, w, self.h), Rect::new(self.x + w, self.y, self.w - w, self.h))
    }
    /// Split off `w` from the right; returns (rest, right).
    pub fn split_right(&self, w: f32) -> (Rect, Rect) {
        let w = w.min(self.w).max(0.0);
        (Rect::new(self.x, self.y, self.w - w, self.h), Rect::new(self.right() - w, self.y, w, self.h))
    }
    /// Split off `h` from the top; returns (top, rest).
    pub fn split_top(&self, h: f32) -> (Rect, Rect) {
        let h = h.min(self.h).max(0.0);
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + h, self.w, self.h - h))
    }
    /// Split off `h` from the bottom; returns (rest, bottom).
    pub fn split_bottom(&self, h: f32) -> (Rect, Rect) {
        let h = h.min(self.h).max(0.0);
        (Rect::new(self.x, self.y, self.w, self.h - h), Rect::new(self.x, self.bottom() - h, self.w, h))
    }
    /// Divide horizontally into `n` equal columns separated by `gap`.
    pub fn columns(&self, n: usize, gap: f32) -> Vec<Rect> {
        if n == 0 {
            return Vec::new();
        }
        let cw = ((self.w - gap * (n as f32 - 1.0)) / n as f32).max(0.0);
        (0..n).map(|i| Rect::new(self.x + i as f32 * (cw + gap), self.y, cw, self.h)).collect()
    }
    /// Divide vertically into `n` equal rows separated by `gap`.
    pub fn rows(&self, n: usize, gap: f32) -> Vec<Rect> {
        if n == 0 {
            return Vec::new();
        }
        let rh = ((self.h - gap * (n as f32 - 1.0)) / n as f32).max(0.0);
        (0..n).map(|i| Rect::new(self.x, self.y + i as f32 * (rh + gap), self.w, rh)).collect()
    }
    pub fn lerp(a: Rect, b: Rect, t: f32) -> Rect {
        Rect::new(lerp(a.x, b.x, t), lerp(a.y, b.y, t), lerp(a.w, b.w, t), lerp(a.h, b.h, t))
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Straight (non-premultiplied) RGBA colour, components 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    /// `0xRRGGBB`, opaque.
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: ((rgb >> 16) & 0xff) as f32 / 255.0,
            g: ((rgb >> 8) & 0xff) as f32 / 255.0,
            b: (rgb & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }
    pub const fn white(a: f32) -> Self {
        Self::rgba(1.0, 1.0, 1.0, a)
    }
    pub const fn black(a: f32) -> Self {
        Self::rgba(0.0, 0.0, 0.0, a)
    }
    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }
    pub fn mul_alpha(self, f: f32) -> Self {
        Self { a: self.a * f, ..self }
    }
    pub fn lerp(a: Color, b: Color, t: f32) -> Color {
        Color::rgba(lerp(a.r, b.r, t), lerp(a.g, b.g, t), lerp(a.b, b.b, t), lerp(a.a, b.a, t))
    }
    /// Relative luminance (sRGB approximation).
    pub fn luminance(&self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
    /// Lift very dark colours so they stay visible on the black island, and
    /// tame very light ones so white text on top keeps contrast.
    pub fn for_island_accent(self) -> Color {
        let l = self.luminance();
        if l < 0.30 {
            // Luminance is linear in r,g,b, so this lands exactly on 0.30.
            Color::lerp(self, Color::white(1.0), (0.30 - l) / (1.0 - l)).with_alpha(self.a)
        } else if l > 0.85 {
            Color::lerp(self, Color::black(1.0), 0.15).with_alpha(self.a)
        } else {
            self
        }
    }
}

/// The island palette. The island itself is pure black; content is white at
/// varying opacity plus a few system-style accent colours.
pub mod palette {
    use super::Color;
    pub const ISLAND: Color = Color::black(1.0);
    pub const BORDER: Color = Color::white(0.07);
    pub const TEXT: Color = Color::white(0.96);
    pub const TEXT_DIM: Color = Color::white(0.62);
    /// Tertiary text: 0.50 keeps ~5:1 contrast on black (0.38 was below AA).
    pub const TEXT_FAINT: Color = Color::white(0.50);
    /// Inert / disabled controls.
    pub const DISABLED: Color = Color::white(0.30);
    pub const SURFACE: Color = Color::white(0.08);
    pub const SURFACE_HOVER: Color = Color::white(0.15);
    pub const SURFACE_PRESSED: Color = Color::white(0.22);
    pub const TRACK: Color = Color::white(0.16);
    pub const GREEN: Color = Color::hex(0x30D158);
    pub const ORANGE: Color = Color::hex(0xFF9F0A);
    pub const RED: Color = Color::hex(0xFF453A);
    pub const BLUE: Color = Color::hex(0x0A84FF);
    pub const CYAN: Color = Color::hex(0x5AC8FA);
    pub const PURPLE: Color = Color::hex(0xBF5AF2);
    pub const YELLOW: Color = Color::hex(0xFFD60A);
    pub const PINK: Color = Color::hex(0xFF375F);
    /// Content sitting on an accent fill (a switch knob, today's date, the
    /// play glyph on an accent disc): white in every theme. Its blue is a hair
    /// under 1 so a look can tell it from content white, which it repaints.
    pub const ON_ACCENT: Color = Color::rgba(1.0, 1.0, 0.998, 1.0);
    /// Dark content on a very light accent fill, in every theme.
    pub const ON_ACCENT_DARK: Color = Color::rgba(0.0, 0.0, 0.002, 0.88);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_splits() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        let (l, rest) = r.split_left(30.0);
        assert_eq!(l, Rect::new(10.0, 20.0, 30.0, 50.0));
        assert_eq!(rest, Rect::new(40.0, 20.0, 70.0, 50.0));
        let (t, b) = r.split_top(10.0);
        assert_eq!(t.h, 10.0);
        assert_eq!(b.y, 30.0);
        let cols = r.columns(4, 4.0);
        assert_eq!(cols.len(), 4);
        assert!((cols[3].right() - r.right()).abs() < 1e-4);
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(!r.contains(Point::new(10.0, 5.0)));
    }

    #[test]
    fn hex_colour() {
        let c = Color::hex(0xFF8000);
        assert!((c.r - 1.0).abs() < 1e-6 && (c.g - 128.0 / 255.0).abs() < 1e-6 && c.b == 0.0);
    }
}
