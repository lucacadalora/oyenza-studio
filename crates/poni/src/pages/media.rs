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
use poni_core::datetime::fmt_duration;
use poni_core::geom::{lerp, palette, Color, Point, Rect};
use poni_core::lrc::Lyrics;
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
const ART_PAUSED_SCALE: f32 = 0.94;
const GAP_ART: f32 = 16.0;
const GAP_LYRICS: f32 = 20.0;
const LYRICS_SHARE: f32 = 0.42;
const MIN_MID_W: f32 = 176.0;
const MIN_LYRICS_W: f32 = 110.0;

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
/// Credited when a result doesn't name its source.
const DEFAULT_SOURCE: &str = "LRCLIB";

/// Where the shown lyrics come from.
fn lyrics_source(lyr: &LyricsState) -> &str {
    if lyr.source.is_empty() {
        DEFAULT_SOURCE
    } else {
        &lyr.source
    }
}

/// The site the credit opens.
fn source_url(source: &str) -> &'static str {
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
const SCROLL_OMEGA: f32 = 18.0;
const ART_OMEGA: f32 = 14.0;

/// A gap this long since the page last drew means it was hidden meanwhile.
const RESUME_GAP_S: f64 = 0.5;
const MAX_DT_S: f64 = 0.25;
const MAX_CACHE: usize = 4096;

// ---------------------------------------------------------------- animation state

#[derive(Default)]
struct Anim {
    last_t: Option<f64>,
    /// Current art scale (0 = not initialised yet).
    art_scale: f32,
    art_vel: f32,
    lyric_vel: f32,
    /// Unsynced lyrics: wanted scroll offset (DIPs); `lyric_pos` follows it.
    scroll_target: f32,
    scroll_vel: f32,
    /// Lyrics credit opacity (0..1), following the pointer over the column.
    credit_k: f32,
    /// Identity of the lyrics shown last frame (track, synced, shape).
    sig: Option<Sig>,
    /// Column width and DPI scale the measurements below were made at.
    env: (u32, u32),
    /// (row, size × 4) → measured (width, height) of a wrapped lyric line.
    sizes: HashMap<(usize, u16), (f32, f32)>,
}

#[derive(Clone, PartialEq)]
struct Sig {
    track: String,
    synced: bool,
    lines: usize,
    first: u64,
    last: u64,
}

struct Frame {
    dt: f32,
    /// The page was not drawn for a while (collapsed or another page).
    resumed: bool,
    /// Resumed while the island is still fading its content in.
    opening: bool,
}

