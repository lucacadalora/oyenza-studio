//! The place itself, painted on a grid of art pixels, one per DIP: a house
//! at the left end, a tree at the right, a fence and a letter box between
//! them, hills behind, and over it all a sky that keeps the day's hours.
//!
//! Two pictures can be painted for any minute: the day side (the sun on its
//! way across, dawn's pink turning into day and day into the golden hour)
//! and the night side (dusk fading into the dark, the moon in its phase, the
//! stars coming out, the window lit until midnight). [`paint`] shows one of
//! them, or both dissolving into each other while the glass ball rolls.
//!
//! Colours never fade: every change, from one hour's palette to the next and
//! from day to night, is an ordered dither between two flat colours, a few
//! pixels a minute, as pixel art does it.

use crate::{Badge, Clock, Moment};

pub(crate) type Rgb = [u8; 3];

/// The first row of the ground.
pub(crate) const HORIZON: i32 = 27;
/// The columns each end keeps for itself: the house (and the glass ball by
/// day) at the left, the tree (and the ball at night) at the right. What is
/// between them is the middle: the fence, the letter box and the clock.
pub(crate) const HOUSE_ZONE: i32 = 30;
pub(crate) const TREE_ZONE: i32 = 31;
/// The middle with no clock in it: the letter box and the fence.
pub(crate) const MID_BARE: i32 = 18;
/// Air on each side of the clock.
pub(crate) const MID_PAD: i32 = 6;
/// "23:59" in the sky's digits: four digits, the colon and the gaps.
pub(crate) const CLOCK_W: i32 = 4 * DIGIT_W + 2 + 4;
/// " AM" after it: a gap and the two small letters.
pub(crate) const MERIDIEM_W: i32 = 3 + 3 + 1 + 5;
/// A badge after the clock: a gap and the glyph.
pub(crate) const BADGE_W: i32 = 3 + 9;

const fn rgb(h: u32) -> Rgb {
    [(h >> 16) as u8, (h >> 8) as u8, h as u8]
}

// ------------------------------------------------------------- materials

/// What a pixel is made of. Each has a colour in every key of the day.
mod mat {
    pub const CLOUD: usize = 0;
    pub const CLOUD_SHADE: usize = 1;
    pub const HILL_FAR: usize = 2;
    pub const HILL_NEAR: usize = 3;
    pub const GRASS_LIGHT: usize = 4;
    pub const GRASS: usize = 5;
    pub const GRASS_DARK: usize = 6;
    pub const PATH: usize = 7;
    pub const PATH_DARK: usize = 8;
    pub const WALL: usize = 9;
    pub const WALL_LINE: usize = 10;
    pub const ROOF: usize = 11;
    pub const ROOF_LIGHT: usize = 12;
    pub const ROOF_DARK: usize = 13;
    pub const WINDOW: usize = 14;
    pub const WINDOW_GLINT: usize = 15;
    pub const SHUTTER: usize = 16;
    pub const SHUTTER_DARK: usize = 17;
    pub const DOOR: usize = 18;
    pub const DOOR_DARK: usize = 19;
    pub const LEAF_LIGHT: usize = 20;
    pub const LEAF: usize = 21;
    pub const LEAF_DARK: usize = 22;
    pub const TRUNK: usize = 23;
    pub const TRUNK_DARK: usize = 24;
    pub const FENCE: usize = 25;
    pub const FENCE_DARK: usize = 26;
    pub const FLOWER_A: usize = 27;
    pub const FLOWER_B: usize = 28;
    pub const FLOWER_C: usize = 29;
    pub const CAT: usize = 30;
    pub const CAT_DARK: usize = 31;
    pub const CAT_LIGHT: usize = 32;
    pub const POST: usize = 33;
    pub const CHIMNEY: usize = 34;
    pub const SMOKE: usize = 35;
    pub const ORB: usize = 36;
    pub const ORB_SHADE: usize = 37;
    pub const HALO: usize = 38;
    pub const STAR: usize = 39;
    pub const STAR_DIM: usize = 40;
    pub const GLOW: usize = 41;
    pub const GLOW_DEEP: usize = 42;
    pub const COUNT: usize = 43;
}

/// The keys of the day: three on the day side, two on the night side.
const DAWN: usize = 0;
const DAY: usize = 1;
const GOLD: usize = 2;
const DUSK: usize = 3;
const NIGHT: usize = 4;

/// Every material in every key: dawn, day, the golden hour, dusk, night.
#[rustfmt::skip]
const PAL: [[u32; 5]; mat::COUNT] = [
    /* cloud        */ [0xFFF2E8, 0xFFFFFF, 0xFBE9C9, 0x5C5088, 0x1F2352],
    /* cloud shade  */ [0xF1BBAA, 0xDCE6F5, 0xEFA238, 0x9A5A7A, 0x191C48],
    /* hill far     */ [0x8E9CC8, 0x8FA8D0, 0x5E79A8, 0x3A3F78, 0x1D2152],
    /* hill near    */ [0x7A8ABB, 0x7C97C4, 0x516A98, 0x2F3468, 0x181B48],
    /* grass light  */ [0x86A870, 0x8CA85C, 0x7A8C45, 0x3F5648, 0x2A2F66],
    /* grass        */ [0x678B5A, 0x6F9048, 0x5C7234, 0x334A40, 0x20245A],
    /* grass dark   */ [0x4F7049, 0x56763A, 0x435628, 0x283B36, 0x191C4C],
    /* path         */ [0xD6CABE, 0xD6CEBB, 0xCDB78F, 0x6A6680, 0x343A72],
    /* path dark    */ [0xBBAEA1, 0xBDB5A1, 0xAB9670, 0x56526C, 0x2A2F64],
    /* wall         */ [0xFDF1E8, 0xFFFFFF, 0xFBE3A4, 0x7C7CA8, 0x3A3F7C],
    /* wall line    */ [0xE8D2C7, 0xE4DED0, 0xE6BF6A, 0x6A6A96, 0x30356C],
    /* roof         */ [0x3A4072, 0x3B4274, 0x363C6C, 0x23274F, 0x1A1D46],
    /* roof light   */ [0x6A73A2, 0x6E78A6, 0x5E6794, 0x3A3F70, 0x2C3166],
    /* roof dark    */ [0x2A305E, 0x2B3160, 0x272C58, 0x1A1D40, 0x101232],
    /* window       */ [0xF8D070, 0x2E376B, 0xDDAA3C, 0xFFD76A, 0xFFD76A],
    /* window glint */ [0xFFF2BF, 0x8E9BD0, 0xF8D680, 0xFFF2B8, 0xFFF2B8],
    /* shutter      */ [0xCC8430, 0xD88A2A, 0xC97A22, 0x7A5A4A, 0x30305E],
    /* shutter dark */ [0xA8691F, 0xB56E1C, 0x9E5D16, 0x5E4438, 0x20203F],
    /* door         */ [0xC0935C, 0xC99B5C, 0xB9853F, 0x6A5648, 0x2A2E60],
    /* door dark    */ [0x876437, 0x8E6836, 0x7E5A2A, 0x4E3E36, 0x1A1C40],
    /* leaf light   */ [0x86A870, 0x86A558, 0xCB8C2C, 0x3F5648, 0x1C2050],
    /* leaf         */ [0x678B58, 0x6B8C46, 0x5E7631, 0x2F4640, 0x141740],
    /* leaf dark    */ [0x4C6D46, 0x4F6E35, 0x445A27, 0x233635, 0x0D0F2E],
    /* trunk        */ [0x9C6030, 0xA8652B, 0x96581F, 0x4E3A34, 0x141740],
    /* trunk dark   */ [0x744722, 0x7E4B20, 0x6C3F18, 0x3A2A2A, 0x0E1030],
    /* fence        */ [0xCC8430, 0xD88A2A, 0xD88A2A, 0x6E5446, 0x2E3268],
    /* fence dark   */ [0xA0641F, 0xA9681C, 0xA9681C, 0x54403A, 0x22265A],
    /* flower a     */ [0xF2C24F, 0xF2C24F, 0xF2C24F, 0x8A7A5A, 0x4A4F8A],
    /* flower b     */ [0xFFFFFF, 0xFFFFFF, 0xFBE9C9, 0x9A9AB8, 0x7A80B8],
    /* flower c     */ [0xE7774A, 0xE7774A, 0xE7774A, 0x8A5A52, 0x4A3F78],
    /* cat          */ [0xE98B2E, 0xE98B2E, 0xE07E24, 0x6E4A3A, 0x151840],
    /* cat dark     */ [0xB8621C, 0xB8621C, 0xA8561A, 0x4E342C, 0x0E1030],
    /* cat light    */ [0xFFF3DF, 0xFFF3DF, 0xFBE3C0, 0x9A8A8A, 0x23264F],
    /* post         */ [0x474F84, 0x4A5488, 0x414A7C, 0x2F3462, 0x1B1E44],
    /* chimney      */ [0x876558, 0x8E6A5A, 0x7E5A4A, 0x4A3E4A, 0x20203F],
    /* smoke        */ [0xFFF2E8, 0xF2F4F8, 0xFBE9C9, 0x6A6690, 0x2D3162],
    /* orb          */ [0xFFF0C8, 0xFFF6D8, 0xFFD27A, 0xFFF6DC, 0xFFF6DC],
    /* orb shade    */ [0xFBD9A0, 0xFBE7B0, 0xF6B955, 0xD9D2B8, 0xD9D2B8],
    /* halo         */ [0xFDE3B4, 0xF7EDD0, 0xF8C46A, 0x5A4F86, 0x2A2F66],
    /* star         */ [0xFFFFFF, 0xFFFFFF, 0xFFFFFF, 0xFFFFFF, 0xFFFFFF],
    /* star dim     */ [0xC8CEFF, 0xC8CEFF, 0xC8CEFF, 0xA9A4D6, 0x8E96D8],
    /* glow         */ [0xE9AE3A, 0xE9AE3A, 0xE9AE3A, 0xE9AE3A, 0xE0A52E],
    /* glow deep    */ [0xC88A1E, 0xC88A1E, 0xC88A1E, 0xC88A1E, 0xB87C1C],
];

