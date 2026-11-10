//! The open Music page in the Pill theme, laid out like the card in ref1:
//! the album art on the left, the player's name in small grey capitals, the
//! title and the artist beside it, the equalizer at the top right in the
//! album's colour, one progress row (elapsed, the bar, time left), and the
//! controls centred under it as bare glyphs.
//!
//! ```text
//! ┌──────┐  SPOTIFY                    ▮▮▮▮ │  earlier line
//! │ art  │  Title                           │  CURRENT LINE
//! │  72  │  Artist                          │  next line
//! └──────┘                                  │  later line
//!  1:04  ━━━━━━━━━━━━━━──────────────  −2:17 │
//!             ⏪       ❚❚       ⏩           │  (Lyrics: source, on hover)
//! ```
//!
//! What the player does is Classic's, exactly: the same seek path
//! (`ui.progress_bar`, `MediaCmd::Prepare` when a drag starts and `Seek` on
//! release, the timeline moved at once by `pages::media::push_media`, which
//! is also where Apple Music's own slider and back button come in), the
//! same commands, widget ids and disabled states, the art's paused shrink,
//! and the same frames: none at rest, ambient ones while music plays, full
//! rate only while something catches up.
//!
//! The synced lyrics are Classic's own column (`pages::media::draw_lyrics`
//! with its scroll state, readings and credit), on the right as in Classic.
//! There the card keeps Classic's height and the sung line its size with a
//! line either side; below the player the card would have to grow by half
//! again to show more than one line.

use crate::commands::{Command, MediaCmd};
use crate::gfx::{Image, TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::media::{self as classic, Anim, Frame, LyricsCtx};
use crate::state::AppState;
use crate::ui::{wid, Ui, WidgetId};
use poni_core::datetime::fmt_duration;
use poni_core::geom::{lerp, palette, Color, Point, Rect};
use poni_core::addons;
use poni_core::nowplaying::lyrics_lead_s;
use poni_core::svgpath::Seg;

// ---------------------------------------------------------------- layout (spec 6.7)

/// Content height with a media session: the play button's target ends here.
/// Classic's height, so the card is as tall as Classic's island.
const H_SESSION: f32 = 148.0;
/// Content height of the empty card: one row, the size of the art.
const H_EMPTY: f32 = 72.0;

const ART: f32 = 72.0;
/// 0.2 × the art, as on ref1's card (and Classic's art radius).
const ART_RADIUS: f32 = 14.0;
/// The note on a cover-less tile.
const TILE_ICON: f32 = 28.0;
const GAP_ART: f32 = 14.0;
/// The player keeps at least Classic's art, gap and middle column beside the
/// lyrics, so the lyrics column is as wide as Classic's at every width.
const MIN_BLOCK_W: f32 = 296.0;

// Text rows, placed by where their capitals start (what ref1 and the spec
// measure): the player's name, the title, the artist.
const SOURCE_CAP: f32 = 8.0;
/// Extra space between the player name's capitals.
const TRACKING: f32 = 0.5;
const TITLE_CAP: f32 = 24.0;
const ARTIST_CAP: f32 = 49.0;
// The empty card's two rows, centred on the tile.
const EMPTY_TITLE_CAP: f32 = 19.0;
const EMPTY_HINT_CAP: f32 = 44.0;

/// The system font's vertical metrics per em (Segoe UI): where the capitals
/// start inside a line box, so a row can be set by its cap top.
const FONT_ASCENT: f32 = 1.079;
const FONT_LINE: f32 = 1.330;
const FONT_CAP: f32 = 0.700;

// The equalizer, top right in the album's colour.
const EQ_BARS: usize = 4;
const EQ_BAR_W: f32 = 4.0;
const EQ_GAP: f32 = 2.0;
const EQ_CY: f32 = 14.0;
const EQ_LOW: f32 = 4.0;
const EQ_HIGH: f32 = 13.0;
/// Space kept between the text column and the equalizer.
const GAP_EQ: f32 = 12.0;

// The progress row: elapsed, the bar, time left.
const ROW_CY: f32 = 90.0;
/// Each time's slot: fits "−4:09"; an hour-long track widens both.
const TIME_SLOT: f32 = 34.0;
const TIME_GAP: f32 = 14.0;
/// Bar thickness (`ui.progress_bar` adds 2 under the pointer).
const BAR_H: f32 = 5.0;

// The controls: bare glyphs on Classic's targets.
const CONTROLS_CY: f32 = 126.0;
const CONTROL_PITCH: f32 = 69.0;
const PLAY_HIT: f32 = 44.0;
const SIDE_HIT: f32 = 38.0;
const PAUSE_BAR_W: f32 = 7.0;
const PAUSE_BAR_H: f32 = 24.0;
const PAUSE_GAP: f32 = 5.0;
const PAUSE_R: f32 = 1.5;
const PLAY_W: f32 = 20.0;
const PLAY_H: f32 = 24.0;
/// A play triangle centred by its box looks left of centre (its weight is
/// in its base): this nudge puts it where the eye expects.
const PLAY_NUDGE: f32 = 1.5;
const PLAY_ROUND: f32 = 2.5;
/// Back / forward: two touching triangles.
const SKIP_W: f32 = 22.0;
const SKIP_H: f32 = 15.0;
const SKIP_ROUND: f32 = 1.5;
/// The disc under a control while the pointer is on it / presses it.
const HOVER_INK: f32 = 0.10;
const PRESS_INK: f32 = 0.16;

// ---------------------------------------------------------------- page

/// The page's content height, or `None` for Classic's.
pub fn height(st: &AppState) -> Option<f32> {
    if !super::on(st.settings.theme) {
        return None;
    }
    Some(if st.media.is_some() { H_SESSION } else { H_EMPTY })
}

/// Draw the page in `area`. `false` = the Classic page draws.
pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) -> bool {
    if !super::on(st.settings.theme) {
        return false;
    }
    // Classic's animation state (art spring, lyrics scroll, lyric clock): one
    // theme draws at a time, so the two pages share it.
    let mut a = classic::take_anim();
    let fr = a.frame(st.now, ui.c.alpha());
    if st.media.is_some() {
        draw_session(ui, st, area, &mut a, &fr);
    } else {
        draw_empty(ui, area);
    }
    classic::put_anim(a);
    true
}

