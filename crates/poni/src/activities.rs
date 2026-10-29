//! What the collapsed island shows: idle pill, live activities (a screen
//! recording, media, timer, the minutes before a prayer, the readout over
//! games), HUD
//! pop-ups (volume, charging, Bluetooth) and notices.
//!
//! Collapsed pills are sized to their content: Windows has no camera
//! cut-out to work around, so empty black would only cover the browser tabs.

use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::island::Geom;
use crate::oyen;
use crate::pages::{self, PageKind};
use crate::state::{AppState, FpsAccess, HudKind, InlineAction, Notice};
use crate::ui::{wid, PillStyle, Ui, PILL_SMALL};
use poni_core::datetime::{fmt_clock, fmt_countdown};
use poni_core::frames::fmt_fps;
use poni_core::geom::{clamp01, palette, Color, Point, Rect};
use poni_core::pomodoro::Phase;
use poni_core::settings::IdleStyle;
use poni_core::sysmon::fmt_pct;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Media,
    Timer,
    MediaTimer,
    /// Prayer times: the minutes before the next prayer.
    Prayer,
    /// Show over games: frame rate, GPU and CPU while a full-screen app is in front.
    Game,
    /// A screen recording with Oyenza Studio Recorder: its dot and time.
    Recording,
    /// Meeting notes: Oyen listening (his head moving with the voices, the
    /// red dot and the time), then writing the notes.
    Notes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudTag {
    Volume,
    Charging,
    Bluetooth,
    LowBattery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Idle,
    Activity(Activity),
    Hud(HudTag),
    Notice,
    Expanded(PageKind),
    /// Stepped aside (`AppState::aside`): an app window sits under where
    /// the island rests (a maximized browser's tabs), so what would rest
    /// there (the clock, the song) waits as the Hidden look's invisible
    /// strip until the pointer reaches the top edge.
    Aside,
}

/// A paused player keeps its pill this long before the island goes idle, so
/// a short pause doesn't morph the island away and back.
const MEDIA_GRACE: f64 = 30.0;
/// The media pill, and its width while it peeks at a new track's title.
const MEDIA_W: f32 = 120.0;
const MEDIA_PEEK_W: f32 = 300.0;
/// After a peek ends its text may still draw this long, so it fades out with
/// the narrowing pill instead of vanishing. Outside this window the title
/// never shows (other morphs and the hover lift also widen the pill).
const PEEK_TAIL: f64 = 1.0;
/// The one neutral track under every progress ring.
const RING_TRACK: Color = Color::white(0.14);
/// Right padding of HUD content (the left icons sit 16 DIPs in).
const HUD_PAD: f32 = 16.0;
/// Bolt icon box and its visible extent (the path spans x 5..19 of 24).
const BOLT: f32 = 11.0;
const BOLT_INSET: f32 = BOLT * 5.0 / 24.0;
const BOLT_VISIBLE: f32 = BOLT * 14.0 / 24.0;
/// Gap between the idle clock and its battery badge.
const BADGE_GAP: f32 = 6.0;
/// Oyen theme: the head's width (DIPs) in the idle "Oyen" look, beside the
/// idle clock and in the media pill, and the clock pill's extra width for him.
const OYEN_IDLE_W: f32 = 30.0;
const OYEN_CLOCK_W: f32 = 22.0;
const OYEN_MEDIA_W: f32 = 24.0;
const OYEN_CLOCK_LEAD: f32 = 12.0 + OYEN_CLOCK_W;
/// The countdown before a prayer: the moon, then "Maghrib 10 min" (152,
/// its words' room), then its OK.
const PRAYER_W: f32 = 152.0 + PRAYER_OK_ROOM;
/// The countdown's OK: a small grey pill concentric with the 32 DIP pill's
/// right end, as a notice's Hide is with the notice's. It puts the
/// countdown away until the prayer's time (`AppState::prayer_countdown_ok`).
pub(crate) const PRAYER_OK_ID: &str = "prayer.ok";
const PRAYER_OK_W: f32 = 40.0;
const PRAYER_OK_H: f32 = 24.0;
const PRAYER_OK_INSET: f32 = (32.0 - PRAYER_OK_H) / 2.0;
/// What the OK takes at the pill's right end, from where the words end.
pub(crate) const PRAYER_OK_ROOM: f32 = 12.0 + PRAYER_OK_W + PRAYER_OK_INSET;
/// The readout over games: room for "999 fps", "GPU 100% · 100°" and
/// "CPU 100%" with dividers (tabular digits keep it still as numbers
/// change), and without the temperature when the driver reports none.
const GAME_W: f32 = 276.0;
const GAME_NO_TEMP_W: f32 = 236.0;
const GAME_DIVIDER: Color = Color::white(0.18);
/// The recording: its dot, then "12:34" (the timer's width) or "1:02:03".
const RECORDING_W: f32 = 96.0;
const RECORDING_LONG_W: f32 = 112.0;
/// A meeting being taken down: Oyen's head, the dot and the time; then
/// Oyen with "Writing notes".
const NOTES_W: f32 = 124.0;
const NOTES_LONG_W: f32 = 140.0;
const NOTES_WRITING_W: f32 = 170.0;
const NOTES_HEAD_W: f32 = 24.0;

/// Modes with the same key share content (no cross-fade between them).
pub fn content_key(m: &Mode) -> u32 {
    match m {
        Mode::Idle => 1,
        Mode::Activity(Activity::Media) => 2,
        Mode::Activity(Activity::Timer) => 3,
        Mode::Activity(Activity::MediaTimer) => 4,
        Mode::Activity(Activity::Prayer) => 5,
        Mode::Activity(Activity::Game) => 6,
        Mode::Activity(Activity::Recording) => 7,
        Mode::Activity(Activity::Notes) => 8,
        Mode::Hud(t) => 10 + *t as u32,
        Mode::Notice => 20,
        Mode::Expanded(_) => 30,
        Mode::Aside => 40,
    }
}

fn hud_tag(k: &HudKind) -> HudTag {
    match k {
        HudKind::Volume { .. } => HudTag::Volume,
        HudKind::Charging { .. } => HudTag::Charging,
        HudKind::Bluetooth { .. } => HudTag::Bluetooth,
        HudKind::LowBattery { .. } => HudTag::LowBattery,
    }
}

/// When a paused session's lingering pill ends (`None` while playing or
/// when nothing is lingering).
fn media_grace_end(st: &AppState) -> Option<f64> {
    let m = st.media.as_ref()?;
    if m.is_playing() {
        return None;
    }
    st.media_paused_at.map(|t| t + MEDIA_GRACE)
}

/// The prayer coming up and the minutes left, while its countdown runs and
/// its OK wasn't clicked.
pub fn prayer_countdown(st: &AppState) -> Option<(poni_core::prayer::Prayer, i64)> {
    if !st.prayer_on() {
        return None;
    }
    let site = st.prayer.site()?;
    poni_core::prayer::countdown(&site, st.prayer_now(), st.settings.prayer.remind_before_min)
        .filter(|(m, _)| st.prayer.countdown_ok != Some(m.at))
        .map(|(m, left)| (m.prayer, left))
}