/// The window and its glint while nobody is up (midnight to five).
const WINDOW_DARK: [u32; 2] = [0x101233, 0x1E2250];

/// The sky of each key, top to horizon: (how far down in percent, colour).
/// Between two stops the rows dither from one flat colour into the next;
/// two stops of one colour are a flat band.
#[rustfmt::skip]
const SKY: [[(u8, u32); 6]; 5] = [
    /* dawn  */ [(0, 0x7C8FC8), (26, 0xA9A4D6), (50, 0xEDB6AE), (74, 0xFBD7A6), (100, 0xFDE8C4), (100, 0xFDE8C4)],
    /* day   */ [(0, 0x8FAEE2), (14, 0x8FAEE2), (40, 0xBCD0F0), (58, 0xDCE7F6), (82, 0xF6EFDD), (100, 0xF6EFDD)],
    /* gold  */ [(0, 0x546E9E), (24, 0x6B84B2), (40, 0xEFA238), (62, 0xF7B852), (82, 0x8D96B2), (100, 0x6E86B4)],
    /* dusk  */ [(0, 0x242A60), (28, 0x3A3A78), (54, 0x7A4E7E), (76, 0xD9814C), (100, 0xF0A85A), (100, 0xF0A85A)],
    /* night */ [(0, 0x0A0B2A), (35, 0x0E1033), (70, 0x191C4A), (100, 0x252A60), (100, 0x252A60), (100, 0x252A60)],
];

// ---------------------------------------------------------------- dither

/// The 4 x 4 ordered-dither threshold at a pixel, 0..1 (the sky's bands).
fn bayer4(x: i32, y: i32) -> f32 {
    (cell4(x, y) as f32 + 0.5) / 16.0
}

fn cell4(x: i32, y: i32) -> u32 {
    const M: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
    M[((y & 3) * 4 + (x & 3)) as usize] as u32
}

/// The 8 x 8 threshold, 0..1: 64 steps, so a slow change moves a few pixels
/// at a time.
pub(crate) fn bayer8(x: i32, y: i32) -> f32 {
    const Q: [u32; 4] = [0, 2, 3, 1];
    let q = Q[((((y >> 2) & 1) << 1) | ((x >> 2) & 1)) as usize];
    ((cell4(x, y) * 4 + q) as f32 + 0.5) / 64.0
}

fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// A steady pseudo-random number in 0..1 for a pixel (or anything numbered).
fn rnd(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) >> 8) as f32 / 16_777_216.0
}

// ------------------------------------------------------------------ sides

/// One side of the day at one minute: two keys and how far the picture has
/// dithered from the first into the second.
#[derive(Clone, Copy)]
struct Side {
    a: usize,
    b: usize,
    f: f32,
    /// Someone is up: the window is lit (night side) and smoke rises.
    lit: bool,
}

impl Side {
    fn key(&self, x: i32, y: i32) -> usize {
        // Offset from the day-night dissolve's pattern, so the two never line up.
        if bayer8(x + 3, y + 5) < self.f {
            self.b
        } else {
            self.a
        }
    }

    fn color(&self, m: usize, x: i32, y: i32) -> Rgb {
        let k = self.key(x, y);
        if k == NIGHT && !self.lit && (m == mat::WINDOW || m == mat::WINDOW_GLINT) {
            return rgb(WINDOW_DARK[m - mat::WINDOW]);
        }
        rgb(PAL[m][k])
    }

    fn sky(&self, x: i32, y: i32) -> Rgb {
        let stops = &SKY[self.key(x, y)];
        let v = (y.clamp(0, HORIZON - 1) * 100) as f32 / (HORIZON - 1) as f32;
        let mut i = 0;
        while i + 2 < stops.len() && v >= stops[i + 1].0 as f32 {
            i += 1;
        }
        let (p0, c0) = stops[i];
        let (p1, c1) = stops[i + 1];
        let f = if p1 > p0 { (v - p0 as f32) / (p1 - p0) as f32 } else { 0.0 };
        rgb(if bayer4(x, y) < f { c1 } else { c0 })
    }
}

/// Where the minute is between sunrise and sunset, 0..1 (outside the day
/// it stays at its ends).
fn day_part(m: &Moment) -> f32 {
    ((m.minute - m.sunrise) / (m.sunset - m.sunrise).max(1.0)).clamp(0.0, 1.0)
}

/// Where the minute is between sunset and the next sunrise, 0..1. By day
/// the night side shows only while the ball rolls: within an hour of
/// sunrise the moon has just set, within an hour of sunset it is about to
/// rise, so each roll carries on from the night it joins; in between (the
/// theme picked in the middle of the day) it stands in the middle of its way.
fn night_part(m: &Moment) -> f32 {
    if !m.night() {
        return if m.minute - m.sunrise < 60.0 {
            1.0
        } else if m.sunset - m.minute < 60.0 {
            0.0
        } else {
            0.5
        };
    }
    let len = (1440.0 - (m.sunset - m.sunrise)).max(1.0);
    let since = if m.minute >= m.sunset { m.minute - m.sunset } else { m.minute + 1440.0 - m.sunset };
    (since / len).clamp(0.0, 1.0)
}

