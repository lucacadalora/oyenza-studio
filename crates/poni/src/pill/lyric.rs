//! The line being sung, for the collapsed music pill: which synced lyric
//! line is current, when the next one starts (a one-shot wake, never a
//! poll), and drawing it with a short cross-fade when it changes.
//! [`super::media`] calls [`draw`] where the title would go and falls back
//! to the title when it returns `false`. Classic, Oyen and Glass sing the
//! line the same way while a song has synced lyrics ([`sings`],
//! `activities::draw_music`); their pill stays compact otherwise.
//!
//! The line is the Music page's (`pages/media.rs`): the player's position,
//! plus its known lag (`lyrics_lead_s`) and the person's own shift
//! (`lyrics_offset_ms`), on a [`LyricClock`] that rides out jumpy reports,
//! picked a moment early ([`LYRIC_LEAD_S`]). The page's clock runs only while
//! the page draws, so the pill keeps its own. Without the equalizer's frames
//! the pill draws about once a line, too seldom for the clock to smooth
//! anything, so it also never steps back a line for a report that lands a
//! moment behind ([`steady`]).
//!
//! The title shows instead (no line, `false`) in the intro before a late
//! first line and in instrumental breaks (the page's • • • rows), for songs
//! without synced lyrics, with lyrics switched off, and once a pause has
//! lasted [`PAUSE_HOLD_S`].
//!
//! Readings (`LyricsState::ruby`: romaji under Japanese, Korean and Chinese)
//! sing along under the line, as on the Music page: the line as it is
//! written, for everyone who reads the script, and under it, smaller and a
//! little dimmer, how it sounds in Latin letters, so anyone can sing it
//! (`romanize::reading`). A song with readings makes the pill two rows tall
//! for the whole song ([`reads`]), so it doesn't grow and shrink line by
//! line; a line that needs none (English in a Japanese song) sits in the
//! middle on its own. Readings run longer than their lines, so the pill
//! takes the wider of the two. Switching readings off
//! (`lyrics_pronunciation`) gives the pill back its one row.

use crate::gfx::{Canvas, TextStyle, Weight};
use crate::state::{AppState, LyricsStatus};
use crate::ui::Ui;
use poni_core::geom::{palette, Point, Rect};
use poni_core::lrc::{LyricLine, Lyrics};
use poni_core::nowplaying::lyrics_lead_s;
use poni_core::romanize;
use poni_core::timeline::LyricClock;
use std::cell::RefCell;

/// As on the Music page: a line turns current this early, so the change
/// lands as the line starts.
const LYRIC_LEAD_S: f64 = 0.12;
/// A pause this long before the first line (the page's intro rule) or after
/// a line (an empty line in the lyrics) gives the pill back to the title.
/// A shorter one keeps the line before it (before the first line: that
/// line), so the title doesn't flash up for a breath.
const GAP_MIN_S: f64 = 3.0;
/// A paused song keeps its line this long, so a short pause doesn't swap the
/// line for the title and back; then the title says what is paused.
const PAUSE_HOLD_S: f64 = 3.0;
/// A line change: the old line rises away and fades out by [`LEAVE_UNTIL`]
/// of the way, the new one rises into place from [`COME_FROM`], so the two
/// barely overlap and never read as one. Lines move up as on the Music page.
const FADE_S: f64 = 0.2;
const RISE: f32 = 4.0;
const LEAVE_UNTIL: f32 = 0.5;
const COME_FROM: f32 = 0.25;
/// A report this far before the line on screen is a seek back, not a
/// jumpy player.
const BACK_S: f64 = LyricClock::JUMP_S;
/// Wakes land this much after a change, so the frame they ask for shows it.
const WAKE_PAD_S: f64 = 0.01;
/// The pill counts as still on screen this long after it last drew, or
/// after the change it was waiting for when it last drew. A line that
/// changed meanwhile fades out only then (one hidden for a while just goes).
const AWAY_S: f64 = 0.5;
/// Narrower than this (the pill mid-morph), there is no room for a line.
const MIN_W: f32 = 20.0;
/// The line is laid out this much wider than its slot on each side: a slot
/// cut to the line's measured width can come out a hair short once laid out
/// (rounding to pixels), and the line would end in an ellipsis.
const SLACK: f32 = 1.0;
/// A line with its reading: the line's middle sits this far above the
/// pill's and the reading's this far below, in boxes this tall, so the two
/// read as one block in the two-row pill (`super::media::H_READS`).
const LINE_DY: f32 = -7.5;
const READING_DY: f32 = 8.5;
const LINE_BOX: f32 = 20.0;
const READING_BOX: f32 = 16.0;
/// The reading is this opaque, as under the Music page's lines.
const READING_ALPHA: f32 = 0.78;

/// The synced line being sung now and when it started (mono seconds), when
/// the song has synced lyrics and a line is current. The start is the app
/// clock's (`st.now`) and stays put for as long as the line is current.
/// (A MacBook's notch asks, to hang the singing pill below the camera.)
#[cfg(target_os = "macos")]
pub fn current(st: &AppState) -> Option<(String, f64)> {
    with_state(|s| {
        let p = s.follow.update(st)?;
        Some((line_text(st, p.line?)?.to_string(), p.started))
    })
    .flatten()
}