/// Where the player and the lyrics go in a content area `w` wide.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Columns {
    /// The player's width (art, text, equalizer, progress row, controls).
    block_w: f32,
    /// Classic's lyrics column: its width, `None` when it doesn't show.
    lyrics_w: Option<f32>,
}

fn columns(w: f32, lyrics_on: bool, scale: f32) -> Columns {
    let lyr_w = (w * classic::LYRICS_SHARE).min(w - classic::GAP_LYRICS - MIN_BLOCK_W).round();
    if lyrics_on && lyr_w >= classic::MIN_LYRICS_W {
        Columns { block_w: classic::snap(w - lyr_w - classic::GAP_LYRICS, scale), lyrics_w: Some(lyr_w) }
    } else {
        Columns { block_w: w, lyrics_w: None }
    }
}

fn draw_session(ui: &mut Ui, st: &mut AppState, area: Rect, a: &mut Anim, fr: &Frame) {
    let now = st.now;
    let accent = eq_color(st.media_accent(), ui.c.look().light);
    let lyrics_on = st.settings.lyrics;
    let pronounce = st.settings.lyrics_pronunciation;
    let offset_s = f64::from(st.settings.lyrics_offset_ms) / 1000.0;
    let Some(m) = st.media.as_ref() else { return };

    let scale = ui.c.scale().max(0.5);
    let snap = |v: f32| classic::snap(v, scale);
    let (x0, y0) = (snap(area.x), snap(area.y));
    let cols = columns(area.w, lyrics_on, scale);
    let block = Rect::new(x0, y0, cols.block_w, H_SESSION);

    let playing = m.is_playing();
    let tl = m.timeline;
    let pos = tl.position_at(now);
    let has_dur = tl.has_duration();
    // Where the lyrics are, as in Classic: the player's position plus its
    // known lag and the user's own shift, on a clock that rides out jumpy
    // reports.
    let lead = lyrics_lead_s(&m.app_id) + offset_s;
    let lyric_pos = a.clock.tick(now, pos + lead, playing && has_dur, tl.rate);
    let mut acts: Vec<MediaCmd> = Vec::new();
    let mut anim = false;

    // Album art, gently shrinking while paused (Classic's spring).
    let art_target = if playing { 1.0 } else { classic::ART_PAUSED_SCALE };
    if a.art_scale <= 0.0 || fr.opening {
        a.art_scale = art_target;
        a.art_vel = 0.0;
    }
    let dt = classic::step_dt(fr.dt, a.art_vel, a.art_scale, art_target);
    classic::follow(&mut a.art_scale, &mut a.art_vel, art_target, classic::ART_OMEGA, dt);
    let art_settled = classic::settle(&mut a.art_scale, &mut a.art_vel, art_target, 0.0005, 0.005);
    anim |= !art_settled;
    draw_art(ui, m.art.as_ref(), Rect::new(x0, y0, ART, ART), a.art_scale, art_settled.then_some(scale));

    // The player's name, the title and the artist, in a column that stops
    // short of the equalizer.
    let tx = snap(x0 + ART + GAP_ART);
    let tw = (block.right() - eq_width() - GAP_EQ - tx).max(0.0);
    let source = source_label(&m.app_name, playing);
    let source_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_FAINT);
    tracked(ui, &source, row(tx, tw, y0 + SOURCE_CAP, source_st.size), &source_st);
    let title = match m.title.trim() {
        "" => "Unknown title",
        t => t,
    };
    let artist = match m.artist.trim() {
        "" => m.album.trim(),
        t => t,
    };
    let title_st = TextStyle::value();
    ui.c.text(title, row(tx, tw, y0 + TITLE_CAP, title_st.size), &title_st);
    let artist_st = TextStyle::headline().weight(Weight::Regular).color(palette::TEXT_DIM);
    ui.c.text(artist, row(tx, tw, y0 + ARTIST_CAP, artist_st.size), &artist_st);

    // The progress row. The slots fit this track's longest time, so the bar
    // holds still while the times count.
    let row_cy = y0 + ROW_CY;
    let times = TextStyle::footnote().weight(Weight::Medium).tabular().color(palette::TEXT_DIM);
    let slot = if has_dur {
        let longest = format!("\u{2212}{}", fmt_duration(tl.duration_s));
        TIME_SLOT.max(ui.c.measure(&longest, &times, 400.0).0.ceil())
    } else {
        TIME_SLOT
    };
    let bar_x = snap(block.x + slot + TIME_GAP);
    let bar = Rect::new(bar_x, snap(row_cy - BAR_H / 2.0), (snap(block.right() - slot - TIME_GAP) - bar_x).max(0.0), BAR_H);
    if has_dur {
        let scrub = ui.progress_bar(wid("media_seek"), bar, tl.progress_at(now), palette::TEXT, m.can_seek);
        if scrub.began {
            // A drag starts: the player can get ready for the seek meanwhile.
            acts.push(MediaCmd::Prepare);
        }
        if let Some(f) = scrub.released {
            acts.push(MediaCmd::Seek(f as f64 * tl.duration_s));
        }
        // While dragging, the times follow the pointer; the player only
        // seeks on release.
        let pos = scrub.held.map_or(pos, |f| f as f64 * tl.duration_s);
        let remaining = (tl.duration_s.floor() - pos.floor()).max(0.0);
        let cap = row_cy - FONT_CAP * times.size / 2.0;
        ui.c.text(&fmt_duration(pos), row(block.x, slot, cap, times.size), &times);
        ui.c.text(&format!("\u{2212}{}", fmt_duration(remaining)), row(block.right() - slot, slot, cap, times.size), &times.right());
    } else {
        // Unknown length (streams, some browsers): a quiet, inert track.
        ui.c.fill_rrect(bar, BAR_H / 2.0, palette::TRACK.mul_alpha(0.6));
    }

    // The controls, centred under the player.
    let (cx, cy) = (block.x + block.w / 2.0, y0 + CONTROLS_CY);
    if control(ui, wid("media_prev"), Point::new(cx - CONTROL_PITCH, cy), SIDE_HIT, Glyph::Back, m.can_prev) {
        acts.push(MediaCmd::Prev);
    }
    let play = if playing { Glyph::Pause } else { Glyph::Play };
    if control(ui, wid("media_play"), Point::new(cx, cy), PLAY_HIT, play, m.can_play_pause) {
        acts.push(MediaCmd::PlayPause);
    }
    if control(ui, wid("media_next"), Point::new(cx + CONTROL_PITCH, cy), SIDE_HIT, Glyph::Forward, m.can_next) {
        acts.push(MediaCmd::Next);
    }

    // The lyrics: Classic's column, as Classic draws it, with its offer of
    // the Japanese readings add-on.
    let mut open_credit = false;
    let mut synced_shown = false;
    let mut get_ja_readings = false;
    if let Some(lyr_w) = cols.lyrics_w {
        let col = Rect::new(snap(x0 + area.w - lyr_w), y0, lyr_w, H_SESSION);
        let offer = if pronounce && st.lyrics.wants_ja_readings { addons::ja_offer(st.addon(addons::JAPANESE_READINGS.id)) } else { None };
        let ctx = LyricsCtx { now, pos: lyric_pos, live: playing && has_dur, can_seek: m.can_seek, scale, track_key: &m.track_key, pronounce, offer };
        let out = classic::draw_lyrics(ui, &st.lyrics, &mut st.ui, col, a, fr, &ctx, &mut acts);
        anim |= out.animating;
        synced_shown = out.synced;
        open_credit = out.open_credit;
        get_ja_readings = out.get_ja_readings;
    }

    // The equalizer dances only on the frames the page gets anyway while
    // music plays (below).
    let drifting = playing && (has_dur || synced_shown);
    let eq = match (playing, drifting) {
        (true, true) => EqState::Dancing,
        (true, false) => EqState::Still,
        (false, _) => EqState::Paused,
    };
    equalizer(ui, block.right(), y0 + EQ_CY, accent, eq, now);

    // Classic's frames: full rate only while something catches up (a lyric
    // scroll, the art's spring, the credit fading); the steady drift of the
    // bar and the lyrics while music plays needs only ambient ones.
    if anim {
        ui.request_anim();
    } else if drifting {
        ui.request_live();
    }

    for c in acts {
        classic::push_media(st, c);
    }
    if open_credit {
        let url = classic::source_url(classic::lyrics_source(&st.lyrics)).to_string();
        st.cmd(Command::OpenUrl(url));
    }
    // As Classic's page: the Add-ons tab's switch; the app downloads it,
    // then reads the song again.
    if get_ja_readings {
        st.settings.set_addon(addons::JAPANESE_READINGS.id, true);
        st.cmd(Command::SaveSettings);
    }
}

