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
//!
//! A look also says what the island is made of: Classic and Oyen paint a
//! solid ground, Glass draws liquid glass there (gfx and the glass shader)
//! and paints content in the light or dark Glass look on top.

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
    /// What the island's body is made of.
    pub material: Material,
    swaps: &'static [Swap],
}

/// The island's body: a solid fill in `ground`, or liquid glass (with
/// `ground` as its nominal tint, used where the glass can't see the screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Solid,
    Glass,
}

const OYEN_ORANGE: Color = Color::hex(crate::oyen::ORANGE);
const OYEN_FOLD: Color = Color::hex(crate::oyen::FOLD_ORANGE);

pub const CLASSIC: Look = Look {
    ground: Color::black(1.0),
    ink: Color::white(1.0),
    accent: None,
    border: palette::BORDER,
    light: false,
    font: None,
    material: Material::Solid,
    swaps: &[],
};

/// Oyen: warm cream paper, espresso ink, Oyen's orange, and Outfit (the Oyen
/// Poni website's typeface) for every Latin letter.
pub const OYEN: Look = Look {
    ground: Color::hex(0xFFF1DE),
    ink: Color::hex(0x2B1B10),
    accent: Some(OYEN_ORANGE),
    border: Color::rgba(0.55, 0.30, 0.10, 0.22),
    light: true,
    font: Some("Outfit"),
    material: Material::Solid,
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

/// Oyen around a MacBook's camera notch. The notch is black hardware that no
/// pixel can colour, so a cream island would wear a black hole in its top
/// edge: here the island is black like the notch (and Classic), and Oyen
/// stays in everything on it: cream ink, his orange, Outfit, his face.
pub const OYEN_NIGHT: Look = Look {
    ground: Color::black(1.0),
    ink: Color::hex(0xFFF1DE),
    accent: Some(OYEN_ORANGE),
    border: Color::rgba(1.0, 0.945, 0.871, 0.07),
    light: false,
    font: Some("Outfit"),
    material: Material::Solid,
    // His orange reads on black as it is, in fills and in text alike; the
    // palette's other accents are made for a dark ground already.
    swaps: &[
        Swap { from: OYEN_ORANGE, fill: OYEN_ORANGE, text: OYEN_ORANGE },
        Swap { from: palette::ORANGE, fill: OYEN_ORANGE, text: OYEN_ORANGE },
    ],
};

/// Glass over a dark backdrop (and in Windows dark mode): white ink, the
/// palette's own accents (made for dark grounds), the system UI font.
pub const GLASS_DARK: Look = Look {
    ground: Color::hex(0x1C1C1E),
    ink: Color::white(1.0),
    accent: None,
    border: Color::white(0.10),
    light: false,
    font: None,
    material: Material::Glass,
    swaps: &[],
};

/// Glass over a light backdrop (and in Windows light mode): near-black ink,
/// and accents in their light-mode shades, deeper still for text.
pub const GLASS_LIGHT: Look = Look {
    ground: Color::hex(0xF2F2F7),
    ink: Color::hex(0x1D1D1F),
    accent: None,
    border: Color::black(0.08),
    light: true,
    font: None,
    material: Material::Glass,
    swaps: &[
        Swap { from: palette::GREEN, fill: Color::hex(0x34C759), text: Color::hex(0x248A3D) },
        Swap { from: palette::ORANGE, fill: Color::hex(0xFF9500), text: Color::hex(0xC93400) },
        Swap { from: palette::RED, fill: Color::hex(0xFF3B30), text: Color::hex(0xD70015) },
        Swap { from: palette::BLUE, fill: Color::hex(0x007AFF), text: Color::hex(0x0040DD) },
        Swap { from: palette::CYAN, fill: Color::hex(0x32ADE6), text: Color::hex(0x0071A4) },
        Swap { from: palette::PURPLE, fill: Color::hex(0xAF52DE), text: Color::hex(0x8944AB) },
        Swap { from: palette::YELLOW, fill: Color::hex(0xFFCC00), text: Color::hex(0xA05A00) },
        Swap { from: palette::PINK, fill: Color::hex(0xFF2D55), text: Color::hex(0xD30F45) },
    ],
};

/// The theme's look. Glass starts dark; the app picks [`glass`] light or
/// dark from what's behind the island and the Windows mode.
pub fn of(theme: Theme) -> &'static Look {
    match theme {
        Theme::Classic => &CLASSIC,
        Theme::Oyen => &OYEN,
        Theme::Glass => &GLASS_DARK,
    }
}