/// Draw the current line centred in `area`, its reading under it when it
/// has one, cross-fading when it changes. Returns how wide the line is (or
/// its reading, the wider of the two) for the pill's width; `None` = no line
/// to show now (draw the title instead).
///
/// `area` is the line's slot at the pill's settled size: a line laid out in
/// a narrower slot ends in an ellipsis. While the pill still grows or
/// shrinks to a new line, clip to the slot it has now, so the line opens
/// out with the pill instead of running over what sits beside it. A line
/// that takes over from the title (after a break, a pause, a new track's
/// title) is drawn at once: the music pill cross-fades that hand-off, and
/// dims the text while paused.
pub fn draw(ui: &mut Ui, st: &AppState, area: Rect) -> Option<f32> {
    with_state(|s| s.draw(ui, st, area)).flatten()
}

/// When the next line starts, for the app's one-shot wake. Also when an
/// instrumental break ends, and once while paused: when the pause has lasted
/// long enough for the title to come back. Nothing while the line can't
/// change.
pub fn next_wake(st: &AppState) -> Option<f64> {
    with_state(|s| s.follow.update(st).and_then(|p| p.wake)).flatten()
}

/// Whether the music pill sings this song: lyrics on, synced lyrics found
/// for this track, and a player that says how long it is. Playing or not,
/// and in the intro and the breaks too (the title shows then), so a pill
/// that sings keeps its size for the whole song.
pub fn sings(st: &AppState) -> bool {
    followed(st).is_some()
}

/// Whether the pill sings this song with readings under its lines (two
/// rows): it sings, readings are on, and some line has one. For the whole
/// song, as [`sings`] is.
pub fn reads(st: &AppState) -> bool {
    sings(st) && st.settings.lyrics_pronunciation && st.lyrics.ruby.iter().any(|p| p.iter().any(|r| !r.roman.is_empty()))
}

/// The lyrics the pill follows for the song playing now (see [`sings`]).
fn followed(st: &AppState) -> Option<&Lyrics> {
    let m = st.media.as_ref()?;
    let lyr = &st.lyrics;
    // A song without a known length has no position to follow.
    if !st.settings.lyrics || lyr.status != LyricsStatus::Found || lyr.track_key != m.track_key || !m.timeline.has_duration() {
        return None;
    }
    lyr.lyrics.as_ref().filter(|l| l.synced)
}

/// How [`draw`] sets the line (its colour aside), for sizing the pill to it.
pub fn style() -> TextStyle {
    TextStyle::body().weight(Weight::Semibold)
}

/// How [`draw`] sets a reading (its colour aside): the Music page's, at the
/// type scale's smallest size.
fn reading_style() -> TextStyle {
    TextStyle::caption().weight(Weight::Medium)
}

// ---------------------------------------------------------------- picking

/// What the pill shows during a stretch of a synced song: a line (an index
/// into `Lyrics::lines`) or a break (`None`), from `start` until `next`
/// (lyric seconds; `None` after the last line).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spot {
    line: Option<usize>,
    start: f64,
    next: Option<f64>,
}

fn has_text(l: &LyricLine) -> bool {
    !l.text.trim().is_empty()
}

/// The spot at lyric time `t` (`None` for lyrics without timings or words).
/// The Music page's rows, except that a pause shorter than [`GAP_MIN_S`]
/// stays with the line before it.
fn spot_at(l: &Lyrics, t: f64) -> Option<Spot> {
    if !l.synced {
        return None;
    }
    let lines = &l.lines;
    let first = lines.iter().position(has_text)?;
    let text_after = |i: usize| lines.iter().skip(i + 1).position(has_text).map(|k| i + 1 + k);
    // When the pause that starts at row `g` ends (at the next line with
    // words; never, after the last), and whether it is long enough to show.
    let pause = |g: usize| {
        let end = text_after(g).map(|j| lines[j].time_s);
        (end, end.is_none_or(|e| e - lines[g].time_s >= GAP_MIN_S))
    };
    // When line `i` gives way: at the next row, or at the line after a
    // pause too short to show.
    let ends = |i: usize| {
        let n = i + 1;
        let row = lines.get(n)?;
        if has_text(row) {
            return Some(row.time_s);
        }
        match pause(n) {
            (end, false) => end,
            (_, true) => Some(row.time_s),
        }
    };
    let line = |i: usize| Spot { line: Some(i), start: lines[i].time_s, next: ends(i) };
    match l.line_index_at(t) {
        Some(r) if r >= first => {
            if has_text(&lines[r]) {
                return Some(line(r));
            }
            // In a pause: from its first empty row, after line `g - 1`.
            let g = (first + 1..=r).rev().take_while(|&i| !has_text(&lines[i])).last().unwrap_or(r);
            Some(match pause(g) {
                (end, true) => Spot { line: None, start: lines[g].time_s, next: end },
                (_, false) => line(g - 1),
            })
        }
        // Before the first line: the intro, unless the line starts soon.
        _ if lines[first].time_s >= GAP_MIN_S => Some(Spot { line: None, start: 0.0, next: Some(lines[first].time_s) }),
        _ => Some(line(first)),
    }
}

/// `new`, unless it steps back to an earlier spot while the song is less
/// than [`BACK_S`] before the one on screen: players report late and
/// unevenly (Apple Music the most), and following such a report would flick
/// the line back and forth. A bigger step back is a seek and is taken.
fn steady(shown: Option<Spot>, new: Spot, t: f64) -> Spot {
    match shown {
        Some(s) if new.start < s.start && t > s.start - BACK_S => s,
        _ => new,
    }
}