/// The day side at this minute: dawn turns into day over the first 45
/// minutes, day into the golden hour over the 75 before sunset.
fn day_side(m: &Moment) -> Side {
    let (since, left) = (m.minute - m.sunrise, m.sunset - m.minute);
    // Smoke while breakfast and supper cook.
    let lit = since < 90.0 || left < 90.0;
    if since < 45.0 {
        Side { a: DAWN, b: DAY, f: (since / 45.0).clamp(0.0, 1.0), lit }
    } else {
        Side { a: DAY, b: GOLD, f: ((75.0 - left) / 60.0).clamp(0.0, 1.0), lit }
    }
}

/// How deep into the night the minute is: 0 at sunset and at sunrise, 1
/// from 35 minutes after the one to 40 before the other. By day it is the
/// same the other way round (as deep as the day is far from its ends), so
/// the night the ball rolls away from at sunrise is the one that was
/// showing, and the theme picked at noon rolls in from the deep night.
fn night_depth(m: &Moment) -> f32 {
    let (since, left) = if !m.night() {
        (m.sunset - m.minute, m.minute - m.sunrise)
    } else if m.minute >= m.sunset {
        (m.minute - m.sunset, m.sunrise + 1440.0 - m.minute)
    } else {
        (m.minute + 1440.0 - m.sunset, m.sunrise - m.minute)
    };
    (since / 35.0).min(left / 40.0).clamp(0.0, 1.0)
}

/// The night side: dusk fades into night, and night back into its glow
/// before sunrise. The house keeps hours: lights out from midnight to five.
fn night_side(m: &Moment) -> Side {
    let asleep = m.night() && m.minute < 300.0;
    Side { a: DUSK, b: NIGHT, f: night_depth(m), lit: !asleep }
}

// -------------------------------------------------------------------- art

/// The picture on the art grid.
pub(crate) struct Art {
    pub w: i32,
    pub h: i32,
    pub px: Vec<Rgb>,
}

impl Art {
    fn new(w: i32, h: i32) -> Art {
        Art { w, h, px: vec![[0, 0, 0]; (w.max(1) * h.max(1)) as usize] }
    }

    fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }

    /// The pixel at (x, y); past an edge, the edge's.
    pub(crate) fn get(&self, x: i32, y: i32) -> Rgb {
        self.px[(y.clamp(0, self.h - 1) * self.w + x.clamp(0, self.w - 1)) as usize]
    }
}

/// A drawing in letters, row after row, `w` to a row. Each letter is a
/// material ([`glyph`]); a dot leaves the pixel alone.
struct Sprite {
    w: i32,
    rows: &'static str,
}

fn glyph(c: u8) -> Option<usize> {
    Some(match c {
        b'R' => mat::ROOF,
        b'r' => mat::ROOF_LIGHT,
        b'D' => mat::ROOF_DARK,
        b'W' => mat::WALL,
        b'L' => mat::WALL_LINE,
        b'G' => mat::WINDOW,
        b'g' => mat::WINDOW_GLINT,
        b'S' => mat::SHUTTER,
        b's' => mat::SHUTTER_DARK,
        b'O' => mat::DOOR,
        b'o' => mat::DOOR_DARK,
        b'C' => mat::CHIMNEY,
        b'P' => mat::FENCE,
        b'p' => mat::FENCE_DARK,
        b'M' => mat::POST,
        b'c' => mat::CAT,
        b'k' => mat::CAT_DARK,
        b'w' => mat::CAT_LIGHT,
        b'B' => mat::CLOUD,
        b'b' => mat::CLOUD_SHADE,
        _ => return None,
    })
}

/// The house: a slate roof with a scalloped eave and a chimney, clapboard
/// walls, a window with shutters and a flower box, and the door.
const HOUSE: Sprite = Sprite {
    w: 18,
    rows: "\
............CC....\
............CC....\
....RRRRRRRRCCR...\
...RRRrrRRRRRRRR..\
..RRRRRRRRrrRRRRR.\
.RRrrRRRRRRRRRrrRR\
RRRRRRRrrRRRRRRRRR\
RRRRRRRRRRRRRRRRRR\
.RWRWRWRWRWRWRWR..\
..LLLLLLLLLLLLLL..\
..SGGGGGSWWoooWW..\
..SGGgGGSWWoOoWW..\
..SGgGGGSLLoOoLL..\
..SGGGGGSWWoODWW..\
..SGGGGGSWWoOoWW..\
..sssssssLLoOoLL..\
..WWWWWWWWWoOoWW..\
..WWWWWWWWWoOoWW..",
};
/// Where things are in the house, from its top-left pixel.
const WINDOW_AT: (i32, i32) = (3, 10);
const CHIMNEY_AT: (i32, i32) = (12, 0);
const DOOR_AT: i32 = 12;

/// The tree's trunk stands this far in from the right end's zone, and its
/// crown is this wide on each side of it.
const TREE_X: i32 = 12;
const CROWN: i32 = 10;

/// Seven pickets on two rails.
const FENCE: Sprite = Sprite {
    w: 13,
    rows: "\
P.P.P.P.P.P.P\
PpPpPpPpPpPpP\
P.P.P.P.P.P.P\
PpPpPpPpPpPpP\
P.P.P.P.P.P.P",
};

const POSTBOX: Sprite = Sprite {
    w: 3,
    rows: "\
MMM\
MMM\
.M.\
.M.\
.M.",
};

const CLOUD_BIG: Sprite = Sprite {
    w: 11,
    rows: "\
...BBBB....\
.BBBBBBBB..\
BBBBBBBBBBB\
.bbbbbbbbb.",
};

const CLOUD_SMALL: Sprite = Sprite {
    w: 7,
    rows: "\
..BBB..\
BBBBBBB\
.bbbbb.",
};

/// The cat, looking left: sitting, curled up asleep, and walking.
const CAT_SITS: Sprite = Sprite {
    w: 6,
    rows: "\
c.c...\
ccc...\
ckc...\
cccc..\
cwcck.\
cwccck",
};
const CAT_NAPS: Sprite = Sprite {
    w: 7,
    rows: "\
.c.ccc.\
ccccccc\
.cccckk",
};
const CAT_WALKS: [Sprite; 2] = [
    Sprite {
        w: 8,
        rows: "\
c.c....k\
ccc...k.\
ckccccc.\
.cccccc.\
.c.c.c.c",
    },
    Sprite {
        w: 8,
        rows: "\
c.c.....\
ccc...kk\
ckccccc.\
.cccccc.\
..c.cc..",
    },
];

fn blit(art: &mut Art, side: &Side, s: &Sprite, x0: i32, y0: i32, flip: bool) {
    for (i, c) in s.rows.bytes().enumerate() {
        let Some(m) = glyph(c) else { continue };
        let (sx, sy) = (i as i32 % s.w, i as i32 / s.w);
        let x = x0 + if flip { s.w - 1 - sx } else { sx };
        let y = y0 + sy;
        let c = side.color(m, x, y);
        art.set(x, y, c);
    }
}

fn height(s: &Sprite) -> i32 {
    s.rows.len() as i32 / s.w
}

// ------------------------------------------------------------- the scene

/// What is where on a grid `w` wide.
struct Layout {
    w: i32,
    house_x: i32,
    /// The tree's trunk.
    tree_x: i32,
    /// The middle: from this column up to (not including) `mid1`.
    mid0: i32,
    mid1: i32,
}

impl Layout {
    fn new(w: i32) -> Layout {
        Layout { w, house_x: 10, tree_x: w - TREE_ZONE + TREE_X, mid0: HOUSE_ZONE, mid1: w - TREE_ZONE }
    }

    /// Where the cat sits by the door, and under the tree (its left column).
    fn door_spot(&self) -> i32 {
        self.house_x + DOOR_AT + 5
    }

    fn tree_spot(&self) -> i32 {
        self.tree_x - 3
    }

    /// How many runs of fence the garden has: one on a short pill, up to
    /// three on a long one (a quarter of the middle).
    fn fence_runs(&self) -> i32 {
        ((self.mid1 - self.mid0) / 4 / (FENCE.w - 1)).clamp(1, 3)
    }