pub fn current_activity(st: &AppState) -> Option<Activity> {
    // A recording comes first: its Stop must be one click away.
    if st.rec_live() && st.record_on() {
        return Some(Activity::Recording);
    }
    // So does a meeting being taken down, and its notes on their way.
    if st.notes.live() && st.notes_on() {
        return Some(Activity::Notes);
    }
    // The few minutes before a prayer come next: they don't come back.
    if prayer_countdown(st).is_some() {
        return Some(Activity::Prayer);
    }
    // Over a game the readout takes the pill; music and timer wait.
    if st.game.active {
        return Some(Activity::Game);
    }
    let media = st.settings.show_media_activity
        && st.media.as_ref().is_some_and(|m| m.is_playing() || media_grace_end(st).is_some_and(|end| st.now < end));
    let timer = st.pomodoro.is_active();
    match (media, timer) {
        (true, true) => Some(Activity::MediaTimer),
        (true, false) => Some(Activity::Media),
        (false, true) => Some(Activity::Timer),
        _ => None,
    }
}

pub fn collapsed_mode(st: &AppState) -> Mode {
    // A notice that stays (a new version's) waits under a volume or
    // brightness change, a game's readout and a recording (its Stop stays
    // one click away), and shows again after.
    let recording = (st.rec_live() && st.record_on()) || (st.notes.listening() && st.notes_on());
    let waits = st.notice.as_ref().is_some_and(|n| n.stays()) && (st.hud.is_some() || st.game.active || recording);
    if st.notice.is_some() && !waits {
        Mode::Notice
    } else if let Some(h) = &st.hud {
        Mode::Hud(hud_tag(&h.kind))
    } else if st.aside && !new_song_peek(st) {
        // What rests waits out of the way; what just happened (the pop-ups
        // and notices above, a new song's title) still drops down.
        Mode::Aside
    } else {
        resting_mode(st)
    }
}

/// What rests on the closed island when nothing pops up: a live activity,
/// else the idle look. Stepping aside asks where this one would sit.
pub fn resting_mode(st: &AppState) -> Mode {
    current_activity(st).map_or(Mode::Idle, Mode::Activity)
}

/// A new song's title is showing for its few seconds (`st.ui.peek_until`).
fn new_song_peek(st: &AppState) -> bool {
    st.now < st.ui.peek_until && matches!(current_activity(st), Some(Activity::Media | Activity::MediaTimer))
}

/// What sits beside the idle clock: a bolt while charging, the percentage
/// when the battery runs low. Nothing otherwise (no ring: it read as a spinner).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClockBadge {
    Charging,
    Low(u8),
}

fn clock_badge(st: &AppState) -> Option<ClockBadge> {
    let p = &st.power;
    if !p.has_battery {
        None
    } else if p.charging {
        Some(ClockBadge::Charging)
    } else if p.percent <= 20 {
        Some(ClockBadge::Low(p.percent))
    } else {
        None
    }
}

pub fn geom(m: &Mode, st: &AppState) -> Geom {
    if let Some(g) = crate::pill::geom(m, st) {
        return g;
    }
    match m {
        Mode::Idle => match st.settings.idle_style {
            // The status bar is the Pill theme's; the other themes show the clock.
            IdleStyle::Clock | IdleStyle::Status => {
                let base = if st.settings.use_24h { 84.0 } else { 104.0 };
                let badge = match clock_badge(st) {
                    Some(ClockBadge::Charging) => 22.0,
                    Some(ClockBadge::Low(_)) => 34.0,
                    None => 0.0,
                };
                let lead = if oyen::on(st.settings.theme) { OYEN_CLOCK_LEAD } else { 0.0 };
                Geom::new(base + badge + lead, 30.0, 14.0)
            }
            IdleStyle::Minimal => Geom::new(150.0, 26.0, 12.0),
            // A little taller than the eyes, so his ears clear the top edge.
            IdleStyle::Eyes if oyen::on(st.settings.theme) => Geom::new(104.0, 36.0, 16.0),
            IdleStyle::Eyes => Geom::new(118.0, 30.0, 14.0),
            IdleStyle::Hidden => hidden_strip(),
        },
        Mode::Activity(Activity::Media) => {
            // A song with synced lyrics sings its line here, as in the Pill
            // theme: the pill is as wide as its words (`pill::media`), and
            // two rows tall when they have readings, the line over them.
            let w = if crate::pill::lyric::sings(st) {
                crate::pill::media::fit_w()
            } else if st.now < st.ui.peek_until {
                MEDIA_PEEK_W
            } else {
                MEDIA_W
            };
            if crate::pill::lyric::reads(st) {
                Geom::new(w, crate::pill::media::H_READS, 22.0)
            } else {
                Geom::new(w, 32.0, 15.0)
            }
        }
        Mode::Activity(Activity::Timer) => {
            // "MM:SS" until the countdown needs another digit.
            let long = fmt_countdown(st.pomodoro.remaining(st.now)).len() > 5;
            Geom::new(if long { 112.0 } else { 96.0 }, 32.0, 15.0)
        }
        Mode::Activity(Activity::MediaTimer) => Geom::new(132.0, 32.0, 15.0),
        Mode::Activity(Activity::Prayer) => Geom::new(PRAYER_W, 32.0, 15.0),
        Mode::Activity(Activity::Game) => Geom::new(if game_temp(st).is_some() { GAME_W } else { GAME_NO_TEMP_W }, 32.0, 15.0),
        Mode::Activity(Activity::Recording) => Geom::new(recording_w(st), 32.0, 15.0),
        Mode::Activity(Activity::Notes) => Geom::new(notes_w(st), 32.0, 15.0),
        Mode::Hud(HudTag::Volume) => Geom::new(316.0, 40.0, 19.0),
        Mode::Hud(HudTag::Charging) | Mode::Hud(HudTag::LowBattery) => Geom::new(300.0, 40.0, 19.0),
        Mode::Hud(HudTag::Bluetooth) => Geom::new(336.0, 46.0, 22.0),
        Mode::Notice => Geom::new(372.0, 52.0, 24.0),
        Mode::Expanded(p) => {
            let h = pages::HEADER_H + pages::content_height(*p, st) + pages::PAD;
            Geom::new(st.settings.expanded_width, h.round(), 30.0)
        }
        Mode::Aside => hidden_strip(),
    }
}

/// The Hidden idle look's island, which a stepped-aside island takes too: a
/// strip at the top edge, drawn invisible, that catches the pointer there.
fn hidden_strip() -> Geom {
    Geom::new(150.0, 5.0, 2.5)
}

/// Needs continuous ambient frames (near 60 fps): animated equalizer while music plays.
/// A paused (lingering) pill is static, once Oyen's notes are gone.
pub fn is_live(m: &Mode, st: &AppState) -> bool {
    if let Some(live) = crate::pill::is_live(m, st) {
        return live;
    }
    let Some(media) = st.media.as_ref() else { return false };
    match m {
        Mode::Activity(Activity::Media) if oyen::on(st.settings.theme) => {
            oyen::music_moving(st.now, media.is_playing(), st.media_paused_at, &st.ui.notes)
        }
        Mode::Activity(Activity::Media) | Mode::Activity(Activity::MediaTimer) => media.is_playing(),
        _ => false,
    }
}

