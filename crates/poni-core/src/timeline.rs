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

    /// Fill a blank report from what the same track showed before.
    ///
    /// Some players (Firefox and browsers built on it, such as Zen) now and
    /// then report a timeline with no length in the middle of a track, often
    /// with no position either. Taken as it is, the bar and the synced lyrics
    /// vanish until the next full report, which may not come before a pause
    /// and play. `shown` is the same track's last timeline: its length stays,
    /// and a report with no position either keeps counting from where
    /// `shown` is at `now`. A report with a length is returned as it is.
    pub fn or_length_of(self, shown: &Timeline, now: f64) -> Timeline {
        if self.has_duration() || !shown.has_duration() {
            return self;
        }
        let duration_s = shown.duration_s;
        if self.position_s > 0.0 {
            return Timeline { position_s: self.position_s.min(duration_s), duration_s, ..self };
        }
        Timeline { position_s: shown.position_at(now), duration_s, sampled_at: now, ..self }
    }
}

/// A seek the player hasn't confirmed yet.
///
/// Players report their position on their own schedule, so the first reports
/// after a seek can still be from before it; passed on, they would pull the
/// bar and lyrics back to where they were until a fresh report pushed them
/// forward again. Reports that disagree with the seek are held back until one
/// agrees or [`PendingSeek::CONFIRM_S`] passes (the seek didn't take: then the
/// player's word stands).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingSeek {
    target_s: f64,
    at: f64,
}

impl PendingSeek {
    /// How long a seek may go unconfirmed.
    pub const CONFIRM_S: f64 = 3.0;
    /// A report this close to where the seek should be by now confirms it
    /// (Apple Music reports about half a second behind, and unevenly).
    pub const TOLERANCE_S: f64 = 1.5;

    /// A seek to `target_s`, asked for at monotonic time `now`.
    pub fn new(target_s: f64, now: f64) -> Self {
        Self { target_s, at: now }
    }

    pub fn target_s(&self) -> f64 {
        self.target_s
    }

    /// The confirmation window is over.
    pub fn expired(&self, now: f64) -> bool {
        now - self.at >= Self::CONFIRM_S
    }

    /// Whether `report` (read at `now`) still predates the seek and should be
    /// held back.
    pub fn holds(&self, report: &Timeline, now: f64) -> bool {
        if self.expired(now) {
            return false;
        }
        let mut expected = self.target_s;
        if report.playing {
            let rate = if report.rate > 0.0 { report.rate } else { 1.0 };
            expected += (now - self.at).max(0.0) * rate;
        }
        if report.duration_s > 0.0 {
            expected = expected.min(report.duration_s);
        }
        (report.position_at(now) - expected).abs() > Self::TOLERANCE_S
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
    fn pending_seek_holds_stale_reports_only() {
        // At 100 s on the clock, a song at 30 s is dragged to 200 s.
        let seek = PendingSeek::new(200.0, 100.0);
        // The player's next report still says ~31 s: held.
        assert!(seek.holds(&Timeline::new(30.0, 260.0, 99.0, true), 100.5));
        // Its report from after the seek, half a second behind (Apple Music): passed.
        assert!(!seek.holds(&Timeline::new(200.7, 260.0, 101.2, true), 101.2));
        // Paused: the target doesn't move.
        assert!(!seek.holds(&Timeline::new(200.0, 260.0, 100.0, false), 102.0));
        assert!(seek.holds(&Timeline::new(30.0, 260.0, 100.0, false), 102.0));
        // A seek past the end lands on the end.
        let end = PendingSeek::new(300.0, 100.0);
        assert!(!end.holds(&Timeline::new(260.0, 260.0, 100.5, true), 100.5));
        // Unconfirmed for CONFIRM_S: the player's word stands again.
        assert!(seek.expired(100.0 + PendingSeek::CONFIRM_S));
        assert!(!seek.holds(&Timeline::new(33.0, 260.0, 103.0, true), 100.0 + PendingSeek::CONFIRM_S));
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

    #[test]
    fn blank_report_keeps_the_tracks_length_and_place() {
        // Shown: 72 s into a 212 s song at 100 s on the clock, playing.
        let shown = Timeline::new(72.0, 212.0, 100.0, true);
        // Zen sends no length and no position at 130 s: the song goes on at 102 s.
        let blank = Timeline::new(0.0, 0.0, 130.0, true);
        let t = blank.or_length_of(&shown, 130.0);
        assert!(t.has_duration());
        assert_eq!(t.duration_s, 212.0);
        assert!((t.position_at(130.0) - 102.0).abs() < 1e-9);
        assert!((t.position_at(131.0) - 103.0).abs() < 1e-9);
        // Paused in the same blank report: it stops where it is.
        let t = Timeline::new(0.0, 0.0, 130.0, false).or_length_of(&shown, 130.0);
        assert!(!t.playing);
        assert!((t.position_at(150.0) - 102.0).abs() < 1e-9);
    }

    #[test]
    fn report_with_a_position_but_no_length_keeps_its_position() {
        let shown = Timeline::new(72.0, 212.0, 100.0, true);
        let t = Timeline::new(95.0, 0.0, 120.0, true).or_length_of(&shown, 120.0);
        assert_eq!((t.position_s, t.duration_s, t.sampled_at), (95.0, 212.0, 120.0));
        // Never past the end.
        let t = Timeline::new(300.0, 0.0, 120.0, true).or_length_of(&shown, 120.0);
        assert_eq!(t.position_s, 212.0);
    }

    #[test]
    fn full_reports_and_lengthless_tracks_pass_unchanged() {
        let shown = Timeline::new(72.0, 212.0, 100.0, true);
        // A full report (a seek, the next song) wins as it is.
        let full = Timeline::new(5.0, 180.0, 120.0, true);
        assert_eq!(full.or_length_of(&shown, 120.0), full);
        // A stream that never had a length stays without one.
        let live = Timeline::new(0.0, 0.0, 100.0, true);
        let blank = Timeline::new(0.0, 0.0, 120.0, true);
        assert_eq!(blank.or_length_of(&live, 120.0), blank);
    }
}