    /// Where the lamp post stands, four fifths of the way along the middle,
    /// clear of the widest clock (12-hour with a badge) and of the tree's
    /// crown; none on a pill too short for it.
    fn lamp_x(&self) -> Option<i32> {
        let mid = self.mid1 - self.mid0;
        (mid >= 100).then_some(self.mid0 + mid * 4 / 5)
    }
}

fn paint_side(m: &Moment, lay: &Layout, h: i32, night: bool) -> Art {
    let side = if night { night_side(m) } else { day_side(m) };
    let mut art = Art::new(lay.w, h);
    let w = lay.w;

    for y in 0..HORIZON.min(h) {
        for x in 0..w {
            art.set(x, y, side.sky(x, y));
        }
    }
    if night {
        stars(&mut art, &side, m, lay);
    }
    clouds(&mut art, &side, m, lay);
    if night {
        moon(&mut art, &side, m, lay);
    } else {
        sun(&mut art, &side, m, lay);
    }
    hills(&mut art, &side, lay);
    ground(&mut art, &side, lay, h);
    bush(&mut art, &side, 4, HORIZON + 1, 4);
    bush(&mut art, &side, w - 6, HORIZON + 1, 4);

    let (hx, hy) = (lay.house_x, HORIZON - height(&HOUSE) + 1);
    blit(&mut art, &side, &HOUSE, hx, hy, false);
    if side.lit {
        smoke(&mut art, &side, m, hx + CHIMNEY_AT.0, hy + CHIMNEY_AT.1);
    }
    if lay.mid1 - lay.mid0 >= MID_BARE {
        blit(&mut art, &side, &POSTBOX, lay.mid0 + 1, HORIZON - 4, false);
        // The garden fence: a run of pickets, longer where the pill is long.
        let runs = lay.fence_runs();
        for k in 0..runs {
            blit(&mut art, &side, &FENCE, lay.mid0 + 5 + k * (FENCE.w - 1), HORIZON - 4, false);
        }
        if runs > 1 {
            bush(&mut art, &side, lay.mid0 + 5 + runs * (FENCE.w - 1) + 3, HORIZON + 1, 3);
        }
    }
    let lamp = lay.lamp_x();
    if let Some(x) = lamp {
        bush(&mut art, &side, lay.mid0 + (lay.mid1 - lay.mid0) * 11 / 20, HORIZON + 1, 2);
        lamp_post(&mut art, &side, x, night);
        if night {
            lamp_pool(&mut art, &side, x);
        }
    }
    tree(&mut art, &side, lay.tree_x, HORIZON + 1);
    cat(&mut art, &side, m, lay, night, (hx + WINDOW_AT.0, hy + WINDOW_AT.1));
    if night && side.lit {
        window_light(&mut art, &side, hx + WINDOW_AT.0 + 2, hy + WINDOW_AT.1 + 2, lay);
    }
    if let Some(x) = lamp.filter(|_| night) {
        lamp_halo(&mut art, &side, x);
    }
    art
}

/// The lamp post's lantern row: under the clock's, clear of the sun's and
/// the moon's way along the top.
const LAMP_TOP: i32 = HORIZON - 11;

/// The lamp post in the garden: a slim post on a foot, and a lantern on
/// top, its glass dark by day and lit from dusk through the night (it isn't
/// the house's, so it stays on while the house sleeps).
fn lamp_post(art: &mut Art, side: &Side, x: i32, night: bool) {
    for y in LAMP_TOP + 3..=HORIZON {
        art.set(x, y, side.color(mat::POST, x, y));
    }
    for dx in [-1, 1] {
        art.set(x + dx, HORIZON, side.color(mat::POST, x + dx, HORIZON));
    }
    let glass = if night { mat::ORB } else { mat::WINDOW };
    for dx in -1..=1 {
        art.set(x + dx, LAMP_TOP, side.color(mat::POST, x + dx, LAMP_TOP));
        art.set(x + dx, LAMP_TOP + 1, side.color(glass, x + dx, LAMP_TOP + 1));
        art.set(x + dx, LAMP_TOP + 2, side.color(if dx == 0 { glass } else { mat::POST }, x + dx, LAMP_TOP + 2));
    }
    art.set(x, LAMP_TOP - 1, side.color(mat::POST, x, LAMP_TOP - 1));
}

/// The lamp's light on the grass at night, under where the cat walks.
fn lamp_pool(art: &mut Art, side: &Side, x: i32) {
    let grain = |x: i32, y: i32| 0.55 * bayer4(x, y) + 0.45 * rnd(x, y, 61);
    let (px, py) = (x as f32, (HORIZON + 3) as f32);
    for y in HORIZON + 1..art.h {
        for gx in x - 10..=x + 10 {
            let (ex, ey) = ((gx as f32 - px) / 10.0, (y as f32 - py) / 3.2);
            let d = (ex * ex + ey * ey).sqrt();
            if grain(gx, y) < 0.7 * (1.0 - d) {
                art.set(gx, y, side.color(if d < 0.4 && rnd(gx, y, 62) < 0.5 { mat::GLOW } else { mat::GLOW_DEEP }, gx, y));
            }
        }
    }
}

/// The lantern's glow in the dark around it.
fn lamp_halo(art: &mut Art, side: &Side, x: i32) {
    let grain = |x: i32, y: i32| 0.55 * bayer4(x, y) + 0.45 * rnd(x, y, 63);
    let cy = LAMP_TOP + 1;
    for dy in -3..=4 {
        for dx in -4..=4 {
            let (gx, gy) = (x + dx, cy + dy);
            let lantern = dx.abs() <= 1 && (-1..=1).contains(&dy);
            if lantern || (dx == 0 && dy > 1) {
                continue;
            }
            let d = ((dx * dx) as f32 + (dy * dy) as f32 * 1.3).sqrt();
            if grain(gx, gy) < 0.8 * (1.0 - d / 4.4) {
                art.set(gx, gy, side.color(if d < 2.4 { mat::GLOW } else { mat::GLOW_DEEP }, gx, gy));
            }
        }
    }
}

/// The sun on its arc from the left horizon to the right one: a disc with a
/// dithered halo, larger and lower as the day ends.
fn sun(art: &mut Art, side: &Side, m: &Moment, lay: &Layout) {
    let u = day_part(m);
    let (cx, cy) = orbit(u, lay.w);
    orb(art, side, cx, cy, 2.4, 5.2, None);
}

/// The moon on the same arc through the night, in tonight's phase.
fn moon(art: &mut Art, side: &Side, m: &Moment, lay: &Layout) {
    let (cx, cy) = orbit(night_part(m), lay.w);
    orb(art, side, cx, cy, 3.4, 6.0, Some(moon_phase(m.day)));
}

/// The row the sun and the moon cross the sky on: above the clock.
const ORBIT_TOP: f32 = 6.0;

