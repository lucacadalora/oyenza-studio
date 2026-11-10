//! Oyenza Studio's Diorama theme: the resting pill as a small window onto a
//! pixel-art place that keeps the day's hours.
//!
//! A raised frame holds a pane with a house, a tree and a sky behind it,
//! and on the pane rolls a glass ball that magnifies what is under it. By
//! day the ball rests on the house at the left end; when the sun sets it
//! rolls to the tree at the right end and night falls behind it, the way a
//! day and night switch flips; at sunrise it rolls back. In between, the
//! sky follows the clock: dawn, day, the golden hour, dusk, the stars, the
//! moon in tonight's phase, a window lit until midnight. The time is
//! written in the sky.
//!
//! The crate paints that picture and nothing else: [`render`] turns a
//! [`Moment`] (the minute, when the sun rises and sets, where the ball is,
//! what the clock says) into the pill's pixels for a [`View`] (its size and
//! the display's scale). No clock, no state, no OS: the app says what time
//! it is and draws the result as an image, so Windows and the Mac show the
//! same pixels, and the tests run anywhere.
//!
//! The picture is made once a minute while the pill rests (with the minute
//! tick the clock already wakes for) and for each frame of the ball's roll,
//! about a second and a half twice a day: it runs for moments, so the
//! workspace builds this crate for size (AGENTS.md, Keep it light).

mod scene;

/// The resting pill's height in DIPs; it is a capsule, so its ends have
/// half of it as their radius.
pub const H: f32 = 38.0;
/// The frame's thickness at rest. Under the pointer the body swells and the
/// frame with it; the pane keeps its size.
pub const BEZEL: f32 = 2.5;
/// Between the frame and the glass ball.
const BALL_GAP: f32 = 1.5;
/// The ball magnifies this much in its middle, and at its rim shows what is
/// this far out from its centre (in radii): a little past its own edge, so
/// the place bends round it.
const BALL_MAG: f32 = 1.42;
const BALL_RIM: f32 = 1.14;
/// How much more red bends than blue at the rim (the ball's coloured edge).
const BALL_FRINGE: f32 = 0.03;
/// Toward the light: up and to the left.
const LIGHT: (f32, f32) = (-0.45, -0.893);

/// What the clock in the sky says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    /// 0..=23, whichever way it is written.
    pub hour: u8,
    pub minute: u8,
    /// Written as 24 hours ("17:05") or as 12 ("5:05 PM").
    pub h24: bool,
}

/// What stands beside the clock on a computer with a battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    /// A bolt while it charges.
    Charging,
    /// A battery in red when it runs low (the percentage, for the key).
    Low(u8),
}

/// One moment of the place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    /// Minutes since local midnight, 0.0..1440.0.
    pub minute: f32,
    /// When the sun rises and sets, in minutes since local midnight.
    pub sunrise: f32,
    pub sunset: f32,
    /// The local date as days since 1 January 2000: the moon's phase, and
    /// where the clouds start their day.
    pub day: i32,
    /// Where the glass ball is: 0.0 on the house (day), 1.0 on the tree
    /// (night). The picture goes from the day side to the night side as it
    /// rolls across; [`Moment::rest_lens`] is where the time puts it.
    pub lens: f32,
    /// The time to write in the sky, or none (the Minimal look).
    pub clock: Option<Clock>,
    pub badge: Option<Badge>,
}

impl Moment {
    /// The sun is down at this minute.
    pub fn night(&self) -> bool {
        self.minute < self.sunrise || self.minute >= self.sunset
    }

    /// Where the ball rests at this minute: on the house by day, on the
    /// tree at night.
    pub fn rest_lens(&self) -> f32 {
        if self.night() {
            1.0
        } else {
            0.0
        }
    }
}

/// The resting pill's width in DIPs for what it shows: the house's end, the
/// tree's end and a middle wide enough for the clock (12-hour times keep
/// room for "12:59 PM", so the pill never changes width with the hour) and
/// the badge. An even number, so the centred pill sits on whole pixels.
pub fn width(clock: Option<Clock>, badge: Option<Badge>) -> f32 {
    let mid = match clock {
        None => scene::MID_BARE,
        Some(c) => {
            let written = scene::CLOCK_W + if c.h24 { 0 } else { scene::MERIDIEM_W } + if badge.is_some() { scene::BADGE_W } else { 0 };
            written + 2 * scene::MID_PAD
        }
    };
    let w = scene::HOUSE_ZONE + mid + scene::TREE_ZONE;
    (w + (w & 1)) as f32
}