/// The player's name in capitals ("SPOTIFY"), with "PAUSED" while it is, as
/// Classic's player line says it.
fn source_label(app_name: &str, playing: bool) -> String {
    let app = app_name.trim().to_uppercase();
    match (app.is_empty(), playing) {
        (false, true) => app,
        (false, false) => format!("{app} \u{b7} PAUSED"),
        (true, false) => "PAUSED".to_string(),
        (true, true) => String::new(),
    }
}

/// Small capitals with a little air between the letters ([`TRACKING`], as
/// on ref1's label), one letter at a time since the text styles carry no
/// letter spacing. Only Latin letters are set apart (other scripts join,
/// shape or run right to left); those, and a name too long for the column,
/// are drawn as ordinary text, ellipsized.
fn tracked(ui: &mut Ui, s: &str, r: Rect, st: &TextStyle) {
    if s.is_empty() || !s.chars().all(|c| (' '..='\u{24f}').contains(&c)) {
        ui.c.text(s, r, st);
        return;
    }
    let mut letters = Vec::with_capacity(s.len());
    let mut x = 0.0f32;
    for (i, c) in s.char_indices() {
        let letter = &s[i..i + c.len_utf8()];
        let w = ui.c.measure(letter, st, 1000.0).0;
        letters.push((letter, x, w));
        x += w + TRACKING;
    }
    if x - TRACKING > r.w {
        ui.c.text(s, r, st);
        return;
    }
    for (letter, dx, w) in letters {
        ui.c.text(letter, Rect::new(r.x + dx, r.y, w + 2.0, r.h), st);
    }
}