/// Where the sun or the moon is, `u` of the way across the sky.
fn orbit(u: f32, w: i32) -> (f32, f32) {
    // Up over the first sixth of the way, along the top above the clock,
    // down over the last sixth.
    let low = (HORIZON - 4) as f32;
    let ramp = |v: f32| {
        let t = (v / 0.17).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let x = 6.0 + u * (w as f32 - 12.0);
    let y = low - ramp(u) * ramp(1.0 - u) * (low - ORBIT_TOP);
    (x.round(), y.round())
}

/// The moon's age as a share of its month: 0 new, 0.5 full. `day` counts
/// from 1 January 2000; the new moon of 6 January 2000 is day 5.76.
fn moon_phase(day: i32) -> f32 {
    ((day as f32 - 5.76) / 29.530_59).rem_euclid(1.0)
}

/// A disc of radius `r` at (cx, cy) with a halo out to `halo`, sparser the
/// further out. `phase` is the moon's: its dark part shows faintly, and the
/// halo is as strong as the part that is lit.
fn orb(art: &mut Art, side: &Side, cx: f32, cy: f32, r: f32, halo: f32, phase: Option<f32>) {
    let reach = halo.ceil() as i32 + 1;
    let glow = phase.map_or(1.0, |p| (1.0 - (2.0 * std::f32::consts::PI * p).cos()) / 2.0);
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (x, y) = (cx as i32 + dx, cy as i32 + dy);
            if y >= HORIZON {
                continue;
            }
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d <= r {
                if let Some(p) = phase {
                    // The terminator: an ellipse across the disc, the lit
                    // limb on the right while it waxes.
                    let k = (2.0 * std::f32::consts::PI * p).cos();
                    let edge = k * (r * r - (dy * dy) as f32).max(0.0).sqrt();
                    let lit = if p < 0.5 { dx as f32 >= edge } else { (dx as f32) <= -edge };
                    if !lit {
                        art.set(x, y, side.color(mat::HALO, x, y));
                        continue;
                    }
                }
                let shade = phase.is_some() && rnd(dx, dy, 7) < 0.22 && d > 0.5;
                art.set(x, y, side.color(if shade { mat::ORB_SHADE } else { mat::ORB }, x, y));
            } else if d <= halo && 0.6 * bayer4(x, y) + 0.4 * rnd(x, y, 8) < 0.62 * glow * (1.0 - (d - r) / (halo - r)) {
                art.set(x, y, side.color(mat::HALO, x, y));
            }
        }
    }
}

/// The stars come out one by one as the night deepens, never where the
/// clock is written; a few change their light each minute.
fn stars(art: &mut Art, side: &Side, m: &Moment, lay: &Layout) {
    let depth = night_depth(m);
    let minute = m.minute as i32;
    let n = lay.w / 4;
    for i in 0..n {
        if rnd(i, 1, 11) * 0.8 + 0.1 > depth {
            continue;
        }
        let x = 2 + (rnd(i, 2, 11) * (lay.w - 4) as f32) as i32;
        let y = 3 + (rnd(i, 3, 11) * (HORIZON - 10) as f32) as i32;
        if m.clock.is_some() && x >= lay.mid0 + 2 && x < lay.mid1 - 2 && (CLOCK_Y - 3..CLOCK_Y + DIGIT_H + 3).contains(&y) {
            continue;
        }
        let dim = rnd(i, 4, 11) < 0.4 || hash(i, minute, 5) & 7 == 0;
        let c = side.color(if dim { mat::STAR_DIM } else { mat::STAR }, x, y);
        art.set(x, y, c);
        // Every seventh is a little cross.
        if i % 7 == 3 && !dim {
            let arm = side.color(mat::STAR_DIM, x, y);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                art.set(x + dx, y + dy, arm);
            }
        }
    }
}

/// Two clouds drifting right, the small one slower: a pixel every few
/// minutes, each day from a different start.
fn clouds(art: &mut Art, side: &Side, m: &Moment, lay: &Layout) {
    let t = m.minute + (m.day.rem_euclid(97) * 37) as f32;
    let lanes: [(&Sprite, f32, i32, i32); 2] = [(&CLOUD_BIG, 4.0, 4, 30), (&CLOUD_SMALL, 7.0, 8, 0)];
    for (s, pace, y, lead) in lanes {
        let span = lay.w + s.w + 14;
        let x = ((t / pace) as i32 + lead).rem_euclid(span) - s.w;
        blit(art, side, s, x, y, false);
    }
}

/// Two rows of far hills along the horizon.
fn hills(art: &mut Art, side: &Side, lay: &Layout) {
    for x in 0..lay.w {
        let fx = x as f32;
        let far = HORIZON - 4 - (1.6 * (fx * 0.11 + 1.0).sin() + 1.1 * (fx * 0.27).sin()).round() as i32;
        let near = HORIZON - 2 - (1.2 * (fx * 0.19 + 2.2).sin()).round() as i32;
        for y in far..HORIZON {
            let m = if y >= near { mat::HILL_NEAR } else { mat::HILL_FAR };
            art.set(x, y, side.color(m, x, y));
        }
    }
}

/// The grass, lighter along its top and in tufts, darker in patches; the
/// path from the door; a flower here and there.
fn ground(art: &mut Art, side: &Side, lay: &Layout, h: i32) {
    let door = lay.house_x + DOOR_AT;
    for y in HORIZON..h {
        let row = y - HORIZON;
        for x in 0..lay.w {
            // Lighter along its top and in tufts, darker toward the front.
            let n = rnd(x.div_euclid(2), y, 21) + 0.035 * row as f32;
            let mut m = if (row == 0 && rnd(x, 0, 22) < 0.75) || n < 0.2 {
                mat::GRASS_LIGHT
            } else if n > 0.86 {
                mat::GRASS_DARK
            } else {
                mat::GRASS
            };
            // The path runs from the doorstep down to the front, widening.
            let lead = door - 1 + row * 2;
            if row >= 1 && x >= lead && x < lead + 4 + row {
                m = if rnd(x, y, 23) < 0.25 { mat::PATH_DARK } else { mat::PATH };
            } else if row >= 2 && rnd(x, y, 24) < 0.035 {
                m = [mat::FLOWER_A, mat::FLOWER_B, mat::FLOWER_C][(hash(x, y, 25) % 3) as usize];
            }
            art.set(x, y, side.color(m, x, y));
        }
    }
}

/// The tree, its trunk on column `cx` and its roots on row `base`: a crown
/// of overlapping rounds, lit from the upper left in patches, over a short
/// trunk that forks into it.
fn tree(art: &mut Art, side: &Side, cx: i32, base: i32) {
    for y in base - 10..=base {
        let foot = (y - (base - 2)).max(0);
        for dx in -1 - foot..=1 + foot {
            let m = if dx > 0 || (foot > 0 && dx == -1 - foot) { mat::TRUNK_DARK } else { mat::TRUNK };
            art.set(cx + dx, y, side.color(m, cx + dx, y));
        }
    }
    // The crown's clumps, back to front: (across, down, radius) from its
    // middle. Each is lit on its own upper left and dark on its lower
    // right, so the crown reads as heaped leaves, not one ball.
    #[rustfmt::skip]
    const CLUMPS: [(f32, f32, f32); 8] = [
        (0.0, -5.2, 4.2), (-4.6, -2.2, 4.3), (4.6, -2.6, 4.5), (-7.2, 1.8, 3.3),
        (7.3, 1.6, 3.5), (0.0, 0.5, 4.6), (-3.4, 3.6, 3.7), (3.4, 3.9, 3.8),
    ];
    let cy = base - 15;
    for dy in -CROWN..=CROWN {
        for dx in -CROWN - 1..=CROWN + 1 {
            let (x, y) = (cx + dx, cy + dy);
            let mut leaf = None;
            for &(ox, oy, r) in &CLUMPS {
                let (ux, uy) = ((dx as f32 - ox) / r, (dy as f32 - oy) / r);
                let d = (ux * ux + uy * uy).sqrt();
                // A ragged edge: a clump ends a little sooner here and there.
                if d > 1.0 - 0.16 * rnd(x, y, 43) {
                    continue;
                }
                // Its own upper left, the crown's upper left, and leaves in
                // little bunches that catch the light or don't.
                let bunch = rnd((x + y.div_euclid(2)).div_euclid(2), y.div_euclid(2), 41) - 0.5;
                let light = 0.45 * (-0.5 * ux - 0.8 * uy) + 0.4 * (-0.45 * dx as f32 - 0.9 * dy as f32) / CROWN as f32 + 0.75 * bunch + 0.3 * (rnd(x, y, 42) - 0.5);
                leaf = Some(if light > 0.3 {
                    mat::LEAF_LIGHT
                } else if light < -0.3 {
                    mat::LEAF_DARK
                } else {
                    mat::LEAF
                });
            }
            if let Some(m) = leaf {
                art.set(x, y, side.color(m, x, y));
            }
        }
    }
}