/// Where the song stands for the pill at `st.now`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pick {
    /// The line being sung (an index into `Lyrics::lines`), `None` in a break.
    line: Option<usize>,
    /// When that line became current (app clock).
    started: f64,
    /// When the pill changes next on its own (app clock): the next line or
    /// break, or the end of a pause's hold.
    wake: Option<f64>,
}

/// The song as the pill follows it, kept between frames.
#[derive(Default)]
struct Follow {
    /// The lyrics followed: track and shape (lines, first and last timing),
    /// told apart the way the Music page does.
    sig: Option<(String, usize, u64, u64)>,
    clock: LyricClock,
    /// The `st.now` the clock was last read at and what it said (lyric
    /// seconds). Several calls a frame read it once.
    read: Option<(f64, f64)>,
    /// The spot on screen and when it became current (app clock).
    spot: Option<Spot>,
    started: f64,
    /// Counts the lyrics followed, so a line from other lyrics never fades
    /// out over the new ones.
    generation: u32,
}

fn line_bits(l: Option<&LyricLine>) -> u64 {
    l.map_or(0, |x| x.time_s.to_bits() ^ x.text.len() as u64)
}

impl Follow {
    fn update(&mut self, st: &AppState) -> Option<Pick> {
        let l = followed(st)?;
        let (m, lyr) = (st.media.as_ref()?, &st.lyrics);
        let now = st.now;
        let playing = m.is_playing();
        let hold_end = if playing { None } else { Some(st.media_paused_at? + PAUSE_HOLD_S) };
        if hold_end.is_some_and(|end| now >= end) {
            return None;
        }

        let shape = (l.lines.len(), line_bits(l.lines.first()), line_bits(l.lines.last()));
        if !self.sig.as_ref().is_some_and(|(k, n, a, b)| *k == lyr.track_key && (*n, *a, *b) == shape) {
            self.sig = Some((lyr.track_key.clone(), shape.0, shape.1, shape.2));
            self.clock.reset();
            self.read = None;
            self.spot = None;
            self.generation = self.generation.wrapping_add(1);
        }

        let tl = m.timeline;
        let rate = if tl.rate.is_finite() && tl.rate > 0.0 { tl.rate } else { 1.0 };
        // Lyric time `t` at app time `at`.
        let (at, t) = match self.read {
            // Asked again before the next frame (an event between frames): what this frame said.
            Some((at, t)) if now <= at => (at, t),
            _ => {
                let lead = lyrics_lead_s(&m.app_id) + f64::from(st.settings.lyrics_offset_ms) / 1000.0;
                let t = self.clock.tick(now, tl.position_at(now) + lead, playing, rate) + LYRIC_LEAD_S;
                self.read = Some((now, t));
                (now, t)
            }
        };
        let spot = steady(self.spot, spot_at(l, t)?, t);
        if self.spot != Some(spot) {
            self.spot = Some(spot);
            self.started = at - (t - spot.start).max(0.0) / rate;
        }
        let wake = match hold_end {
            // Paused: the line can't change, but the title comes back once the hold is over.
            Some(end) => spot.line.map(|_| end),
            None => spot.next.map(|n| at + (n - t).max(0.0) / rate + WAKE_PAD_S),
        };
        Some(Pick { line: spot.line, started: self.started, wake })
    }
}

fn line_text(st: &AppState, i: usize) -> Option<&str> {
    st.lyrics.lyrics.as_ref()?.lines.get(i).map(|l| l.text.trim())
}

/// Line `i`'s reading in Latin letters, when readings are on and it has one.
fn reading_at(st: &AppState, i: usize) -> Option<String> {
    let pieces = st.lyrics.ruby.get(i).filter(|_| st.settings.lyrics_pronunciation)?;
    romanize::reading(pieces)
}

/// Tells line `i`'s readings apart without building them: they change when
/// readings are switched on or off, or when the Japanese readings add-on
/// comes in and the song is read again.
fn reading_sig(st: &AppState, i: usize) -> usize {
    match st.lyrics.ruby.get(i) {
        Some(pieces) if st.settings.lyrics_pronunciation => pieces.iter().map(|p| p.roman.len() + 1).sum(),
        _ => 0,
    }
}

// ---------------------------------------------------------------- drawing

/// The line on screen, and the change that brought it.
struct Shown {
    line: usize,
    text: String,
    /// Its reading in Latin letters ([`reading_at`]), built once for the
    /// line, and what it was built from ([`reading_sig`]).
    reading: Option<String>,
    sig: usize,
    /// The lyrics it is from ([`Follow::generation`]).
    generation: u32,
    /// The width of the slot it was last drawn in: a line that leaves keeps
    /// its layout while the pill takes the next line's size.
    slot_w: f32,
    /// The line it replaced, with its reading (and its slot's width), rising
    /// away while this one comes in.
    leaving: Option<(String, Option<String>, f32)>,
    /// When the change began (app clock; long ago for a line that just showed).
    since: f64,
}

#[derive(Default)]
struct State {
    follow: Follow,
    shown: Option<Shown>,
    /// Until when (app clock) the pill counts as still on screen.
    seen_until: f64,
}

thread_local! {
    // The pill only ever draws on the UI thread.
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.try_with(|s| s.try_borrow_mut().ok().map(|mut s| f(&mut s))).ok().flatten()
}