/// Nothing playing: the card keeps the player's first row, a blank tile with
/// the note and what to do, and nothing that would pretend to control.
fn draw_empty(ui: &mut Ui, area: Rect) {
    let scale = ui.c.scale().max(0.5);
    let (x0, y0) = (classic::snap(area.x, scale), classic::snap(area.y, scale));
    tile(ui, Rect::new(x0, y0, ART, ART), ART_RADIUS, 1.0);
    let tx = classic::snap(x0 + ART + GAP_ART, scale);
    let tw = (area.right() - tx).max(0.0);
    let title = TextStyle::value();
    ui.c.text("Nothing playing", row(tx, tw, y0 + EMPTY_TITLE_CAP, title.size), &title);
    let hint = TextStyle::body().color(palette::TEXT_DIM);
    ui.c.text("Play music in Spotify, a browser or any media app", row(tx, tw, y0 + EMPTY_HINT_CAP, hint.size), &hint);
}

/// A one-line, vertically centred text box whose capitals start at `cap_top`.
fn row(x: f32, w: f32, cap_top: f32, size: f32) -> Rect {
    Rect::new(x, cap_top - (FONT_ASCENT - FONT_CAP) * size, w, FONT_LINE * size)
}

/// The cover (or a blank tile) scaled about its centre by `k`; on whole
/// pixels when `px_scale` is given (i.e. at rest).
fn draw_art(ui: &mut Ui, art: Option<&Image>, r: Rect, k: f32, px_scale: Option<f32>) {
    let mut rr = Rect::from_center(r.center(), r.w * k, r.h * k);
    if let Some(s) = px_scale {
        let (x0, y0) = (classic::snap(rr.x, s), classic::snap(rr.y, s));
        rr = Rect::new(x0, y0, classic::snap(rr.right(), s) - x0, classic::snap(rr.bottom(), s) - y0);
    }
    let radius = ART_RADIUS * k;
    match art {
        Some(img) => {
            ui.c.image(img, rr, radius);
            // Hairline so dark covers keep an edge on the black card.
            ui.c.stroke_rrect(rr, radius, Color::white(0.08), 1.0);
        }
        None => tile(ui, rr, radius, k),
    }
}

