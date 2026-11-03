//! The island's equalizer, from the sound the PC is playing.
//!
//! Four bands, each a band-pass biquad (RBJ cookbook, Q 0.9) followed by a
//! peak follower with a fast attack and a slow release: a bar jumps with the
//! beat and falls back gently. Feed it the 16 kHz mono 16-bit packets
//! `ielts::Downmix` makes of a loopback capture; read the four levels at any
//! time.
//!
//! Why not an FFT: four bars need four filters, not a transform, and this
//! costs a few microseconds per packet, allocates nothing and needs no
//! per-device tuning. The top band stays under the 8 kHz Nyquist of 16 kHz
//! input.

use crate::geom::Color;

/// The equalizer's bars, left to right: a visualizer of the lows to the
/// highs. The collapsed pill draws twelve of them and the Music page one per
/// band, so there are enough to keep the bars as narrow as the pill's.
pub const BANDS: usize = 64;

/// The bands, log-spaced from the lows to the highs: every bar covers as much
/// music as its neighbour. The top stays under the 8 kHz Nyquist of 16 kHz
/// input.
const LOW_HZ: f32 = 55.0;
const HIGH_HZ: f32 = 4_000.0;
const Q: f32 = 6.0;

/// Band `i`'s centre, in Hz.
fn center(i: usize) -> f32 {
    LOW_HZ * (HIGH_HZ / LOW_HZ).powf(i as f32 / (BANDS - 1) as f32)
}
/// The share of the way to a packet's peak a bar keeps: rising at once, a
/// tail taking about a third of a second to settle.
const ATTACK: f32 = 0.9;
const RELEASE: f32 = 0.3;
/// How far below full scale a sound still shows: dBFS. A normal mix puts a
/// band around -20 dB and a quiet passage near -40, so the bars use their
/// whole height instead of sitting at the bottom.
const FLOOR_DB: f32 = -60.0;
/// The auto-gain: the share of full height a frame is scaled to, the gain's
/// bounds, and how fast its reference follows the loudest bar РІР‚вЂќ up at once,
/// down over a few seconds.
const FULL: f32 = 0.9;
/// How much lower the top bar sits than the bottom one: a display choice, as
/// in a player's own bars.
const TILT: f32 = 0.35;
const REFERENCE_START: f32 = 0.3;
const REFERENCE_RISE: f32 = 0.06;
const REFERENCE_FALL: f32 = 0.006;

/// Input sample rate ([`poni_core::ielts::RATE`], kept here so the filters
/// stay one constant).
const RATE: f32 = 16_000.0;

/// The four band levels of what has been pushed, 0..1.
pub struct Bands {
    coef: [[f32; 5]; BANDS],
    /// Each band's last two inputs and outputs, for both stages.
    state: [[f32; 8]; BANDS],
    level: [f32; BANDS],
    /// The frame's loudest bar, eased: the reference the whole frame is
    /// scaled by, so a quiet track fills the bars a loud one does.
    reference: [f32; BANDS],
    /// The last packet's level per band, in dBFS: what the filters heard.
    last_db: [f32; BANDS],
}

impl Default for Bands {
    fn default() -> Self {
        Self::new()
    }
}

impl Bands {
    pub fn new() -> Self {
        Self {
            coef: std::array::from_fn(|i| biquad(center(i))),
            state: [[0.0; 8]; BANDS],
            level: [0.0; BANDS],
            reference: [REFERENCE_START; BANDS],
            last_db: [f32::NEG_INFINITY; BANDS],
        }
    }