/// Opacity and rise (DIPs, up is negative) of the line coming in and of the
/// one leaving, `k` (0..1) of the way through a change.
fn fade(k: f32) -> ((f32, f32), (f32, f32)) {
    let k = k.clamp(0.0, 1.0);
    // Ease-out: quick to start, gentle to land.
    let ease = |x: f32| 1.0 - (1.0 - x).powi(3);
    let come = ease(((k - COME_FROM) / (1.0 - COME_FROM)).clamp(0.0, 1.0));
    let gone = (k / LEAVE_UNTIL).min(1.0);
    ((come, RISE * (1.0 - come)), ((1.0 - gone) * (1.0 - gone), -RISE * ease(gone)))
}

impl State {
    /// Whether the pill has been on screen up to `now` (it drew on the frame
    /// before, or the change it was waiting for is due), noting this draw:
    /// the next one counts as on screen until the change this one waits for
    /// (`wake`), give or take [`AWAY_S`].
    fn seen(&mut self, now: f64, wake: Option<f64>) -> bool {
        let on = now <= self.seen_until;
        self.seen_until = wake.unwrap_or(now).max(now) + AWAY_S;
        on
    }

    /// The line on screen once line `i` (`text`, its readings told apart by
    /// `sig`, [`reading_sig`]) is current at `now`. A change from a line that
    /// was on screen until now, from the same lyrics, starts a fade between
    /// the two; any other line just shows (it takes over from the title, or
    /// from a pill that was away). `reading` builds the line's reading, once
    /// a line, and again when its readings change under it.
    fn show(&mut self, now: f64, on_screen: bool, i: usize, text: &str, sig: usize, reading: impl FnOnce() -> Option<String>) -> &mut Shown {
        let generation = self.follow.generation;
        let shown = match self.shown.take() {
            Some(mut s) if s.line == i && s.text == text => {
                // The same line, read again (readings switched, or the
                // Japanese readings add-on came in): new words under it, no fade.
                if s.sig != sig {
                    s.reading = reading();
                    s.sig = sig;
                }
                s
            }
            old => {
                let leaving = old.filter(|s| on_screen && s.generation == generation).map(|s| (s.text, s.reading, s.slot_w));
                let since = if leaving.is_some() { now } else { f64::NEG_INFINITY };
                Shown { line: i, text: text.to_string(), reading: reading(), sig, generation, slot_w: 0.0, leaving, since }
            }
        };
        self.shown.insert(shown)
    }

    fn draw(&mut self, ui: &mut Ui, st: &AppState, area: Rect) -> Option<f32> {
        let now = st.now;
        let pick = self.follow.update(st);
        let on_screen = self.seen(now, pick.and_then(|p| p.wake));
        let Some((i, text)) = pick.and_then(|p| p.line.and_then(|i| line_text(st, i).map(|t| (i, t)))) else {
            self.shown = None;
            return None;
        };
        let shown = self.show(now, on_screen, i, text, reading_sig(st, i), || reading_at(st, i));
        let line_w = ui.c.measure(&shown.text, &style(), 10_000.0).0;
        let reading_w = shown.reading.as_ref().map_or(0.0, |r| ui.c.measure(r, &reading_style(), 10_000.0).0);
        let w = line_w.max(reading_w);
        if area.w < MIN_W {
            return Some(w);
        }
        let area = area.inset(-SLACK, 0.0);
        shown.slot_w = area.w;
        // On battery saver a change is a plain swap: one frame a line, not a
        // fade's worth (the change repeats all song long).
        let k = if st.power.saver { 1.0 } else { ((now - shown.since) / FADE_S) as f32 };
        if k >= 1.0 {
            shown.leaving = None;
            words(ui.c, &shown.text, shown.reading.as_deref(), area);
            return Some(w);
        }
        // Moving lines glide between pixels; at rest they sit on whole pixels.
        ui.request_anim();
        let snap = ui.c.set_text_snap(false);
        let ((come_a, come_dy), (gone_a, gone_dy)) = fade(k);
        if let Some((old, old_reading, w)) = shown.leaving.as_ref().filter(|_| gone_a > 0.0) {
            let r = Rect::from_center(area.center(), *w, area.h).offset(0.0, gone_dy);
            ui.c.with_alpha(gone_a, |c| words(c, old, old_reading.as_deref(), r));
        }
        ui.c.with_alpha(come_a, |c| words(c, &shown.text, shown.reading.as_deref(), area.offset(0.0, come_dy)));
        ui.c.set_text_snap(snap);
        Some(w)
    }
}

/// Where a line and its reading go in `area`: a line alone in the middle,
/// or a line above the middle and its reading under it.
fn rows(area: Rect, reading: bool) -> (Rect, Option<Rect>) {
    if !reading {
        return (area, None);
    }
    let c = area.center();
    let line = Rect::from_center(Point::new(c.x, c.y + LINE_DY), area.w, LINE_BOX);
    (line, Some(Rect::from_center(Point::new(c.x, c.y + READING_DY), area.w, READING_BOX)))
}

