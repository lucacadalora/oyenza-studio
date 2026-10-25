//! Music page, the flagship screen: album art, track info, a seekable
//! progress bar, transport controls and time-synced lyrics.
//!
//! ```text
//! ┌──────────┐  Title                        │  earlier line
//! │          │  Artist                       │  previous line
//! │   art    │  • Player                     │  CURRENT LINE
//! │ 104×104  │  ━━━━━━━━━━━━━━━━━──────────  │  next line
//! └──────────┘  1:23                 −2:10   │  later line
//!                  ⏮      ( ▶ )      ⏭       │  (Lyrics: source, on hover)
//! ```
//!
//! The app lays pages out at the island's final size (and masks them with
//! the island's outline while it springs open), so nothing reflows here.
//! The art's top edge and the title's cap height share the content top.
//! All units are DIPs; static geometry is snapped to device pixels.
//!
//! Animation state that has no home in `PageUiState` (velocities, the last
//! frame time, a text-measurement cache) lives in a UI-thread `thread_local`.

use crate::commands::{Command, MediaCmd};
use crate::gfx::{ts, TextStyle, Weight};
use crate::icons::Icon;
use crate::state::{AppState, LyricsState, LyricsStatus, MediaSession, PageUiState};
use crate::ui::{wid, wid_n, Ui, WidgetId};
use poni_core::addons;
use poni_core::datetime::fmt_duration;
use poni_core::geom::{lerp, palette, Color, Point, Rect};
use poni_core::lrc::Lyrics;
use poni_core::nowplaying::lyrics_lead_s;
use poni_core::romanize::Ruby;
use poni_core::timeline::LyricClock;
use std::cell::Cell;
use std::collections::HashMap;

// ---------------------------------------------------------------- layout

/// Content height with a media session (the play button's bottom + air).
const H_SESSION: f32 = 148.0;
/// Content height of the "Nothing playing" state.
const H_EMPTY: f32 = 124.0;

const ART: f32 = 104.0;
const ART_RADIUS: f32 = 14.0;
/// The art's first ink is the content top, like every page.
const ART_TOP: f32 = 0.0;
/// Art shrinks a little while paused (animated).
pub(crate) const ART_PAUSED_SCALE: f32 = 0.94;
const GAP_ART: f32 = 16.0;
pub(crate) const GAP_LYRICS: f32 = 20.0;
pub(crate) const LYRICS_SHARE: f32 = 0.42;
const MIN_MID_W: f32 = 176.0;
pub(crate) const MIN_LYRICS_W: f32 = 110.0;

// Middle column rows (offsets from the content top). The title's text box
// starts above the top so its cap height (not its line box) meets the art's
// top edge; the header leaves room for that. Title, artist and player sit on
// a ~20 DIP rhythm; the bar + times group has about the same air (~14 DIPs
// of ink gap) above it and above the controls.
const TITLE_Y: f32 = -6.0;
const TITLE_H: f32 = 22.0;
const ARTIST_Y: f32 = 16.0;
const SOURCE_Y: f32 = 37.0;
const SOURCE_H: f32 = 18.0;
const BAR_Y: f32 = 66.0;
const BAR_H: f32 = 4.0;
const TIMES_Y: f32 = 75.0;
const TIMES_H: f32 = 14.0;
const CONTROLS_CY: f32 = 122.0;

const PLAY_D: f32 = 44.0;
const SIDE_BTN: f32 = 38.0;
const CONTROL_ICON: f32 = 20.0;

// ---------------------------------------------------------------- lyrics

const LYRIC_MIN: f32 = 13.0;
const LYRIC_MAX: f32 = 16.0;
const LYRIC_GAP: f32 = 12.0;
/// Uniform leading for lyric rows, so a wrapped line reads as one unit.
const LYRIC_LEADING: f32 = 1.2;
/// Pronunciation under Japanese / Korean / Chinese lines (Apple Music's
/// "Show Pronunciation" layout): each piece of the line with its reading set
/// small under it, pieces wrapping as whole units.
const ROMAN_SCALE: f32 = 0.72;
/// Pronounced lines are set a little smaller, so a line and its reading
/// usually fit one row and the lines around it stay in view.
const RUBY_SIZE: f32 = 0.9;
const ROMAN_ALPHA: f32 = 0.78;
const ROMAN_GAP: f32 = 1.0;
/// Space between pieces, as a fraction of the lyric size.
const PIECE_GAP: f32 = 0.42;
/// Extra space between wrapped lines of pieces.
const RUBY_LINE_GAP: f32 = 4.0;
/// Highlight a line slightly early so the scroll lands as the line starts.
const LYRIC_LEAD_S: f64 = 0.12;
/// Show "intro" dots before the first line when it starts this late.
const INTRO_MIN_S: f64 = 3.0;
/// Rows drawn on each side of the current one (2 visible + 1 entering).
const ROWS_ABOVE: usize = 3;
const ROWS_BELOW: usize = 4;
/// Longer jumps (seeks) land this close to the target instead of scrolling
/// through every line in between.
const MAX_SCROLL_ROWS: f32 = 3.0;
const JUMP_LAND_ROWS: f32 = 2.0;
const EDGE_FADE: f32 = 26.0;
const CREDIT_H: f32 = 14.0;
const CREDIT_GAP: f32 = 4.0;
/// The credit fades in while the pointer is over the lyrics column.
const CREDIT_FADE_S: f32 = 0.15;
/// The Japanese readings offer shrinks to fit the column, to no less than this
/// of the caption size (beyond that it's cut).
const OFFER_MIN_SCALE: f32 = 0.85;
/// Credited when a result doesn't name its source.
const DEFAULT_SOURCE: &str = "LRCLIB";

/// Where the shown lyrics come from.
pub(crate) fn lyrics_source(lyr: &LyricsState) -> &str {
    if lyr.source.is_empty() {
        DEFAULT_SOURCE
    } else {
        &lyr.source
    }
}

/// The site the credit opens.
pub(crate) fn source_url(source: &str) -> &'static str {
    match source {
        "Apple Music" => "https://music.apple.com",
        _ => "https://lrclib.net",
    }
}

const PLAIN_SIZE: f32 = ts::BODY;
const PLAIN_GAP: f32 = 5.0;
const STANZA_GAP: f32 = 6.0;
const PLAIN_MAX_LINES: usize = 400;
const SCROLL_STEP: f32 = 46.0;

// Critically damped follow rates (rad/s): settle time is roughly 4/ω.
const LYRIC_OMEGA: f32 = 12.0;
/// A lyric scroll that starts on a slow ambient frame (the clock runs near
/// 30 fps while the lyrics rest) first moves this much time, not the whole
/// gap since the last frame, so it doesn't open with a jump.
const START_STEP_S: f32 = 1.0 / 120.0;
const SCROLL_OMEGA: f32 = 18.0;
pub(crate) const ART_OMEGA: f32 = 14.0;

/// A gap this long since the page last drew means it was hidden meanwhile.
const RESUME_GAP_S: f64 = 0.5;
const MAX_DT_S: f64 = 0.25;
const MAX_CACHE: usize = 4096;

// ---------------------------------------------------------------- animation state