    /// Take a packet of 16 kHz mono samples.
    pub fn push(&mut self, pcm: &[i16]) {
        if pcm.is_empty() {
            return;
        }
        let mut peak = [0.0f32; BANDS];
        for &s in pcm {
            let x = f32::from(s) / 32_768.0;
            for b in 0..BANDS {
                let [b0, b1, b2, a1, a2] = self.coef[b];
                // Two stages: one shallow biquad per band let a bass line
                // light the neighbouring bar as brightly as its own.
                let [x1, x2, y1, y2, u1, u2, v1, v2] = self.state[b];
                let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
                let v = b0 * y + b1 * u1 + b2 * u2 - a1 * v1 - a2 * v2;
                self.state[b] = [x, x1, y, y1, y, u1, v, v1];
                peak[b] = peak[b].max(v.abs());
            }
        }
        for b in 0..BANDS {
            // Decibels, not amplitude: a mix spreads wide, and a linear scale
            // left every bar near the floor.
            let db = 20.0 * peak[b].max(1e-9).log10();
            self.last_db[b] = db;
            let target = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
            let k = if target > self.level[b] { ATTACK } else { RELEASE };
            self.level[b] += (target - self.level[b]) * k;
        }
        // Each bar is measured against its own recent peak: a beat fills it,
        // the quiet between beats drops it back. Without this a track mixed
        // quietly leaves every bar near the floor.
        for b in 0..BANDS {
            let top = self.level[b].max(0.02);
            let k = if top > self.reference[b] { REFERENCE_RISE } else { REFERENCE_FALL };
            self.reference[b] += (top - self.reference[b]) * k;
        }
    }

    /// What each band heard in the last packet, in dBFS (NEG_INFINITY for
    /// nothing at all). For the log while tuning, not for drawing.
    pub fn last_db(&self) -> [f32; BANDS] {
        self.last_db
    }

    /// The bars' heights, 0..1, left to right.
    pub fn levels(&self) -> [f32; BANDS] {
        std::array::from_fn(|i| {
            // Below the floor a band is silent, not merely quiet.
            if self.level[i] < 0.001 {
                return 0.0;
            }
            let abs = self.level[i];
            let against_peak = (abs / self.reference[i].max(0.02)).clamp(0.0, 1.0);
            // The display leans on the lows, as a player's bars do: the highs
            // sit lower even when they carry as much.
            let tilt = 1.0 - TILT * i as f32 / (BANDS - 1) as f32;
            // Half the height comes from the band's own recent peak, so a quiet
            // mix still moves; half from how loud it is against the others.
            (0.5 * abs + 0.5 * against_peak).powf(0.9) * FULL * tilt
        })
    }
}

/// How fast the strip's colour drifts through the hues, in degrees a second:
/// a full turn takes about half a minute, slow enough to read as calm.
pub const DRIFT_PER_SEC: f32 = 12.0;

/// The strip runs from the brand's cyan to its violet, left to right.
const HUE_FROM: f32 = 190.0;
const HUE_TO: f32 = 275.0;
/// How far a beat bends the strip's hue: a sharp rise in loudness sends the
/// whole strip toward magenta for a moment.
const PUNCH_HUE: f32 = 70.0;

/// Bar `i` of `n`, in the strip's gradient, with `punch` (0..1, from the
/// app's reading of a sudden rise) bending every bar's hue together, and
/// `phase` in degrees sliding the whole gradient, so it drifts even in
/// silence. Pure, so the look can be checked without a screen.
pub fn bar_color(i: usize, n: usize, punch: f32, phase: f32) -> Color {
    let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
    let hue = (HUE_FROM + (HUE_TO - HUE_FROM) * t + PUNCH_HUE * punch.clamp(0.0, 1.0) + phase).rem_euclid(360.0);
    let (r, g, b) = hsv(hue, 0.85, 1.0);
    Color::rgba(r, g, b, 0.95)
}

/// Hue in degrees, saturation and value 0..1, to red, green and blue 0..1.
fn hsv(hue: f32, sat: f32, val: f32) -> (f32, f32, f32) {
    let h = hue.rem_euclid(360.0) / 60.0;
    let c = val * sat;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = val - c;
    (r + m, g + m, b + m)
}