/// Next time the collapsed content changes on its own: eye blinks, a paused
/// pill giving way to idle, the title peek ending. One-shot wakes, so the
/// app stays at zero CPU in between.
pub fn next_wake(m: &Mode, st: &AppState) -> Option<f64> {
    let now = st.now;
    let mut next: Option<f64> = None;
    let mut consider = |t: f64| {
        if t > now {
            next = Some(next.map_or(t, |n: f64| n.min(t)));
        }
    };
    if *m == Mode::Idle && oyen_idle(st) {
        if !night(st) {
            consider(oyen::next_rest_move(now));
        }
    } else if *m == Mode::Idle && st.settings.idle_style == IdleStyle::Eyes {
        let phase = now % BLINK_PERIOD;
        consider(if phase < BLINK_LEN { now - phase + BLINK_LEN } else { now - phase + BLINK_PERIOD });
    }
    if !matches!(m, Mode::Expanded(_)) {
        if let Some(end) = media_grace_end(st) {
            consider(end);
        }
    }
    if *m == Mode::Activity(Activity::Media) {
        consider(st.ui.peek_until);
        // The next sung line: the equalizer's frames stop while paused or
        // on battery saver, and Oyen's when his notes are gone.
        if let Some(t) = crate::pill::lyric::next_wake(st) {
            consider(t);
        }
    }
    if let Some(t) = crate::pill::next_wake(m, st) {
        consider(t);
    }
    next
}

const BLINK_PERIOD: f64 = 5.5;
const BLINK_LEN: f64 = 0.14;

/// Oyen shows in the idle pill (the clock and "Oyen" looks of his theme).
fn oyen_idle(st: &AppState) -> bool {
    oyen::on(st.settings.theme) && matches!(st.settings.idle_style, IdleStyle::Clock | IdleStyle::Status | IdleStyle::Eyes)
}

/// Oyen sleeps from midnight to five.
pub(crate) fn night(st: &AppState) -> bool {
    st.local.hour < 5
}

/// Oyen at rest in the idle pill; he follows the cursor while it hovers.
fn draw_oyen_rest(ui: &mut Ui, st: &AppState, body: Rect, cx: f32, w: f32, bar_y: f32) {
    let look = ui.input.mouse.map(|p| (p.x - body.center().x) / (body.w / 2.0));
    let (frame, moving) = oyen::rest(st.now, night(st), look);
    if moving {
        ui.request_anim();
    }
    oyen::peek(ui.c, &frame, cx, bar_y, w, body.y + 1.0);
}

pub fn draw(ui: &mut Ui, st: &mut AppState, m: &Mode, body: Rect) {
    if crate::pill::draw(ui, st, m, body) {
        return;
    }
    match m {
        Mode::Idle => draw_idle(ui, st, body),
        Mode::Activity(a) => draw_activity(ui, st, *a, body),
        Mode::Hud(_) => draw_hud(ui, st, body),
        Mode::Notice => draw_notice(ui, st, body),
        Mode::Expanded(_) | Mode::Aside => {}
    }
}

/// Snap a DIP coordinate to the device-pixel grid (crisp small outlines at
/// 100 % and 150 %).
fn snap(ui: &Ui, v: f32) -> f32 {
    let s = ui.c.scale().max(0.1);
    (v * s).round() / s
}

fn draw_idle(ui: &mut Ui, st: &mut AppState, body: Rect) {
    let cy = body.center().y;
    match st.settings.idle_style {
        IdleStyle::Clock | IdleStyle::Status => {
            // Oyen theme: he peeks on the left and the clock centres in the rest.
            let body = if oyen::on(st.settings.theme) {
                draw_oyen_rest(ui, st, body, body.x + 11.0 + OYEN_CLOCK_W / 2.0, OYEN_CLOCK_W, body.bottom() - 5.0);
                Rect::new(body.x + OYEN_CLOCK_LEAD, body.y, body.w - OYEN_CLOCK_LEAD, body.h)
            } else {
                body
            };
            let time = fmt_clock(&st.local, st.settings.use_24h);
            let ts = TextStyle::body().weight(Weight::Semibold).tabular();
            let Some(badge) = clock_badge(st) else {
                ui.c.text(&time, body, &ts.center());
                return;
            };
            // Badge + gap + time, centred as one group.
            let (tw, _) = ui.c.measure(&time, &ts, 400.0);
            let pct_ts = TextStyle::footnote().weight(Weight::Semibold).tabular().color(palette::RED);
            let (bw, pct) = match badge {
                ClockBadge::Charging => (BOLT_VISIBLE, String::new()),
                ClockBadge::Low(p) => {
                    let s = format!("{p}%");
                    (ui.c.measure(&s, &pct_ts, 100.0).0, s)
                }
            };
            let x0 = body.center().x - (bw + BADGE_GAP + tw) / 2.0;
            match badge {
                ClockBadge::Charging => {
                    let r = Rect::new(x0 - BOLT_INSET, cy - BOLT / 2.0, BOLT, BOLT);
                    ui.c.icon(Icon::Bolt, r, palette::GREEN);
                }
                ClockBadge::Low(_) => ui.c.text(&pct, Rect::new(x0, body.y, bw + 2.0, body.h), &pct_ts),
            }
            ui.c.text(&time, Rect::new(x0 + bw + BADGE_GAP, body.y, tw + 2.0, body.h), &ts);
        }
        IdleStyle::Eyes if oyen::on(st.settings.theme) => {
            draw_oyen_rest(ui, st, body, body.center().x, OYEN_IDLE_W, body.bottom() - 7.0);
        }
        IdleStyle::Eyes => {
            // Eyes glance toward the cursor while hovering.
            let look = ui.input.mouse.map(|p| ((p.x - body.center().x) / body.w).clamp(-0.5, 0.5) * 6.0).unwrap_or(0.0);
            draw_eyes(ui, st, body.center().x, cy, look);
        }
        IdleStyle::Minimal | IdleStyle::Hidden => {}
    }
}

/// The Eyes look's pair of eyes, centred on (`cx`, `cy`) and turned `look`
/// DIPs sideways, blinking on their own wake ([`next_wake`]). A Mac's notch
/// draws them beside the camera (`mac/notch.rs`).
pub fn draw_eyes(ui: &mut Ui, st: &AppState, cx: f32, cy: f32, look: f32) {
    let blink = (st.now % BLINK_PERIOD) < BLINK_LEN;
    for dx in [-11.0f32, 11.0] {
        let c = Point::new(cx + dx + look, cy);
        let h = if blink { 2.5 } else { 11.0 };
        ui.c.fill_rrect(Rect::from_center(c, 6.5, h), 3.25, Color::white(0.95));
    }
}

/// Little equalizer (4 bars): animated while playing, low and static when paused.
fn equalizer(ui: &mut Ui, r: Rect, color: Color, playing: bool, t: f64) {
    let n = 4;
    let gap = 2.5;
    let bw = (r.w - gap * (n as f32 - 1.0)) / n as f32;
    for i in 0..n {
        let phase = t * (5.0 + i as f64 * 1.3) + i as f64 * 1.7;
        let amp = if playing { 0.35 + 0.65 * (0.5 + 0.5 * phase.sin()) as f32 } else { 0.25 };
        let h = (r.h * amp).max(3.0);
        let x = r.x + i as f32 * (bw + gap);
        ui.c.fill_rrect(Rect::new(x, r.center().y - h / 2.0, bw, h), bw / 2.0, color);
    }
}

