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
//! and paints content in the light or dark Glass look on top, and Diorama
//! sets its ground in a raised frame ([`Frame`]). And its shape: every
//! theme hangs from the top edge like a notch, except Pill, which floats
//! below it as a capsule (`crate::settings::Theme::Pill`), and Diorama,
//! which is Pill with a picture in its resting capsule.
//!
//! Diorama has two looks, a day and a night ([`diorama`]): cream paper with
//! slate ink while the sun is up, the night sky's indigo with cream ink and
//! the lit window's gold once it sets, so the whole island turns with the
//! picture in its resting pill.

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
    /// The island's outline.
    pub shape: Shape,
    /// A raised frame round the ground, as the Diorama picture has: the
    /// floating body is drawn as that frame with the ground as its pane.
    pub frame: Option<Frame>,
    swaps: &'static [Swap],
}

/// The raised frame the Diorama theme sets its island's ground in: the
/// resting pill's own frame (`studio_diorama`'s), grown to every shape the
/// island takes. Its colours are drawn as they are, outside the look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Its face, where the light neither catches it nor turns away.
    pub face: Color,
    /// Toward the light (its top) and away from it (its bottom, and the
    /// firm line round its edge).
    pub lit: Color,
    pub unlit: Color,
    /// What its shadow on the pane darkens toward.
    pub shade: Color,
    /// The island's drop shadow: black by day; at night the lit window's
    /// warm glow, so the island glows on the screen.
    pub shadow: Color,
}

/// The island's body: a solid fill in `ground`, or liquid glass (with
/// `ground` as its nominal tint, used where the glass can't see the screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Solid,
    Glass,
}

/// The island's outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Hangs from the top edge: a flat top flush with the screen, concave
    /// shoulders flaring into the edge, rounded bottom corners.
    Notch,
    /// Floats a little below the top edge, every corner rounded: a capsule
    /// while it rests, a card while it's open.
    Pill,
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
    shape: Shape::Notch,
    frame: None,
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
    shape: Shape::Notch,
    frame: None,
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
    shape: Shape::Notch,
    frame: None,
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
    shape: Shape::Notch,
    frame: None,
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
    shape: Shape::Notch,
    frame: None,
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

/// Pill: Classic's black and white (pages paint exactly as they ask) in a
/// floating capsule, with a faint rim (the spec's white at 0.10, a little
/// brighter than Classic's edge) so it holds its shape on a dark screen.
pub const PILL: Look = Look {
    ground: Color::black(1.0),
    ink: Color::white(1.0),
    accent: None,
    border: Color::white(0.10),
    light: false,
    font: None,
    material: Material::Solid,
    shape: Shape::Pill,
    frame: None,
    swaps: &[],
};

/// The Diorama theme's own accent, as the app keeps it (`AppState::accent`,
/// in place of the Windows accent colour): the roof's indigo, brightened to
/// read as a fill. Each Diorama look swaps it for its own: itself by day
/// (the roof for text), the lit window's gold at night.
pub const DIORAMA_ACCENT: Color = Color::hex(0x4C58A8);
/// The lit window's gold (`studio_diorama`'s window light and its glow).
const WINDOW_LIGHT: Color = Color::hex(0xFFD76A);
const WINDOW_GLOW: Color = Color::hex(0xE9AE3A);