/// A blank tile with a note: no cover, or nothing playing.
fn tile(ui: &mut Ui, r: Rect, radius: f32, k: f32) {
    ui.c.fill_rrect(r, radius, palette::SURFACE);
    let s = TILE_ICON * k;
    ui.c.icon(Icon::Music, Rect::from_center(r.center(), s, s), palette::TEXT_FAINT);
}

fn eq_width() -> f32 {
    EQ_BARS as f32 * EQ_BAR_W + (EQ_BARS - 1) as f32 * EQ_GAP
}

/// How the equalizer shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EqState {
    /// Playing, on frames that come anyway: each bar at its own pace (the
    /// collapsed equalizer's rhythm).
    Dancing,
    /// Playing with no frames coming (a stream with no length and no synced
    /// lyrics): one still snapshot of the dance, so a stray frame (the
    /// pointer moving) never jolts it.
    Still,
    /// Paused: low and dimmed, as Classic dims its player dot.
    Paused,
}

/// A still dance: the bars' heights as a share of the range (the mockup's).
const EQ_STILL: [f32; EQ_BARS] = [0.55, 1.0, 0.25, 0.8];
/// The paused equalizer's opacity (Classic's paused player dot).
const EQ_PAUSED_ALPHA: f32 = 0.45;

/// Four bars ending at `right`.
fn equalizer(ui: &mut Ui, right: f32, cy: f32, color: Color, eq: EqState, t: f64) {
    let x0 = right - eq_width();
    let color = if eq == EqState::Paused { color.mul_alpha(EQ_PAUSED_ALPHA) } else { color };
    for i in 0..EQ_BARS {
        let k = match eq {
            EqState::Dancing => (0.5 + 0.5 * (t * (5.0 + i as f64 * 1.3) + i as f64 * 1.7).sin()) as f32,
            EqState::Still => EQ_STILL[i],
            EqState::Paused => 0.0,
        };
        let h = lerp(EQ_LOW, EQ_HIGH, k);
        let x = x0 + i as f32 * (EQ_BAR_W + EQ_GAP);
        ui.c.fill_rrect(Rect::new(x, cy - h / 2.0, EQ_BAR_W, h), EQ_BAR_W / 2.0, color);
    }
}