impl Anim {
    fn frame(&mut self, now: f64, content_alpha: f32) -> Frame {
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

fn take_anim() -> Box<Anim> {
    ANIM.try_with(|c| c.take()).ok().flatten().unwrap_or_default()
}

fn put_anim(a: Box<Anim>) {
    let _ = ANIM.try_with(|c| c.set(Some(a)));
}

/// Exact critically damped spring step toward `target` (never overshoots,
/// stable for any `dt`).
fn follow(pos: &mut f32, vel: &mut f32, target: f32, omega: f32, dt: f32) {
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

/// Snap to the target once close and slow enough; returns `true` when settled.
fn settle(pos: &mut f32, vel: &mut f32, target: f32, eps: f32, vel_eps: f32) -> bool {
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

fn snap(v: f32, scale: f32) -> f32 {
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
    if st.media.is_some() {
        H_SESSION
    } else {
        H_EMPTY
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
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
    let has_dur = tl.has_duration();
    let mut acts: Vec<MediaCmd> = Vec::new();
    let mut anim = false;

    // Album art (gently shrinks while paused).
    let art_target = if playing { 1.0 } else { ART_PAUSED_SCALE };
    if a.art_scale <= 0.0 || fr.opening {
        a.art_scale = art_target;
        a.art_vel = 0.0;
    }
    follow(&mut a.art_scale, &mut a.art_vel, art_target, ART_OMEGA, fr.dt);
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
        let progress = tl.progress_at(now);
        if let Some(f) = ui.progress_bar(wid("media_seek"), bar, progress, accent, seekable) {
            acts.push(MediaCmd::Seek(f as f64 * tl.duration_s));
        }
        if hover {
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
    if show_lyrics {
        let ctx = LyricsCtx { now, pos, live: playing && has_dur, can_seek: m.can_seek, scale, track_key: &m.track_key };
        let out = draw_lyrics(ui, &st.lyrics, &mut st.ui, col, a, fr, &ctx, &mut acts);
        anim |= out.animating;
        synced_shown = out.synced;
        open_credit = out.open_credit;
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
}

/// Queue a media command; seeks also move the local timeline right away so
/// the bar and lyrics don't wait for the player to report back.
fn push_media(st: &mut AppState, c: MediaCmd) {
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

/// The big play/pause: a white disc with a black glyph.
fn play_button(ui: &mut Ui, c: Point, playing: bool, enabled: bool) -> bool {
    let icon = if playing { Icon::Pause } else { Icon::Play };
    let icon_r = Rect::from_center(c, CONTROL_ICON, CONTROL_ICON);
    if !enabled {
        ui.c.fill_circle(c, PLAY_D / 2.0, palette::SURFACE);
        ui.c.icon(icon, icon_r, palette::DISABLED);
        return false;
    }
    let resp = ui.button(wid("media_play"), Rect::from_center(c, PLAY_D, PLAY_D));
    let (radius, fill) = if resp.pressed {
        (PLAY_D / 2.0 - 1.5, Color::white(0.82))
    } else if resp.hovered {
        (PLAY_D / 2.0 + 1.0, Color::white(1.0))
    } else {
        (PLAY_D / 2.0, palette::TEXT)
    };
    ui.c.fill_circle(c, radius, fill);
    ui.c.icon(icon, icon_r, Color::black(1.0));
    resp.clicked
}

// ---------------------------------------------------------------- lyrics

struct LyricsCtx<'a> {
    now: f64,
    /// Playback position (seconds).
    pos: f64,
    /// Playing with a known length: frames are coming anyway.
    live: bool,
    can_seek: bool,
    scale: f32,
    track_key: &'a str,
}

#[derive(Default)]
struct LyricsOut {
    animating: bool,
    synced: bool,
    open_credit: bool,
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
fn draw_lyrics(
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
        a.sizes.clear();
        a.lyric_vel = 0.0;
        a.scroll_target = 0.0;
        a.scroll_vel = 0.0;
    }
    let env = (col.w.to_bits(), ctx.scale.to_bits());
    if a.env != env || a.sizes.len() > MAX_CACHE {
        a.env = env;
        a.sizes.clear();
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
            out.animating = draw_synced(ui, l, pui, vp, a, fr, fresh, ctx, acts);
            format!("Lyrics: {source}")
        }
        View::Plain(l) => {
            out.animating = draw_plain(ui, l, pui, vp, a, fr, fresh, ctx.scale);
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
    if a.credit_k > 0.0 {
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

/// One row of the synced view (a line, or an instrumental/intro gap).
struct Row<'a> {
    r: usize,
    /// Distance from the scroll position, in rows.
    d: f32,
    size: f32,
    text: &'a str,
    wrap_w: f32,
    w: f32,
    h: f32,
    /// Centre relative to the row under the scroll position.
    c: f32,
}

#[allow(clippy::too_many_arguments)]
fn draw_synced(
    ui: &mut Ui,
    l: &Lyrics,
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
    follow(&mut pui.lyric_pos, &mut a.lyric_vel, target, LYRIC_OMEGA, fr.dt);
    // 0.006 rows ≈ 0.15 DIP: stop the frame clock once motion is invisible.
    let settled = settle(&mut pui.lyric_pos, &mut a.lyric_vel, target, 0.006, 0.03);
    let p = pui.lyric_pos.clamp(0.0, (n_rows - 1) as f32);
    let k = (p.floor() as usize).min(n_rows - 1);
    let f = (p - k as f32).clamp(0.0, 1.0);

    // Measure the rows around the scroll position. Size follows closeness to
    // the current row; the wrap width scales with it, so a line breaks at the
    // same words whatever its size and heights change smoothly.
    let lo = k.saturating_sub(ROWS_ABOVE);
    let hi = (k + ROWS_BELOW).min(n_rows - 1);
    let mut rows: Vec<Row> = Vec::with_capacity(hi - lo + 1);
    for r in lo..=hi {
        let d = (r as f32 - p).abs();
        let e = (1.0 - d).clamp(0.0, 1.0);
        let size = ((LYRIC_MIN + (LYRIC_MAX - LYRIC_MIN) * e) * 4.0).round() / 4.0;
        let text = if r < off { "" } else { lines[r - off].text.trim() };
        let wrap_w = vp.w * size / LYRIC_MAX;
        let (w, h) = if text.is_empty() { (dots_width(size), size * LYRIC_LEADING) } else { measure_row(ui, a, r, size, text, wrap_w) };
        rows.push(Row { r, d, size, text, wrap_w, w, h, c: 0.0 });
    }
    let ik = k - lo;
    for i in ik + 1..rows.len() {
        rows[i].c = rows[i - 1].c + rows[i - 1].h / 2.0 + LYRIC_GAP + rows[i].h / 2.0;
    }
    for i in (0..ik).rev() {
        rows[i].c = rows[i + 1].c - rows[i + 1].h / 2.0 - LYRIC_GAP - rows[i].h / 2.0;
    }
    let anchor_c = match rows.get(ik + 1) {
        Some(next) => lerp(rows[ik].c, next.c, f),
        None => rows[ik].c,
    };
    let anchor_y = vp.y + vp.h / 2.0;

    let wide = Rect::new(vp.x - 10.0, vp.y, vp.w + 20.0, vp.h);
    ui.c.clip(wide);
    for row in &rows {
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
        let mut alpha = base * fade;

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
            draw_dots(ui, vp.x, cy, row.size, alpha, lit, breathe);
        } else {
            let ts = lyric_style(row.size).color(Color::white(alpha)).top();
            ui.c.text(row.text, Rect::new(vp.x, y, row.wrap_w, row.h + 2.0), &ts);
        }
    }
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
fn draw_plain(ui: &mut Ui, l: &Lyrics, pui: &mut PageUiState, vp: Rect, a: &mut Anim, fr: &Frame, fresh: bool, scale: f32) -> bool {
    let lines = &l.lines;
    let start = lines.iter().position(|x| !x.text.trim().is_empty()).unwrap_or(lines.len());
    let end = (start + PLAIN_MAX_LINES).min(lines.len());
    let shown = &lines[start..end];

    let mut heights: Vec<f32> = Vec::with_capacity(shown.len());
    for (i, line) in shown.iter().enumerate() {
        let text = line.text.trim();
        let h = if text.is_empty() { STANZA_GAP } else { measure_row(ui, a, i, PLAIN_SIZE, text, vp.w).1 };
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
    let mut y = if settled { snap(vp.y - offset, scale) } else { vp.y - offset };
    for (line, &h) in shown.iter().zip(&heights) {
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
                ui.c.text(text, Rect::new(vp.x, y, vp.w, h + 2.0), &ts.color(Color::white(alpha)));
            }
        }
        y += h + PLAIN_GAP;
    }
    ui.c.unclip();
    !settled
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn intersect_clamps() {
        let r = intersect(Rect::new(0.0, 0.0, 10.0, 10.0), Rect::new(5.0, 5.0, 10.0, 10.0));
        assert_eq!(r, Rect::new(5.0, 5.0, 5.0, 5.0));
        let none = intersect(Rect::new(0.0, 0.0, 2.0, 2.0), Rect::new(5.0, 5.0, 1.0, 1.0));
        assert_eq!((none.w, none.h), (0.0, 0.0));
    }
}
