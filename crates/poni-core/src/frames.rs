//! A game's frame rate, from the frames it hands to Windows.
//!
//! `poni::services::frames` reads Windows' own log of each Present call
//! (event tracing, the way PresentMon and Xbox Game Bar measure it; nothing
//! is loaded into the game) and passes every frame of the game's process
//! here: where it was logged and when. Nothing in this module touches the OS.
//!
//! One frame can be logged more than once. The runtime the game draws with
//! logs its Present call (DXGI for Direct3D 10 to 12, or Direct3D 9), and the
//! graphics kernel logs the same frame again on its way to the screen. A
//! game can also present into more than one swap chain (a launcher, a video
//! behind the menu), and Vulkan and OpenGL games show up only in the
//! kernel's events. So frames are counted per [`Source`], and the rate is the
//! busiest runtime source's; the kernel's events count only when the game
//! made no runtime call lately (Vulkan, OpenGL).

use std::collections::{HashMap, VecDeque};

/// Where a frame was logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A DXGI Present call (Direct3D 10, 11 and 12): the swap chain's address.
    Dxgi(u64),
    /// A Direct3D 9 Present call: the swap chain's address.
    D3d9(u64),
    /// One kind of graphics kernel event (its event id): flips, blits and
    /// the frames handed to the compositor.
    Kernel(u16),
}

impl Source {
    fn runtime(self) -> bool {
        !matches!(self, Source::Kernel(_))
    }
}

/// The rate is measured over the last second of frames.
pub const WINDOW_S: f64 = 1.0;
/// Frames are kept this long, so a second's worth is there however the
/// trace's buffers arrive.
const KEEP_S: f64 = 2.0;
/// No frame for this long: the game stopped drawing (paused in the
/// background, minimised, between levels).
pub const STALE_S: f64 = 2.0;
/// At most this many frames per source (2 s at 1000 fps).
const MAX_KEEP: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRate {
    pub fps: f32,
    /// The average time from one frame to the next (ms).
    pub frame_ms: f32,
    /// The longest time between two frames in the second (ms): a hitch shows here.
    pub worst_ms: f32,
}

/// One process's frames, by source.
#[derive(Clone, Debug)]
pub struct FrameCounter {
    /// Clock ticks per second of the timestamps.
    freq: i64,
    sources: HashMap<Source, VecDeque<i64>>,
}

impl FrameCounter {
    pub fn new(freq: i64) -> Self {
        Self { freq: freq.max(1), sources: HashMap::new() }
    }

    pub fn clear(&mut self) {
        self.sources.clear();
    }

    /// A frame logged at `t` (clock ticks). The trace hands over a buffer at
    /// a time, so a frame may come in a little after a later one.
    pub fn push(&mut self, src: Source, t: i64) {
        let keep = self.ticks(KEEP_S);
        let q = self.sources.entry(src).or_default();
        let at = q.iter().rposition(|&x| x <= t).map_or(0, |p| p + 1);
        q.insert(at, t);
        let newest = q.back().copied().unwrap_or(t);
        while q.len() > MAX_KEEP || q.front().is_some_and(|&x| newest - x > keep) {
            q.pop_front();
        }
    }

    /// The frame rate at `now` (the frames' clock): the busiest runtime
    /// source, else the busiest kernel one. None before two frames, and
    /// once the game has stopped drawing.
    pub fn rate(&self, now: i64) -> Option<FrameRate> {
        let stale = self.ticks(STALE_S);
        let window = self.ticks(WINDOW_S);
        let best = |runtime: bool| {
            self.sources
                .iter()
                .filter(|(s, q)| s.runtime() == runtime && q.back().is_some_and(|&t| now - t <= stale))
                .filter_map(|(_, q)| window_rate(q, window, self.freq))
                .max_by(|a, b| a.fps.total_cmp(&b.fps))
        };
        best(true).or_else(|| best(false))
    }

    fn ticks(&self, s: f64) -> i64 {
        (s * self.freq as f64) as i64
    }
}

