//! How the island paints in each theme: its ground, its ink and its accents.
//!
//! Pages and activities draw in one vocabulary, the Classic island's: a black
//! ground, white content at some opacity (text, icons, washes, tracks) and a
//! few system-style accents (`geom::palette`). A [`Look`] maps that vocabulary
//! onto its own colours as the canvas paints, so every page follows the theme
//! without knowing about it, including pages written later.
//!
//! * Greys (white, black and everything between, at any opacity) are read as
//!   "white at that brightness over the black island" and repainted between
//!   the look's ground and ink: Classic's white text becomes the ink, its 8 %
//!   white wash becomes an 8 % ink wash, the black island becomes the ground.
//! * The palette's accents swap for the look's own, with a deeper shade for
//!   text so small coloured labels stay readable on a light ground.
//! * Anything else (album-art colours, Oyen's own face, `palette::ON_ACCENT`)
//!   is left alone.

use crate::geom::{palette, Color};
use crate::settings::Theme;

/// An accent and what a look paints instead: `fill` for shapes, `text` for
/// coloured text and icons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swap {
    pub from: Color,
    pub fill: Color,
    pub text: Color,
}

#[derive(Debug, PartialEq)]
pub struct Look {
    /// The island's fill: what Classic paints black.
    pub ground: Color,
    /// Content: what Classic paints white.
    pub ink: Color,
    /// The theme's own accent; `None` follows the Windows accent colour.
    pub accent: Option<Color>,
    /// Hairline around the island (Classic: a faint white edge on black).
    pub border: Color,
    /// Light ground with dark content.
    pub light: bool,
    /// Embedded font family the look writes in; `None` = the system UI font.
    pub font: Option<&'static str>,
    swaps: &'static [Swap],
}

const OYEN_ORANGE: Color = Color::hex(crate::oyen::ORANGE);
const OYEN_FOLD: Color = Color::hex(crate::oyen::FOLD_ORANGE);

pub const CLASSIC: Look =
    Look { ground: Color::black(1.0), ink: Color::white(1.0), accent: None, border: palette::BORDER, light: false, font: None, swaps: &[] };

/// Oyen: warm cream paper, espresso ink, Oyen's orange, and Outfit (the Oyen
/// Poni website's typeface) for every Latin letter.
pub const OYEN: Look = Look {
    ground: Color::hex(0xFFF1DE),
    ink: Color::hex(0x2B1B10),
    accent: Some(OYEN_ORANGE),
    border: Color::rgba(0.55, 0.30, 0.10, 0.22),
    light: true,
    font: Some("Outfit"),
    swaps: &[
        Swap { from: OYEN_ORANGE, fill: OYEN_ORANGE, text: OYEN_FOLD },
        Swap { from: palette::ORANGE, fill: OYEN_ORANGE, text: OYEN_FOLD },
        Swap { from: palette::GREEN, fill: Color::hex(0x2DB45A), text: Color::hex(0x1B7F3E) },
        Swap { from: palette::RED, fill: Color::hex(0xE5484D), text: Color::hex(0xC9302C) },
        Swap { from: palette::BLUE, fill: Color::hex(0x2F7CF6), text: Color::hex(0x1A5FCC) },
        Swap { from: palette::CYAN, fill: Color::hex(0x2AA5D6), text: Color::hex(0x0F7299) },
        Swap { from: palette::PURPLE, fill: Color::hex(0xA552D8), text: Color::hex(0x8337B8) },
        Swap { from: palette::YELLOW, fill: Color::hex(0xE8A33D), text: Color::hex(0xA8670A) },
        Swap { from: palette::PINK, fill: Color::hex(0xF0477A), text: Color::hex(0xC92B5E) },
    ],
};

pub fn of(theme: Theme) -> &'static Look {
    match theme {
        Theme::Classic => &CLASSIC,
        Theme::Oyen => &OYEN,
    }
}

impl Look {
    /// The colour to paint for a shape the page asked to fill or stroke in `c`.
    pub fn paint(&self, c: Color) -> Color {
        self.map(c, false)
    }

    /// The colour to paint for text (and icons) the page asked for in `c`.
    pub fn paint_text(&self, c: Color) -> Color {
        self.map(c, true)
    }

    fn map(&self, c: Color, text: bool) -> Color {
        if self.swaps.is_empty() && self.ground == CLASSIC.ground && self.ink == CLASSIC.ink {
            return c;
        }
        if c.r == c.g && c.g == c.b {
            let base = match c.r {
                v if v <= 0.0 => self.ground,
                v if v >= 1.0 => self.ink,
                v => Color::lerp(self.ground, self.ink, v),
            };
            return base.with_alpha(c.a);
        }
        match self.swaps.iter().find(|s| same_rgb(s.from, c)) {
            Some(s) => (if text { s.text } else { s.fill }).with_alpha(c.a),
            None => c,
        }
    }
}

fn same_rgb(a: Color, b: Color) -> bool {
    (a.r - b.r).abs() < 1e-4 && (a.g - b.g).abs() < 1e-4 && (a.b - b.b).abs() < 1e-4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_paints_what_it_is_given() {
        for c in [Color::white(0.62), Color::black(1.0), palette::GREEN, Color::rgba(0.16, 0.16, 0.16, 1.0), Color::hex(0x123456)] {
            assert_eq!(CLASSIC.paint(c), c);
            assert_eq!(CLASSIC.paint_text(c), c);
        }
    }

    #[test]
    fn oyen_turns_greys_into_ink_on_cream() {
        assert_eq!(OYEN.paint(Color::black(1.0)), OYEN.ground);
        assert_eq!(OYEN.paint(Color::white(1.0)), OYEN.ink);
        // White at 62 % keeps its opacity, now in ink.
        assert_eq!(OYEN.paint(palette::TEXT_DIM), OYEN.ink.with_alpha(0.62));
        // An opaque mid grey lands between the ground and the ink.
        let g = OYEN.paint(Color::rgba(0.16, 0.16, 0.16, 1.0));
        assert_eq!(g, Color::lerp(OYEN.ground, OYEN.ink, 0.16));
        // The invisible hit-mask colour stays (nearly) invisible.
        assert!(OYEN.paint(Color::black(1.0 / 255.0)).a < 0.01);
    }

    #[test]
    fn oyen_swaps_accents_and_deepens_them_for_text() {
        let fill = OYEN.paint(palette::ORANGE);
        let text = OYEN.paint_text(palette::ORANGE);
        assert_eq!(fill, OYEN_ORANGE);
        assert_eq!(text, OYEN_FOLD);
        assert!(text.luminance() < fill.luminance());
        // Opacity survives the swap.
        assert_eq!(OYEN.paint(palette::GREEN.mul_alpha(0.5)).a, 0.5);
        // Colours the look doesn't know (album art, Oyen's face) pass through,
        // and so does content on an accent fill.
        let art = Color::hex(0x7A3EE0);
        assert_eq!(OYEN.paint(art), art);
        assert_eq!(OYEN.paint_text(palette::ON_ACCENT), palette::ON_ACCENT);
    }
}
