//! Frame timing.
//!
//! The frame clock thread wakes up "just after" each vertical blank, but how
//! long after varies by a fraction of a millisecond with thread scheduling.
//! Fed straight into the springs, that jitter makes every frame advance by a
//! slightly different amount of time: micro-stutter, most visible at high
//! refresh rates where a frame is only ~7 ms. [`VsyncTimeline`] locks onto the
//! display's rhythm instead, so consecutive frames are exactly a whole number
//! of refresh periods apart.
//!
//! [`MotionStats`] measures how smoothly one island morph was drawn (for the
//! log and the CI smoke test).

/// Phase-locked vblank timeline.
#[derive(Clone, Copy, Debug)]
pub struct VsyncTimeline {
    /// Current refresh period estimate (seconds).
    period: f64,
    /// Period reported by the display mode (seconds); bounds the estimate.
    nominal: f64,
    /// The last vblank we locked onto (seconds, monotonic).
    last: Option<f64>,
    /// Score of wake-ups that don't fit the rhythm (far off the grid, or
    /// extra ones inside a period); fitting ones pay it back.
    off_rhythm: u32,
}

/// Wake-ups further than this many periods apart restart the lock.
const RESYNC_PERIODS: f64 = 30.0;
/// How strongly one wake-up pulls the locked phase towards it.
const PHASE_GAIN: f64 = 0.1;
/// How strongly one wake-up corrects the period estimate.
const PERIOD_GAIN: f64 = 0.02;
/// An [`VsyncTimeline::off_rhythm`] score this high means: re-read the
/// display's rate. (A steady display with 3 ms of wake-up jitter at 144 Hz
/// stays near 10; locked at 60 Hz on a 144 Hz display it passes 20 within
/// about a second.)
pub const OFF_RHYTHM_LIMIT: u32 = 20;

impl VsyncTimeline {
    /// `period` in seconds (e.g. 1/144). Non-finite or silly values fall back to 60 Hz.
    pub fn new(period: f64) -> Self {
        let p = sane_period(period);
        Self { period: p, nominal: p, last: None, off_rhythm: 0 }
    }

    pub fn period(&self) -> f64 {
        self.period
    }

    /// The display mode changed (another monitor, a new refresh rate).
    pub fn set_nominal_period(&mut self, period: f64) {
        let p = sane_period(period);
        if (p - self.nominal).abs() > 1e-6 {
            self.nominal = p;
            self.period = p;
            self.last = None;
        }
        self.off_rhythm = 0;
    }

    /// Forget the phase (the clock was parked); the next wake-up starts afresh.
    pub fn reset(&mut self) {
        self.last = None;
        self.off_rhythm = 0;
    }

    /// How badly recent wake-ups miss the rhythm. A sustained high score
    /// means the display's rate changed under us (60 <-> 144 Hz).
    pub fn off_rhythm(&self) -> u32 {
        self.off_rhythm
    }

    /// Feed the time a vblank wait returned. Returns the locked timestamp and
    /// how many refresh periods passed since the previous one (0 = a spurious
    /// second wake-up within the same period: skip it).
    pub fn lock(&mut self, raw: f64) -> (f64, u32) {
        let Some(last) = self.last else {
            self.last = Some(raw);
            return (raw, 1);
        };
        let elapsed = raw - last;
        if !(elapsed.is_finite()) || elapsed < 0.0 || elapsed > RESYNC_PERIODS * self.period {
            self.last = Some(raw);
            return (raw, 1);
        }
        // Wake-ups only ever come late, never early: one that is up to 3/4
        // of a period late still belongs to the next vblank (rounding would
        // count it as two and then drop the next, on-time one as spurious).
        let n = (elapsed / self.period + 0.25).floor();
        if n < 1.0 {
            self.off_rhythm += 1;
            return (last, 0);
        }
        let predicted = last + n * self.period;
        let err = raw - predicted;
        self.off_rhythm = if err.abs() > 0.3 * self.period { self.off_rhythm + 1 } else { self.off_rhythm.saturating_sub(1) };
        // Track slow drift (a 59.94 Hz display reporting 60, clock skew)
        // without passing the per-frame jitter on.
        self.period = (self.period + PERIOD_GAIN * err / n).clamp(self.nominal * 0.97, self.nominal * 1.03);
        let locked = predicted + PHASE_GAIN * err;
        self.last = Some(locked);
        (locked, n as u32)
    }
}

fn sane_period(p: f64) -> f64 {
    if p.is_finite() && (1.0 / 500.0..=1.0 / 20.0).contains(&p) {
        p
    } else {
        1.0 / 60.0
    }
}