/// Album art (or a note glyph), dimmed while the player is paused.
fn art_or_icon(ui: &mut Ui, st: &AppState, r: Rect, playing: bool) {
    let art = st.media.as_ref().and_then(|m| m.art.as_ref());
    ui.c.with_alpha(if playing { 1.0 } else { 0.55 }, |c| match art {
        Some(art) => c.image(art, r, 6.0),
        None => {
            c.fill_rrect(r, 6.0, palette::SURFACE_HOVER);
            c.icon(Icon::Music, r.inset(4.0, 4.0), palette::TEXT_DIM);
        }
    });
}

fn phase_color(p: Phase) -> Color {
    match p {
        Phase::Focus => palette::ORANGE,
        Phase::ShortBreak | Phase::LongBreak => palette::GREEN,
    }
}

fn countdown_style(col: Color, paused: bool) -> TextStyle {
    TextStyle::body().weight(Weight::Semibold).tabular().color(if paused { palette::TEXT_DIM } else { col })
}

/// Alpha of the title peek: it grows with the pill (so the growing body
/// never cuts the text) and only around a peek.
fn peek_alpha(st: &AppState, w: f32) -> f32 {
    let p = st.ui.peek_until;
    if p <= 0.0 || st.now >= p + PEEK_TAIL {
        return 0.0;
    }
    clamp01((w - MEDIA_W) / (MEDIA_PEEK_W - MEDIA_W))
}

/// "Title · artist" between `x0` and `x1`, title first; the artist only
/// when there is room for more than an ellipsis.
fn draw_peek(ui: &mut Ui, st: &AppState, x0: f32, x1: f32, body: Rect, alpha: f32) {
    let Some(m) = st.media.as_ref() else { return };
    let (main, rest) = if m.title.trim().is_empty() { (m.artist.trim(), "") } else { (m.title.trim(), m.artist.trim()) };
    let avail = x1 - x0;
    if main.is_empty() || avail < 20.0 {
        return;
    }
    let title_st = TextStyle::body().weight(Weight::Semibold);
    let artist_st = TextStyle::body().color(palette::TEXT_DIM);
    ui.c.with_alpha(alpha, |c| {
        let (tw, _) = c.measure(main, &title_st, 10_000.0);
        if tw >= avail {
            c.text(main, Rect::new(x0, body.y, avail, body.h), &title_st);
            return;
        }
        c.text(main, Rect::new(x0, body.y, tw + 2.0, body.h), &title_st);
        let room = avail - tw;
        if !rest.is_empty() && room >= 32.0 {
            c.text(&format!(" \u{b7} {rest}"), Rect::new(x0 + tw, body.y, room, body.h), &artist_st);
        }
    });
}

/// Music, collapsed: the album art on the left, the equalizer (Oyen, in his
/// theme) on the right, and between them the line being sung while the song
/// has synced lyrics, as in the Pill theme (`pill::media::middle`: the line,
/// with its reading in Latin letters under it when it has one, or the title
/// in the intro, the breaks and a new track's peek), else a new track's
/// title for a moment. In the menu bar's line of a MacBook with a
/// notch the camera takes the middle, so there it draws no words (`words`
/// false): a sung line hangs below the notch instead (`mac/notch.rs`).
pub fn draw_music(ui: &mut Ui, st: &mut AppState, body: Rect, words: bool) {
    let cy = body.center().y;
    let art = Rect::new(body.x + 12.0, cy - 11.0, 22.0, 22.0);
    let playing = st.media.as_ref().is_some_and(|m| m.is_playing());
    art_or_icon(ui, st, art, playing);
    let eq = Rect::new(body.right() - 34.0, cy - 8.0, 20.0, 16.0);
    let sings = words && crate::pill::lyric::sings(st);
    let alpha = if sings { 1.0 } else { peek_alpha(st, body.w) };
    if oyen::on(st.settings.theme) {
        // Oyen bops to the music where the equalizer would be, and a
        // note floats up beside him on every beat (the ones on his
        // left make way for the words: the sung line or the title).
        let (cx, bar_y) = (body.right() - 26.0, body.bottom() - 5.0);
        st.ui.notes.step(st.now, playing);
        let sky = Rect::new(body.x, body.y + 1.0, body.w, body.h - 1.0);
        oyen::notes(ui.c, &st.ui.notes, st.now, cx, bar_y, sky, 1.0 - alpha);
        let f = oyen::music(st.now, playing, st.media_paused_at);
        oyen::peek(ui.c, &f, cx, bar_y, OYEN_MEDIA_W, body.y + 1.0);
    } else {
        equalizer(ui, eq, st.media_accent(), playing, st.now);
    }
    if sings {
        use crate::pill::media::{middle, PAUSED_TEXT, TEXT_CLEAR};
        middle(ui, st, body, art.right() + TEXT_CLEAR, eq.x - TEXT_CLEAR, if playing { 1.0 } else { PAUSED_TEXT });
    } else if alpha > 0.01 {
        draw_peek(ui, st, art.right() + 10.0, eq.x - 10.0, body, alpha);
    }
}

fn draw_activity(ui: &mut Ui, st: &mut AppState, a: Activity, body: Rect) {
    let cy = body.center().y;
    let art = Rect::new(body.x + 12.0, cy - 11.0, 22.0, 22.0);
    let playing = st.media.as_ref().is_some_and(|m| m.is_playing());
    match a {
        Activity::Media => draw_music(ui, st, body, true),
        Activity::Timer => {
            // Ring (with the phase glyph) on the left, digits right after it.
            let p = &st.pomodoro;
            let col = phase_color(p.phase);
            let paused = !p.is_running();
            let c = Point::new(body.x + 12.0 + 9.0, cy);
            ui.c.ring(c, 9.0, 2.4, p.progress(st.now), RING_TRACK, col);
            let glyph = Rect::from_center(c, 9.0, 9.0);
            if paused {
                ui.c.icon(Icon::Pause, glyph, palette::TEXT_DIM);
            } else if p.phase != Phase::Focus {
                ui.c.icon(Icon::Coffee, glyph, col);
            }
            let text = fmt_countdown(p.remaining(st.now));
            ui.c.text(&text, Rect::new(body.x + 40.0, body.y, 64.0, body.h), &countdown_style(col, paused));
        }
        Activity::Prayer => {
            // No Oyen here, in any theme: the moon, the minutes and OK only.
            let Some((prayer, left)) = prayer_countdown(st) else { return };
            let col = st.accent;
            ui.c.icon(Icon::Moon, Rect::new(body.x + 14.0, cy - 8.0, 16.0, 16.0), col);
            let text = format!("{} {left} min", prayer.name());
            let ts = TextStyle::body().weight(Weight::Semibold).tabular().color(col).right();
            let end = draw_prayer_ok(ui, body);
            ui.c.text(&text, Rect::new(body.x + 36.0, body.y, end - body.x - 36.0, body.h), &ts);
        }
        Activity::Game => draw_game(ui, st, body),
        Activity::Recording => draw_recording(ui, st, body),
        Activity::Notes => draw_notes(ui, st, body),
        Activity::MediaTimer => {
            // Art on the left; the ring sits right next to its countdown.
            art_or_icon(ui, st, art, playing);
            let p = &st.pomodoro;
            let col = phase_color(p.phase);
            let paused = !p.is_running();
            let text = fmt_countdown(p.remaining(st.now));
            let ts = countdown_style(col, paused);
            let (tw, _) = ui.c.measure(&text, &ts, 200.0);
            let right = body.right() - 14.0;
            ui.c.text(&text, Rect::new(right - 64.0, body.y, 64.0, body.h), &ts.right());
            let c = Point::new(right - tw - 6.0 - 7.0, cy);
            ui.c.ring(c, 7.0, 2.2, p.progress(st.now), RING_TRACK, col);
        }
    }
}