/// The album's colour for the equalizer, lifted so a dark or dull cover still
/// reads on the black card (HSL lightness at least 0.62, saturation at least
/// 0.55, as the spec's mockup); a grey cover stays grey. On a `light` look's
/// ground (Diorama by day) a colour is deepened instead (lightness at most
/// 0.42), so a bright cover's bars don't fade into the cream; grey is the
/// look's to paint.
pub fn eq_color(c: Color, light: bool) -> Color {
    let (h, s, l) = to_hsl(c);
    let grey = s < 0.08;
    let s = if grey { s } else { s.max(0.55) };
    let l = if light && !grey { l.min(0.42) } else { l.max(0.62) };
    from_hsl(h, s, l).with_alpha(c.a)
}

fn to_hsl(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (c.r.clamp(0.0, 1.0), c.g.clamp(0.0, 1.0), c.b.clamp(0.0, 1.0));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s.min(1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(1.0) * 6.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::rgba(r + m, g + m, b + m, 1.0)
}

// ---------------------------------------------------------------- controls

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    Play,
    Pause,
    Back,
    Forward,
}

/// A transport control: a bare white glyph on Classic's target, a faint disc
/// under the pointer. A disabled one is drawn inert and takes no input, as in
/// Classic.
fn control(ui: &mut Ui, id: WidgetId, c: Point, hit: f32, g: Glyph, enabled: bool) -> bool {
    if !enabled {
        glyph(ui, g, c, palette::DISABLED);
        return false;
    }
    let resp = ui.button(id, Rect::from_center(c, hit, hit));
    if resp.pressed || resp.hovered {
        ui.c.fill_circle(c, hit / 2.0, Color::white(if resp.pressed { PRESS_INK } else { HOVER_INK }));
    }
    glyph(ui, g, c, if resp.pressed { palette::TEXT.mul_alpha(0.8) } else { palette::TEXT });
    resp.clicked
}

/// Draw `g` centred on `c`, its straight edges on whole pixels.
fn glyph(ui: &mut Ui, g: Glyph, c: Point, color: Color) {
    let s = ui.c.scale().max(0.5);
    let snap = |v: f32| classic::snap(v, s);
    match g {
        Glyph::Pause => {
            let (x, y) = (snap(c.x - PAUSE_GAP / 2.0 - PAUSE_BAR_W), snap(c.y - PAUSE_BAR_H / 2.0));
            for dx in [0.0, PAUSE_BAR_W + PAUSE_GAP] {
                ui.c.fill_rrect(Rect::new(x + dx, y, PAUSE_BAR_W, PAUSE_BAR_H), PAUSE_R, color);
            }
        }
        Glyph::Play => {
            let x = snap(c.x - PLAY_W / 2.0 + PLAY_NUDGE);
            let (top, bottom) = (snap(c.y - PLAY_H / 2.0), snap(c.y + PLAY_H / 2.0));
            let tri = [(x, top), (x + PLAY_W, (top + bottom) / 2.0), (x, bottom)];
            ui.c.path(&rounded(&tri, PLAY_ROUND), Some(color), None);
        }
        Glyph::Back | Glyph::Forward => {
            let half = SKIP_W / 2.0;
            let x0 = snap(c.x - half);
            let (top, bottom) = (snap(c.y - SKIP_H / 2.0), snap(c.y + SKIP_H / 2.0));
            let mid = (top + bottom) / 2.0;
            for i in 0..2 {
                let x = x0 + i as f32 * half;
                let tri = if g == Glyph::Forward { [(x, top), (x + half, mid), (x, bottom)] } else { [(x + half, top), (x, mid), (x + half, bottom)] };
                ui.c.path(&rounded(&tri, SKIP_ROUND), Some(color), None);
            }
        }
    }
}