/// A round bush `r` across, sitting on the row `base`.
fn bush(art: &mut Art, side: &Side, cx: i32, base: i32, r: i32) {
    for dy in -r..=0 {
        for dx in -r..=r {
            if dx * dx + dy * dy * 2 > r * r {
                continue;
            }
            let (x, y) = (cx + dx, base + dy);
            let n = rnd(x, y, 31);
            let m = if dy < -r / 2 && n < 0.5 {
                mat::LEAF_LIGHT
            } else if n > 0.78 {
                mat::LEAF_DARK
            } else {
                mat::LEAF
            };
            art.set(x, y, side.color(m, x, y));
        }
    }
}

/// Three puffs over the chimney, moving with the minute.
fn smoke(art: &mut Art, side: &Side, m: &Moment, cx: i32, top: i32) {
    let step = m.minute as i32 & 1;
    for i in 0..3 {
        let (x, y) = (cx + i + step, top - 2 - 2 * i + step);
        if (x + y) & 1 == 0 || i == 0 {
            art.set(x, y, side.color(mat::SMOKE, x, y));
        }
        if i > 0 {
            art.set(x + 1, y, side.color(mat::SMOKE, x + 1, y));
        }
    }
}

/// The cat keeps the day's hours: out at dawn, under the tree by day and
/// asleep there around noon, home for the golden hour, in the lit window
/// after dark, and nowhere to be seen from midnight to five.
fn cat(art: &mut Art, side: &Side, m: &Moment, lay: &Layout, night: bool, window: (i32, i32)) {
    if night {
        if side.lit {
            // Two ears and a back against the light.
            let ink = rgb(0x2A1D3A);
            for (dx, dy) in [(1, 2), (3, 2), (1, 3), (2, 3), (3, 3), (1, 4), (2, 4), (3, 4)] {
                art.set(window.0 + dx, window.1 + dy, ink);
            }
        }
        return;
    }
    let (since, left) = (m.minute - m.sunrise, m.sunset - m.minute);
    let noon = (m.sunrise + m.sunset) / 2.0;
    let (door, tree) = (lay.door_spot() as f32, lay.tree_spot() as f32);
    let base = HORIZON + 1;
    let step = (m.minute as i32 & 1) as usize;
    if since < 45.0 {
        blit(art, side, &CAT_SITS, door as i32, base - 5, true);
    } else if since < 75.0 {
        let x = door + (tree - 6.0 - door) * (since - 45.0) / 30.0;
        blit(art, side, &CAT_WALKS[step], x as i32, base - 4, true);
    } else if left < 30.0 {
        blit(art, side, &CAT_SITS, door as i32, base - 5, false);
    } else if left < 60.0 {
        let x = tree - 6.0 + (door - (tree - 6.0)) * (60.0 - left) / 30.0;
        blit(art, side, &CAT_WALKS[step], x as i32, base - 4, false);
    } else if (m.minute - noon).abs() < 75.0 {
        blit(art, side, &CAT_NAPS, tree as i32 - 7, base - 2, false);
    } else {
        blit(art, side, &CAT_SITS, tree as i32 - 6, base - 5, false);
    }
}

/// The lit window's light on what is around it: amber on the wall beside
/// it, a lamp by the door, and a pool on the grass in front of the house,
/// each thinning out in a dither that is part pattern, part chance.
fn window_light(art: &mut Art, side: &Side, cx: i32, cy: i32, lay: &Layout) {
    let strength = 0.6 + 0.4 * side.f;
    let grain = |x: i32, y: i32| 0.55 * bayer4(x, y) + 0.45 * rnd(x, y, 51);
    for dy in -4..=5 {
        for dx in -5..=7 {
            let (x, y) = (cx + dx, cy + dy);
            // The glass itself is already lit.
            if (-2..=2).contains(&dx) && (-2..=2).contains(&dy) {
                continue;
            }
            let d = ((dx * dx) as f32 + (dy * dy) as f32 * 1.4).sqrt();
            let on_wall = x >= lay.house_x + 2 && x < lay.house_x + 16 && y < HORIZON;
            if on_wall && grain(x, y) < strength * (1.0 - d / 5.0) {
                art.set(x, y, side.color(if d < 3.2 { mat::GLOW } else { mat::GLOW_DEEP }, x, y));
            }
        }
    }
    // The lamp beside the door.
    let (lx, ly) = (lay.house_x + 15, cy - 1);
    art.set(lx, ly, side.color(mat::ORB, lx, ly));
    art.set(lx, ly + 1, side.color(mat::GLOW, lx, ly + 1));
    let (px, py) = (cx as f32 + 5.0, (HORIZON + 3) as f32);
    for y in HORIZON + 1..art.h {
        for x in cx - 9..cx + 22 {
            let (ex, ey) = ((x as f32 - px) / 13.0, (y as f32 - py) / 3.6);
            let d = (ex * ex + ey * ey).sqrt();
            if grain(x, y) < 0.8 * strength * (1.0 - d) {
                art.set(x, y, side.color(if d < 0.35 && rnd(x, y, 52) < 0.5 { mat::GLOW } else { mat::GLOW_DEEP }, x, y));
            }
        }
    }
}

// -------------------------------------------------------------- the clock

/// The row the clock's digits start on.
const CLOCK_Y: i32 = 11;
/// A digit's width; all are [`DIGIT_H`] tall.
pub(crate) const DIGIT_W: i32 = 5;
pub(crate) const DIGIT_H: i32 = 8;

/// The sky's digits, 5 x 8, their stems two pixels wide: a row is five
/// bits, the leftmost pixel the highest.
#[rustfmt::skip]
const DIGITS: [[u16; 8]; 10] = [
    [0b01110, 0b11011, 0b11011, 0b11011, 0b11011, 0b11011, 0b11011, 0b01110],
    [0b00110, 0b01110, 0b00110, 0b00110, 0b00110, 0b00110, 0b00110, 0b01111],
    [0b01110, 0b11011, 0b00011, 0b00110, 0b01100, 0b11000, 0b11000, 0b11111],
    [0b11110, 0b00011, 0b00011, 0b01110, 0b00011, 0b00011, 0b00011, 0b11110],
    [0b00011, 0b00111, 0b01111, 0b11011, 0b11011, 0b11111, 0b00011, 0b00011],
    [0b11111, 0b11000, 0b11000, 0b11110, 0b00011, 0b00011, 0b11011, 0b01110],
    [0b01110, 0b11000, 0b11000, 0b11110, 0b11011, 0b11011, 0b11011, 0b01110],
    [0b11111, 0b00011, 0b00011, 0b00110, 0b00110, 0b01100, 0b01100, 0b01100],
    [0b01110, 0b11011, 0b11011, 0b01110, 0b11011, 0b11011, 0b11011, 0b01110],
    [0b01110, 0b11011, 0b11011, 0b11011, 0b01111, 0b00011, 0b00011, 0b01110],
];

/// A, P and M, five rows tall (M is five wide, the others three).
const LETTER_A: [u16; 5] = [0b010, 0b101, 0b111, 0b101, 0b101];
const LETTER_P: [u16; 5] = [0b110, 0b101, 0b110, 0b100, 0b100];
const LETTER_M: [u16; 5] = [0b10001, 0b11011, 0b10101, 0b10001, 0b10001];

/// Ink for what is written over the picture: its cells, to be outlined.
struct Ink {
    cells: Vec<(i32, i32, Rgb)>,
}

impl Ink {
    fn bits(&mut self, rows: &[u16], w: i32, x: i32, y: i32, c: Rgb) {
        for (r, bits) in rows.iter().enumerate() {
            for i in 0..w {
                if bits >> (w - 1 - i) & 1 == 1 {
                    self.cells.push((x + i, y + r as i32, c));
                }
            }
        }
    }