#[derive(Default)]
pub(crate) struct Anim {
    last_t: Option<f64>,
    /// Current art scale (0 = not initialised yet).
    pub(crate) art_scale: f32,
    pub(crate) art_vel: f32,
    lyric_vel: f32,
    /// Unsynced lyrics: wanted scroll offset (DIPs); `lyric_pos` follows it.
    scroll_target: f32,
    scroll_vel: f32,
    /// Lyrics credit opacity (0..1), following the pointer over the column.
    credit_k: f32,
    /// Steady playback clock for synced lyrics (eases over jumpy reports).
    pub(crate) clock: LyricClock,
    /// Identity of the lyrics shown last frame (track, synced, shape).
    sig: Option<Sig>,
    /// Column width and DPI scale the measurements below were made at.
    env: (u32, u32),
    /// (row, size × 4) → measured (width, height) of a wrapped lyric line.
    sizes: HashMap<(usize, u16), (f32, f32)>,
    /// (row, size × 4) → where the pieces of a pronounced line go.
    rubies: HashMap<(usize, u16), RubyLay>,
}

#[derive(Clone, PartialEq)]
struct Sig {
    track: String,
    synced: bool,
    lines: usize,
    first: u64,
    last: u64,
}

pub(crate) struct Frame {
    pub(crate) dt: f32,
    /// The page was not drawn for a while (collapsed or another page).
    resumed: bool,
    /// Resumed while the island is still fading its content in.
    pub(crate) opening: bool,
}

impl Anim {
    pub(crate) fn frame(&mut self, now: f64, content_alpha: f32) -> Frame {
        let gap = self.last_t.map_or(f64::INFINITY, |t| now - t);
        self.last_t = Some(now);
        let resumed = !(0.0..=RESUME_GAP_S).contains(&gap);
        let dt = if (0.0..=MAX_DT_S).contains(&gap) { gap as f32 } else { 1.0 / 60.0 };
        Frame { dt, resumed, opening: resumed && content_alpha < 0.98 }
    }

    fn sig_matches(&self, track: &str, l: Option<&Lyrics>) -> bool {
        let Some(s) = &self.sig else { return false };
        let (synced, lines, first, last) = lyrics_shape(l);
        s.track == track && s.synced == synced && s.lines == lines && s.first == first && s.last == last
    }

    fn set_sig(&mut self, track: &str, l: Option<&Lyrics>) {
        let (synced, lines, first, last) = lyrics_shape(l);
        self.sig = Some(Sig { track: track.to_string(), synced, lines, first, last });
    }
}

fn lyrics_shape(l: Option<&Lyrics>) -> (bool, usize, u64, u64) {
    match l {
        Some(l) => (
            l.synced,
            l.lines.len(),
            l.lines.first().map_or(0, |x| x.time_s.to_bits() ^ x.text.len() as u64),
            l.lines.last().map_or(0, |x| x.time_s.to_bits() ^ x.text.len() as u64),
        ),
        None => (false, 0, 0, 0),
    }
}

thread_local! {
    // Pages only ever run on the UI thread.
    static ANIM: Cell<Option<Box<Anim>>> = const { Cell::new(None) };
}

pub(crate) fn take_anim() -> Box<Anim> {
    ANIM.try_with(|c| c.take()).ok().flatten().unwrap_or_default()
}

pub(crate) fn put_anim(a: Box<Anim>) {
    let _ = ANIM.try_with(|c| c.set(Some(a)));
}

/// Exact critically damped spring step toward `target` (never overshoots,
/// stable for any `dt`).
pub(crate) fn follow(pos: &mut f32, vel: &mut f32, target: f32, omega: f32, dt: f32) {
    if !pos.is_finite() || !vel.is_finite() {
        *pos = target;
        *vel = 0.0;
        return;
    }
    let x = *pos - target;
    let e = (-omega * dt).exp();
    let k = (*vel + omega * x) * dt;
    *pos = target + (x + k) * e;
    *vel = (*vel - omega * k) * e;
}

/// Time to step a follow this frame. A motion that starts from rest moves
/// at most [`START_STEP_S`], whatever the gap since the last (maybe ambient)
/// frame; one already under way keeps the real gap.
pub(crate) fn step_dt(dt: f32, vel: f32, pos: f32, target: f32) -> f32 {
    if vel == 0.0 && pos != target {
        dt.min(START_STEP_S)
    } else {
        dt
    }
}

/// A lyric row's type size for closeness `e` (0 = far, 1 = the sung line),
/// and the quarter point its text is laid out at (it is drawn scaled by
/// `size / size_q`, so the size glides while layouts stay cached).
fn lyric_size(e: f32) -> (f32, f32) {
    let size = LYRIC_MIN + (LYRIC_MAX - LYRIC_MIN) * e.clamp(0.0, 1.0);
    (size, (size * 4.0).round() / 4.0)
}