/// Diorama by day: the picture's cream frame round a pane of paper a
/// little lighter than it, slate ink (the clock's slate deepened, so text
/// at 62 % still reads at 4.5:1), the roof's indigo for its accent, and the
/// place's own colours for the palette's: green is the grass, orange the
/// cat, yellow the window's gold. Outfit, as Oyen writes.
pub const DIORAMA_DAY: Look = Look {
    ground: Color::hex(0xF8F3E7),
    ink: Color::hex(0x14173A),
    accent: Some(DIORAMA_ACCENT),
    // The frame's unlit edge, at half.
    border: Color::rgba(0xBF as f32 / 255.0, 0xB4 as f32 / 255.0, 0x9C as f32 / 255.0, 0.5),
    light: true,
    font: Some("Outfit"),
    material: Material::Solid,
    shape: Shape::Pill,
    frame: Some(Frame {
        face: Color::hex(0xEFE9DB),
        lit: Color::hex(0xFFFFFF),
        unlit: Color::hex(0xBFB49C),
        shade: Color::hex(0x10142E),
        shadow: Color::black(1.0),
    }),
    swaps: &[
        Swap { from: DIORAMA_ACCENT, fill: DIORAMA_ACCENT, text: Color::hex(0x3B4274) },
        Swap { from: palette::GREEN, fill: Color::hex(0x6F9048), text: Color::hex(0x4A6A30) },
        Swap { from: palette::ORANGE, fill: Color::hex(0xE98B2E), text: Color::hex(0x9E5418) },
        Swap { from: palette::YELLOW, fill: Color::hex(0xF2C24F), text: Color::hex(0x86600E) },
        Swap { from: palette::BLUE, fill: Color::hex(0x5577C8), text: Color::hex(0x34509E) },
        Swap { from: palette::RED, fill: Color::hex(0xD9584A), text: Color::hex(0xB0392D) },
        Swap { from: palette::CYAN, fill: Color::hex(0x5AA2D6), text: Color::hex(0x26668F) },
        Swap { from: palette::PURPLE, fill: Color::hex(0x8B5CB8), text: Color::hex(0x6E3F99) },
        Swap { from: palette::PINK, fill: Color::hex(0xDE5C82), text: Color::hex(0xB03A60) },
    ],
};

/// Diorama at night: the night frame round a pane of deep indigo between
/// the night sky and the frame's face, cream ink, and the lit window's gold
/// for its accent and the palette's warm colours. The palette's others are
/// made for a dark ground already; only blue, which sinks into the indigo,
/// is lifted. The island's shadow is the window's glow.
pub const DIORAMA_NIGHT: Look = Look {
    ground: Color::hex(0x1B1D40),
    ink: Color::hex(0xFFF8E8),
    accent: Some(WINDOW_GLOW),
    // The frame's lit edge, at half: a rim that holds on a dark screen.
    border: Color::rgba(0x55 as f32 / 255.0, 0x55 as f32 / 255.0, 0x8A as f32 / 255.0, 0.5),
    light: false,
    font: Some("Outfit"),
    material: Material::Solid,
    shape: Shape::Pill,
    frame: Some(Frame {
        face: Color::hex(0x2B2C56),
        lit: Color::hex(0x55558A),
        unlit: Color::hex(0x11122A),
        shade: Color::hex(0x10142E),
        // The window's glow, deepened halfway to black: a warm shadow.
        shadow: Color::hex(0x75571D),
    }),
    swaps: &[
        Swap { from: DIORAMA_ACCENT, fill: WINDOW_GLOW, text: WINDOW_LIGHT },
        Swap { from: WINDOW_GLOW, fill: WINDOW_GLOW, text: WINDOW_LIGHT },
        Swap { from: palette::ORANGE, fill: WINDOW_GLOW, text: Color::hex(0xF2B550) },
        Swap { from: palette::YELLOW, fill: WINDOW_LIGHT, text: WINDOW_LIGHT },
        Swap { from: palette::BLUE, fill: Color::hex(0x3D8BFF), text: Color::hex(0x7AB0FF) },
    ],
};

/// The theme's look. Glass starts dark; the app picks [`glass`] light or
/// dark from what's behind the island and the Windows mode, and
/// [`diorama`] by day or at night from the picture's sun.
pub fn of(theme: Theme) -> &'static Look {
    match theme {
        Theme::Classic => &CLASSIC,
        Theme::Oyen => &OYEN,
        Theme::Glass => &GLASS_DARK,
        Theme::Pill => &PILL,
        Theme::Diorama => &DIORAMA_DAY,
    }
}