/// A line centred in `area`, with its reading under it when it has one.
fn words(c: &mut Canvas, text: &str, reading: Option<&str>, area: Rect) {
    let (line, under) = rows(area, reading.is_some());
    c.text(text, line, &style().center().color(palette::TEXT));
    if let (Some(reading), Some(under)) = (reading, under) {
        c.text(reading, under, &reading_style().center().color(palette::TEXT.mul_alpha(READING_ALPHA)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{LyricsState, MediaSession};
    use poni_core::lrc::parse_lrc;
    use poni_core::romanize::Ruby;
    use poni_core::settings::Settings;
    use poni_core::store::Store;
    use poni_core::timeline::Timeline;

    // Original placeholder text, not real song lyrics: a late first line, a
    // long break, a pause too short to show, and a last line.
    const SONG: &str = "\
[00:10.00]Paper lanterns on the line
[00:14.00]Count the windows one by one
[00:18.00]
[00:30.00]Morning folds the map away
[00:31.50]
[00:32.50]Half a breath and on again
[00:40.00]Last light over the bay
";

    fn spot(line: Option<usize>, start: f64, next: Option<f64>) -> Spot {
        Spot { line, start, next }
    }

    fn song(lrc: &str) -> AppState {
        let mut st = AppState::new(Settings::default(), Store::default());
        let key = MediaSession::make_track_key("Paper Boats", "The Lanterns", "Harbor Lights");
        st.media = Some(MediaSession {
            app_id: "Spotify.exe".into(),
            app_name: "Spotify".into(),
            title: "Paper Boats".into(),
            artist: "The Lanterns".into(),
            album: "Harbor Lights".into(),
            art: None,
            art_color: None,
            timeline: Timeline::new(0.0, 200.0, 0.0, true),
            can_prev: true,
            can_next: true,
            can_play_pause: true,
            can_seek: true,
            track_key: key.clone(),
        });
        st.lyrics = LyricsState { track_key: key, status: LyricsStatus::Found, lyrics: Some(parse_lrc(lrc)), source: "Sample".into(), ruby: Vec::new(), wants_ja_readings: false };
        st
    }

    /// The player reports `pos` at `now`.
    fn report(st: &mut AppState, now: f64, pos: f64, playing: bool) {
        st.now = now;
        st.media.as_mut().unwrap().timeline = Timeline::new(pos, 200.0, now, playing);
    }

    #[test]
    fn picks_the_line_being_sung() {
        let l = parse_lrc(SONG);
        // The first line starts late: the intro shows the title until then.
        assert_eq!(spot_at(&l, 2.0), Some(spot(None, 0.0, Some(10.0))));
        assert_eq!(spot_at(&l, 9.99), Some(spot(None, 0.0, Some(10.0))));
        assert_eq!(spot_at(&l, 10.0), Some(spot(Some(0), 10.0, Some(14.0))));
        // A line followed by a long break ends where the break starts.
        assert_eq!(spot_at(&l, 16.0), Some(spot(Some(1), 14.0, Some(18.0))));
        assert_eq!(spot_at(&l, 50.0), Some(spot(Some(6), 40.0, None)));
        assert_eq!(spot_at(&l, 35.0), Some(spot(Some(5), 32.5, Some(40.0))));
    }

    #[test]
    fn long_breaks_show_the_title_and_short_ones_keep_the_line() {
        let l = parse_lrc(SONG);
        // 12 s of instrumental: the title, until the next line.
        assert_eq!(spot_at(&l, 18.0), Some(spot(None, 18.0, Some(30.0))));
        assert_eq!(spot_at(&l, 29.0), Some(spot(None, 18.0, Some(30.0))));
        // A 1 s pause: the line before it stays, through to the next line.
        let held = spot(Some(3), 30.0, Some(32.5));
        assert_eq!(spot_at(&l, 31.0), Some(held));
        assert_eq!(spot_at(&l, 32.0), Some(held));
        // Lyrics that end on an empty line end on a break.
        let l = parse_lrc("[00:10.00]Only line\n[00:20.00]\n");
        assert_eq!(spot_at(&l, 15.0), Some(spot(Some(0), 10.0, Some(20.0))));
        assert_eq!(spot_at(&l, 25.0), Some(spot(None, 20.0, None)));
        // Consecutive empty rows are one break.
        let l = parse_lrc("[00:10.00]One\n[00:12.00]\n[00:15.00]\n[00:20.00]Two\n");
        assert_eq!(spot_at(&l, 16.0), Some(spot(None, 12.0, Some(20.0))));
        assert_eq!(spot_at(&l, 11.0), Some(spot(Some(0), 10.0, Some(12.0))));
    }

    #[test]
    fn an_early_first_line_shows_from_the_start() {
        // As on the Music page: no intro when the first line comes within 3 s.
        let l = parse_lrc("[00:01.50]Straight in\n[00:05.00]Next\n");
        assert_eq!(spot_at(&l, 0.0), Some(spot(Some(0), 1.5, Some(5.0))));
        assert_eq!(spot_at(&l, 2.0), Some(spot(Some(0), 1.5, Some(5.0))));
        // An empty first row (LRCLIB's "[00:00.00]") counts as the intro.
        let l = parse_lrc("[00:00.00]\n[00:01.00]Soon\n[00:04.00]Then\n");
        assert_eq!(spot_at(&l, 0.5), Some(spot(Some(1), 1.0, Some(4.0))));
        let l = parse_lrc("[00:00.00]\n[00:05.00]Late\n");
        assert_eq!(spot_at(&l, 1.0), Some(spot(None, 0.0, Some(5.0))));
        // Two lines on one stamp: the second is the one sung (as on the page).
        let l = parse_lrc("[00:10.00]Under\n[00:10.00]Over\n[00:12.00]After\n");
        assert_eq!(spot_at(&l, 11.0), Some(spot(Some(1), 10.0, Some(12.0))));
    }

    #[test]
    fn nothing_to_follow_without_timed_words() {
        assert_eq!(spot_at(&parse_lrc("plain words\nno stamps\n"), 5.0), None);
        assert_eq!(spot_at(&parse_lrc("[00:01.00]\n[00:02.00]\n"), 5.0), None);
        assert_eq!(spot_at(&Lyrics::default(), 5.0), None);
    }

    #[test]
    fn a_jumpy_report_never_steps_the_line_back() {
        let line = spot(Some(1), 14.0, Some(18.0));
        let before = spot(Some(0), 10.0, Some(14.0));
        // Reported 0.6 s before the line on screen: it stays.
        assert_eq!(steady(Some(line), before, 13.4), line);
        // 1.5 s back: a seek, taken.
        assert_eq!(steady(Some(line), before, 12.5), before);
        // Forward is always taken.
        assert_eq!(steady(Some(before), line, 14.1), line);
        assert_eq!(steady(None, before, 10.5), before);

        // Through the app state: frames a second apart (no equalizer), as with wakes.
        let mut st = song(SONG);
        let mut f = Follow::default();
        report(&mut st, 100.0, 10.05, true);
        assert_eq!(f.update(&st).unwrap().line, Some(0));
        // The next report lands 0.45 s behind, before the first line: the line holds.
        report(&mut st, 101.0, 9.6, true);
        assert_eq!(f.update(&st).unwrap().line, Some(0));
        // A seek back to the intro.
        report(&mut st, 102.0, 4.0, true);
        assert_eq!(f.update(&st).unwrap().line, None);
    }

    #[test]
    fn the_players_lag_and_the_persons_shift_move_the_line() {
        // The first line (10 s) turns current 0.12 s early, as on the page...
        let at = |app: &str, offset_ms: i32, pos: f64| {
            let mut st = song(SONG);
            st.media.as_mut().unwrap().app_id = app.into();
            st.settings.lyrics_offset_ms = offset_ms;
            report(&mut st, 100.0, pos, true);
            Follow::default().update(&st).unwrap().line
        };
        assert_eq!(at("Spotify.exe", 0, 9.87), None);
        assert_eq!(at("Spotify.exe", 0, 9.89), Some(0));
        // ...0.45 s earlier still for Apple Music, which reports late...
        let apple = "AppleInc.AppleMusicWin_nzyj5cx40ttqa!App";
        assert_eq!(at(apple, 0, 9.42), None);
        assert_eq!(at(apple, 0, 9.44), Some(0));
        // ...and as the person shifted it (positive: earlier).
        assert_eq!(at("Spotify.exe", 500, 9.39), Some(0));
        assert_eq!(at("Spotify.exe", -1000, 10.87), None);
        assert_eq!(at("Spotify.exe", -1000, 10.89), Some(0));
    }

    #[test]
    fn wakes_once_a_line_when_the_next_one_starts() {
        let mut st = song(SONG);
        let mut f = Follow::default();
        // 9 s in: the first line starts in 0.88 s (10 - 9 - 0.12).
        report(&mut st, 100.0, 9.0, true);
        let p = f.update(&st).unwrap();
        assert_eq!(p.line, None);
        assert!((p.wake.unwrap() - (100.88 + WAKE_PAD_S)).abs() < 1e-9, "{:?}", p.wake);
        // Woken then: the first line, and the next wake at the second (14 s).
        st.now = p.wake.unwrap();
        let p = f.update(&st).unwrap();
        assert_eq!(p.line, Some(0));
        let next = 100.0 + (14.0 - 9.0 - LYRIC_LEAD_S) + WAKE_PAD_S;
        assert!((p.wake.unwrap() - next).abs() < 1e-6, "{:?} vs {next}", p.wake);
        // In the long break: woken when the line after it starts (30 s).
        report(&mut st, 200.0, 20.0, true);
        let p = f.update(&st).unwrap();
        assert_eq!(p.line, None);
        assert!((p.wake.unwrap() - (200.0 + 30.0 - 20.12 + WAKE_PAD_S)).abs() < 1e-6);
        // After the last line: no more wakes.
        report(&mut st, 300.0, 45.0, true);
        assert_eq!(f.update(&st).unwrap().wake, None);
        // At double speed the next line comes twice as soon.
        report(&mut st, 400.0, 9.0, true);
        st.media.as_mut().unwrap().timeline.rate = 2.0;
        let p = Follow::default().update(&st).unwrap();
        assert!((p.wake.unwrap() - (400.0 + 0.88 / 2.0 + WAKE_PAD_S)).abs() < 1e-9);
    }

    #[test]
    fn a_line_keeps_its_start_while_it_is_sung() {
        let mut st = song(SONG);
        let mut f = Follow::default();
        report(&mut st, 100.0, 11.0, true);
        let p = f.update(&st).unwrap();
        // 1.12 s into the first line (with the 0.12 s lead).
        assert!((p.started - 98.88).abs() < 1e-9);
        st.now = 101.5;
        assert_eq!(f.update(&st).unwrap().started, p.started);
        // Asked again before the next frame (an event): the same answer.
        st.now = 101.2;
        assert_eq!(f.update(&st), f.update(&st));
    }

    #[test]
    fn a_pause_holds_the_line_then_gives_it_back_to_the_title() {
        let mut st = song(SONG);
        let mut f = Follow::default();
        report(&mut st, 100.0, 12.0, true);
        assert_eq!(f.update(&st).unwrap().line, Some(0));
        // Paused at 101 s: the line holds, with one wake for when the hold ends.
        report(&mut st, 101.0, 13.0, false);
        st.media_paused_at = Some(101.0);
        let p = f.update(&st).unwrap();
        assert_eq!((p.line, p.wake), (Some(0), Some(101.0 + PAUSE_HOLD_S)));
        st.now = 101.0 + PAUSE_HOLD_S;
        assert_eq!(f.update(&st), None);
        // Playing again: the line is back.
        report(&mut st, 110.0, 13.0, true);
        st.media_paused_at = None;
        assert_eq!(f.update(&st).unwrap().line, Some(0));
        // Paused in a break: nothing to hold, no wake.
        report(&mut st, 120.0, 20.0, false);
        st.media_paused_at = Some(120.0);
        assert_eq!(f.update(&st).map(|p| (p.line, p.wake)), Some((None, None)));
        // A session that was never playing here: the title.
        st.media_paused_at = None;
        assert_eq!(f.update(&st), None);
    }

    #[test]
    fn only_synced_lyrics_for_this_song_when_lyrics_are_on() {
        let mut st = song(SONG);
        report(&mut st, 100.0, 12.0, true);
        assert!(Follow::default().update(&st).is_some());
        st.settings.lyrics = false;
        assert_eq!(Follow::default().update(&st), None);
        st.settings.lyrics = true;
        st.lyrics.status = LyricsStatus::Loading;
        assert_eq!(Follow::default().update(&st), None);
        st.lyrics.status = LyricsStatus::Found;
        st.lyrics.track_key = "another song".into();
        assert_eq!(Follow::default().update(&st), None);
        // Unsynced words, or a player that doesn't say how long the song is.
        let mut st = song("plain words\nno stamps\n");
        report(&mut st, 100.0, 12.0, true);
        assert_eq!(Follow::default().update(&st), None);
        let mut st = song(SONG);
        report(&mut st, 100.0, 12.0, true);
        st.media.as_mut().unwrap().timeline.duration_s = 0.0;
        assert_eq!(Follow::default().update(&st), None);
    }

    #[test]
    fn new_lyrics_start_over() {
        let mut st = song(SONG);
        let mut f = Follow::default();
        report(&mut st, 100.0, 35.0, true);
        assert_eq!(f.update(&st).unwrap().line, Some(5));
        // Another song whose first line is earlier than the line on screen:
        // not held back, it's not the same song.
        let other = song("[00:30.00]Another song\n[00:40.00]Its next line\n");
        st.lyrics = other.lyrics.clone();
        st.media.as_mut().unwrap().track_key = other.media.as_ref().unwrap().track_key.clone();
        report(&mut st, 101.0, 34.5, true);
        assert_eq!(f.update(&st).unwrap().line, Some(0));
    }

    #[test]
    fn only_a_line_that_was_on_screen_fades_out() {
        let mut s = State::default();
        // First draw: nothing was on screen before, so the line just shows.
        assert!(!s.seen(100.0, Some(103.0)));
        let first = s.show(100.0, false, 0, "First", 0, || None);
        assert_eq!((first.leaving.clone(), first.since), (None, f64::NEG_INFINITY));
        // The same line a frame later: nothing new.
        assert!(s.seen(100.016, Some(103.0)));
        assert_eq!(s.show(100.016, true, 0, "First", 0, || None).since, f64::NEG_INFINITY);
        // No frames until the wake for the next line (no equalizer): still on screen.
        assert!(s.seen(103.02, Some(106.0)));
        s.shown.as_mut().unwrap().slot_w = 180.0;
        let next = s.show(103.02, true, 1, "Second", 0, || None);
        // It leaves in the slot it had.
        assert_eq!((next.leaving.clone(), next.since), (Some(("First".to_string(), None, 180.0)), 103.02));
        // Back after a while away (the page was open): the old line just goes.
        assert!(!s.seen(120.0, Some(121.0)));
        let back = s.show(120.0, false, 5, "Sixth", 0, || None);
        assert_eq!((back.leaving.clone(), back.since), (None, f64::NEG_INFINITY));
        // Other lyrics (another song): never fade a line of the last one out over them.
        assert!(s.seen(120.016, None));
        s.follow.generation += 1;
        assert_eq!(s.show(120.016, true, 0, "Another song", 0, || None).leaving, None);
        // After the title (a break, drawn every frame): the line takes over at
        // once, the music pill fades the hand-off.
        s.shown = None;
        assert_eq!(s.show(130.0, true, 1, "After the break", 0, || None).since, f64::NEG_INFINITY);
    }

    /// The pieces of a line of [`SONG_JA`], as a reader gives them.
    fn pieces(parts: &[(&str, &str)]) -> Vec<Ruby> {
        parts.iter().map(|(text, roman)| Ruby { text: (*text).into(), roman: (*roman).into() }).collect()
    }

    // Original placeholder text, not real song lyrics: two Japanese lines
    // around one in English, which needs no reading.
    const SONG_JA: &str = "\
[00:10.00]紙の舟よ
[00:14.00]Paper boats
[00:18.00]光を見つける
";

    fn song_ja() -> AppState {
        let mut st = song(SONG_JA);
        st.lyrics.ruby = vec![
            pieces(&[("紙の", "kami no"), ("舟よ", "fune yo")]),
            Vec::new(),
            pieces(&[("光を", "hikari wo"), ("見つける", "mitsukeru")]),
        ];
        st
    }

    #[test]
    fn a_song_with_readings_sings_them_under_its_lines() {
        let mut st = song_ja();
        assert!(reads(&st));
        assert_eq!(reading_at(&st, 0).as_deref(), Some("kami no fune yo"));
        // The English line needs none: it sits in the middle on its own.
        assert_eq!(reading_at(&st, 1), None);
        assert_eq!(reading_at(&st, 2).as_deref(), Some("hikari wo mitsukeru"));
        // Readings switched off: one row, the lines as they are written.
        st.settings.lyrics_pronunciation = false;
        assert!(!reads(&st));
        assert_eq!(reading_at(&st, 0), None);
        assert_ne!(reading_sig(&st, 0), reading_sig(&song_ja(), 0));
        // A song with nothing to read, or one the pill doesn't sing.
        assert!(!reads(&song(SONG)));
        let mut st = song_ja();
        st.settings.lyrics = false;
        assert!(!reads(&st));
    }

    #[test]
    fn a_line_brings_its_reading_and_takes_it_along_when_it_leaves() {
        let st = song_ja();
        let mut s = State::default();
        assert!(!s.seen(100.0, Some(103.0)));
        let first = s.show(100.0, false, 0, "紙の舟よ", reading_sig(&st, 0), || reading_at(&st, 0));
        assert_eq!(first.reading.as_deref(), Some("kami no fune yo"));
        first.slot_w = 180.0;
        // The next line rises in while this one rises away, its reading with it.
        assert!(s.seen(103.0, Some(106.0)));
        let next = s.show(103.0, true, 2, "光を見つける", reading_sig(&st, 2), || reading_at(&st, 2));
        assert_eq!(next.reading.as_deref(), Some("hikari wo mitsukeru"));
        assert_eq!(next.leaving, Some(("紙の舟よ".to_string(), Some("kami no fune yo".to_string()), 180.0)));
    }

    #[test]
    fn readings_that_come_in_under_a_line_show_without_a_fade() {
        // Before the Japanese readings add-on: only the kana read.
        let mut st = song_ja();
        st.lyrics.ruby[2] = pieces(&[("光を", "wo"), ("見つける", "")]);
        let mut s = State::default();
        let mut built = 0;
        assert!(!s.seen(100.0, Some(103.0)));
        s.show(100.0, false, 2, "光を見つける", reading_sig(&st, 2), || {
            built += 1;
            reading_at(&st, 2)
        });
        assert_eq!(s.shown.as_ref().unwrap().reading.as_deref(), Some("wo 見つける"));
        // Frame after frame, the same readings: built once.
        for i in 1..5 {
            assert!(s.seen(100.0 + f64::from(i) * 0.016, Some(103.0)));
            s.show(100.0 + f64::from(i) * 0.016, true, 2, "光を見つける", reading_sig(&st, 2), || {
                built += 1;
                reading_at(&st, 2)
            });
        }
        assert_eq!(built, 1);
        // The add-on comes in and the song is read again: the same line, its new reading at once.
        let read = song_ja();
        assert!(s.seen(100.1, Some(103.0)));
        let now = s.show(100.1, true, 2, "光を見つける", reading_sig(&read, 2), || reading_at(&read, 2));
        assert_eq!((now.reading.as_deref(), now.leaving.clone(), now.since), (Some("hikari wo mitsukeru"), None, f64::NEG_INFINITY));
    }

    #[test]
    fn the_reading_sits_under_its_line_and_a_line_alone_in_the_middle() {
        let area = Rect::new(0.0, 0.0, 300.0, 46.0);
        assert_eq!(rows(area, false), (area, None));
        let (line, under) = rows(area, true);
        let under = under.unwrap();
        assert!(line.center().y < area.center().y && under.center().y > area.center().y);
        assert!(line.bottom() <= under.y + 4.0, "the boxes barely overlap: {line:?} over {under:?}");
        // Both inside the two-row pill, centred across it.
        assert!(line.y >= area.y && under.bottom() <= area.bottom());
        assert_eq!((line.center().x, under.center().x), (150.0, 150.0));
    }

    #[test]
    fn a_change_brings_the_new_line_up_as_the_old_one_leaves() {
        let ((a0, dy0), (g0, gy0)) = fade(0.0);
        assert_eq!((a0, dy0, g0, gy0), (0.0, RISE, 1.0, 0.0));
        let ((a1, dy1), (g1, gy1)) = fade(1.0);
        assert_eq!((a1, dy1, g1, gy1), (1.0, 0.0, 0.0, -RISE));
        // The old line is gone halfway; the new one starts a quarter of the way in.
        assert_eq!(fade(LEAVE_UNTIL).1 .0, 0.0);
        assert_eq!(fade(COME_FROM).0 .0, 0.0);
        let ((a, _), (g, _)) = fade(0.4);
        assert!(a < 0.6 && g < 0.1, "they barely overlap: {a} in, {g} out");
        let mut prev = fade(0.0);
        for i in 1..=100 {
            let f = fade(i as f32 / 100.0);
            assert!(f.0 .0 >= prev.0 .0 && f.0 .1 <= prev.0 .1, "the new line only comes in");
            assert!(f.1 .0 <= prev.1 .0 && f.1 .1 <= prev.1 .1, "the old line only leaves");
            prev = f;
        }
    }
}