/// The theme's look on a screen where the island wraps a camera notch (the
/// Mac app on a MacBook): the island meets black hardware there, so Oyen
/// goes black and Glass is the dark glass (with a black strip where it
/// touches the notch, drawn by the Mac app).
pub fn around_notch(theme: Theme) -> &'static Look {
    match theme {
        Theme::Classic => &CLASSIC,
        Theme::Oyen => &OYEN_NIGHT,
        Theme::Glass => &GLASS_DARK,
    }
}

/// The Glass look for a light (`true`) or dark backdrop.
pub fn glass(light: bool) -> &'static Look {
    if light {
        &GLASS_LIGHT
    } else {
        &GLASS_DARK
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

    #[test]
    fn oyen_around_a_notch_is_black_with_oyen_on_it() {
        // The island's own black is the notch's black, exactly.
        assert_eq!(OYEN_NIGHT.paint(palette::ISLAND), palette::ISLAND);
        assert_eq!(OYEN_NIGHT.ground, CLASSIC.ground);
        // White content becomes Oyen's cream, at the opacity it asked for.
        assert_eq!(OYEN_NIGHT.paint(Color::white(1.0)), OYEN.ground);
        assert_eq!(OYEN_NIGHT.paint(palette::TEXT_DIM), OYEN.ground.with_alpha(0.62));
        // His orange in fills and text, bright enough on black.
        assert_eq!(OYEN_NIGHT.paint(palette::ORANGE), OYEN_ORANGE);
        assert_eq!(OYEN_NIGHT.paint_text(palette::ORANGE), OYEN_ORANGE);
        assert!(OYEN_ORANGE.luminance() > 0.3);
        // The palette's dark-ground accents and the content on them stay.
        assert_eq!(OYEN_NIGHT.paint(palette::GREEN), palette::GREEN);
        assert_eq!(OYEN_NIGHT.paint_text(palette::ON_ACCENT), palette::ON_ACCENT);
        assert!(!OYEN_NIGHT.light);
        assert_eq!(OYEN_NIGHT.font, OYEN.font);
        assert_eq!(OYEN_NIGHT.material, Material::Solid);
    }

    #[test]
    fn around_a_notch_only_oyen_changes() {
        assert_eq!(*around_notch(Theme::Classic), CLASSIC);
        assert_eq!(*around_notch(Theme::Oyen), OYEN_NIGHT);
        assert_eq!(*around_notch(Theme::Glass), GLASS_DARK);
        // Away from a notch (and on Windows) every theme keeps its look.
        assert_eq!(*of(Theme::Oyen), OYEN);
    }

    #[test]
    fn glass_is_glass_and_the_others_are_solid() {
        assert_eq!(CLASSIC.material, Material::Solid);
        assert_eq!(OYEN.material, Material::Solid);
        assert_eq!(GLASS_DARK.material, Material::Glass);
        assert_eq!(GLASS_LIGHT.material, Material::Glass);
        assert_eq!(*of(Theme::Glass), GLASS_DARK);
        assert_eq!(*glass(true), GLASS_LIGHT);
        assert_eq!(*glass(false), GLASS_DARK);
    }

    #[test]
    fn light_glass_writes_dark_ink_and_deepens_accent_text() {
        assert_eq!(GLASS_LIGHT.paint(Color::white(1.0)), GLASS_LIGHT.ink);
        assert_eq!(GLASS_LIGHT.paint(palette::TEXT_DIM), GLASS_LIGHT.ink.with_alpha(0.62));
        // Black content (a glyph cut out of a white disc) takes the glass's ground.
        assert_eq!(GLASS_LIGHT.paint(Color::black(1.0)), GLASS_LIGHT.ground);
        let fill = GLASS_LIGHT.paint(palette::BLUE);
        let text = GLASS_LIGHT.paint_text(palette::BLUE);
        assert!(text.luminance() < fill.luminance());
        // Dark glass keeps the palette's accents, made for dark grounds.
        assert_eq!(GLASS_DARK.paint(palette::GREEN), palette::GREEN);
        assert_eq!(GLASS_DARK.paint(Color::white(0.5)), Color::white(0.5));
        assert_eq!(GLASS_DARK.paint_text(palette::ON_ACCENT), palette::ON_ACCENT);
    }
}