/// The countdown's OK in the right end of its pill `body` (the app acts on
/// it at mouse-down, as on a notice's inline pill). Returns where the words
/// end.
pub(crate) fn draw_prayer_ok(ui: &mut Ui, body: Rect) -> f32 {
    let r = Rect::new(body.right() - PRAYER_OK_INSET - PRAYER_OK_W, body.center().y - PRAYER_OK_H / 2.0, PRAYER_OK_W, PRAYER_OK_H);
    ui.pill(wid(PRAYER_OK_ID), r, "OK", None, PillStyle::Utility);
    body.right() - PRAYER_OK_ROOM
}

/// The meeting's pill: listening, "0:42" (an hour needs more); then the
/// notes on their way.
fn notes_w(st: &AppState) -> f32 {
    match st.notes.phase {
        poni_core::notes::Phase::Listening { .. } if st.notes.elapsed(st.now) >= 3600.0 => NOTES_LONG_W,
        poni_core::notes::Phase::Listening { .. } => NOTES_W,
        _ => NOTES_WRITING_W,
    }
}

/// Oyen's head on the left, listening (it lifts with the voices) or
/// writing; then the red dot and the time, or "Writing notes". In every
/// theme: the notes are his.
fn draw_notes(ui: &mut Ui, st: &AppState, body: Rect) {
    let cy = body.center().y;
    let listening = st.notes.listening();
    let what = if listening { crate::oyen::Coach::Listen { level: st.notes.mic.max(st.notes.pc) } } else { crate::oyen::Coach::Write };
    crate::oyen::coach(ui.c, Point::new(body.x + 12.0 + NOTES_HEAD_W / 2.0, cy - 1.0), NOTES_HEAD_W, ui.now, what);
    // The head follows the voices, and the pencil its line.
    ui.request_live();
    let x = body.x + 12.0 + NOTES_HEAD_W + 8.0;
    if listening {
        ui.c.fill_circle(Point::new(x + 5.0, cy), 5.0, palette::RED);
        let ts = TextStyle::body().weight(Weight::Semibold).tabular().color(palette::TEXT);
        let text = poni_core::notes::fmt_length(st.notes.elapsed(st.now));
        ui.c.text(&text, Rect::new(x + 14.0, body.y, body.right() - 14.0 - x - 14.0, body.h), &ts.right());
    } else {
        let ts = TextStyle::footnote().weight(Weight::Semibold).color(palette::TEXT_DIM);
        ui.c.text("Writing notes\u{2026}", Rect::new(x, body.y, body.right() - 12.0 - x, body.h), &ts);
    }
}

/// The recording pill: "0:42" fits the timer's width; an hour needs more.
fn recording_w(st: &AppState) -> f32 {
    if st.rec_elapsed() >= 3600.0 {
        RECORDING_LONG_W
    } else {
        RECORDING_W
    }
}

/// The red dot, then the time right-aligned as the timer's countdown.
fn draw_recording(ui: &mut Ui, st: &AppState, body: Rect) {
    recording_mark(ui, st, Point::new(body.x + 12.0 + 6.0, body.center().y));
    let (text, ts) = recording_time(st);
    ui.c.text(&text, Rect::new(body.x + 30.0, body.y, body.w - 30.0 - 14.0, body.h), &ts.right());
}

/// The recording's mark at `c`: the red dot; a solid pause mark while
/// paused, so the dimmed time still reads on the light Oyen ground; a ring
/// while it starts or saves. Shared with the notch's left ear.
pub(crate) fn recording_mark(ui: &mut Ui, st: &AppState, c: Point) {
    match st.rec.phase {
        Some(poni_core::recorder::Phase::Recording) => ui.c.fill_circle(c, 5.0, palette::RED),
        Some(poni_core::recorder::Phase::Paused) => ui.c.icon(Icon::Pause, Rect::from_center(c, 11.0, 11.0), palette::TEXT),
        _ => ui.c.stroke_circle(c, 4.4, palette::TEXT_DIM, 1.4),
    }
}

/// The recording's time and its style (dimmed unless it records), for the
/// caller to align. Shared with the notch's right ear.
pub(crate) fn recording_time(st: &AppState) -> (String, TextStyle) {
    let recording = st.rec.phase == Some(poni_core::recorder::Phase::Recording);
    let ts = TextStyle::body().weight(Weight::Semibold).tabular().color(if recording { palette::TEXT } else { palette::TEXT_DIM });
    (poni_core::recorder::fmt_length(st.rec_elapsed()), ts)
}

/// The GPU's temperature for the readout, when its driver reports one.
fn game_temp(st: &AppState) -> Option<f32> {
    st.system.latest.as_ref()?.gpu.as_ref()?.temp_c
}

/// "144 fps | GPU 97% · 71° | CPU 38%", centred. The frame rate is a dim
/// dash until Windows lets Poni read frames and while the game isn't
/// drawing; the rest comes from the System page's readers.
fn draw_game(ui: &mut Ui, st: &AppState, body: Rect) {
    let reading = st.system.latest.as_ref();
    let label = TextStyle::footnote().color(palette::TEXT_DIM);
    let value = TextStyle::body().weight(Weight::Semibold).tabular();
    let big = TextStyle::headline().weight(Weight::Bold).tabular();
    let dash = || "\u{2013}".to_string();
    let fps = match (st.game.rate, &st.game.access) {
        (Some(r), _) => [(fmt_fps(r.fps), big), (" fps".into(), label)],
        (None, FpsAccess::Ready | FpsAccess::Unknown | FpsAccess::Checking) => [(dash(), big), (" fps".into(), label)],
        // Waiting for the one-time OK: the slot stays, dimmer.
        (None, _) => [(dash(), big.color(palette::DISABLED)), (" fps".into(), label.color(palette::DISABLED))],
    };
    let gpu = match reading.and_then(|r| r.gpu.as_ref()) {
        Some(g) => match g.temp_c {
            Some(t) => format!("{} \u{b7} {t:.0}\u{b0}", fmt_pct(g.util)),
            None => fmt_pct(g.util),
        },
        None => dash(),
    };
    let cpu = reading.and_then(|r| r.cpu.as_ref()).map_or_else(dash, |c| fmt_pct(c.load));
    let groups = [fps, [("GPU ".into(), label), (gpu, value)], [("CPU ".into(), label), (cpu, value)]];

    const GAP: f32 = 11.0;
    let widths: Vec<[f32; 2]> = groups.iter().map(|g| g.each_ref().map(|(t, ts)| ui.c.measure(t, ts, 200.0).0)).collect();
    let total = widths.iter().flatten().sum::<f32>() + (groups.len() - 1) as f32 * (2.0 * GAP + 1.0);
    let mut x = (body.center().x - total / 2.0).max(body.x + 12.0);
    let cy = body.center().y;
    for (i, (group, ws)) in groups.iter().zip(&widths).enumerate() {
        if i > 0 {
            x += GAP;
            ui.c.fill_rect(Rect::new(snap(ui, x), cy - 8.0, 1.0, 16.0), GAME_DIVIDER);
            x += 1.0 + GAP;
        }
        for ((t, ts), w) in group.iter().zip(ws) {
            // Smaller words sit a pixel low, nearer the numbers' baseline.
            let dy = if ts.size < value.size { 1.0 } else { 0.0 };
            ui.c.text(t, Rect::new(x, body.y + dy, w + 2.0, body.h), ts);
            x += w;
        }
    }
}