/// What to paint the picture for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The pill's body in DIPs: its resting size, or a little more while it
    /// swells under the pointer.
    pub w: f32,
    pub h: f32,
    /// Its resting size ([`width`] x [`H`]): the pane and the place behind
    /// it keep this size, centred; what the body has beyond it is frame.
    pub rest_w: f32,
    pub rest_h: f32,
    /// Device pixels per DIP.
    pub scale: f32,
}

/// The pill's pixels: premultiplied BGRA, top-down rows, `w * 4` to a row,
/// every pixel opaque (the app draws it clipped to the pill's outline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

type Rgbf = [f32; 3];

fn mix(a: Rgbf, b: Rgbf, t: f32) -> Rgbf {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

const fn rgbf(h: u32) -> Rgbf {
    [(h >> 16 & 255) as f32, (h >> 8 & 255) as f32, (h & 255) as f32]
}

/// The frame by day (cream) and by night (indigo): its face, where the
/// light catches it and where it turns away.
const FRAME_DAY: [Rgbf; 3] = [rgbf(0xEFE9DB), rgbf(0xFFFFFF), rgbf(0xBFB49C)];
const FRAME_NIGHT: [Rgbf; 3] = [rgbf(0x2B2C56), rgbf(0x55558A), rgbf(0x11122A)];
/// What shadows darken toward.
const SHADE: Rgbf = rgbf(0x10142E);

/// The glass ball on the pane, in art pixels.
struct Ball {
    cx: f32,
    cy: f32,
    r: f32,
}

impl Ball {
    /// Where the ball shows the picture from at (x, y), for a colour that
    /// bends `bend` times as much as green. `None` outside the ball.
    fn through(&self, x: f32, y: f32, bend: f32) -> Option<(f32, f32)> {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let t2 = (dx * dx + dy * dy) / (self.r * self.r);
        if t2 >= 1.0 {
            return None;
        }
        // Magnified evenly over the middle, squeezed toward the rim.
        let k = (1.0 + (BALL_MAG * BALL_RIM - 1.0) * t2 * t2) / BALL_MAG * (1.0 + (bend - 1.0) * t2);
        Some((self.cx + dx * k, self.cy + dy * k))
    }
}

/// Paint the pill for `m` in `v`.
pub fn render(m: &Moment, v: &View) -> Picture {
    let s = if v.scale.is_finite() { v.scale.clamp(0.5, 8.0) } else { 1.0 };
    let aw = (v.rest_w.round() as i32).clamp(8, 1024);
    let ah = (v.rest_h.round() as i32).clamp(8, 256);
    let pw = ((v.w * s).round() as i32).clamp(1, 8192);
    let ph = ((v.h * s).round() as i32).clamp(1, 2048);
    let art = scene::paint(m, aw, ah);
    let dark = scene::night_share(m.lens);

    // The place stays its size, centred on whole pixels; the body in DIPs.
    let ox = ((pw as f32 - aw as f32 * s) / 2.0).round() / s;
    let oy = ((ph as f32 - ah as f32 * s) / 2.0).round() / s;
    let (bw, bh) = (pw as f32 / s, ph as f32 / s);
    let r_out = bw.min(bh) / 2.0;
    let frame = BEZEL + ox.min(oy).max(0.0);
    let ball_r = ah as f32 / 2.0 - BEZEL - BALL_GAP;
    let ball = Ball { cx: ah as f32 / 2.0 + m.lens.clamp(0.0, 1.0) * (aw - ah) as f32, cy: ah as f32 / 2.0, r: ball_r };
    let whole = (s - s.round()).abs() < 1e-3;

    let face = mix(FRAME_DAY[0], FRAME_NIGHT[0], dark);
    let lit = mix(FRAME_DAY[1], FRAME_NIGHT[1], dark);
    let unlit = mix(FRAME_DAY[2], FRAME_NIGHT[2], dark);

    let mut bgra = vec![0u8; (pw * ph * 4) as usize];
    for py in 0..ph {
        for px in 0..pw {
            let (x, y) = ((px as f32 + 0.5) / s, (py as f32 + 0.5) / s);
            // How deep inside the capsule's edge, and which way is out.
            let ax = x.clamp(r_out, (bw - r_out).max(r_out));
            let (dx, dy) = (x - ax, y - bh / 2.0);
            let dist = (dx * dx + dy * dy).sqrt();
            let depth = r_out - dist;
            let (nx, ny) = if dist > 1e-3 { (dx / dist, dy / dist) } else { (0.0, -1.0) };
            let toward = nx * LIGHT.0 + ny * LIGHT.1;
            let inner = depth - frame;
            let pane = (inner * s + 0.5).clamp(0.0, 1.0);

            let mut c = [0.0f32; 3];
            if pane < 1.0 {
                // The frame: a rounded rim, lit where it faces the light.
                let t = (depth / frame).clamp(0.0, 1.0);
                let lam = (std::f32::consts::PI * t).cos() * toward;
                let mut f = if lam > 0.0 { mix(face, lit, lam) } else { mix(face, unlit, -lam) };
                // A firmer line where it ends, so cream holds on a white window.
                f = mix(f, unlit, 0.4 * (1.0 - smoothstep(0.0, 0.9, depth)));
                c = f;
            }
            if pane > 0.0 {
                let (u, w) = (x - ox, y - oy);
                let (bx, by) = (u - ball.cx, w - ball.cy);
                let bd = (bx * bx + by * by).sqrt();
                let near_ball = bd < ball.r + 1.0;
                // What shows here: the place, through the ball where it is.
                let n = if near_ball || !whole { 3 } else { 1 };
                let mut sum = [0u32; 3];
                for j in 0..n {
                    for i in 0..n {
                        let fx = (px as f32 + (i as f32 + 0.5) / n as f32) / s - ox;
                        let fy = (py as f32 + (j as f32 + 0.5) / n as f32) / s - oy;
                        match ball.through(fx, fy, 1.0) {
                            Some((gx, gy)) if near_ball => {
                                let (rx, ry) = ball.through(fx, fy, 1.0 - BALL_FRINGE).unwrap_or((gx, gy));
                                let (qx, qy) = ball.through(fx, fy, 1.0 + BALL_FRINGE).unwrap_or((gx, gy));
                                sum[0] += art.get(rx.floor() as i32, ry.floor() as i32)[0] as u32;
                                sum[1] += art.get(gx.floor() as i32, gy.floor() as i32)[1] as u32;
                                sum[2] += art.get(qx.floor() as i32, qy.floor() as i32)[2] as u32;
                            }
                            _ => {
                                let p = art.get(fx.floor() as i32, fy.floor() as i32);
                                sum[0] += p[0] as u32;
                                sum[1] += p[1] as u32;
                                sum[2] += p[2] as u32;
                            }
                        }
                    }
                }
                let k = (n * n) as f32;
                let mut p = [sum[0] as f32 / k, sum[1] as f32 / k, sum[2] as f32 / k];

                // The frame's shadow on the pane, deepest under its lit side.
                if inner < 7.0 {
                    p = mix(p, SHADE, 0.5 * (-inner.max(0.0) / 1.6).exp() * (0.58 + 0.42 * toward));
                }
                if near_ball {
                    let glass = ((ball.r - bd) * s + 0.5).clamp(0.0, 1.0);
                    let t = (bd / ball.r).min(1.0);
                    let facing = if bd > 1e-3 { (bx * LIGHT.0 + by * LIGHT.1) / bd } else { 0.0 };
                    let bright = 1.0 - 0.55 * dark;
                    // Darker toward its rim; a crescent of light along the
                    // rim on the lit side; a soft window of light above
                    // the middle; a little light coming back opposite.
                    let rim = 0.42 * smoothstep(0.84, 1.0, t);
                    let arc = 0.8 * smoothstep(0.78, 0.92, t) * (1.0 - smoothstep(0.92, 1.0, t)) * smoothstep(0.3, 0.95, facing);
                    let hd = ((bx + 0.36 * ball.r).powi(2) + (by + 0.44 * ball.r).powi(2)).sqrt() / (0.36 * ball.r);
                    let spot = 0.4 * (1.0 - hd * hd).max(0.0).powi(2);
                    let back = 0.2 * smoothstep(0.72, 0.96, t) * smoothstep(0.45, 1.0, -facing);
                    let mut g = mix(p, SHADE, rim);
                    g = mix(g, [255.0; 3], ((arc + spot) * bright + back * 0.6).min(1.0));
                    // Outside it, its shadow on the pane, down and to the right.
                    let sd = ((bx - 1.0).powi(2) + (by - 2.2).powi(2)).sqrt();
                    let out = mix(p, SHADE, 0.3 * (1.0 - smoothstep(ball.r - 1.5, ball.r + 3.0, sd)));
                    p = mix(out, g, glass);
                } else if bd < ball.r + 5.0 {
                    let sd = ((bx - 1.0).powi(2) + (by - 2.2).powi(2)).sqrt();
                    p = mix(p, SHADE, 0.3 * (1.0 - smoothstep(ball.r - 1.5, ball.r + 3.0, sd)));
                }
                c = mix(c, p, pane);
            }
            let o = ((py * pw + px) * 4) as usize;
            bgra[o] = c[2].round().clamp(0.0, 255.0) as u8;
            bgra[o + 1] = c[1].round().clamp(0.0, 255.0) as u8;
            bgra[o + 2] = c[0].round().clamp(0.0, 255.0) as u8;
            bgra[o + 3] = 255;
        }
    }
    Picture { w: pw as u32, h: ph as u32, bgra }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moment(minute: f32) -> Moment {
        let mut m = Moment { minute, sunrise: 360.0, sunset: 1080.0, day: 9800, lens: 0.0, clock: Some(Clock { hour: (minute / 60.0) as u8, minute: (minute % 60.0) as u8, h24: true }), badge: None };
        m.lens = m.rest_lens();
        m
    }

    fn view(m: &Moment, scale: f32) -> View {
        let w = width(m.clock, m.badge);
        View { w, h: H, rest_w: w, rest_h: H, scale }
    }

    fn luma(p: &Picture) -> f32 {
        let n = (p.w * p.h) as f32;
        p.bgra.chunks(4).map(|c| 0.11 * c[0] as f32 + 0.59 * c[1] as f32 + 0.3 * c[2] as f32).sum::<f32>() / n
    }

    #[test]
    fn the_pill_is_as_wide_as_what_it_shows() {
        let h24 = Clock { hour: 13, minute: 27, h24: true };
        let h12 = Clock { h24: false, ..h24 };
        assert_eq!(width(None, None), 80.0);
        assert_eq!(width(Some(h24), None), 100.0);
        assert_eq!(width(Some(h12), None), 112.0);
        assert_eq!(width(Some(h24), Some(Badge::Charging)), 112.0);
        assert_eq!(width(Some(h12), Some(Badge::Low(12))), 124.0);
        // Even, so the centred pill lands on whole pixels at 100 %.
        for w in [width(None, None), width(Some(h24), None), width(Some(h12), Some(Badge::Charging))] {
            assert_eq!(w % 2.0, 0.0);
        }
    }

    #[test]
    fn the_ball_rests_on_the_house_by_day_and_on_the_tree_at_night() {
        assert_eq!(moment(720.0).rest_lens(), 0.0);
        assert_eq!(moment(360.0).rest_lens(), 0.0);
        assert_eq!(moment(359.0).rest_lens(), 1.0);
        assert_eq!(moment(1079.0).rest_lens(), 0.0);
        assert_eq!(moment(1080.0).rest_lens(), 1.0);
        assert_eq!(moment(30.0).rest_lens(), 1.0);
    }

    #[test]
    fn the_picture_is_the_body_in_pixels_and_opaque() {
        let m = moment(600.0);
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 3.0] {
            let p = render(&m, &view(&m, scale));
            assert_eq!((p.w, p.h), ((100.0 * scale).round() as u32, (38.0 * scale).round() as u32), "{scale}");
            assert_eq!(p.bgra.len(), (p.w * p.h * 4) as usize);
            assert!(p.bgra.chunks(4).all(|c| c[3] == 255), "{scale}: opaque");
        }
        // Under the pointer the body is larger and so is the picture.
        let lifted = View { w: 106.0, h: 44.0, ..view(&m, 2.0) };
        let p = render(&m, &lifted);
        assert_eq!((p.w, p.h), (212, 88));
    }

    #[test]
    fn the_same_moment_paints_the_same_pixels() {
        let m = moment(1100.0);
        assert_eq!(render(&m, &view(&m, 1.5)), render(&m, &view(&m, 1.5)));
        // A minute later something has moved.
        assert_ne!(render(&m, &view(&m, 1.0)), render(&moment(1101.0), &view(&m, 1.0)));
    }

    #[test]
    fn night_is_darker_than_day_and_the_golden_hour_between() {
        let shot = |minute| {
            let m = moment(minute);
            luma(&render(&m, &view(&m, 1.0)))
        };
        let (noon, gold, night, small_hours) = (shot(720.0), shot(1070.0), shot(1320.0), shot(180.0));
        assert!(noon > 150.0, "noon {noon}");
        assert!(gold < noon && gold > night, "gold {gold}");
        assert!(night < 60.0, "night {night}");
        // Lights out after midnight: darker still.
        assert!(small_hours < night, "{small_hours} vs {night}");
    }

    #[test]
    fn at_whole_scales_the_place_is_drawn_pixel_for_pixel() {
        // Away from the frame's shadow and the ball every art pixel is a
        // block of one flat colour.
        let m = moment(720.0);
        let p = render(&m, &view(&m, 2.0));
        let at = |x: u32, y: u32| {
            let o = ((y * p.w + x) * 4) as usize;
            [p.bgra[o], p.bgra[o + 1], p.bgra[o + 2]]
        };
        for ay in 14..24u32 {
            for ax in 44..60u32 {
                let c = at(ax * 2, ay * 2);
                assert_eq!([at(ax * 2 + 1, ay * 2), at(ax * 2, ay * 2 + 1), at(ax * 2 + 1, ay * 2 + 1)], [c, c, c], "art pixel {ax},{ay}");
            }
        }
    }

    #[test]
    fn the_clock_stays_clear_of_the_ball() {
        for (clock, badge) in [
            (Clock { hour: 23, minute: 59, h24: true }, None),
            (Clock { hour: 12, minute: 59, h24: false }, None),
            (Clock { hour: 12, minute: 59, h24: false }, Some(Badge::Low(9))),
        ] {
            let w = width(Some(clock), badge) as i32;
            let written = scene::CLOCK_W + if clock.h24 { 0 } else { scene::MERIDIEM_W } + if badge.is_some() { scene::BADGE_W } else { 0 };
            let (x0, x1) = ((scene::HOUSE_ZONE + w - scene::TREE_ZONE - written) / 2 - 1, (scene::HOUSE_ZONE + w - scene::TREE_ZONE + written) / 2 + 1);
            let r = H / 2.0 - BEZEL - BALL_GAP;
            assert!(x0 as f32 >= H / 2.0 + r, "{clock:?}: starts at {x0}, the ball ends at {}", H / 2.0 + r);
            assert!(x1 as f32 <= w as f32 - H / 2.0 - r, "{clock:?}: ends at {x1}");
        }
    }

    #[test]
    fn odd_views_do_not_panic() {
        let m = moment(720.0);
        for v in [
            View { w: 0.0, h: 0.0, rest_w: 0.0, rest_h: 0.0, scale: 1.0 },
            View { w: 40.0, h: 12.0, rest_w: 100.0, rest_h: 38.0, scale: 1.0 },
            View { w: 100.0, h: 38.0, rest_w: 100.0, rest_h: 38.0, scale: f32::NAN },
            View { w: 300.0, h: 200.0, rest_w: 100.0, rest_h: 38.0, scale: 0.1 },
        ] {
            let p = render(&m, &v);
            assert_eq!(p.bgra.len(), (p.w * p.h * 4) as usize);
        }
        let odd = Moment { sunrise: 900.0, sunset: 300.0, lens: 7.0, minute: -5.0, ..m };
        render(&odd, &view(&m, 1.0));
    }
}