/// The rate over the last `window` ticks of one source's frames.
fn window_rate(q: &VecDeque<i64>, window: i64, freq: i64) -> Option<FrameRate> {
    let last = *q.back()?;
    let start = q.iter().rposition(|&t| last - t > window).map_or(0, |p| p + 1);
    let first = *q.get(start)?;
    let n = q.len() - start;
    if n < 2 || last <= first {
        return None;
    }
    let span = (last - first) as f64 / freq as f64;
    let fps = (n - 1) as f64 / span;
    let worst = q.range(start..).zip(q.range(start + 1..)).map(|(a, b)| b - a).max().unwrap_or(0);
    Some(FrameRate { fps: fps as f32, frame_ms: (1000.0 / fps) as f32, worst_ms: (worst as f64 * 1000.0 / freq as f64) as f32 })
}

/// A sharp drop in the frame rate, worth showing the readout for: this
/// second's rate is under 70 % of the typical rate before it (the median of
/// up to the last 30 seconds) and at least 15 fps lower. Needs 10 seconds of
/// rates first; seconds without one (NaN) don't count.
pub fn sharp_drop(before: &[f32], now: f32) -> bool {
    let mut v: Vec<f32> = before.iter().rev().filter(|x| x.is_finite() && **x > 0.0).take(30).copied().collect();
    if v.len() < 10 || !now.is_finite() {
        return false;
    }
    v.sort_by(f32::total_cmp);
    let typical = v[v.len() / 2];
    now < typical * 0.7 && typical - now >= 15.0
}

/// "144": whole frames per second (the readout writes "fps" beside it).
pub fn fmt_fps(fps: f32) -> String {
    format!("{}", fps.round().clamp(0.0, 9999.0) as u32)
}