/// Snap to the target once close and slow enough; returns `true` when settled.
pub(crate) fn settle(pos: &mut f32, vel: &mut f32, target: f32, eps: f32, vel_eps: f32) -> bool {
    if (*pos - target).abs() < eps && vel.abs() < vel_eps {
        *pos = target;
        *vel = 0.0;
        true
    } else {
        false
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

pub(crate) fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

// ---------------------------------------------------------------- page

pub fn height(st: &AppState) -> f32 {
    if let Some(h) = crate::pill::music::height(st) {
        return h;
    }
    if st.media.is_some() {
        H_SESSION
    } else {
        H_EMPTY
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if crate::pill::music::draw(ui, st, area) {
        return;
    }
    let mut a = take_anim();
    let fr = a.frame(st.now, ui.c.alpha());
    if st.media.is_some() {
        draw_session(ui, st, area, &mut a, &fr);
    } else {
        let r = Rect::new(area.x, area.y, area.w, H_EMPTY);
        super::empty_state(ui, r, Icon::Music, "Nothing playing", "Play music in Spotify, a browser or any media app");
    }
    put_anim(a);
}

fn draw_session(ui: &mut Ui, st: &mut AppState, area: Rect, a: &mut Anim, fr: &Frame) {
    let now = st.now;
    let accent = st.media_accent();
    let lyrics_enabled = st.settings.lyrics;
    let Some(m) = st.media.as_ref() else { return };

    let scale = ui.c.scale().max(0.5);
    let w = area.w;
    let x0 = snap(area.x, scale);
    let y0 = snap(area.y, scale);

    // Columns.
    let lyr_w = (w * LYRICS_SHARE).min(w - ART - GAP_ART - GAP_LYRICS - MIN_MID_W).round();
    let show_lyrics = lyrics_enabled && lyr_w >= MIN_LYRICS_W;
    let mid_x = snap(x0 + ART + GAP_ART, scale);
    let mid_right = if show_lyrics { x0 + w - lyr_w - GAP_LYRICS } else { x0 + w };
    let mid = Rect::new(mid_x, y0, snap(mid_right - mid_x, scale).max(0.0), H_SESSION);
    let col = Rect::new(snap(x0 + w - lyr_w, scale), y0, lyr_w, H_SESSION);

    let playing = m.is_playing();
    let tl = m.timeline;
    let pos = tl.position_at(now);
    // Where the lyrics are: the player's position, plus its known lag and the
    // user's own shift, on a clock that rides out jumpy reports.
    let lead = lyrics_lead_s(&m.app_id) + f64::from(st.settings.lyrics_offset_ms) / 1000.0;
    let lyric_pos = a.clock.tick(now, pos + lead, playing && tl.has_duration(), tl.rate);
    let has_dur = tl.has_duration();
    let mut acts: Vec<MediaCmd> = Vec::new();
    let mut anim = false;

    // Album art (gently shrinks while paused).
    let art_target = if playing { 1.0 } else { ART_PAUSED_SCALE };
    if a.art_scale <= 0.0 || fr.opening {
        a.art_scale = art_target;
        a.art_vel = 0.0;
    }
    let dt = step_dt(fr.dt, a.art_vel, a.art_scale, art_target);
    follow(&mut a.art_scale, &mut a.art_vel, art_target, ART_OMEGA, dt);
    let art_settled = settle(&mut a.art_scale, &mut a.art_vel, art_target, 0.0005, 0.005);
    anim |= !art_settled;
    let art_px = if art_settled { Some(scale) } else { None };
    draw_art(ui, m, Rect::new(x0, y0 + ART_TOP, ART, ART), a.art_scale, art_px);

    // Title, artist, player.
    let title = match m.title.trim() {
        "" => "Unknown title",
        t => t,
    };
    let artist = match m.artist.trim() {
        "" => m.album.trim(),
        t => t,
    };
    ui.c.text(title, Rect::new(mid.x, y0 + TITLE_Y, mid.w, TITLE_H), &TextStyle::headline());
    if !artist.is_empty() {
        ui.c.text(artist, Rect::new(mid.x, y0 + ARTIST_Y, mid.w, 18.0), &TextStyle::body().color(palette::TEXT_DIM));
    }
    let app = m.app_name.trim();
    let source = match (app.is_empty(), playing) {
        (false, true) => Some(app.to_string()),
        (false, false) => Some(format!("{app} \u{b7} Paused")),
        (true, false) => Some("Paused".to_string()),
        (true, true) => None,
    };
    if let Some(label) = source {
        let dot = if playing { accent } else { accent.mul_alpha(0.45) };
        draw_source(ui, mid.x, y0 + SOURCE_Y, mid.w, &label, dot, scale);
    }

    // Progress + times.
    let bar = Rect::new(mid.x, snap(y0 + BAR_Y, scale), mid.w, BAR_H);
    if has_dur {
        let seekable = m.can_seek;
        let hover = seekable && ui.hovered(Rect::new(bar.x, bar.y - 6.0, bar.w, bar.h + 12.0));
        let scrub = ui.progress_bar(wid("media_seek"), bar, tl.progress_at(now), accent, seekable);
        if scrub.began {
            // A drag starts: the player can get ready for the seek meanwhile.
            acts.push(MediaCmd::Prepare);
        }
        if let Some(f) = scrub.released {
            acts.push(MediaCmd::Seek(f as f64 * tl.duration_s));
        }
        // While dragging, the knob and times follow the pointer; the player
        // only seeks on release.
        let (progress, pos) = match scrub.held {
            Some(f) => (f, f as f64 * tl.duration_s),
            None => (tl.progress_at(now), pos),
        };
        if hover || scrub.held.is_some() {
            let c = Point::new(bar.x + bar.w * progress, bar.y + bar.h / 2.0);
            ui.c.fill_circle(c, 6.0, palette::TEXT);
        }
        let ts = TextStyle::footnote().tabular().color(palette::TEXT_DIM);
        let half = mid.w / 2.0;
        let remaining = (tl.duration_s.floor() - pos.floor()).max(0.0);
        ui.c.text(&fmt_duration(pos), Rect::new(mid.x, y0 + TIMES_Y, half, TIMES_H), &ts);
        ui.c.text(&format!("\u{2212}{}", fmt_duration(remaining)), Rect::new(mid.x + half, y0 + TIMES_Y, half, TIMES_H), &ts.right());
    } else {
        // Unknown length (streams, some browsers): a quiet, inert track.
        ui.c.fill_rrect(bar, BAR_H / 2.0, palette::TRACK.mul_alpha(0.6));
    }

    // Transport controls.
    let cx = mid.x + mid.w / 2.0;
    let cy = y0 + CONTROLS_CY;
    let spacing = ((mid.w - SIDE_BTN) / 2.0).clamp(40.0, 60.0);
    if side_button(ui, wid("media_prev"), Point::new(cx - spacing, cy), Icon::Prev, m.can_prev) {
        acts.push(MediaCmd::Prev);
    }
    if play_button(ui, Point::new(cx, cy), playing, m.can_play_pause) {
        acts.push(MediaCmd::PlayPause);
    }
    if side_button(ui, wid("media_next"), Point::new(cx + spacing, cy), Icon::Next, m.can_next) {
        acts.push(MediaCmd::Next);
    }

    // Lyrics.
    let mut open_credit = false;
    let mut synced_shown = false;
    let mut get_ja_readings = false;
    if show_lyrics {
        let pronounce = st.settings.lyrics_pronunciation;
        // A Japanese song whose kanji went unread: the readings add-on coming
        // in (the app switched it on), or offered after a switch-off by hand.
        let offer = if pronounce && st.lyrics.wants_ja_readings { addons::ja_offer(st.addon(addons::JAPANESE_READINGS.id)) } else { None };
        let ctx = LyricsCtx { now, pos: lyric_pos, live: playing && has_dur, can_seek: m.can_seek, scale, track_key: &m.track_key, pronounce, offer };
        let out = draw_lyrics(ui, &st.lyrics, &mut st.ui, col, a, fr, &ctx, &mut acts);
        anim |= out.animating;
        synced_shown = out.synced;
        open_credit = out.open_credit;
        get_ja_readings = out.get_ja_readings;
    }

    // Real transitions (a lyric scroll catching up, a press) get full-rate
    // frames; the steady drift of the progress bar and lyrics while music
    // plays only needs ambient ~30 fps.
    if anim {
        ui.request_anim();
    } else if playing && (has_dur || synced_shown) {
        ui.request_live();
    }

    for c in acts {
        push_media(st, c);
    }
    if open_credit {
        let url = source_url(lyrics_source(&st.lyrics)).to_string();
        st.cmd(Command::OpenUrl(url));
    }
    // The same switch as the Add-ons tab's row: the app downloads it, then
    // reads the song again.
    if get_ja_readings {
        st.settings.set_addon(addons::JAPANESE_READINGS.id, true);
        st.cmd(Command::SaveSettings);
    }
}

/// Queue a media command; seeks also move the local timeline right away so
/// the bar and lyrics don't wait for the player to report back.
pub(crate) fn push_media(st: &mut AppState, c: MediaCmd) {
    let c = match c {
        MediaCmd::Seek(t) => {
            let now = st.now;
            let mut t = if t.is_finite() { t.max(0.0) } else { 0.0 };
            if let Some(ms) = st.media.as_mut() {
                if ms.timeline.duration_s > 0.0 {
                    t = t.min(ms.timeline.duration_s);
                }
                ms.timeline.position_s = t;
                ms.timeline.sampled_at = now;
            }
            MediaCmd::Seek(t)
        }
        other => other,
    };
    st.media_cmd(c);
}

/// Cover art (or a placeholder tile) scaled about its centre by `scale`;
/// snapped to device pixels when `px_scale` is given (i.e. at rest).
fn draw_art(ui: &mut Ui, m: &MediaSession, r: Rect, scale: f32, px_scale: Option<f32>) {
    let mut rr = Rect::from_center(r.center(), r.w * scale, r.h * scale);
    if let Some(s) = px_scale {
        let (x0, y0) = (snap(rr.x, s), snap(rr.y, s));
        rr = Rect::new(x0, y0, snap(rr.right(), s) - x0, snap(rr.bottom(), s) - y0);
    }
    let radius = ART_RADIUS * scale;
    match &m.art {
        Some(img) => {
            ui.c.image(img, rr, radius);
            // Hairline so dark covers keep an edge on the black island.
            ui.c.stroke_rrect(rr, radius, Color::white(0.08), 1.0);
        }
        None => {
            ui.c.fill_rrect(rr, radius, palette::SURFACE);
            let s = 40.0 * scale;
            ui.c.icon(Icon::Music, Rect::from_center(rr.center(), s, s), palette::TEXT_FAINT);
        }
    }
}

/// The player's name after a small accent dot, as plain secondary text.
fn draw_source(ui: &mut Ui, x: f32, y: f32, max_w: f32, label: &str, dot: Color, scale: f32) {
    let ts = TextStyle::footnote().color(palette::TEXT_DIM);
    let (dot_r, gap) = (3.0, 6.0);
    let tx = x + 2.0 * dot_r + gap;
    let tw = (x + max_w - tx).max(0.0);
    if tw < 24.0 {
        return;
    }
    ui.c.fill_circle(Point::new(x + dot_r, snap(y + SOURCE_H / 2.0, scale)), dot_r, dot);
    ui.c.text(label, Rect::new(tx, y, tw, SOURCE_H), &ts);
}

/// Previous / next. Disabled buttons are drawn inert and take no input.
fn side_button(ui: &mut Ui, id: WidgetId, c: Point, icon: Icon, enabled: bool) -> bool {
    let r = Rect::from_center(c, SIDE_BTN, SIDE_BTN);
    if enabled {
        ui.icon_button(id, r, icon, palette::TEXT, CONTROL_ICON).clicked
    } else {
        ui.c.icon(icon, Rect::from_center(c, CONTROL_ICON, CONTROL_ICON), palette::DISABLED);
        false
    }
}

/// The big play/pause: a white disc with a black glyph (on a light look, its
/// accent disc with a white glyph).
fn play_button(ui: &mut Ui, c: Point, playing: bool, enabled: bool) -> bool {
    let icon = if playing { Icon::Pause } else { Icon::Play };
    let icon_r = Rect::from_center(c, CONTROL_ICON, CONTROL_ICON);
    if !enabled {
        ui.c.fill_circle(c, PLAY_D / 2.0, palette::SURFACE);
        ui.c.icon(icon, icon_r, palette::DISABLED);
        return false;
    }
    let resp = ui.button(wid("media_play"), Rect::from_center(c, PLAY_D, PLAY_D));
    // A look with its own accent (Oyen's, cream or black) plays in it; the others in white.
    let (rest, hover, press, glyph) = match ui.c.look().accent {
        Some(a) => (a, Color::lerp(a, Color::white(1.0), 0.08), Color::lerp(a, Color::black(1.0), 0.15), palette::ON_ACCENT),
        None => (palette::TEXT, Color::white(1.0), Color::white(0.82), Color::black(1.0)),
    };
    let (radius, fill) = if resp.pressed {
        (PLAY_D / 2.0 - 1.5, press)
    } else if resp.hovered {
        (PLAY_D / 2.0 + 1.0, hover)
    } else {
        (PLAY_D / 2.0, rest)
    };
    ui.c.fill_circle(c, radius, fill);
    ui.c.icon(icon, icon_r, glyph);
    resp.clicked
}

// ---------------------------------------------------------------- lyrics

pub(crate) struct LyricsCtx<'a> {
    pub(crate) now: f64,
    /// Playback position (seconds).
    pub(crate) pos: f64,
    /// Playing with a known length: frames are coming anyway.
    pub(crate) live: bool,
    pub(crate) can_seek: bool,
    pub(crate) scale: f32,
    pub(crate) track_key: &'a str,
    /// Show the pronunciation pieces (`LyricsState::ruby`) under lines.
    pub(crate) pronounce: bool,
    /// In the credit's place, always shown: the Japanese readings add-on's
    /// line (`poni_core::addons::ja_offer`) and whether a click gets it.
    pub(crate) offer: Option<(String, bool)>,
}

#[derive(Default)]
pub(crate) struct LyricsOut {
    pub(crate) animating: bool,
    pub(crate) synced: bool,
    pub(crate) open_credit: bool,
    /// The offer was clicked: switch the Japanese readings add-on on.
    pub(crate) get_ja_readings: bool,
}

enum View<'a> {
    Status { msg: &'static str, busy: bool },
    Synced(&'a Lyrics),
    Plain(&'a Lyrics),
}

fn lyrics_view<'a>(lyr: &'a LyricsState, track_key: &str) -> View<'a> {
    if lyr.track_key != track_key {
        return View::Status { msg: "Finding lyrics\u{2026}", busy: true };
    }
    match &lyr.status {
        LyricsStatus::Loading => View::Status { msg: "Finding lyrics\u{2026}", busy: true },
        LyricsStatus::Instrumental => View::Status { msg: "Instrumental", busy: false },
        LyricsStatus::NotFound => View::Status { msg: "No synced lyrics for this song", busy: false },
        LyricsStatus::Error(_) | LyricsStatus::Idle => View::Status { msg: "Lyrics unavailable", busy: false },
        LyricsStatus::Found => match &lyr.lyrics {
            Some(l) if !l.is_empty() && l.synced => View::Synced(l),
            Some(l) if !l.is_empty() => View::Plain(l),
            _ => View::Status { msg: "No synced lyrics for this song", busy: false },
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_lyrics(
    ui: &mut Ui,
    lyr: &LyricsState,
    pui: &mut PageUiState,
    col: Rect,
    a: &mut Anim,
    fr: &Frame,
    ctx: &LyricsCtx,
    acts: &mut Vec<MediaCmd>,
) -> LyricsOut {
    if pui.lyric_track != ctx.track_key {
        pui.lyric_track = ctx.track_key.to_string();
        pui.lyric_pos = 0.0;
    }
    let view = lyrics_view(lyr, ctx.track_key);
    let shown = match view {
        View::Synced(l) | View::Plain(l) => Some(l),
        View::Status { .. } => None,
    };
    let fresh = !a.sig_matches(ctx.track_key, shown);
    if fresh {
        a.set_sig(ctx.track_key, shown);
        a.clock.reset();
        a.sizes.clear();
        a.rubies.clear();
        a.lyric_vel = 0.0;
        a.scroll_target = 0.0;
        a.scroll_vel = 0.0;
    }
    let env = (col.w.to_bits(), ctx.scale.to_bits());
    if a.env != env || a.sizes.len() + a.rubies.len() > MAX_CACHE {
        a.env = env;
        a.sizes.clear();
        a.rubies.clear();
    }

    let vp = Rect::new(col.x, col.y, col.w, col.h - CREDIT_H - CREDIT_GAP);
    let mut out = LyricsOut::default();
    let source = lyrics_source(lyr);
    let credit = match view {
        View::Status { msg, busy } => {
            let pulse = if busy && ctx.live { Some(ctx.now) } else { None };
            draw_status(ui, vp, msg, pulse);
            format!("Lyrics: {source}")
        }
        View::Synced(l) => {
            out.synced = true;
            out.animating = draw_synced(ui, l, &lyr.ruby, pui, vp, a, fr, fresh, ctx, acts);
            format!("Lyrics: {source}")
        }
        View::Plain(l) => {
            out.animating = draw_plain(ui, l, &lyr.ruby, pui, vp, a, fr, fresh, ctx);
            format!("Lyrics: {source} \u{b7} Unsynced")
        }
    };

    // The attribution only shows while the pointer is over the column (the
    // zone includes the credit's own hit slop so it stays while reaching it).
    let hot = ui.hovered(Rect::new(col.x - 6.0, col.y, col.w + 12.0, col.h + 5.0));
    let target = if hot { 1.0 } else { 0.0 };
    if fr.resumed {
        a.credit_k = target;
    } else if a.credit_k != target {
        let step = fr.dt / CREDIT_FADE_S;
        a.credit_k = if target > a.credit_k { (a.credit_k + step).min(target) } else { (a.credit_k - step).max(target) };
        out.animating = true;
    }
    if let Some((line, click)) = &ctx.offer {
        out.get_ja_readings = draw_offer(ui, col, line, *click);
    } else if a.credit_k > 0.0 {
        out.open_credit = draw_credit(ui, col, &credit, smoothstep(a.credit_k));
    }
    out
}

/// Centred icon + message for loading / missing lyrics.
fn draw_status(ui: &mut Ui, vp: Rect, msg: &str, pulse: Option<f64>) {
    let c = vp.center();
    let k = pulse.map_or(1.0, |t| 0.7 + 0.3 * (t * 3.2).sin() as f32);
    ui.c.icon(Icon::Lyrics, Rect::from_center(Point::new(c.x, c.y - 13.0), 20.0, 20.0), palette::TEXT_FAINT.mul_alpha(k));
    let ts = TextStyle::footnote().color(palette::TEXT_DIM).center().wrap().top();
    ui.c.text(msg, Rect::new(vp.x, c.y + 4.0, vp.w, 36.0), &ts);
}

/// Attribution at the bottom of the column, faded in by `alpha`; clicking it
/// opens the lyrics' source.
fn draw_credit(ui: &mut Ui, col: Rect, label: &str, alpha: f32) -> bool {
    let ts = TextStyle::caption();
    let (tw, _) = ui.c.measure(label, &ts, col.w);
    let r = Rect::new(col.x, col.bottom() - CREDIT_H, (tw + 2.0).min(col.w), CREDIT_H);
    let resp = ui.button(wid("media_lyrics_credit"), r.inset(-6.0, -5.0));
    let color = if resp.hovered { palette::TEXT } else { palette::TEXT_DIM };
    ui.c.with_alpha(alpha, |c| c.text(label, r, &ts.color(color)));
    resp.clicked
}

/// The Japanese readings add-on's line where the credit goes, shown while the
/// song needs it; `click`: it takes a click (returns true when clicked).
/// Where it's wider than the column (the Mac's font is wider, or the island
/// narrower) it's set a little smaller, so its end, the size, still shows.
fn draw_offer(ui: &mut Ui, col: Rect, line: &str, click: bool) -> bool {
    let mut ts = TextStyle::caption();
    let full = ui.c.measure(line, &ts, 10_000.0).0;
    if full > col.w - 2.0 {
        ts.size *= ((col.w - 2.0) / full).max(OFFER_MIN_SCALE);
    }
    let (tw, _) = ui.c.measure(line, &ts, col.w);
    let r = Rect::new(col.x, col.bottom() - CREDIT_H, (tw + 2.0).min(col.w), CREDIT_H);
    if !click {
        ui.c.text(line, r, &ts.color(palette::TEXT_DIM));
        return false;
    }
    let resp = ui.button(wid("media_ja_readings"), r.inset(-6.0, -5.0));
    let color = if resp.hovered { palette::TEXT } else { palette::TEXT_DIM };
    ui.c.text(line, r, &ts.color(color));
    resp.clicked
}

/// Lyric text style at `size` (wrapped, uniform leading).
fn lyric_style(size: f32) -> TextStyle {
    TextStyle::new(size).weight(Weight::Semibold).wrap().line_spacing(LYRIC_LEADING)
}

/// Wrapped lyric line size, cached per (row, size) for the current lyrics.
fn measure_row(ui: &mut Ui, a: &mut Anim, row: usize, size: f32, text: &str, wrap_w: f32) -> (f32, f32) {
    let key = (row, (size * 4.0).round() as u16);
    if let Some(v) = a.sizes.get(&key) {
        return *v;
    }
    let ts = lyric_style(size);
    let (w, h) = ui.c.measure(text, &ts, wrap_w);
    let v = (w.min(wrap_w), h.max(size));
    a.sizes.insert(key, v);
    v
}

/// A lyric piece at `size` on one line (no wrapping, same leading as rows).
fn piece_style(size: f32) -> TextStyle {
    TextStyle::new(size).weight(Weight::Semibold).line_spacing(LYRIC_LEADING).top()
}

/// The reading under a piece: smaller, never below the type scale's floor.
fn roman_style(size: f32) -> TextStyle {
    TextStyle::new((size * ROMAN_SCALE).max(ts::CAPTION)).weight(Weight::Medium).top()
}

/// One piece of a pronounced line, placed relative to the row's top left.
#[derive(Clone)]
struct Placed {
    text: String,
    roman: String,
    x: f32,
    y: f32,
    text_w: f32,
    roman_w: f32,
}

/// Where the pieces of a pronounced line go.
#[derive(Clone, Default)]
struct RubyLay {
    pieces: Vec<Placed>,
    w: f32,
    h: f32,
}

/// Height of one line of pieces with their readings.
fn ruby_line_h(size: f32) -> f32 {
    let rs = roman_style(size).size;
    size * LYRIC_LEADING + ROMAN_GAP + rs * 1.25
}

/// Lay the pieces out left to right, each as wide as the wider of its text
/// and its reading, wrapping whole pieces at `wrap_w`. Spaces between words
/// become the same gap as the one set between pieces.
fn layout_ruby(ui: &mut Ui, pieces: &[Ruby], size: f32, wrap_w: f32) -> RubyLay {
    let (ps, rs) = (piece_style(size), roman_style(size));
    let line_h = ruby_line_h(size);
    let gap = (size * PIECE_GAP).round();
    let space = ui.c.measure("a a", &ps, 10_000.0).0 - ui.c.measure("aa", &ps, 10_000.0).0;
    let mut lay = RubyLay { pieces: Vec::with_capacity(pieces.len()), ..Default::default() };
    let (mut x, mut y, mut any) = (0.0f32, 0.0f32, false);
    for p in pieces {
        // A run with no reading (English inside a Korean line, say) wraps
        // word by word like ordinary text; a piece with a reading stays whole.
        let words: Vec<&str> = if p.roman.is_empty() { p.text.split_whitespace().collect() } else { vec![p.text.trim()] };
        for (i, text) in words.into_iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            let tw = ui.c.measure(text, &ps, 10_000.0).0;
            let rw = if p.roman.is_empty() { 0.0 } else { ui.c.measure(&p.roman, &rs, 10_000.0).0 };
            let w = tw.max(rw);
            if any {
                x += if i > 0 { space } else { gap };
            }
            if any && x + w > wrap_w {
                x = 0.0;
                y += line_h + RUBY_LINE_GAP;
            }
            lay.pieces.push(Placed { text: text.to_string(), roman: p.roman.clone(), x, y, text_w: tw, roman_w: rw });
            x += w;
            lay.w = lay.w.max(x.min(wrap_w));
            any = true;
        }
    }
    lay.h = y + line_h;
    lay
}

/// A pronounced line's layout, cached per (row, size) for the current lyrics.
fn measure_ruby(ui: &mut Ui, a: &mut Anim, row: usize, size: f32, pieces: &[Ruby], wrap_w: f32) -> (f32, f32) {
    let key = (row, (size * 4.0).round() as u16);
    if let Some(l) = a.rubies.get(&key) {
        return (l.w, l.h);
    }
    let l = layout_ruby(ui, pieces, size, wrap_w);
    let v = (l.w, l.h);
    a.rubies.insert(key, l);
    v
}

/// Draw a pronounced line measured by [`measure_ruby`] with its top left at
/// (`x`, `y`): each piece in lyric type, its reading small under it.
fn draw_ruby(ui: &mut Ui, a: &Anim, row: usize, size: f32, x: f32, y: f32, alpha: f32) {
    let Some(lay) = a.rubies.get(&(row, (size * 4.0).round() as u16)) else { return };
    let ps = piece_style(size).color(Color::white(alpha));
    let rs = roman_style(size).color(Color::white(alpha * ROMAN_ALPHA));
    let text_h = size * LYRIC_LEADING;
    for p in &lay.pieces {
        ui.c.text(&p.text, Rect::new(x + p.x, y + p.y, p.text_w + 2.0, text_h + 2.0), &ps);
        if p.roman_w > 0.0 {
            ui.c.text(&p.roman, Rect::new(x + p.x, y + p.y + text_h + ROMAN_GAP, p.roman_w + 2.0, rs.size * 1.25 + 2.0), &rs);
        }
    }
}

/// One row of the synced view (a line, or an instrumental/intro gap).
struct Row<'a> {
    r: usize,
    /// Distance from the scroll position, in rows that show.
    d: f32,
    /// Type size: it glides with `d`, no steps.
    size: f32,
    /// The quarter point the text is laid out at (layouts are cached per
    /// size), drawn scaled by `k` = `size` / `size_q` (within 1 %).
    size_q: f32,
    k: f32,
    /// Height of the layout box at `size_q` (`h` is its scaled height).
    lay_h: f32,
    text: &'a str,
    /// The line's pieces with their readings, when it is shown pronounced.
    ruby: Option<&'a [Ruby]>,
    wrap_w: f32,
    w: f32,
    h: f32,
    /// 1 for a line; how far a gap is open (`gap_open`).
    open: f32,
    /// Centre relative to the row under the scroll position.
    c: f32,
}

/// How far a gap row (the intro, an instrumental break) `d` rows from the
/// scroll position is open: it opens as the scroll reaches it and folds
/// away, dots and room, once the next line starts, as in Apple Music. Left
/// open, its dots sat above the sung lines for the rest of the song.
fn gap_open(d: f32) -> f32 {
    (1.0 - d).clamp(0.0, 1.0)
}

/// Space between two neighbouring rows that are `above` and `below` open: a
/// folded gap gives back its share on both sides, so the lines around it
/// close up to one `LYRIC_GAP`.
fn row_gap(above: f32, below: f32) -> f32 {
    LYRIC_GAP * (above + below) / 2.0
}

/// Rows `lo..=hi` to lay out around row `k` of `n`: `above` and `below`
/// rows with text on each side (folded gaps take no room, so they don't count).
fn row_window(k: usize, n: usize, above: usize, below: usize, is_gap: impl Fn(usize) -> bool) -> (usize, usize) {
    let (mut lo, mut left) = (k, above);
    while lo > 0 && left > 0 {
        lo -= 1;
        left -= usize::from(!is_gap(lo));
    }
    let (mut hi, mut left) = (k, below);
    while hi + 1 < n && left > 0 {
        hi += 1;
        left -= usize::from(!is_gap(hi));
    }
    (lo, hi)
}

/// Each row's distance from the scroll position `p` (rows `rs`, open `open`),
/// leaving out the folded part of the gaps in between: the line just before
/// an instrumental break stays the previous line once the break has folded.
fn shown_distances(rs: &[usize], open: &[f32], p: f32) -> Vec<f32> {
    rs.iter()
        .map(|&r| {
            let r = r as f32;
            let folded: f32 = rs.iter().zip(open).filter(|(&g, _)| (g as f32 - r) * (g as f32 - p) < 0.0).map(|(_, o)| 1.0 - o).sum();
            ((r - p).abs() - folded).max(0.0)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_synced(
    ui: &mut Ui,
    l: &Lyrics,
    rubies: &[Vec<Ruby>],
    pui: &mut PageUiState,
    vp: Rect,
    a: &mut Anim,
    fr: &Frame,
    fresh: bool,
    ctx: &LyricsCtx,
    acts: &mut Vec<MediaCmd>,
) -> bool {
    let lines = &l.lines;
    // Row 0 is a virtual "intro" row when the first line starts late.
    let off = usize::from(lines.first().is_some_and(|f| f.time_s >= INTRO_MIN_S));
    let n_rows = lines.len() + off;
    if n_rows == 0 {
        return false;
    }
    let t = ctx.pos + LYRIC_LEAD_S;
    let cur = l.line_index_at(t).map_or(0, |i| i + off);
    let target = cur as f32;

    // Critically damped follow of the current row.
    let diff = target - pui.lyric_pos;
    if fresh || !pui.lyric_pos.is_finite() || (fr.resumed && diff.abs() > 1.5) {
        pui.lyric_pos = target;
        a.lyric_vel = 0.0;
    } else if diff.abs() > MAX_SCROLL_ROWS {
        pui.lyric_pos = target - JUMP_LAND_ROWS * diff.signum();
        a.lyric_vel = 0.0;
    }
    let dt = step_dt(fr.dt, a.lyric_vel, pui.lyric_pos, target);
    follow(&mut pui.lyric_pos, &mut a.lyric_vel, target, LYRIC_OMEGA, dt);
    // 0.006 rows ≈ 0.15 DIP: stop the frame clock once motion is invisible.
    let settled = settle(&mut pui.lyric_pos, &mut a.lyric_vel, target, 0.006, 0.03);
    let p = pui.lyric_pos.clamp(0.0, (n_rows - 1) as f32);
    let k = (p.floor() as usize).min(n_rows - 1);
    let f = (p - k as f32).clamp(0.0, 1.0);

    // Measure the rows around the scroll position. Size follows closeness to
    // the current row; the wrap width scales with it, so a line breaks at the
    // same words whatever its size and heights change smoothly. The text is
    // laid out at the nearest quarter point and drawn scaled by the rest, so
    // a line grows and shrinks smoothly instead of a quarter point at a time.
    let text_of = |r: usize| if r < off { "" } else { lines[r - off].text.trim() };
    let (lo, hi) = row_window(k, n_rows, ROWS_ABOVE, ROWS_BELOW, |r| text_of(r).is_empty());
    let rs: Vec<usize> = (lo..=hi).collect();
    let open: Vec<f32> = rs.iter().map(|&r| if text_of(r).is_empty() { gap_open((r as f32 - p).abs()) } else { 1.0 }).collect();
    let dist = shown_distances(&rs, &open, p);
    let mut rows: Vec<Row> = Vec::with_capacity(rs.len());
    for (i, &r) in rs.iter().enumerate() {
        let (d, open) = (dist[i], open[i]);
        let (size, size_q) = lyric_size(1.0 - d);
        let k = size / size_q;
        let text = text_of(r);
        let ruby = if ctx.pronounce && !text.is_empty() { rubies.get(r - off).map(Vec::as_slice).filter(|p| !p.is_empty()) } else { None };
        let wrap_w = vp.w * size_q / LYRIC_MAX;
        let (w, lay_h) = match ruby {
            _ if text.is_empty() => (dots_width(size) / k, size_q * LYRIC_LEADING * open),
            Some(p) => measure_ruby(ui, a, r, size_q * RUBY_SIZE, p, wrap_w),
            None => measure_row(ui, a, r, size_q, text, wrap_w),
        };
        rows.push(Row { r, d, size, size_q, k, lay_h, text, ruby, wrap_w, w: w * k, h: lay_h * k, open, c: 0.0 });
    }
    let ik = k - lo;
    for i in ik + 1..rows.len() {
        rows[i].c = rows[i - 1].c + rows[i - 1].h / 2.0 + row_gap(rows[i - 1].open, rows[i].open) + rows[i].h / 2.0;
    }
    for i in (0..ik).rev() {
        rows[i].c = rows[i + 1].c - rows[i + 1].h / 2.0 - row_gap(rows[i].open, rows[i + 1].open) - rows[i].h / 2.0;
    }
    let anchor_c = match rows.get(ik + 1) {
        Some(next) => lerp(rows[ik].c, next.c, f),
        None => rows[ik].c,
    };
    let anchor_y = vp.y + vp.h / 2.0;

    let wide = Rect::new(vp.x - 10.0, vp.y, vp.w + 20.0, vp.h);
    ui.c.clip(wide);
    // Moving lines glide between pixels; at rest they sit on whole pixels.
    let snap_was = ui.c.set_text_snap(settled);
    for row in &rows {
        if row.open <= 0.01 {
            // A folded gap.
            continue;
        }
        let mut y = anchor_y + row.c - anchor_c - row.h / 2.0;
        if settled {
            y = snap(y, ctx.scale);
        }
        if y > vp.bottom() || y + row.h < vp.y {
            continue;
        }
        let cy = y + row.h / 2.0;
        let fade = smoothstep((cy - vp.y).min(vp.bottom() - cy) / EDGE_FADE);
        if fade <= 0.01 {
            continue;
        }
        // Current 1.0 → neighbours 0.40 → outer rows 0.24 (then the edge
        // fade): a clear step, so the sung line stands out from its context.
        let base = if row.d <= 1.0 { lerp(1.0, 0.40, row.d) } else { lerp(0.40, 0.24, (row.d - 1.0).min(1.0)) };
        let mut alpha = base * fade * row.open;

        let is_line = row.r >= off;
        if ctx.can_seek && is_line && fade > 0.4 {
            let pill = Rect::new(vp.x - 8.0, y - 3.0, row.w + 16.0, row.h + 6.0);
            let resp = ui.button(wid_n("media_lyric", row.r as u64), intersect(pill, wide));
            if resp.hovered {
                let bg = if resp.pressed { 0.12 } else { 0.07 };
                ui.c.fill_rrect(pill, 8.0, Color::white(bg * fade));
                alpha = alpha.max(0.8 * fade);
            }
            if resp.clicked {
                acts.push(MediaCmd::Seek(lines[row.r - off].time_s));
            }
        }

        if row.text.is_empty() {
            // Instrumental gap: three dots that fill up as it plays.
            let current = row.r == cur && row.d < 0.5;
            let lit = current.then(|| {
                if row.r < off {
                    lines.first().map_or(0.0, |first| (t / first.time_s.max(0.05)).clamp(0.0, 1.0) as f32)
                } else {
                    l.line_progress(row.r - off, t)
                }
            });
            let breathe = if current && ctx.live { 1.0 + 0.12 * (ctx.now * 3.4).sin() as f32 } else { 1.0 };
            // The dots shrink away as the gap folds.
            draw_dots(ui, vp.x, cy, row.size * row.open, alpha, lit, breathe);
        } else {
            // Laid out at `size_q`, scaled by `k` about the row's top left.
            let m0 = ui.c.save_transform();
            if row.k != 1.0 {
                ui.c.scale_about(Point::new(vp.x, y), row.k);
            }
            if row.ruby.is_some() {
                draw_ruby(ui, a, row.r, row.size_q * RUBY_SIZE, vp.x, y, alpha);
            } else {
                let ts = lyric_style(row.size_q).color(Color::white(alpha)).top();
                ui.c.text(row.text, Rect::new(vp.x, y, row.wrap_w, row.lay_h + 2.0), &ts);
            }
            ui.c.restore_transform(m0);
        }
    }
    ui.c.set_text_snap(snap_was);
    ui.c.unclip();
    !settled
}

fn dots_width(size: f32) -> f32 {
    28.0 * size / LYRIC_MAX
}

fn draw_dots(ui: &mut Ui, x: f32, cy: f32, size: f32, alpha: f32, lit: Option<f32>, breathe: f32) {
    let k = size / LYRIC_MAX;
    let r = 3.0 * k * breathe;
    let step = 11.0 * k;
    for j in 0..3 {
        let a = match lit {
            Some(p) => alpha * lerp(0.35, 1.0, (p * 3.0 - j as f32).clamp(0.0, 1.0)),
            None => alpha,
        };
        ui.c.fill_circle(Point::new(x + 3.0 * k + j as f32 * step, cy), r, Color::white(a));
    }
}

/// Unsynced lyrics: a static excerpt that scrolls with the mouse wheel.
#[allow(clippy::too_many_arguments)]
fn draw_plain(
    ui: &mut Ui,
    l: &Lyrics,
    rubies: &[Vec<Ruby>],
    pui: &mut PageUiState,
    vp: Rect,
    a: &mut Anim,
    fr: &Frame,
    fresh: bool,
    ctx: &LyricsCtx,
) -> bool {
    let scale = ctx.scale;
    let lines = &l.lines;
    let start = lines.iter().position(|x| !x.text.trim().is_empty()).unwrap_or(lines.len());
    let end = (start + PLAIN_MAX_LINES).min(lines.len());
    let shown = &lines[start..end];
    let pieces = |i: usize| if ctx.pronounce { rubies.get(start + i).map(Vec::as_slice).filter(|p| !p.is_empty()) } else { None };

    let mut heights: Vec<f32> = Vec::with_capacity(shown.len());
    for (i, line) in shown.iter().enumerate() {
        let text = line.text.trim();
        let h = match pieces(i) {
            _ if text.is_empty() => STANZA_GAP,
            Some(p) => measure_ruby(ui, a, i, PLAIN_SIZE, p, vp.w).1,
            None => measure_row(ui, a, i, PLAIN_SIZE, text, vp.w).1,
        };
        heights.push(h);
    }
    let total = heights.iter().sum::<f32>() + PLAIN_GAP * heights.len().saturating_sub(1) as f32;
    let max_off = (total - vp.h).max(0.0);

    if fresh || !pui.lyric_pos.is_finite() {
        pui.lyric_pos = 0.0;
        a.scroll_target = 0.0;
        a.scroll_vel = 0.0;
    }
    if max_off > 0.0 {
        let notches = ui.wheel_in(vp);
        if notches != 0.0 {
            a.scroll_target -= notches * SCROLL_STEP;
        }
    }
    a.scroll_target = a.scroll_target.clamp(0.0, max_off);
    follow(&mut pui.lyric_pos, &mut a.scroll_vel, a.scroll_target, SCROLL_OMEGA, fr.dt);
    let settled = settle(&mut pui.lyric_pos, &mut a.scroll_vel, a.scroll_target, 0.3, 3.0);
    let offset = pui.lyric_pos.clamp(0.0, max_off);

    // Fade an edge only while there is more text beyond it.
    let top_k = (offset / EDGE_FADE).clamp(0.0, 1.0);
    let bottom_k = ((max_off - offset) / EDGE_FADE).clamp(0.0, 1.0);
    let ts = lyric_style(PLAIN_SIZE).top();
    ui.c.clip(vp);
    let snap_was = ui.c.set_text_snap(settled);
    let mut y = if settled { snap(vp.y - offset, scale) } else { vp.y - offset };
    for (i, (line, &h)) in shown.iter().zip(&heights).enumerate() {
        if y > vp.bottom() {
            break;
        }
        let text = line.text.trim();
        if y + h >= vp.y && !text.is_empty() {
            let cy = y + h / 2.0;
            let ft = lerp(1.0, smoothstep((cy - vp.y) / EDGE_FADE), top_k);
            let fb = lerp(1.0, smoothstep((vp.bottom() - cy) / EDGE_FADE), bottom_k);
            let alpha = palette::TEXT_DIM.a * ft * fb;
            if alpha > 0.01 {
                match pieces(i) {
                    Some(_) => draw_ruby(ui, a, i, PLAIN_SIZE, vp.x, y, alpha),
                    None => ui.c.text(text, Rect::new(vp.x, y, vp.w, h + 2.0), &ts.color(Color::white(alpha))),
                }
            }
        }
        y += h + PLAIN_GAP;
    }
    ui.c.set_text_snap(snap_was);
    ui.c.unclip();
    !settled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyric_size_glides_and_rests_on_whole_layouts() {
        let mut prev = lyric_size(0.0).0;
        for i in 1..=1000 {
            let (size, size_q) = lyric_size(i as f32 / 1000.0);
            // No steps: 1/1000 of the way moves the size 0.003 points.
            assert!((size - prev).abs() < 0.0031, "step {} -> {}", prev, size);
            prev = size;
            // Laid out on a quarter point, drawn within 1 % of it.
            assert_eq!((size_q * 4.0).fract(), 0.0);
            assert!((size / size_q - 1.0).abs() <= 0.01, "{size} vs {size_q}");
        }
        // At rest a row is drawn unscaled, exactly as laid out.
        assert_eq!(lyric_size(0.0), (LYRIC_MIN, LYRIC_MIN));
        assert_eq!(lyric_size(1.0), (LYRIC_MAX, LYRIC_MAX));
    }

    #[test]
    fn a_scroll_from_rest_opens_with_one_short_step() {
        // An ambient frame 28 ms after the last one: the new scroll moves 1/120 s.
        assert_eq!(step_dt(0.028, 0.0, 2.0, 3.0), START_STEP_S);
        // Already moving, or nothing to do: the real gap.
        assert_eq!(step_dt(0.028, 0.4, 2.3, 3.0), 0.028);
        assert_eq!(step_dt(0.028, 0.0, 3.0, 3.0), 0.028);
        // A short gap stays as it is.
        assert_eq!(step_dt(0.004, 0.0, 2.0, 3.0), 0.004);
    }

    #[test]
    fn follow_converges_without_overshoot() {
        let (mut p, mut v) = (0.0f32, 0.0f32);
        let mut max = 0.0f32;
        for _ in 0..120 {
            follow(&mut p, &mut v, 5.0, LYRIC_OMEGA, 1.0 / 60.0);
            max = max.max(p);
        }
        assert!((p - 5.0).abs() < 0.01);
        assert!(max <= 5.0 + 1e-4);
    }

    #[test]
    fn follow_is_stable_for_big_steps() {
        let (mut p, mut v) = (10.0f32, 0.0f32);
        follow(&mut p, &mut v, 0.0, LYRIC_OMEGA, 5.0);
        assert!(p.is_finite() && p.abs() < 0.01);
    }

    #[test]
    fn gaps_show_only_while_they_play() {
        assert_eq!(gap_open(0.0), 1.0);
        assert_eq!(gap_open(0.25), 0.75);
        assert_eq!(gap_open(1.0), 0.0);
        assert_eq!(gap_open(3.0), 0.0);
    }

    #[test]
    fn a_folded_gap_leaves_one_gap_between_lines() {
        assert_eq!(row_gap(1.0, 1.0), LYRIC_GAP);
        // Line, open gap, line: two gaps. Line, folded gap, line: one.
        assert_eq!(row_gap(1.0, 1.0) + row_gap(1.0, 1.0), 2.0 * LYRIC_GAP);
        assert_eq!(row_gap(1.0, 0.0) + row_gap(0.0, 1.0), LYRIC_GAP);
        assert_eq!(row_gap(1.0, 0.0) + row_gap(0.0, 0.0) + row_gap(0.0, 1.0), LYRIC_GAP);
    }

    #[test]
    fn window_looks_past_folded_gaps() {
        let none = |_: usize| false;
        assert_eq!(row_window(0, 20, ROWS_ABOVE, ROWS_BELOW, none), (0, 4));
        assert_eq!(row_window(10, 20, ROWS_ABOVE, ROWS_BELOW, none), (7, 14));
        assert_eq!(row_window(18, 20, ROWS_ABOVE, ROWS_BELOW, none), (15, 19));
        // Row 0 is the intro, rows 12-13 a break.
        let gaps = |r: usize| r == 0 || r == 12 || r == 13;
        assert_eq!(row_window(1, 20, ROWS_ABOVE, ROWS_BELOW, gaps), (0, 5));
        assert_eq!(row_window(10, 20, ROWS_ABOVE, ROWS_BELOW, gaps), (7, 16));
        assert_eq!(row_window(15, 20, ROWS_ABOVE, ROWS_BELOW, gaps), (10, 19));
    }

    #[test]
    fn lines_around_a_folded_gap_stay_neighbours() {
        // Line 5, a break at 6, line 7.
        let rs = [5, 6, 7];
        // The break plays.
        assert_eq!(shown_distances(&rs, &[1.0, 1.0, 1.0], 6.0), vec![1.0, 0.0, 1.0]);
        // Half folded on the way to line 7.
        assert_eq!(shown_distances(&rs, &[1.0, 0.5, 1.0], 6.5), vec![1.0, 0.5, 0.5]);
        // Line 7 is sung: line 5 is the line before it, not two rows up.
        assert_eq!(shown_distances(&rs, &[1.0, 0.0, 1.0], 7.0), vec![1.0, 1.0, 0.0]);
    }

    #[test]
    fn intersect_clamps() {
        let r = intersect(Rect::new(0.0, 0.0, 10.0, 10.0), Rect::new(5.0, 5.0, 10.0, 10.0));
        assert_eq!(r, Rect::new(5.0, 5.0, 5.0, 5.0));
        let none = intersect(Rect::new(0.0, 0.0, 2.0, 2.0), Rect::new(5.0, 5.0, 1.0, 1.0));
        assert_eq!((none.w, none.h), (0.0, 0.0));
    }
}