/// The theme's look on a screen where the island wraps a camera notch (the
/// Mac app on a MacBook): the island meets black hardware there, so Oyen
/// goes black and Glass is the dark glass (with a black strip where it
/// touches the notch, drawn by the Mac app). Pill and Diorama float below
/// the notch and keep their looks.
pub fn around_notch(theme: Theme) -> &'static Look {
    match theme {
        Theme::Classic => &CLASSIC,
        Theme::Oyen => &OYEN_NIGHT,
        Theme::Glass => &GLASS_DARK,
        Theme::Pill => &PILL,
        Theme::Diorama => &DIORAMA_DAY,
    }
}

/// The Diorama look by day (`false`) or at night, as the picture in its
/// resting pill shows.
pub fn diorama(night: bool) -> &'static Look {
    if night {
        &DIORAMA_NIGHT
    } else {
        &DIORAMA_DAY
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
    fn pill_floats_and_paints_like_classic() {
        assert_eq!(PILL.shape, Shape::Pill);
        assert_eq!(*of(Theme::Pill), PILL);
        assert_eq!(*around_notch(Theme::Pill), PILL);
        for look in [&CLASSIC, &OYEN, &OYEN_NIGHT, &GLASS_DARK, &GLASS_LIGHT] {
            assert_eq!(look.shape, Shape::Notch);
        }
        // Black and white like Classic: what a page asks for is what it gets.
        for c in [Color::white(0.62), Color::black(1.0), palette::GREEN, Color::hex(0x123456)] {
            assert_eq!(PILL.paint(c), c);
            assert_eq!(PILL.paint_text(c), c);
        }
        assert_eq!(PILL.material, Material::Solid);
        // Only Diorama's island is set in a frame.
        for look in [&CLASSIC, &OYEN, &OYEN_NIGHT, &GLASS_DARK, &GLASS_LIGHT, &PILL] {
            assert_eq!(look.frame, None);
        }
    }

    /// WCAG contrast of `fg` drawn at `alpha` over `bg` (blended as the
    /// renderers blend, in sRGB), against `bg`.
    fn contrast(fg: Color, alpha: f32, bg: Color) -> f32 {
        let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let lum = |c: Color| 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
        let seen = Color::lerp(bg, fg, alpha);
        let (a, b) = (lum(seen), lum(bg));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn diorama_floats_by_day_and_at_night_in_its_frame() {
        assert_eq!(*of(Theme::Diorama), DIORAMA_DAY);
        assert_eq!(*around_notch(Theme::Diorama), DIORAMA_DAY);
        assert_eq!(*diorama(false), DIORAMA_DAY);
        assert_eq!(*diorama(true), DIORAMA_NIGHT);
        for look in [&DIORAMA_DAY, &DIORAMA_NIGHT] {
            assert_eq!(look.shape, Shape::Pill);
            assert_eq!(look.material, Material::Solid);
            assert_eq!(look.font, OYEN.font);
            assert!(look.frame.is_some());
        }
        assert!(DIORAMA_DAY.light && !DIORAMA_NIGHT.light);
        // Greys go between the pane and the ink, as in every look.
        for look in [&DIORAMA_DAY, &DIORAMA_NIGHT] {
            assert_eq!(look.paint(palette::ISLAND), look.ground);
            assert_eq!(look.paint(Color::white(1.0)), look.ink);
            assert_eq!(look.paint(palette::TEXT_DIM), look.ink.with_alpha(0.62));
            assert_eq!(look.paint_text(palette::ON_ACCENT), palette::ON_ACCENT);
        }
        // The frame is the picture's (studio_diorama's FRAME_DAY and
        // FRAME_NIGHT): cream by day, indigo at night, and the pane between
        // its face and, at night, the night sky.
        let (day, night) = (DIORAMA_DAY.frame.unwrap(), DIORAMA_NIGHT.frame.unwrap());
        assert_eq!((day.face, day.lit, day.unlit), (Color::hex(0xEFE9DB), Color::hex(0xFFFFFF), Color::hex(0xBFB49C)));
        assert_eq!((night.face, night.lit, night.unlit), (Color::hex(0x2B2C56), Color::hex(0x55558A), Color::hex(0x11122A)));
        assert!(DIORAMA_DAY.ground.luminance() > day.face.luminance());
        assert!(DIORAMA_NIGHT.ground.luminance() < night.face.luminance() && DIORAMA_NIGHT.ground.luminance() > Color::hex(0x0E1033).luminance());
        // By day the shadow is Pill's black; at night it is warm (the window's glow).
        assert_eq!(day.shadow, Color::black(1.0));
        assert!(night.shadow.r > night.shadow.g && night.shadow.g > night.shadow.b);
        // The theme's own accent: its indigo by day, the window's gold at night.
        assert_eq!(DIORAMA_DAY.paint(DIORAMA_ACCENT), DIORAMA_ACCENT);
        assert_eq!(DIORAMA_NIGHT.paint(DIORAMA_ACCENT), WINDOW_GLOW);
        assert_eq!(DIORAMA_NIGHT.paint_text(DIORAMA_ACCENT), WINDOW_LIGHT);
        // By day the palette takes the place's colours (the grass, the cat),
        // deeper for text; at night orange and yellow take the window's.
        assert_eq!(DIORAMA_DAY.paint(palette::GREEN), Color::hex(0x6F9048));
        assert_eq!(DIORAMA_DAY.paint(palette::ORANGE), Color::hex(0xE98B2E));
        assert_eq!(DIORAMA_NIGHT.paint(palette::ORANGE), WINDOW_GLOW);
        assert_eq!(DIORAMA_NIGHT.paint(palette::YELLOW), WINDOW_LIGHT);
        assert_eq!(DIORAMA_NIGHT.paint(palette::GREEN), palette::GREEN);
        for s in DIORAMA_DAY.swaps {
            assert!(s.text.luminance() <= s.fill.luminance(), "{s:?}: text no lighter than its fill on cream");
        }
    }

    #[test]
    fn diorama_text_reads_at_four_and_a_half_to_one() {
        // Ink, ink at 62 % (palette::TEXT_DIM), the accent as text and
        // every accent's text shade, on each look's ground. By day: 15.6,
        // 4.75, 8.6, then 5.6 (green) 5.1 (orange) 5.1 (yellow) 6.8 (blue)
        // 5.5 (red) 5.6 (cyan) 6.7 (purple) 5.2 (pink). At night: 15.3,
        // 6.6, 11.7, then the palette's own (blue lifted to 7.3; purple and
        // pink are the lowest at 4.6).
        let accents = [palette::GREEN, palette::ORANGE, palette::YELLOW, palette::BLUE, palette::RED, palette::CYAN, palette::PURPLE, palette::PINK, DIORAMA_ACCENT];
        for look in [&DIORAMA_DAY, &DIORAMA_NIGHT] {
            let g = look.ground;
            let mut said = vec![("ink", contrast(look.ink, 1.0, g)), ("ink at 62 %", contrast(look.ink, palette::TEXT_DIM.a, g))];
            said.push(("accent", contrast(look.paint_text(look.accent.unwrap()), 1.0, g)));
            for a in accents {
                said.push(("an accent's text", contrast(look.paint_text(a), 1.0, g)));
            }
            for (what, c) in said {
                assert!(c >= 4.5, "{}: {what} at {c:.2}:1", if look.light { "day" } else { "night" });
            }
        }
        // The numbers the comment gives, so they stay true.
        assert!((contrast(DIORAMA_DAY.ink, 0.62, DIORAMA_DAY.ground) - 4.75).abs() < 0.05);
        assert!((contrast(DIORAMA_NIGHT.ink, 0.62, DIORAMA_NIGHT.ground) - 6.64).abs() < 0.05);
        assert!((contrast(DIORAMA_NIGHT.paint_text(palette::BLUE), 1.0, DIORAMA_NIGHT.ground) - 7.31).abs() < 0.05);
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
