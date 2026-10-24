//! HDR desktops for the Glass theme's screen copy.
//!
//! With HDR on, Windows composes the desktop in scRGB: linear light, the
//! sRGB primaries, half floats, 1.0 = 80 nits. Ordinary (SDR) windows sit at
//! the "SDR content brightness" Windows is set to, so their white is well
//! above 1.0. The glass samples 8-bit sRGB, so each copy is mapped down:
//! divided by that white, clipped (HDR highlights brighter than white turn
//! white), then gamma-encoded. SDR windows come out the way they look.

/// SDR white when Windows can't say (200 nits, in scRGB units).
pub const DEFAULT_WHITE: f32 = 2.5;

/// SDR white in scRGB units for Windows' `SDRWhiteLevel` (1000 = 80 nits),
/// kept between 80 and 600 nits.
pub fn white_from_level(level: u32) -> f32 {
    (level as f32 / 1000.0).clamp(1.0, 7.5)
}

/// An IEEE 754 half float as f32.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let man = (h & 0x3ff) as f32;
    let mag = match exp {
        0 => man * 2f32.powi(-24),
        31 if man == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    };
    sign * mag
}

/// sRGB encoding of a linear value in 0..1.
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Every half-float bit pattern mapped to its 8-bit sRGB value, for one SDR
/// white: three table lookups a pixel.
pub struct HalfToSrgb {
    white: f32,
    table: Vec<u8>,
}

impl HalfToSrgb {
    pub fn new(white: f32) -> Self {
        let white = if white.is_finite() && white > 0.0 { white } else { DEFAULT_WHITE };
        let table = (0..=u16::MAX)
            .map(|h| {
                let v = half_to_f32(h) / white;
                // Negative (out of gamut) and NaN are black, beyond white is white.
                let v = if v > 0.0 { v.min(1.0) } else { 0.0 };
                (srgb_encode(v) * 255.0).round() as u8
            })
            .collect();
        Self { white, table }
    }

    pub fn white(&self) -> f32 {
        self.white
    }

    /// One row of RGBA half floats (8 bytes a pixel, little-endian) into
    /// BGRA 8-bit (4 bytes a pixel), alpha opaque.
    pub fn convert_row(&self, src: &[u8], dst: &mut [u8]) {
        let half = |p: &[u8], i: usize| u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]) as usize;
        for (s, d) in src.chunks_exact(8).zip(dst.chunks_exact_mut(4)) {
            d[0] = self.table[half(s, 2)];
            d[1] = self.table[half(s, 1)];
            d[2] = self.table[half(s, 0)];
            d[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_read_right() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0x4000), 2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0xBC00), -1.0);
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x0001), 2f32.powi(-24));
        assert!(half_to_f32(0x7C00).is_infinite());
        assert!(half_to_f32(0x7E00).is_nan());
        // 2.5 = 1.25 * 2^1
        assert_eq!(half_to_f32(0x4100), 2.5);
    }

    #[test]
    fn sdr_white_lands_on_white_and_mid_grey_on_mid_grey() {
        let t = HalfToSrgb::new(2.5);
        let px = |h: u16| {
            let mut d = [0u8; 4];
            let s = [h.to_le_bytes(), h.to_le_bytes(), h.to_le_bytes(), 0x3C00u16.to_le_bytes()].concat();
            t.convert_row(&s, &mut d);
            d
        };
        assert_eq!(px(0x4100), [255, 255, 255, 255], "SDR white (2.5) is white");
        assert_eq!(px(0x4500), [255, 255, 255, 255], "a highlight above white clips");
        assert_eq!(px(0x0000), [0, 0, 0, 255]);
        assert_eq!(px(0xBC00), [0, 0, 0, 255], "out of gamut is black");
        assert_eq!(px(0x7E00), [0, 0, 0, 255], "NaN is black");
        // sRGB 50 % grey is 0.214 linear: 0.214 * 2.5 = 0.535 scRGB (half 0x3848).
        let g = px(0x3848)[0];
        assert!((126..=130).contains(&g), "mid grey came out {g}");
    }

    #[test]
    fn rows_turn_rgba_into_bgra() {
        let t = HalfToSrgb::new(1.0);
        let s = [0x3C00u16, 0x0000, 0x0000, 0x3C00].iter().flat_map(|h| h.to_le_bytes()).collect::<Vec<u8>>();
        let mut d = [0u8; 4];
        t.convert_row(&s, &mut d);
        assert_eq!(d, [0, 0, 255, 255], "red arrives in the R byte of BGRA");
    }

    #[test]
    fn windows_levels_map_to_scrgb() {
        assert_eq!(white_from_level(1000), 1.0);
        assert_eq!(white_from_level(2500), 2.5);
        assert_eq!(white_from_level(0), 1.0);
        assert_eq!(white_from_level(100_000), 7.5);
        assert_eq!(HalfToSrgb::new(f32::NAN).white(), DEFAULT_WHITE);
    }
}