/// Refresh rate (Hz) to frame period, treating 0/1 ("hardware default") as 60 Hz.
pub fn period_for_hz(hz: u32) -> f64 {
    if hz >= 20 {
        1.0 / hz as f64
    } else {
        1.0 / 60.0
    }
}

/// Present ambient motion (an equalizer, a drifting progress bar) on every
/// n-th refresh so it runs near `target_fps` without being an uneven
/// cadence: 144 Hz at 60 fps -> every 2nd frame (72 fps), 60 Hz -> every
/// frame. Pass the display mode's rate, not a measured one: a 59.94 or
/// 60.0001 Hz estimate must not flip between two divisors.
pub fn ambient_divisor(refresh_hz: f64, target_fps: f64) -> u32 {
    if !(refresh_hz.is_finite() && target_fps.is_finite()) || target_fps <= 0.0 {
        return 1;
    }
    ((refresh_hz / target_fps + 0.05).floor() as u32).max(1)
}

/// How smoothly one motion (an island morph) was drawn.
#[derive(Clone, Debug)]
pub struct MotionStats {
    pub name: &'static str,
    period: f64,
    start: f64,
    last: f64,
    pub frames: u32,
    /// Gaps between drawn frames longer than 1.5 refresh periods.
    pub hitches: u32,
    /// Longest gap between two drawn frames (seconds).
    pub worst_gap: f64,
    /// Animation time the first frame advanced (seconds).
    pub first_step: f64,
}

impl MotionStats {
    pub fn begin(name: &'static str, t: f64, period: f64, first_step: f64) -> Self {
        Self { name, period, start: t, last: t, frames: 1, hitches: 0, worst_gap: 0.0, first_step }
    }

    pub fn frame(&mut self, t: f64) {
        let gap = t - self.last;
        if gap > 1.5 * self.period {
            self.hitches += 1;
        }
        self.worst_gap = self.worst_gap.max(gap);
        self.last = t;
        self.frames += 1;
    }

    pub fn duration(&self) -> f64 {
        self.last - self.start
    }

    /// Mean frames per second over the motion.
    pub fn fps(&self) -> f64 {
        let d = self.duration();
        if d > 0.0 {
            (self.frames.saturating_sub(1)) as f64 / d
        } else {
            0.0
        }
    }