/// A closed polygon with its corners rounded by about `r` (a quadratic
/// through each corner).
fn rounded(pts: &[(f32, f32)], r: f32) -> Vec<Seg> {
    let n = pts.len();
    let toward = |p: (f32, f32), q: (f32, f32)| {
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let k = (r / (dx * dx + dy * dy).sqrt().max(1e-3)).min(0.5);
        (p.0 + dx * k, p.1 + dy * k)
    };
    let mut segs = Vec::with_capacity(2 * n + 1);
    for i in 0..n {
        let p = pts[i];
        let a = toward(p, pts[(i + n - 1) % n]);
        let b = toward(p, pts[(i + 1) % n]);
        segs.push(if i == 0 { Seg::MoveTo(a.0, a.1) } else { Seg::LineTo(a.0, a.1) });
        segs.push(Seg::QuadTo(p.0, p.1, b.0, b.1));
    }
    segs.push(Seg::Close);
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lyrics_column_is_classics_at_every_width() {
        for width in [480.0f32, 620.0, 760.0, 900.0] {
            let w = width - 2.0 * crate::pages::PAD;
            let cols = columns(w, true, 1.0);
            // Classic: (w × share).min(w − art 104 − gap 16 − gap 20 − middle 176).
            let classic_w = (w * classic::LYRICS_SHARE).min(w - 316.0).round();
            assert_eq!(cols.lyrics_w, Some(classic_w), "at {width}");
            assert!(cols.block_w >= MIN_BLOCK_W - 0.5, "player {} at {width}", cols.block_w);
            assert!((cols.block_w + classic::GAP_LYRICS + classic_w - w).abs() <= 0.5);
        }
        // Lyrics off: the player takes the whole card.
        assert_eq!(columns(584.0, false, 1.0), Columns { block_w: 584.0, lyrics_w: None });
    }

    #[test]
    fn rows_start_their_capitals_where_asked() {
        let r = row(0.0, 100.0, 24.0, 18.0);
        // The line box is centred in the row, so the capitals start ascent − cap below its top.
        assert!((r.y + (FONT_ASCENT - FONT_CAP) * 18.0 - 24.0).abs() < 1e-4);
        assert!((r.h - FONT_LINE * 18.0).abs() < 1e-4);
    }

    #[test]
    fn the_player_name_reads_in_capitals() {
        assert_eq!(source_label("Spotify", true), "SPOTIFY");
        assert_eq!(source_label(" Apple Music ", false), "APPLE MUSIC \u{b7} PAUSED");
        assert_eq!(source_label("", false), "PAUSED");
        assert_eq!(source_label("", true), "");
    }

    #[test]
    fn the_equalizer_lifts_dark_covers_and_keeps_grey_ones_grey() {
        let (_, s, l) = to_hsl(eq_color(Color::hex(0x5A1020), false));
        assert!(l >= 0.619 && s >= 0.549, "lifted to s {s} l {l}");
        // Already bright and vivid: unchanged.
        let pink = Color::hex(0xFF78B0);
        let out = eq_color(pink, false);
        assert!((out.r - pink.r).abs() < 0.01 && (out.g - pink.g).abs() < 0.01 && (out.b - pink.b).abs() < 0.01);
        // Grey stays grey (no hue to lift into).
        let grey = eq_color(Color::rgba(0.3, 0.3, 0.3, 1.0), false);
        assert!((grey.r - grey.g).abs() < 1e-4 && (grey.g - grey.b).abs() < 1e-4 && grey.r >= 0.619);
        // On a light ground a bright cover's colour deepens; grey stays as on black.
        let (_, s, l) = to_hsl(eq_color(pink, true));
        assert!(l <= 0.421 && s >= 0.549, "deepened to s {s} l {l}");
        assert_eq!(eq_color(Color::rgba(0.3, 0.3, 0.3, 1.0), true), grey);
        // Round trip.
        let c = Color::hex(0x3478F6);
        let (h, s, l) = to_hsl(c);
        let back = from_hsl(h, s, l);
        assert!((back.r - c.r).abs() < 1e-3 && (back.g - c.g).abs() < 1e-3 && (back.b - c.b).abs() < 1e-3);
    }

    #[test]
    fn rounded_corners_stay_inside_the_polygon() {
        let segs = rounded(&[(0.0, 0.0), (20.0, 12.0), (0.0, 24.0)], 2.5);
        assert_eq!(segs.len(), 7);
        assert!(matches!(segs[0], Seg::MoveTo(..)) && matches!(segs[6], Seg::Close));
        for s in &segs {
            if let Seg::LineTo(x, y) | Seg::MoveTo(x, y) | Seg::QuadTo(_, _, x, y) = *s {
                assert!((0.0..=20.0).contains(&x) && (0.0..=24.0).contains(&y), "{x}, {y}");
            }
        }
    }
}
