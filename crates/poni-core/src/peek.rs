//! The Eyes look on a MacBook's notch plays hide and seek with the camera:
//! the eyes peek out beside it on the side the pointer is on and look at the
//! pointer wherever it is on the screen. When the pointer crosses to the
//! other side, they duck behind the camera and come out there. The notch
//! has no pixels, so the Mac itself hides their way across.
//!
//! Only the numbers live here (where the eyes are on their way, where they
//! look); the Mac app draws them (`mac/notch.rs`).

use crate::geom::Point;

/// A whole way across, from one side of the camera to the other (seconds).
pub const CROSS: f64 = 0.6;
/// The eyes look all the way toward a pointer this far from them (DIPs),
/// less toward a nearer one.
const REACH: f32 = 160.0;
/// Where they look moves in steps of this much of the way, so a pointer that
/// moves a little doesn't redraw them.
const STEP: f32 = 0.125;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peek {
    /// The pointer, from the island's top centre (DIPs, y down), wherever it
    /// is on the screen; `None` until it first moves.
    pub pointer: Option<Point>,
    /// When it last moved (mono seconds).
    pub moved_at: f64,
    /// The eyes are left of the camera, or on their way there.
    pub left: bool,
    /// Where the last way across started (-1 the left spot, 1 the right one)
    /// and when.
    from: f32,
    since: f64,
}

impl Default for Peek {
    /// Right of the camera, until the pointer says otherwise.
    fn default() -> Self {
        Peek { pointer: None, moved_at: f64::NEG_INFINITY, left: false, from: 1.0, since: 0.0 }
    }
}

impl Peek {
    /// The pointer moved to `p` at `now`. Past the camera's edge on the
    /// other side (`half` DIPs from the middle) the eyes set off across;
    /// over the camera they stay where they are.
    pub fn point(&mut self, p: Point, now: f64, half: f32) {
        self.pointer = Some(p);
        self.moved_at = now;
        let left = if p.x < -half {
            true
        } else if p.x > half {
            false
        } else {
            return;
        };
        if left != self.left {
            self.from = self.along(now).0;
            self.since = now;
            self.left = left;
        }
    }

    /// Where the eyes are at `now`, from -1 (their spot left of the camera)
    /// to 1 (right of it), and whether they are still on their way. A way
    /// back from halfway takes half as long.
    pub fn along(&self, now: f64) -> (f32, bool) {
        let to = if self.left { -1.0 } else { 1.0 };
        let span = (to - self.from).abs();
        if span < 1e-4 {
            return (to, false);
        }
        let t = ((now - self.since) / (CROSS * f64::from(span) / 2.0)).clamp(0.0, 1.0) as f32;
        (self.from + (to - self.from) * smooth(t), t < 1.0)
    }

    /// The pointer moved less than `secs` seconds before `now`.
    pub fn moved_within(&self, now: f64, secs: f64) -> bool {
        now - self.moved_at < secs
    }
}

/// Where eyes at `eye` look for a pointer at `p` (the same space): toward
/// it, -1 to 1 on each axis, all the way once it is [`REACH`] away, in
/// steps of [`STEP`].
pub fn gaze(eye: Point, p: Point) -> (f32, f32) {
    let (dx, dy) = (p.x - eye.x, p.y - eye.y);
    let d = dx.hypot(dy);
    if d < 1.0 {
        return (0.0, 0.0);
    }
    let k = (d / REACH).min(1.0) / d;
    let q = |v: f32| (v / STEP).round() * STEP + 0.0;
    (q(dx * k), q(dy * k))
}

/// Smoothstep on 0..1: they start slowly, hurry behind the camera and slow
/// down as they come out.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: f32 = 92.5;

    #[test]
    fn they_rest_right_of_the_camera_until_the_pointer_moves() {
        let p = Peek::default();
        assert_eq!(p.along(0.0), (1.0, false));
        assert_eq!(p.along(5_000.0), (1.0, false));
        assert!(!p.moved_within(5_000.0, 60.0));
    }

    #[test]
    fn crossing_over_the_camera_sends_them_across_behind_it() {
        let mut p = Peek::default();
        p.point(Point::new(-300.0, 200.0), 10.0, HALF);
        assert!(p.left);
        let (start, moving) = p.along(10.0);
        assert_eq!(start, 1.0);
        assert!(moving);
        // Halfway through the time they are behind the camera's middle.
        let (mid, _) = p.along(10.0 + CROSS / 2.0);
        assert!(mid.abs() < 1e-4, "{mid}");
        assert_eq!(p.along(10.0 + CROSS), (-1.0, false));
        assert!(p.moved_within(10.5, 1.0));
    }

    #[test]
    fn over_the_camera_they_stay_on_their_side() {
        let mut p = Peek::default();
        p.point(Point::new(-HALF + 1.0, 10.0), 10.0, HALF);
        assert!(!p.left);
        p.point(Point::new(-400.0, 10.0), 11.0, HALF);
        p.point(Point::new(HALF - 1.0, 10.0), 12.0, HALF);
        assert!(p.left);
        assert_eq!(p.along(13.0), (-1.0, false));
    }

    #[test]
    fn turning_back_halfway_starts_from_where_they_are() {
        let mut p = Peek::default();
        p.point(Point::new(-300.0, 0.0), 10.0, HALF);
        let t = 10.0 + CROSS / 2.0;
        let (mid, _) = p.along(t);
        p.point(Point::new(300.0, 0.0), t, HALF);
        assert!(!p.left);
        let (now_at, moving) = p.along(t);
        assert!((now_at - mid).abs() < 1e-4, "{now_at} vs {mid}");
        assert!(moving);
        // The way back is half the way across, so it takes half the time.
        assert_eq!(p.along(t + CROSS / 2.0), (1.0, false));
    }

    #[test]
    fn they_look_toward_the_pointer_and_all_the_way_from_afar() {
        let eye = Point::new(120.0, 16.0);
        assert_eq!(gaze(eye, eye), (0.0, 0.0));
        // Straight below and far: all the way down.
        assert_eq!(gaze(eye, Point::new(120.0, 900.0)), (0.0, 1.0));
        // Far to the left, level: all the way left.
        assert_eq!(gaze(eye, Point::new(-800.0, 16.0)), (-1.0, 0.0));
        // Near: part of the way.
        let (x, y) = gaze(eye, Point::new(160.0, 16.0));
        assert_eq!((x, y), (0.25, 0.0));
        // Down and to the right: both, never more than all the way.
        let (x, y) = gaze(eye, Point::new(520.0, 416.0));
        assert!(x > 0.6 && y > 0.6 && x.hypot(y) <= 1.0 + STEP, "{x} {y}");
    }

    #[test]
    fn a_small_move_keeps_where_they_look() {
        let eye = Point::new(120.0, 16.0);
        assert_eq!(gaze(eye, Point::new(600.0, 500.0)), gaze(eye, Point::new(602.0, 501.0)));
    }
}
