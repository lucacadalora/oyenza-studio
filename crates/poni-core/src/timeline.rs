//! Playback position extrapolation.
//!
//! Media sessions (Windows GSMTC) report a position snapshot that is only
//! "current as of" its update time, and many players update it rarely. To drive
//! a progress bar and synced lyrics we extrapolate from that snapshot.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timeline {
    /// Position in seconds at `sampled_at`.
    pub position_s: f64,
    /// Track length in seconds (0 when unknown).
    pub duration_s: f64,
    /// Monotonic clock (seconds) when `position_s` was valid.
    pub sampled_at: f64,
    pub playing: bool,
    /// Playback rate (1.0 normal).
    pub rate: f64,
}

impl Timeline {
    pub fn new(position_s: f64, duration_s: f64, sampled_at: f64, playing: bool) -> Self {
        Self { position_s, duration_s, sampled_at, playing, rate: 1.0 }
    }

    /// Estimated position at monotonic time `now`, clamped to the track.
    pub fn position_at(&self, now: f64) -> f64 {
        let mut p = self.position_s;
        if self.playing {
            let rate = if self.rate > 0.0 { self.rate } else { 1.0 };
            p += (now - self.sampled_at).max(0.0) * rate;
        }
        if self.duration_s > 0.0 {
            p = p.min(self.duration_s);
        }
        p.max(0.0)
    }

    /// 0..=1 progress, or 0 when duration is unknown.
    pub fn progress_at(&self, now: f64) -> f32 {
        if self.duration_s <= 0.0 {
            0.0
        } else {
            (self.position_at(now) / self.duration_s).clamp(0.0, 1.0) as f32
        }
    }

    pub fn has_duration(&self) -> bool {
        self.duration_s > 0.5
    }
}

/// A steady clock for synced lyrics, fed every frame with the player's
/// position estimate.
///
/// Some players report a jumpy position (Apple Music for Windows is the worst
/// for it): each report can land a few hundred milliseconds off the last
/// one. Followed directly, the lyrics stutter and can flick back a line. This
/// clock runs at the playback rate on its own and eases towards the reports
/// instead, never running backwards while playing; a big difference (a seek,
/// a new track) is taken at once.
#[derive(Clone, Copy, Debug, Default)]
pub struct LyricClock {
    t: f64,
    last_now: Option<f64>,
}

impl LyricClock {
    /// Differences bigger than this are seeks, not jitter.
    pub const JUMP_S: f64 = 1.0;
    /// How fast the clock eases onto the reports (time constant, seconds).
    pub const EASE_S: f64 = 0.6;
    /// A gap between frames this long means the lyrics weren't shown meanwhile.
    pub const GAP_S: f64 = 0.5;

    /// Forget the past (new track, lyrics shown again).
    pub fn reset(&mut self) {
        self.last_now = None;
    }

    /// The position (seconds) the lyrics should show at `now`, given the
    /// player's estimate `pos` and whether it is playing at `rate`.
    pub fn tick(&mut self, now: f64, pos: f64, playing: bool, rate: f64) -> f64 {
        let dt = self.last_now.map_or(f64::INFINITY, |l| now - l);
        self.last_now = Some(now);
        if !playing || !pos.is_finite() || !self.t.is_finite() || !(0.0..=Self::GAP_S).contains(&dt) {
            self.t = pos;
            return self.t;
        }
        let rate = if rate.is_finite() && rate > 0.0 { rate } else { 1.0 };
        let predicted = self.t + dt * rate;
        let err = pos - predicted;
        if err.abs() > Self::JUMP_S {
            self.t = pos;
            return self.t;
        }
        let eased = predicted + err * (1.0 - (-dt / Self::EASE_S).exp());
        self.t = eased.max(self.t);
        self.t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrapolates_while_playing() {
        let t = Timeline::new(10.0, 200.0, 100.0, true);
        assert!((t.position_at(105.0) - 15.0).abs() < 1e-9);
        assert!((t.progress_at(105.0) - 0.075).abs() < 1e-6);
    }

    #[test]
    fn frozen_while_paused() {
        let t = Timeline::new(10.0, 200.0, 100.0, false);
        assert_eq!(t.position_at(150.0), 10.0);
    }

    #[test]
    fn clamps_to_duration_and_zero() {
        let t = Timeline::new(190.0, 200.0, 0.0, true);
        assert_eq!(t.position_at(100.0), 200.0);
        let t = Timeline::new(-3.0, 0.0, 0.0, false);
        assert_eq!(t.position_at(0.0), 0.0);
        assert_eq!(t.progress_at(0.0), 0.0);
    }

    #[test]
    fn clock_going_backwards_does_not_rewind() {
        let t = Timeline::new(10.0, 200.0, 100.0, true);
        assert_eq!(t.position_at(90.0), 10.0);
    }

    #[test]
    fn lyric_clock_follows_a_steady_player() {
        let mut c = LyricClock::default();
        let mut out = 0.0;
        for i in 0..300 {
            let now = i as f64 / 60.0;
            out = c.tick(now, 10.0 + now, true, 1.0);
        }
        assert!((out - (10.0 + 299.0 / 60.0)).abs() < 1e-6);
    }

    #[test]
    fn lyric_clock_smooths_jitter_and_never_runs_back() {
        let mut c = LyricClock::default();
        let mut prev = f64::NEG_INFINITY;
        let mut worst: f64 = 0.0;
        for i in 0..600 {
            let now = i as f64 / 60.0;
            // A report that jumps +-0.3 s every half second.
            let jitter = if (i / 30) % 2 == 0 { 0.3 } else { -0.3 };
            let t = c.tick(now, 5.0 + now + jitter, true, 1.0);
            assert!(t >= prev, "ran backwards at frame {i}: {t} < {prev}");
            prev = t;
            if i > 60 {
                worst = worst.max((t - (5.0 + now)).abs());
            }
        }
        assert!(worst < 0.3, "strays {worst:.3} s from the true position");
    }

    #[test]
    fn lyric_clock_takes_seeks_pauses_and_gaps_at_once() {
        let mut c = LyricClock::default();
        c.tick(0.0, 10.0, true, 1.0);
        c.tick(1.0 / 60.0, 10.0 + 1.0 / 60.0, true, 1.0);
        // Seek back 30 s.
        assert_eq!(c.tick(2.0 / 60.0, 3.0, true, 1.0), 3.0);
        // Paused: exactly the player's position, even backwards.
        assert_eq!(c.tick(3.0 / 60.0, 2.5, false, 1.0), 2.5);
        // Hidden for a while: snaps to the report.
        c.tick(4.0 / 60.0, 2.5, true, 1.0);
        assert_eq!(c.tick(10.0, 50.0, true, 1.0), 50.0);
    }
}