fn draw_hud(ui: &mut Ui, st: &mut AppState, body: Rect) {
    let Some(hud) = st.hud.clone() else { return };
    let cy = body.center().y;
    let icon_r = Rect::new(body.x + 16.0, cy - 10.0, 20.0, 20.0);
    let label_st = TextStyle::body().weight(Weight::Semibold);
    match hud.kind {
        HudKind::Volume { level, muted } => {
            let icon = if muted || level <= 0.001 {
                Icon::VolumeMute
            } else if level < 0.5 {
                Icon::VolumeLow
            } else {
                Icon::Volume
            };
            ui.c.icon(icon, icon_r, palette::TEXT);
            // The bar stops 62 DIPs short so "100%" and "Muted" keep a gap.
            let bar = Rect::new(body.x + 48.0, cy - 3.0, body.w - 48.0 - 62.0, 6.0);
            // The bar glides to new levels; the number is exact immediately.
            let target = if muted { 0.0 } else { level };
            let cur = st.ui.hud_level;
            let shown = if (cur - target).abs() > 0.002 {
                ui.request_anim();
                cur + (target - cur) * 0.32
            } else {
                target
            };
            st.ui.hud_level = shown;
            ui.c.fill_rrect(bar, 3.0, palette::TRACK);
            if shown > 0.0 {
                // Classic fills the level in white; a look with its own accent (Oyen's) in it.
                let level = match ui.c.look().accent {
                    Some(a) => a,
                    None => Color::white(0.95),
                };
                ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * shown).max(6.0), bar.h), 3.0, level);
            }
            let pct = if muted { "Muted".to_string() } else { format!("{}%", (level * 100.0).round() as i32) };
            ui.c.text(&pct, Rect::new(body.right() - HUD_PAD - 48.0, body.y, 48.0, body.h), &label_st.tabular().right());
        }
        HudKind::Charging { percent, plugged } => {
            let (icon, color, label) = if plugged { (Icon::Bolt, palette::GREEN, "Charging") } else { (Icon::Battery, palette::TEXT, "On battery") };
            ui.c.icon(icon, icon_r, color);
            ui.c.text(label, Rect::new(body.x + 46.0, body.y, 140.0, body.h), &label_st);
            battery_badge(ui, body.right() - HUD_PAD, cy, percent, color);
        }
        HudKind::LowBattery { percent } => {
            ui.c.icon(Icon::Battery, icon_r, palette::RED);
            ui.c.text("Low battery", Rect::new(body.x + 46.0, body.y, 140.0, body.h), &label_st);
            battery_badge(ui, body.right() - HUD_PAD, cy, percent, palette::RED);
        }
        HudKind::Bluetooth { name, connected, battery, audio } => {
            let c = Point::new(body.x + 30.0, cy);
            ui.c.fill_circle(c, 15.0, if connected { palette::BLUE } else { Color::white(0.16) });
            let glyph = if connected { palette::ON_ACCENT } else { palette::TEXT };
            ui.c.icon(if audio { Icon::Headphones } else { Icon::Bluetooth }, Rect::from_center(c, 17.0, 17.0), glyph);
            let text_x = body.x + 54.0;
            let text_right = match battery {
                Some(b) => battery_badge(ui, body.right() - HUD_PAD, cy, b, if b <= 20 { palette::RED } else { palette::GREEN }) - 12.0,
                None => body.right() - HUD_PAD,
            };
            let tw = text_right - text_x;
            ui.c.text(&name, Rect::new(text_x, cy - 17.0, tw, 18.0), &label_st);
            ui.c.text(
                if connected { "Connected" } else { "Disconnected" },
                Rect::new(text_x, cy + 1.0, tw, 16.0),
                &TextStyle::footnote().color(palette::TEXT_DIM),
            );
        }
    }
}

/// A small drawn battery and the percentage, right-aligned to `right` and
/// centred on `cy`. Returns the badge's left edge.
fn battery_badge(ui: &mut Ui, right: f32, cy: f32, percent: u8, color: Color) -> f32 {
    let ts = TextStyle::body().weight(Weight::Semibold).tabular();
    let label = format!("{percent}%");
    let (tw, _) = ui.c.measure(&label, &ts, 100.0);
    let tx = right - tw;
    ui.c.text(&label, Rect::new(tx, cy - 10.0, tw + 2.0, 20.0), &ts);
    // 20x10 outline, a 1.5x4 nub on the right, 6 DIPs before the number.
    let x = snap(ui, tx - 6.0 - 1.5 - 20.0);
    let y = snap(ui, cy - 5.0);
    ui.c.stroke_rrect(Rect::new(x, y, 20.0, 10.0), 3.0, palette::TEXT_DIM, 1.3);
    ui.c.fill_rrect(Rect::new(x + 20.0, y + 3.0, 1.5, 4.0), 0.75, palette::TEXT_DIM);
    let p = (percent as f32 / 100.0).clamp(0.0, 1.0);
    if p > 0.0 {
        ui.c.fill_rrect(Rect::new(x + 2.0, y + 2.0, (16.0 * p).max(3.0), 6.0), 1.5, color);
    }
    x
}

/// The inline action already happened: the pill shows a check until the
/// notice leaves (the app gives it 0.6 s more).
fn inline_done(st: &AppState, a: InlineAction) -> bool {
    match a {
        InlineAction::StartFocus => st.pomodoro.is_active(),
        // Oyenza: the sum is in place once Show took it.
        InlineAction::ShowAmount => st.notice.as_ref().is_some_and(|n| n.reveal.is_none()),
        InlineAction::StopAdzan => !st.prayer.adzan,
        // Done once clicked: the app puts the check in the notice's icon.
        InlineAction::WhatsNew => st.notice.as_ref().is_some_and(|n| n.icon == Icon::Check),
        // Hide and Open take the notice away at once.
        InlineAction::Hide | InlineAction::Open => false,
    }
}