/// A constant-peak-gain band-pass biquad at `f0`: `b0, b1, b2, a1, a2`,
/// normalised by `a0`.
fn biquad(f0: f32) -> [f32; 5] {
    let w0 = 2.0 * std::f32::consts::PI * f0 / RATE;
    let alpha = w0.sin() / (2.0 * Q);
    let a0 = 1.0 + alpha;
    let cos = w0.cos();
    [alpha / a0, 0.0, -alpha / a0, -2.0 * cos / a0, (1.0 - alpha) / a0]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `secs` of a sine at `hz`, amplitude `amp`, as 16 kHz samples.
    fn sine(hz: f32, amp: f32, secs: f32) -> Vec<i16> {
        let n = (RATE * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE;
                (amp * (2.0 * std::f32::consts::PI * hz * t).sin() * 32_768.0) as i16
            })
            .collect()
    }

    /// The band whose centre sits closest to `hz`.
    fn nearest(hz: f32) -> usize {
        (0..BANDS).min_by(|a, b| (center(*a) - hz).abs().total_cmp(&(center(*b) - hz).abs())).expect("a band")
    }

    /// Feed `secs` of a sine the way the loopback delivers it: 40 ms packets.
    fn feed(b: &mut Bands, hz: f32, amp: f32, secs: f32) {
        for packet in sine(hz, amp, secs).chunks((RATE * 0.04) as usize) {
            b.push(packet);
        }
    }

    #[test]
    fn each_bar_answers_its_own_pitch() {
        for hz in [45.0, 120.0, 400.0, 1_200.0, 2_000.0, 3_800.0] {
            let bar = nearest(hz);
            let mut b = Bands::new();
            feed(&mut b, hz, 0.5, 0.6);
            let l = b.levels();
            // The tone's own bar has to stand out and be the loudest: the
            // bars either side of it also rise, because each bar is measured
            // against its own recent peak.
            let loudest = (0..BANDS).max_by(|x, y| l[*x].total_cmp(&l[*y])).expect("a bar");
            assert!(loudest.abs_diff(bar) <= 2, "{hz} Hz peaks at bar {loudest}, not {bar}: {l:?}");
            assert!(l[bar] > 0.4, "{hz} Hz should fill bar {bar}: {l:?}");
        }
    }

    /// A stand-in for music: pink-ish noise with a kick every half second, at
    /// the level a real track puts out.
    fn music(secs: f32) -> Vec<i16> {
        let n = (RATE * secs) as usize;
        let mut seed = 2_469_135u32;
        let mut low = 0.0f32;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((seed >> 16) as f32 / 32_768.0) - 1.0;
            // A one-pole low pass tilts it toward the lows, like a mix.
            low += (white - low) * 0.08;
            let t = i as f32 / RATE;
            let beat = if (t * 2.0) % 1.0 < 0.08 { 0.6 } else { 0.0 };
            let v = (low * 0.5 + beat * (2.0 * std::f32::consts::PI * 55.0 * t).sin()) * 0.35;
            out.push((v.clamp(-1.0, 1.0) * 32_767.0) as i16);
        }
        out
    }

    #[test]
    fn a_track_makes_the_bars_jump() {
        let mut b = Bands::new();
        let song = music(3.0);
        let packets: Vec<&[i16]> = song.chunks((RATE * 0.04) as usize).collect();
        let mut frames: Vec<[f32; BANDS]> = Vec::new();
        for p in &packets {
            b.push(p);
            frames.push(b.levels());
        }
        // The last two seconds, past the reference settling.
        let tail = &frames[frames.len() / 3..];
        let mean: f32 = tail.iter().map(|f| f.iter().sum::<f32>() / BANDS as f32).sum::<f32>() / tail.len() as f32;
        let peak = tail.iter().map(|f| f.iter().copied().fold(0.0f32, f32::max)).fold(0.0f32, f32::max);
        // Every frame differs from the frame before it: the bars move.
        let moved = tail.windows(2).map(|w| w[0].iter().zip(&w[1]).map(|(a, c)| (a - c).abs()).sum::<f32>() / BANDS as f32).sum::<f32>() / (tail.len() - 1) as f32;
        // And the lows stand above the highs, as a real mix does.
        let low_band = tail.iter().map(|f| f[..8].iter().sum::<f32>() / 8.0).sum::<f32>() / tail.len() as f32;
        let high_band = tail.iter().map(|f| f[BANDS - 8..].iter().sum::<f32>() / 8.0).sum::<f32>() / tail.len() as f32;
        println!("PROBE mean={mean:.2} peak={peak:.2} moved={moved:.3} low={low_band:.2} high={high_band:.2}");
        assert!(peak > 0.7, "some bar has to reach the top: {peak}");
        assert!(mean > 0.25, "the strip has to sit up, not near the floor: {mean}");
        // Each bar has to swing over the tail, not sit at one height: that is
        // what makes a beat visible.
        let mut swing = 0.0f32;
        for b in 0..BANDS {
            let hi = tail.iter().map(|f| f[b]).fold(0.0f32, f32::max);
            let lo = tail.iter().map(|f| f[b]).fold(1.0f32, f32::min);
            swing += hi - lo;
        }
        swing /= BANDS as f32;
        println!("SWING {swing:.2}");
        assert!(swing > 0.1, "each bar has to swing over the track, not sit still: {swing}");
        assert!(moved > 0.002, "the bars have to change between frames: {moved}");
        assert!(low_band > high_band, "the lows stand above the highs: {low_band} vs {high_band}");
    }

    #[test]
    fn a_bar_falls_back_when_the_sound_stops() {
        let mut b = Bands::new();
        feed(&mut b, 500.0, 0.6, 0.4);
        assert!(b.levels()[nearest(500.0)] > 0.5, "the tone shows: {:?}", b.levels());
        // A second of silence: two dozen packets of nothing.
        let quiet = vec![0i16; (RATE * 0.04) as usize];
        for _ in 0..25 {
            b.push(&quiet);
        }
        for v in b.levels() {
            assert!(v < 0.05, "silence settles every bar: {:?}", b.levels());
        }
    }

    #[test]
    fn a_quiet_track_fills_the_bars_as_a_loud_one_does() {
        // The same music, once mixed hot and once mixed quietly: both have to
        // fill the strip, or a quiet track looks like a dead equalizer.
        let bar = nearest(500.0);
        let mut hot = Bands::new();
        feed(&mut hot, 500.0, 0.6, 2.0);
        let mut quiet = Bands::new();
        feed(&mut quiet, 500.0, 0.05, 2.0);
        let (hot, quiet) = (hot.levels()[bar], quiet.levels()[bar]);
        assert!(hot > 0.6, "a hot track fills the bar: {hot}");
        assert!(quiet > 0.45, "a quiet one still shows: {quiet}");
    }

    #[test]
    fn a_beat_bends_the_strips_colour() {
        // The strip runs cyan to violet, so its ends must differ...
        let left = bar_color(0, BANDS, 0.0, 0.0);
        let right = bar_color(BANDS - 1, BANDS, 0.0, 0.0);
        assert!((left.b - right.b).abs() > 0.1 || (left.r - right.r).abs() > 0.2, "a gradient across the bars: {left:?} {right:?}");
        // ...and a beat moves every bar together.
        let quiet = bar_color(5, BANDS, 0.0, 0.0);
        let hit = bar_color(5, BANDS, 1.0, 0.0);
        assert!((quiet.r - hit.r).abs() + (quiet.g - hit.g).abs() + (quiet.b - hit.b).abs() > 0.3, "a punch shifts the hue: {quiet:?} {hit:?}");
        // Nothing leaves the colour space.
        for i in 0..BANDS {
            for punch in [0.0, 0.5, 1.0, 2.0] {
                for phase in [0.0, 90.0, 200.0, 359.0] {
                    let c = bar_color(i, BANDS, punch, phase);
                    assert!((0.0..=1.0).contains(&c.r) && (0.0..=1.0).contains(&c.g) && (0.0..=1.0).contains(&c.b), "{c:?}");
                }
            }
        }
    }

    #[test]
    fn the_strip_drifts_through_the_colours_without_a_sound() {
        // Silence: the bars stand low, but the colour still moves, so the
        // pill never looks frozen.
        let still = bar_color(5, BANDS, 0.0, 0.0);
        let drifted = bar_color(5, BANDS, 0.0, 60.0);
        let moved = (still.r - drifted.r).abs() + (still.g - drifted.g).abs() + (still.b - drifted.b).abs();
        assert!(moved > 0.3, "the same bar takes another colour a moment later: {still:?} {drifted:?}");
        // A whole turn is the same colour again.
        let back = bar_color(5, BANDS, 0.0, 360.0);
        assert!((still.r - back.r).abs() < 0.01 && (still.g - back.g).abs() < 0.01 && (still.b - back.b).abs() < 0.01, "{still:?} {back:?}");
    }

    #[test]
    fn hsv_names_the_primaries() {
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        let red = hsv(0.0, 1.0, 1.0);
        let green = hsv(120.0, 1.0, 1.0);
        let blue = hsv(240.0, 1.0, 1.0);
        assert!(near(red.0, 1.0) && near(red.1, 0.0) && near(red.2, 0.0), "{red:?}");
        assert!(near(green.1, 1.0) && near(green.0, 0.0), "{green:?}");
        assert!(near(blue.2, 1.0) && near(blue.0, 0.0), "{blue:?}");
    }
}