    /// Onto the picture, each cell with a one-pixel edge of `edge` around it.
    fn stamp(&self, art: &mut Art, edge: Rgb) {
        for &(x, y, _) in &self.cells {
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                art.set(x + dx, y + dy, edge);
            }
        }
        for &(x, y, c) in &self.cells {
            art.set(x, y, c);
        }
    }
}

const CLOCK_INK: Rgb = rgb(0xFFF8E8);
const CLOCK_SLATE: Rgb = rgb(0x2B3160);
const BOLT_INK: Rgb = rgb(0x5FD068);
const LOW_INK: Rgb = rgb(0xFF6A5E);

/// The hour as the clock writes it, and whether it is past noon.
fn hour_shown(c: &Clock) -> (u8, bool) {
    if c.h24 {
        (c.hour % 24, false)
    } else {
        ((c.hour + 11) % 12 + 1, c.hour % 24 >= 12)
    }
}

/// How wide the clock and its badge are written.
fn written_w(c: &Clock, badge: Option<Badge>) -> i32 {
    let (hour, _) = hour_shown(c);
    let digits = if c.h24 || hour >= 10 { 4 } else { 3 };
    digits * (DIGIT_W + 1) + 2 + if c.h24 { 0 } else { MERIDIEM_W } + if badge.is_some() { BADGE_W } else { 0 }
}

/// The time in the sky between the house and the tree, cream with a dark
/// edge so it reads on every sky; beside it a bolt while the battery
/// charges, or a red battery when it runs low.
fn write_clock(art: &mut Art, lay: &Layout, c: &Clock, badge: Option<Badge>, dark: f32) {
    let (hour, pm) = hour_shown(c);
    let w = written_w(c, badge);
    let mut x = (lay.mid0 + lay.mid1 - w) / 2;
    let y = CLOCK_Y;
    let mut ink = Ink { cells: Vec::new() };
    let digit = |ink: &mut Ink, x: &mut i32, d: u8| {
        ink.bits(&DIGITS[(d % 10) as usize], DIGIT_W, *x, y, CLOCK_INK);
        *x += DIGIT_W + 1;
    };
    if c.h24 || hour >= 10 {
        digit(&mut ink, &mut x, hour / 10);
    }
    digit(&mut ink, &mut x, hour % 10);
    ink.bits(&[0b11, 0b11], 2, x, y + 2, CLOCK_INK);
    ink.bits(&[0b11, 0b11], 2, x, y + 5, CLOCK_INK);
    x += 3;
    digit(&mut ink, &mut x, c.minute / 10);
    digit(&mut ink, &mut x, c.minute % 10);
    x -= 1;
    if !c.h24 {
        x += 3;
        ink.bits(if pm { &LETTER_P } else { &LETTER_A }, 3, x, y + DIGIT_H - 5, CLOCK_INK);
        ink.bits(&LETTER_M, 5, x + 4, y + DIGIT_H - 5, CLOCK_INK);
        x += 9;
    }
    match badge {
        Some(Badge::Charging) => {
            ink.bits(&[0b00010, 0b00110, 0b01100, 0b11111, 0b00110, 0b01100, 0b01000], 5, x + 4, y, BOLT_INK);
        }
        Some(Badge::Low(_)) => {
            ink.bits(&[0b111111110, 0b100000011, 0b100000011, 0b100000011, 0b111111110], 9, x + 3, y + 2, CLOCK_INK);
            ink.bits(&[0b11, 0b11, 0b11], 2, x + 4, y + 3, LOW_INK);
        }
        None => {}
    }
    // By day the roof's slate with a cream edge, which only shows where the
    // sky is dark or golden; at night cream on the dark.
    if dark > 0.5 {
        ink.stamp(art, rgb(0x0A0B2A));
    } else {
        for cell in &mut ink.cells {
            if cell.2 == CLOCK_INK {
                cell.2 = CLOCK_SLATE;
            }
        }
        ink.stamp(art, CLOCK_INK);
    }
}

// ------------------------------------------------------------------ paint