fn draw_notice(ui: &mut Ui, st: &mut AppState, body: Rect) {
    let Some(n) = st.notice.clone() else { return };
    let cy = body.center().y;
    // Disc and inline pill keep the same 16 DIP inset from the ends.
    let c = Point::new(body.x + 32.0, cy);
    ui.c.fill_circle(c, 16.0, n.color.with_alpha(0.22));
    ui.c.icon(n.icon, Rect::from_center(c, 18.0, 18.0), n.color);

    let inline = n.inline.map(|a| {
        let (label, icon, color) = inline_look(st, &n, a);
        let w = ui.pill_width(label, icon.is_some(), true).max(if icon.is_some() { 76.0 } else { 0.0 });
        let r = Rect::new(body.right() - 16.0 - w, cy - PILL_SMALL / 2.0, w, PILL_SMALL);
        (a, label, icon, color, r)
    });

    let tx = c.x + 16.0 + 10.0;
    let text_right = inline.as_ref().map_or(body.right() - 22.0, |(.., r)| r.x - 12.0);
    let tw = (text_right - tx).max(0.0);
    notice_title(ui, &n, Rect::new(tx, cy - 18.0, tw, 20.0), TextStyle::headline());
    ui.c.text(&n.subtitle, Rect::new(tx, cy + 1.0, tw, 16.0), &TextStyle::footnote().color(palette::TEXT_DIM));

    // The whole notice is a button (handled on mouse-down by the app); the
    // inline pill is registered after it so it wins the hit test.
    ui.button(wid("notice"), body);
    if let Some((a, label, icon, color, r)) = inline {
        if inline_done(st, a) {
            ui.c.fill_rrect(r, r.h / 2.0, color.with_alpha(0.20));
            ui.c.icon(Icon::Check, Rect::from_center(r.center(), 14.0, 14.0), color);
        } else {
            let style = if icon.is_some() { PillStyle::Primary(color) } else { PillStyle::Utility };
            ui.pill(wid("notice.act"), r, label, icon, style);
        }
    }
}

/// An inline action's label, icon and colour. Hide is a plain grey pill
/// beside the notice's own words (no icon, no colour), as in Screen Studio.
pub(crate) fn inline_look(st: &AppState, n: &Notice, a: InlineAction) -> (&'static str, Option<Icon>, Color) {
    match a {
        // Orange for focus; a break starts in its own green.
        InlineAction::StartFocus => ("Start", Some(Icon::Play), phase_color(st.pomodoro.phase)),
        InlineAction::ShowAmount => ("Show", Some(Icon::Eye), n.color),
        InlineAction::StopAdzan => ("Stop", Some(Icon::Stop), n.color),
        InlineAction::WhatsNew => ("What's new", Some(Icon::Open), n.color),
        // Another app's notification: open that app.
        InlineAction::Open => ("Open", Some(Icon::Open), n.color),
        InlineAction::Hide => ("Hide", None, palette::TEXT_DIM),
    }
}

/// Space between a notice's title and its dimmer words.
const TITLE_DIM_GAP: f32 = 5.0;

/// A notice's title in `r`, then its dimmer words after it on the line
/// (`title_dim`: the version after "Update available") while they fit.
pub(crate) fn notice_title(ui: &mut Ui, n: &Notice, r: Rect, style: TextStyle) {
    text_with_dim(ui, &n.title, &n.title_dim, r, style);
}

/// `text` in `r`, then `dim` after it on the line in a dimmer colour while
/// it fits (also the Update sheet's title in Settings).
pub(crate) fn text_with_dim(ui: &mut Ui, text: &str, dim: &str, r: Rect, style: TextStyle) {
    if dim.is_empty() {
        ui.c.text(text, r, &style);
        return;
    }
    let w = ui.c.measure(text, &style, r.w.max(1.0)).0.ceil().min(r.w);
    ui.c.text(text, Rect::new(r.x, r.y, w + 1.0, r.h), &style);
    let x = r.x + w + TITLE_DIM_GAP;
    if r.right() > x {
        ui.c.text(dim, Rect::new(x, r.y, r.right() - x, r.h), &style.color(palette::TEXT_DIM));
    }
}

/// How wide [`notice_title`] draws the title and its dimmer words.
pub(crate) fn notice_title_width(ui: &mut Ui, n: &Notice, style: &TextStyle) -> f32 {
    text_with_dim_width(ui, &n.title, &n.title_dim, style)
}