/// "6.9 ms", "16 ms": frame time with a decimal only while it's short.
pub fn fmt_frame_ms(ms: f32) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{:.0} ms", ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: i64 = 10_000_000; // 100 ns ticks, as Windows' clocks often run

    /// `n` frames `step_ms` apart from `t0` (ticks), from one source.
    fn frames(c: &mut FrameCounter, src: Source, t0: i64, n: usize, step_ms: f64) -> i64 {
        let step = (step_ms * F as f64 / 1000.0) as i64;
        for i in 0..n {
            c.push(src, t0 + i as i64 * step);
        }
        t0 + (n as i64 - 1) * step
    }

    #[test]
    fn steady_frames_give_their_rate() {
        let mut c = FrameCounter::new(F);
        let last = frames(&mut c, Source::Dxgi(1), 0, 300, 1000.0 / 144.0);
        let r = c.rate(last).unwrap();
        assert!((r.fps - 144.0).abs() < 0.5, "{r:?}");
        assert!((r.frame_ms - 6.94).abs() < 0.05, "{r:?}");
        assert!((r.worst_ms - 6.94).abs() < 0.05, "{r:?}");
        assert_eq!(fmt_fps(r.fps), "144");
    }

    #[test]
    fn a_hitch_shows_as_the_worst_frame() {
        let mut c = FrameCounter::new(F);
        let t = frames(&mut c, Source::Dxgi(1), 0, 60, 1000.0 / 60.0);
        // One frame took 100 ms, then 60 fps again.
        let last = frames(&mut c, Source::Dxgi(1), t + F / 10, 20, 1000.0 / 60.0);
        let r = c.rate(last).unwrap();
        assert!((r.worst_ms - 100.0).abs() < 0.5, "{r:?}");
        assert!(r.fps < 60.0 && r.fps > 50.0, "{r:?}");
    }

    #[test]
    fn the_kernel_copy_of_a_frame_doesnt_count_twice() {
        // Each Present is logged by DXGI and again by the kernel (as a flip
        // and as the frame handed to the compositor): still 60 fps.
        let mut c = FrameCounter::new(F);
        let step = F / 60;
        for i in 0..120 {
            let t = i * step;
            c.push(Source::Dxgi(7), t);
            c.push(Source::Kernel(0xa8), t + 10);
            c.push(Source::Kernel(0xd7), t + 20);
        }
        let r = c.rate(119 * step + 20).unwrap();
        assert!((r.fps - 60.0).abs() < 0.5, "{r:?}");
    }

    #[test]
    fn the_busiest_swap_chain_counts() {
        // A game drawing at 120 fps and a 30 fps video in the same process.
        let mut c = FrameCounter::new(F);
        let a = frames(&mut c, Source::Dxgi(1), 0, 240, 1000.0 / 120.0);
        frames(&mut c, Source::Dxgi(2), 0, 60, 1000.0 / 30.0);
        let r = c.rate(a).unwrap();
        assert!((r.fps - 120.0).abs() < 0.5, "{r:?}");
    }

    #[test]
    fn vulkan_and_opengl_count_from_the_kernel() {
        let mut c = FrameCounter::new(F);
        let last = frames(&mut c, Source::Kernel(0xa8), 0, 200, 1000.0 / 90.0);
        let r = c.rate(last).unwrap();
        assert!((r.fps - 90.0).abs() < 0.5, "{r:?}");
        // Direct3D 9 is a runtime too: it wins over the kernel's count.
        let last = frames(&mut c, Source::D3d9(3), 0, 100, 1000.0 / 50.0);
        let r = c.rate(last).unwrap();
        assert!((r.fps - 50.0).abs() < 0.5, "{r:?}");
    }

    #[test]
    fn a_game_that_stopped_drawing_has_no_rate() {
        let mut c = FrameCounter::new(F);
        let last = frames(&mut c, Source::Dxgi(1), 0, 100, 10.0);
        assert!(c.rate(last + F).is_some());
        assert!(c.rate(last + 3 * F).is_none());
        // A stale runtime source lets the kernel's count through (a game
        // that moved from Direct3D to Vulkan).
        let k = frames(&mut c, Source::Kernel(0xfc), last + 3 * F, 100, 10.0);
        assert!((c.rate(k).unwrap().fps - 100.0).abs() < 0.5);
        c.clear();
        assert!(c.rate(k).is_none());
    }

    #[test]
    fn needs_two_frames_and_keeps_two_seconds() {
        let mut c = FrameCounter::new(F);
        c.push(Source::Dxgi(1), 5);
        assert!(c.rate(5).is_none());
        // Ten seconds at 100 fps keeps only the last two seconds.
        let last = frames(&mut c, Source::Dxgi(1), 10, 1000, 10.0);
        let kept = c.sources[&Source::Dxgi(1)].len();
        assert!((200..=202).contains(&kept), "{kept}");
        assert!((c.rate(last).unwrap().fps - 100.0).abs() < 0.5);
    }

    #[test]
    fn frames_arriving_late_go_in_order() {
        let mut c = FrameCounter::new(F);
        let step = F / 100;
        // Two buffers handed over out of order.
        for i in 50..100 {
            c.push(Source::Dxgi(1), i * step);
        }
        for i in 0..50 {
            c.push(Source::Dxgi(1), i * step);
        }
        let r = c.rate(99 * step).unwrap();
        assert!((r.fps - 100.0).abs() < 0.5, "{r:?}");
        assert!((r.worst_ms - 10.0).abs() < 0.1, "{r:?}");
    }

    #[test]
    fn a_sharp_drop_shows_the_readout() {
        let steady = vec![144.0f32; 30];
        assert!(sharp_drop(&steady, 90.0));
        assert!(!sharp_drop(&steady, 130.0));
        // A 30 fps game losing 10 fps isn't sharp enough; 60 to 40 is.
        assert!(!sharp_drop(&[30.0; 20], 20.0));
        assert!(sharp_drop(&[60.0; 20], 40.0));
        // Too little history, and seconds without a rate, don't count.
        assert!(!sharp_drop(&[144.0; 9], 60.0));
        let mut gaps = vec![f32::NAN; 40];
        gaps.extend([144.0f32; 12]);
        assert!(sharp_drop(&gaps, 60.0));
        assert!(!sharp_drop(&steady, f32::NAN));
        // The typical rate is the median: one old spike or dip doesn't move it.
        let mut bumpy = vec![144.0f32; 29];
        bumpy.push(400.0);
        assert!(!sharp_drop(&bumpy, 120.0));
        // Only the last 30 seconds count: a slower minute ago doesn't hide a drop now.
        let mut older = vec![60.0f32; 30];
        older.extend([144.0f32; 30]);
        assert!(sharp_drop(&older, 90.0));
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_fps(59.6), "60");
        assert_eq!(fmt_fps(0.2), "0");
        assert_eq!(fmt_frame_ms(6.944), "6.9 ms");
        assert_eq!(fmt_frame_ms(16.67), "17 ms");
    }
}