/// How far the picture has gone over to the night side for a ball `lens` of
/// the way across: nothing changes at the ends of its roll, all of it in
/// the middle.
pub(crate) fn night_share(lens: f32) -> f32 {
    let t = ((lens - 0.12) / 0.76).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far a moment's picture has gone over to the night side: as far as
/// the ball has rolled, or where a slide by hand says (`Moment::dark`).
pub(crate) fn darkness(m: &Moment) -> f32 {
    m.dark.map_or_else(|| night_share(m.lens), |d| d.clamp(0.0, 1.0))
}

/// The picture for a moment, `w` x `h` art pixels.
pub(crate) fn paint(m: &Moment, w: i32, h: i32) -> Art {
    let lay = Layout::new(w);
    let share = darkness(m);
    let mut art = if share <= 0.0 {
        paint_side(m, &lay, h, false)
    } else if share >= 1.0 {
        paint_side(m, &lay, h, true)
    } else {
        let mut day = paint_side(m, &lay, h, false);
        let night = paint_side(m, &lay, h, true);
        for y in 0..h {
            for x in 0..w {
                if bayer8(x, y) < share {
                    day.px[(y * w + x) as usize] = night.px[(y * w + x) as usize];
                }
            }
        }
        day
    };
    if let Some(c) = m.clock.as_ref().filter(|_| lay.mid1 - lay.mid0 >= CLOCK_W + 2) {
        write_clock(&mut art, &lay, c, m.badge, share);
    }
    art
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_cover_their_range_once() {
        let mut seen4 = [false; 16];
        let mut seen8 = [false; 64];
        for y in 0..8 {
            for x in 0..8 {
                seen4[(bayer4(x, y) * 16.0) as usize] = true;
                seen8[(bayer8(x, y) * 64.0) as usize] = true;
            }
        }
        assert!(seen4.iter().all(|s| *s) && seen8.iter().all(|s| *s));
        // The pattern repeats, also left of and above the origin.
        assert_eq!(bayer8(-3, -5), bayer8(5, 3));
    }

    #[test]
    fn sprites_are_whole_rows_of_known_letters() {
        for s in [&HOUSE, &FENCE, &POSTBOX, &CLOUD_BIG, &CLOUD_SMALL, &CAT_SITS, &CAT_NAPS, &CAT_WALKS[0], &CAT_WALKS[1]] {
            assert_eq!(s.rows.len() as i32 % s.w, 0, "{} wide", s.w);
            assert!(s.rows.bytes().all(|c| c == b'.' || glyph(c).is_some()));
        }
        assert_eq!(height(&HOUSE), 18);
    }

    #[test]
    fn the_twelve_hour_clock_writes_twelve_at_noon_and_midnight() {
        let at = |hour| hour_shown(&Clock { hour, minute: 0, h24: false });
        assert_eq!((at(0), at(1), at(11), at(12), at(13), at(23)), ((12, false), (1, false), (11, false), (12, true), (1, true), (11, true)));
        assert_eq!(hour_shown(&Clock { hour: 23, minute: 0, h24: true }), (23, false));
    }

    #[test]
    fn digits_differ_and_fill_their_box() {
        for (i, a) in DIGITS.iter().enumerate() {
            assert!(a.iter().all(|r| *r < 32), "{i} is five wide");
            assert!(a[0] != 0 && a[7] != 0, "{i} is eight tall");
            for (j, b) in DIGITS.iter().enumerate().skip(i + 1) {
                let apart: u32 = a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum();
                assert!(apart >= 3, "{i} and {j} differ by {apart} pixels");
            }
        }
    }

    fn at(hhmm: u32) -> Moment {
        let minute = (hhmm / 100 * 60 + hhmm % 100) as f32;
        let mut m = Moment { minute, sunrise: 360.0, sunset: 1080.0, day: 9817, lens: 0.0, dark: None, clock: Some(Clock { hour: (hhmm / 100) as u8, minute: (hhmm % 100) as u8, h24: true }), badge: None };
        m.lens = m.rest_lens();
        m
    }

    #[test]
    fn the_day_side_goes_from_dawn_to_day_to_gold() {
        let keys = |hhmm| {
            let s = day_side(&at(hhmm));
            (s.a, s.b, s.f)
        };
        assert_eq!(keys(600), (DAWN, DAY, 0.0));
        assert_eq!(keys(645), (DAY, GOLD, 0.0));
        assert_eq!(keys(1200), (DAY, GOLD, 0.0));
        // The light starts to turn 75 minutes before sunset and is golden
        // for the last quarter of an hour.
        assert_eq!(keys(1645), (DAY, GOLD, 0.0));
        assert!((keys(1715).2 - 0.5).abs() < 1e-5);
        assert_eq!(keys(1745), (DAY, GOLD, 1.0));
        assert_eq!(keys(1759), (DAY, GOLD, 1.0));
        // Past its ends it stays what it was there (the side that is not
        // showing while the ball rolls).
        assert_eq!(keys(2200), (DAY, GOLD, 1.0));
        assert_eq!(keys(300), (DAWN, DAY, 0.0));
    }

    #[test]
    fn the_night_deepens_after_sunset_and_thins_before_sunrise() {
        let depth = |hhmm| night_depth(&at(hhmm));
        assert_eq!(depth(1800), 0.0);
        assert!((depth(1817) - 17.0 / 35.0).abs() < 1e-5);
        assert_eq!((depth(1835), depth(2359), depth(0), depth(300)), (1.0, 1.0, 1.0, 1.0));
        assert_eq!(depth(520), 1.0);
        assert!((depth(540) - 0.5).abs() < 1e-5);
        assert!(depth(559) < 0.03);
        // By day the night side is the deep night, and at the day's two
        // ends the night it has just been or is about to be.
        assert_eq!((depth(1200), depth(640), depth(1725)), (1.0, 1.0, 1.0));
        assert_eq!(depth(600), 0.0);
        assert!((depth(620) - 0.5).abs() < 1e-5);
        assert!(depth(1759) < 0.03);
    }

    #[test]
    fn the_rolls_join_the_night_they_leave_and_enter() {
        // A minute before sunrise and at it, the night side is the same
        // glow with the moon gone down; a minute before sunset and at it,
        // the day side is the same gold with the sun gone down.
        let side = |hhmm, night: bool| paint_side(&at(hhmm), &Layout::new(100), 38, night).px;
        let differ = |a: &[Rgb], b: &[Rgb]| a.iter().zip(b).filter(|(x, y)| x != y).count();
        // A minute of the dissolve, the clouds a step on, a star or two.
        assert!(differ(&side(559, true), &side(600, true)) < 200, "{}", differ(&side(559, true), &side(600, true)));
        assert!(differ(&side(1759, false), &side(1800, false)) < 200, "{}", differ(&side(1759, false), &side(1800, false)));
        assert_eq!((night_part(&at(559)) > 0.99, night_part(&at(600)), night_part(&at(1759)), night_part(&at(1800)), night_part(&at(1200))), (true, 1.0, 0.0, 0.0, 0.5));
    }

    #[test]
    fn the_house_keeps_hours() {
        let lit = |hhmm| night_side(&at(hhmm)).lit;
        // Lit from dusk to midnight and again from five; dark in between.
        assert!(lit(1800) && lit(2130) && lit(2359));
        assert!(!lit(0) && !lit(230) && !lit(459));
        assert!(lit(500) && lit(559));
        // Smoke at breakfast and at supper.
        let smoke = |hhmm| day_side(&at(hhmm)).lit;
        assert!(smoke(600) && smoke(729) && !smoke(730) && !smoke(1200) && !smoke(1630) && smoke(1631) && smoke(1759));
    }

    #[test]
    fn the_sun_and_the_moon_cross_above_the_clock() {
        for w in [80, 100, 112, 124] {
            // They rise and set behind the hills, at the two ends.
            let (x0, y0) = orbit(0.0, w);
            let (x1, y1) = orbit(1.0, w);
            assert_eq!((x0, x1), (6.0, w as f32 - 6.0));
            assert!(y0 >= (HORIZON - 5) as f32 && y1 >= (HORIZON - 5) as f32);
            // Over the middle of their way they keep to the top row, the
            // moon's disc (3.4 across each way) clear of the clock's edge.
            for i in 17..=83 {
                let (_, y) = orbit(i as f32 / 100.0, w);
                assert!(y + 3.4 < (CLOCK_Y - 1) as f32, "{w}: at {i} % the row is {y}");
            }
        }
    }

    #[test]
    fn the_time_is_written_between_the_house_and_the_tree() {
        // The picture with the clock and without differ only in the middle.
        for hhmm in [1008, 2130] {
            let with = at(hhmm);
            let without = Moment { clock: None, ..with };
            let (a, b) = (paint(&with, 100, 38), paint(&without, 100, 38));
            let lay = Layout::new(100);
            let mut changed = 0;
            for y in 0..38 {
                for x in 0..100 {
                    if a.get(x, y) != b.get(x, y) {
                        changed += 1;
                        // The digits and their edge, and the stars that keep
                        // clear of them (a cross star reaches one pixel on).
                        assert!(x > lay.mid0 && x < lay.mid1 - 1, "{hhmm}: {x},{y}");
                        assert!((CLOCK_Y - 4..CLOCK_Y + DIGIT_H + 4).contains(&y), "{hhmm}: {x},{y}");
                    }
                }
            }
            assert!(changed > 100, "{hhmm}: {changed} pixels of clock");
        }
        // A pill too narrow for the clock leaves it out rather than cut it.
        assert_eq!(paint(&at(1008), 80, 38).px, paint(&Moment { clock: None, ..at(1008) }, 80, 38).px);
    }

    #[test]
    fn the_ball_takes_the_picture_from_day_to_night() {
        assert_eq!((night_share(0.0), night_share(0.12), night_share(0.5), night_share(0.88), night_share(1.0)), (0.0, 0.0, 0.5, 1.0, 1.0));
        // Halfway across, half the pixels are the night's.
        let m = at(1800);
        let (day, night) = (paint(&Moment { lens: 0.0, ..m }, 100, 38), paint(&Moment { lens: 1.0, ..m }, 100, 38));
        let mid = paint(&Moment { lens: 0.5, clock: None, ..m }, 100, 38);
        let (day, night) = (Art { px: paint(&Moment { lens: 0.0, clock: None, ..m }, 100, 38).px, ..day }, Art { px: paint(&Moment { lens: 1.0, clock: None, ..m }, 100, 38).px, ..night });
        let from_night = mid.px.iter().zip(&day.px).zip(&night.px).filter(|((p, d), n)| p != d && p == n).count();
        let from_day = mid.px.iter().zip(&day.px).zip(&night.px).filter(|((p, d), n)| p == d && p != n).count();
        assert!(mid.px.iter().zip(&day.px).zip(&night.px).all(|((p, d), n)| p == d || p == n));
        assert!((from_night as f32 / (from_night + from_day) as f32 - 0.5).abs() < 0.03, "{from_night} night, {from_day} day");
    }

    #[test]
    fn the_moon_is_new_and_full_on_the_days_it_was() {
        // 6 January 2000 was a new moon, 21 January a full one.
        assert!(moon_phase(6) < 0.03 || moon_phase(6) > 0.97);
        assert!((moon_phase(20) - 0.5).abs() < 0.04);
        // And 9 November 2026 (day 9809) a new one again.
        assert!(moon_phase(9809) < 0.05 || moon_phase(9809) > 0.95, "{}", moon_phase(9809));
    }
}