/// How wide [`text_with_dim`] draws `text` and `dim` with room to spare.
pub(crate) fn text_with_dim_width(ui: &mut Ui, text: &str, dim: &str, style: &TextStyle) -> f32 {
    let w = ui.c.measure(text, style, 2000.0).0;
    if dim.is_empty() {
        w
    } else {
        w.ceil() + TITLE_DIM_GAP + ui.c.measure(dim, style, 2000.0).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{LyricsState, LyricsStatus, MediaSession};
    use poni_core::lrc::parse_lrc;
    use poni_core::settings::Settings;
    use poni_core::store::Store;
    use poni_core::timeline::Timeline;

    /// Music playing 12 s into a song, with these lyrics found for it.
    fn playing(lrc: Option<&str>) -> AppState {
        let mut st = AppState::new(Settings::default(), Store::default());
        let key = MediaSession::make_track_key("Paper Boats", "The Lanterns", "");
        st.media = Some(MediaSession {
            app_id: "Spotify.exe".into(),
            app_name: "Spotify".into(),
            title: "Paper Boats".into(),
            artist: "The Lanterns".into(),
            album: String::new(),
            art: None,
            art_color: None,
            timeline: Timeline::new(12.0, 200.0, 0.0, true),
            can_prev: true,
            can_next: true,
            can_play_pause: true,
            can_seek: true,
            track_key: key.clone(),
        });
        if let Some(l) = lrc {
            st.lyrics = LyricsState { track_key: key, status: LyricsStatus::Found, lyrics: Some(parse_lrc(l)), source: "Sample".into(), ruby: Vec::new(), wants_ja_readings: false };
        }
        st
    }

    #[test]
    fn music_sings_its_line_in_every_theme_while_the_song_has_synced_lyrics() {
        let media = Mode::Activity(Activity::Media);
        // No lyrics: Classic's compact pill, art and bars.
        assert_eq!(geom(&media, &playing(None)).w, MEDIA_W);
        // Synced lyrics: as wide as its words (what `pill::media` measured).
        let sung = playing(Some("[00:10.00]Paper lanterns on the line\n[00:14.00]Count the windows\n"));
        assert!(crate::pill::lyric::sings(&sung));
        assert_eq!(geom(&media, &sung).w, crate::pill::media::fit_w());
        // Lyrics switched off, or words without timings: compact again.
        let mut off = playing(Some("[00:10.00]Paper lanterns on the line\n"));
        off.settings.lyrics = false;
        assert_eq!(geom(&media, &off).w, MEDIA_W);
        assert_eq!(geom(&media, &playing(Some("plain words\nno stamps\n"))).w, MEDIA_W);
        // Paused, a singing pill keeps its size (the title shows after a while).
        let mut paused = sung;
        paused.media.as_mut().unwrap().timeline = Timeline::new(12.0, 200.0, 0.0, false);
        assert!(crate::pill::lyric::sings(&paused));
    }

    #[test]
    fn music_with_readings_is_two_rows_tall_in_every_theme() {
        use poni_core::romanize::Ruby;
        use poni_core::settings::Theme;
        let media = Mode::Activity(Activity::Media);
        let mut st = playing(Some("[00:10.00]紙の舟よ\n[00:14.00]Paper boats\n"));
        assert_eq!(geom(&media, &st).h, 32.0, "no readings: one row");
        st.lyrics.ruby = vec![vec![Ruby { text: "紙の舟よ".into(), roman: "kami no fune yo".into() }], Vec::new()];
        for theme in [Theme::Classic, Theme::Oyen, Theme::Glass, Theme::Pill] {
            st.settings.theme = theme;
            assert_eq!(geom(&media, &st).h, crate::pill::media::H_READS, "{theme:?}: the line over its reading");
        }
        // Readings switched off: one row again, as for a song without them.
        st.settings.lyrics_pronunciation = false;
        assert_eq!(geom(&media, &st).h, 32.0);
    }

    /// Prayer times on in Kota Jakarta with a song singing its line, the
    /// prayer clock `secs` before today's Ashar; and today's times there.
    fn before_ashar(secs: i64) -> (AppState, poni_core::prayer::DayTimes) {
        use poni_core::addons::{Status, PRAYER_TIMES};
        let mut st = playing(Some("[00:10.00]Paper lanterns on the line\n[00:14.00]Count the windows\n"));
        st.addons.insert(PRAYER_TIMES.id, Status::Ready);
        let list = poni_core::prayer::PlaceList::parse(
            r#"{"places":[{"id":"jakarta","label":"Kota Jakarta","kind":"kota","prov":"DKI Jakarta","lat":-6.1833,"lng":106.8333,"tz":"Asia/Jakarta"}]}"#,
        )
        .expect("the place");
        st.prayer.place = list.find("jakarta").cloned();
        st.prayer.places = Some(std::sync::Arc::new(list));
        let site = st.prayer.site().expect("its site");
        let times = poni_core::prayer::day_times(&site, site.local_day(st.unix)).expect("today's times");
        st.prayer.clock_shift = times.ashar - secs - st.unix;
        (st, times)
    }

    #[test]
    fn ok_on_the_prayer_countdown_gives_the_island_back_until_that_prayer() {
        use poni_core::prayer::Prayer;
        let (mut st, times) = before_ashar(8 * 60);
        // The countdown comes before the song...
        assert_eq!(prayer_countdown(&st), Some((Prayer::Ashar, 8)));
        assert_eq!(collapsed_mode(&st), Mode::Activity(Activity::Prayer));
        // ...until OK: the song and its line again, until Ashar.
        assert_eq!(st.prayer_countdown_ok().map(|m| (m.prayer, m.at)), Some((Prayer::Ashar, times.ashar)));
        assert_eq!(prayer_countdown(&st), None);
        assert_eq!(collapsed_mode(&st), Mode::Activity(Activity::Media));
        st.prayer.clock_shift += 7 * 60;
        assert_eq!(prayer_countdown(&st), None, "a minute before Ashar");
        // Maghrib counts down as ever.
        st.prayer.clock_shift = times.maghrib - 5 * 60 - st.unix;
        assert_eq!(prayer_countdown(&st), Some((Prayer::Maghrib, 5)));
        assert_eq!(collapsed_mode(&st), Mode::Activity(Activity::Prayer));
    }

    #[test]
    fn ok_with_no_countdown_running_puts_nothing_away() {
        let (mut st, _) = before_ashar(30 * 60);
        assert_eq!(prayer_countdown(&st), None);
        assert_eq!(st.prayer_countdown_ok(), None);
        assert_eq!(st.prayer.countdown_ok, None);
        // Ashar's countdown still comes in its time.
        st.prayer.clock_shift += 25 * 60;
        assert_eq!(prayer_countdown(&st).map(|(p, _)| p), Some(poni_core::prayer::Prayer::Ashar));
    }

    #[test]
    fn a_notice_that_stays_waits_under_a_volume_change_and_a_games_readout() {
        let mut st = AppState::new(Settings::default(), Store::default());
        st.notify(Icon::Download, palette::GREEN, "Update available", "", f64::INFINITY, None);
        assert!(st.notice.as_ref().is_some_and(|n| n.stays()));
        assert_eq!(collapsed_mode(&st), Mode::Notice);
        st.hud = Some(crate::state::Hud { kind: HudKind::Volume { level: 0.5, muted: false }, until: st.now + 2.0 });
        assert_eq!(collapsed_mode(&st), Mode::Hud(HudTag::Volume));
        st.hud = None;
        st.game.active = true;
        assert_eq!(collapsed_mode(&st), Mode::Activity(Activity::Game));
        st.game.active = false;
        assert_eq!(collapsed_mode(&st), Mode::Notice, "and back once they're gone");
        // A notice with a time limit still shows over both.
        st.notify(Icon::Download, palette::GREEN, "Saved", "", 5.0, None);
        st.hud = Some(crate::state::Hud { kind: HudKind::Volume { level: 0.5, muted: false }, until: st.now + 2.0 });
        st.game.active = true;
        assert_eq!(collapsed_mode(&st), Mode::Notice);
    }

    #[test]
    fn a_notice_that_stays_waits_under_a_recording() {
        let mut st = AppState::new(Settings::default(), Store::default());
        st.recorder = true;
        st.store.recorder_island = true;
        st.notify(Icon::Download, palette::GREEN, "Update available", "", f64::INFINITY, None);
        st.rec.phase = Some(poni_core::recorder::Phase::Recording);
        assert_eq!(collapsed_mode(&st), Mode::Activity(Activity::Recording), "Stop stays one click away");
        st.rec.phase = Some(poni_core::recorder::Phase::Paused);
        assert_eq!(collapsed_mode(&st), Mode::Activity(Activity::Recording));
        st.rec.phase = None;
        assert_eq!(collapsed_mode(&st), Mode::Notice, "and back once the recording ends");
        // A notice with a time limit still shows over it.
        st.rec.phase = Some(poni_core::recorder::Phase::Recording);
        st.notify(Icon::Download, palette::GREEN, "Saved", "", 5.0, None);
        assert_eq!(collapsed_mode(&st), Mode::Notice);
    }

    #[test]
    fn stepped_aside_what_rests_waits_and_what_happens_still_shows() {
        let media = Mode::Activity(Activity::Media);
        let mut idle = AppState::new(Settings::default(), Store::default());
        assert_eq!(collapsed_mode(&idle), Mode::Idle);
        idle.aside = true;
        assert_eq!(collapsed_mode(&idle), Mode::Aside);
        assert_eq!(resting_mode(&idle), Mode::Idle, "where it would rest doesn't change");
        // The strip is the Hidden look's, whatever the idle look.
        assert_eq!(geom(&Mode::Aside, &idle), hidden_strip());
        idle.settings.idle_style = IdleStyle::Hidden;
        assert_eq!(geom(&Mode::Idle, &idle), geom(&Mode::Aside, &idle));

        let mut song = playing(Some("[00:10.00]Paper lanterns on the line\n"));
        song.aside = true;
        assert_eq!(collapsed_mode(&song), Mode::Aside);
        assert_eq!(resting_mode(&song), media);
        // A new song's title drops down for its few seconds, then waits again.
        song.ui.peek_until = song.now + 3.0;
        assert_eq!(collapsed_mode(&song), media);
        song.now += 3.0;
        assert_eq!(collapsed_mode(&song), Mode::Aside);
        // Pop-ups and notices show over the window as before.
        song.hud = Some(crate::state::Hud { kind: HudKind::Volume { level: 0.5, muted: false }, until: song.now + 2.0 });
        assert_eq!(collapsed_mode(&song), Mode::Hud(HudTag::Volume));
        // Nothing moving on the strip: no frames while it waits.
        song.hud = None;
        assert!(!is_live(&Mode::Aside, &song));
        assert!(is_live(&media, &song));
    }
}