    /// One log line, e.g. `motion expand: 44 frames in 302 ms (143.7 fps at 144 Hz), 0 hitches, worst gap 7.1 ms, first step 6.9 ms`.
    pub fn summary(&self) -> String {
        format!(
            "motion {}: {} frames in {:.0} ms ({:.1} fps at {:.0} Hz), {} hitches, worst gap {:.1} ms, first step {:.1} ms",
            self.name,
            self.frames,
            self.duration() * 1000.0,
            self.fps(),
            1.0 / self.period,
            self.hitches,
            self.worst_gap * 1000.0,
            self.first_step * 1000.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random jitter in 0..1.
    fn jitter(i: u64) -> f64 {
        let x = i.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((x >> 33) % 10_000) as f64 / 10_000.0
    }

    #[test]
    fn jittered_wakeups_become_even_steps() {
        let p = 1.0 / 144.0;
        let mut tl = VsyncTimeline::new(p);
        let mut prev = None;
        for i in 0..2000u64 {
            // Wake 0.1..0.9 ms after each vblank.
            let raw = 10.0 + i as f64 * p + 0.0001 + 0.0008 * jitter(i);
            let (t, n) = tl.lock(raw);
            if i > 0 {
                assert_eq!(n, 1, "frame {i}");
            }
            if let (Some(pt), true) = (prev, i > 200) {
                let step: f64 = t - pt;
                assert!((step - p).abs() < 0.00012, "step {step} vs period {p} at {i}");
            }
            prev = Some(t);
        }
        assert!((tl.period() - p).abs() < 1e-5, "period drifted to {}", tl.period());
        assert!(tl.off_rhythm() < 5, "steady display scored {} off rhythm", tl.off_rhythm());
    }

    #[test]
    fn missed_vblanks_count_whole_periods() {
        let p = 1.0 / 60.0;
        let mut tl = VsyncTimeline::new(p);
        tl.lock(1.0);
        let (_, n) = tl.lock(1.0 + p + 0.0002);
        assert_eq!(n, 1);
        let (t, n) = tl.lock(1.0 + 3.0 * p + 0.0003);
        assert_eq!(n, 2);
        assert!((t - (1.0 + 3.0 * p)).abs() < 0.0005);
    }

    #[test]
    fn a_late_wakeup_counts_once() {
        let p = 1.0 / 144.0;
        let mut tl = VsyncTimeline::new(p);
        tl.lock(1.0);
        // 0.6 of a period late (a busy scheduler), then the next one on time.
        let (_, n) = tl.lock(1.0 + 1.6 * p);
        assert_eq!(n, 1);
        let (_, n) = tl.lock(1.0 + 2.0 * p + 0.0001);
        assert_eq!(n, 1);
        let (_, n) = tl.lock(1.0 + 3.0 * p + 0.0001);
        assert_eq!(n, 1);
    }

    #[test]
    fn a_rate_change_shows_as_off_rhythm() {
        // Locked at 60 Hz, the display switches to 144 Hz.
        let mut tl = VsyncTimeline::new(1.0 / 60.0);
        for i in 0..10 {
            tl.lock(i as f64 / 60.0);
        }
        assert_eq!(tl.off_rhythm(), 0);
        let t0 = 10.0 / 60.0;
        for i in 1..=144 {
            tl.lock(t0 + i as f64 / 144.0);
        }
        assert!(tl.off_rhythm() >= OFF_RHYTHM_LIMIT, "off rhythm {}", tl.off_rhythm());
        tl.set_nominal_period(1.0 / 144.0);
        assert_eq!(tl.off_rhythm(), 0);
    }

    #[test]
    fn spurious_early_wakeup_is_skipped() {
        let p = 1.0 / 120.0;
        let mut tl = VsyncTimeline::new(p);
        tl.lock(5.0);
        let (t, n) = tl.lock(5.0 + 0.2 * p);
        assert_eq!(n, 0);
        assert_eq!(t, 5.0);
        let (_, n) = tl.lock(5.0 + p);
        assert_eq!(n, 1);
    }

    #[test]
    fn long_gap_or_reset_resyncs() {
        let p = 1.0 / 60.0;
        let mut tl = VsyncTimeline::new(p);
        tl.lock(1.0);
        let (t, n) = tl.lock(9.0);
        assert_eq!((t, n), (9.0, 1));
        tl.reset();
        let (t, n) = tl.lock(9.5);
        assert_eq!((t, n), (9.5, 1));
    }

    #[test]
    fn tracks_a_slightly_slower_display() {
        // Reports 60 Hz, really 59.94 Hz.
        let real = 1.0 / 59.94;
        let mut tl = VsyncTimeline::new(1.0 / 60.0);
        for i in 0..3000u64 {
            let (_, n) = tl.lock(i as f64 * real + 0.0003 * jitter(i));
            assert!(i == 0 || n == 1, "frame {i} counted {n} periods");
        }
        // Within the noise of the jitter (0.3 ms), far from the 60 Hz it reported.
        assert!((tl.period() - real).abs() < 1e-5, "period {} vs {real}", tl.period());
    }

    #[test]
    fn insane_periods_fall_back_to_60_hz() {
        assert_eq!(VsyncTimeline::new(f64::NAN).period(), 1.0 / 60.0);
        assert_eq!(VsyncTimeline::new(0.0).period(), 1.0 / 60.0);
        assert_eq!(period_for_hz(1), 1.0 / 60.0);
        assert_eq!(period_for_hz(144), 1.0 / 144.0);
    }

    #[test]
    fn ambient_divisors() {
        assert_eq!(ambient_divisor(144.0, 60.0), 2);
        assert_eq!(ambient_divisor(120.0, 60.0), 2);
        assert_eq!(ambient_divisor(60.0, 60.0), 1);
        assert_eq!(ambient_divisor(59.94, 60.0), 1);
        assert_eq!(ambient_divisor(240.0, 60.0), 4);
        assert_eq!(ambient_divisor(144.0, 30.0), 4);
        assert_eq!(ambient_divisor(f64::NAN, 60.0), 1);
        // Measured or fractional rates don't flip between two divisors.
        assert_eq!(ambient_divisor(59.94, 30.0), 2);
        assert_eq!(ambient_divisor(59.0, 30.0), 1 + 1);
        assert_eq!(ambient_divisor(60.0001, 30.0), 2);
        assert_eq!(ambient_divisor(119.88, 60.0), 2);
        assert_eq!(ambient_divisor(239.76, 60.0), 4);
        assert_eq!(ambient_divisor(165.0, 60.0), 2);
    }

    #[test]
    fn motion_stats_count_hitches() {
        let p = 1.0 / 144.0;
        let mut m = MotionStats::begin("expand", 0.0, p, p);
        for i in 1..=40 {
            m.frame(i as f64 * p);
        }
        m.frame(42.0 * p); // one missed vblank
        assert_eq!(m.frames, 42);
        assert_eq!(m.hitches, 1);
        assert!((m.worst_gap - 2.0 * p).abs() < 1e-9);
        assert!((m.fps() - 41.0 / (42.0 * p)).abs() < 1e-6);
        assert!(m.summary().starts_with("motion expand: 42 frames"));
    }
}
