//! What the collapsed island shows: idle pill, live activities (media,
//! timer), HUD pop-ups (volume, charging, Bluetooth) and notices.
//!
//! Collapsed pills are sized to their content: Windows has no camera
//! cut-out to work around, so empty black would only cover the browser tabs.

use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::island::Geom;
use crate::oyen;
use crate::pages::{self, PageKind};
use crate::state::{AppState, HudKind, InlineAction};
use crate::ui::{wid, PillStyle, Ui, PILL_SMALL};
use poni_core::datetime::{fmt_clock, fmt_countdown};
use poni_core::geom::{clamp01, palette, Color, Point, Rect};
use poni_core::pomodoro::Phase;
use poni_core::settings::IdleStyle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Media,
    Timer,
    MediaTimer,
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

/// Modes with the same key share content (no cross-fade between them).
pub fn content_key(m: &Mode) -> u32 {
    match m {
        Mode::Idle => 1,
        Mode::Activity(Activity::Media) => 2,
        Mode::Activity(Activity::Timer) => 3,
        Mode::Activity(Activity::MediaTimer) => 4,
        Mode::Hud(t) => 10 + *t as u32,
        Mode::Notice => 20,
        Mode::Expanded(_) => 30,
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

pub fn current_activity(st: &AppState) -> Option<Activity> {
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
    if st.notice.is_some() {
        Mode::Notice
    } else if let Some(h) = &st.hud {
        Mode::Hud(hud_tag(&h.kind))
    } else if let Some(a) = current_activity(st) {
        Mode::Activity(a)
    } else {
        Mode::Idle
    }
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
    match m {
        Mode::Idle => match st.settings.idle_style {
            IdleStyle::Clock => {
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
            IdleStyle::Hidden => Geom::new(150.0, 5.0, 2.5),
        },
        Mode::Activity(Activity::Media) => {
            let w = if st.now < st.ui.peek_until { MEDIA_PEEK_W } else { MEDIA_W };
            Geom::new(w, 32.0, 15.0)
        }
        Mode::Activity(Activity::Timer) => {
            // "MM:SS" until the countdown needs another digit.
            let long = fmt_countdown(st.pomodoro.remaining(st.now)).len() > 5;
            Geom::new(if long { 112.0 } else { 96.0 }, 32.0, 15.0)
        }
        Mode::Activity(Activity::MediaTimer) => Geom::new(132.0, 32.0, 15.0),
        Mode::Hud(HudTag::Volume) => Geom::new(316.0, 40.0, 19.0),
        Mode::Hud(HudTag::Charging) | Mode::Hud(HudTag::LowBattery) => Geom::new(300.0, 40.0, 19.0),
        Mode::Hud(HudTag::Bluetooth) => Geom::new(336.0, 46.0, 22.0),
        Mode::Notice => Geom::new(372.0, 52.0, 24.0),
        Mode::Expanded(p) => {
            let h = pages::HEADER_H + pages::content_height(*p, st) + pages::PAD;
            Geom::new(st.settings.expanded_width, h.round(), 30.0)
        }
    }
}

/// Needs continuous ambient frames (near 60 fps): animated equalizer while music plays.
/// A paused (lingering) pill is static, once Oyen's notes are gone.
pub fn is_live(m: &Mode, st: &AppState) -> bool {
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
    }
    next
}

const BLINK_PERIOD: f64 = 5.5;
const BLINK_LEN: f64 = 0.14;

/// Oyen shows in the idle pill (the clock and "Oyen" looks of his theme).
fn oyen_idle(st: &AppState) -> bool {
    oyen::on(st.settings.theme) && matches!(st.settings.idle_style, IdleStyle::Clock | IdleStyle::Eyes)
}

/// Oyen sleeps from midnight to five.
fn night(st: &AppState) -> bool {
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
    match m {
        Mode::Idle => draw_idle(ui, st, body),
        Mode::Activity(a) => draw_activity(ui, st, *a, body),
        Mode::Hud(_) => draw_hud(ui, st, body),
        Mode::Notice => draw_notice(ui, st, body),
        Mode::Expanded(_) => {}
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
        IdleStyle::Clock => {
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
            let blink = (st.now % BLINK_PERIOD) < BLINK_LEN;
            // Eyes glance toward the cursor while hovering.
            let look = ui.input.mouse.map(|p| ((p.x - body.center().x) / body.w).clamp(-0.5, 0.5) * 6.0).unwrap_or(0.0);
            for dx in [-11.0f32, 11.0] {
                let c = Point::new(body.center().x + dx + look, cy);
                let h = if blink { 2.5 } else { 11.0 };
                ui.c.fill_rrect(Rect::from_center(c, 6.5, h), 3.25, Color::white(0.95));
            }
        }
        IdleStyle::Minimal | IdleStyle::Hidden => {}
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

fn draw_activity(ui: &mut Ui, st: &mut AppState, a: Activity, body: Rect) {
    let cy = body.center().y;
    let art = Rect::new(body.x + 12.0, cy - 11.0, 22.0, 22.0);
    let playing = st.media.as_ref().is_some_and(|m| m.is_playing());
    match a {
        Activity::Media => {
            art_or_icon(ui, st, art, playing);
            let eq = Rect::new(body.right() - 34.0, cy - 8.0, 20.0, 16.0);
            let alpha = peek_alpha(st, body.w);
            if oyen::on(st.settings.theme) {
                // Oyen bops to the music where the equalizer would be, and a
                // note floats up beside him on every beat (the ones on his
                // left make way for a new track's title).
                let (cx, bar_y) = (body.right() - 26.0, body.bottom() - 5.0);
                st.ui.notes.step(st.now, playing);
                let sky = Rect::new(body.x, body.y + 1.0, body.w, body.h - 1.0);
                oyen::notes(ui.c, &st.ui.notes, st.now, cx, bar_y, sky, 1.0 - alpha);
                let f = oyen::music(st.now, playing, st.media_paused_at);
                oyen::peek(ui.c, &f, cx, bar_y, OYEN_MEDIA_W, body.y + 1.0);
            } else {
                equalizer(ui, eq, st.media_accent(), playing, st.now);
            }
            if alpha > 0.01 {
                draw_peek(ui, st, art.right() + 10.0, eq.x - 10.0, body, alpha);
            }
        }
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
                // Classic fills the level in white; a light look in its accent.
                let level = match ui.c.look().accent.filter(|_| ui.c.look().light) {
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
        let (label, icon, color) = match a {
            // Orange for focus; a break starts in its own green.
            InlineAction::StartFocus => ("Start".to_string(), Icon::Play, phase_color(st.pomodoro.phase)),
            InlineAction::ShowAmount => ("Show".to_string(), Icon::Eye, n.color),
        };
        let w = ui.pill_width(&label, true, true).max(76.0);
        let r = Rect::new(body.right() - 16.0 - w, cy - PILL_SMALL / 2.0, w, PILL_SMALL);
        (a, label, icon, color, r)
    });

    let tx = c.x + 16.0 + 10.0;
    let text_right = inline.as_ref().map_or(body.right() - 22.0, |(.., r)| r.x - 12.0);
    let tw = (text_right - tx).max(0.0);
    ui.c.text(&n.title, Rect::new(tx, cy - 18.0, tw, 20.0), &TextStyle::headline());
    ui.c.text(&n.subtitle, Rect::new(tx, cy + 1.0, tw, 16.0), &TextStyle::footnote().color(palette::TEXT_DIM));

    // The whole notice is a button (handled on mouse-down by the app); the
    // inline pill is registered after it so it wins the hit test.
    ui.button(wid("notice"), body);
    if let Some((a, label, icon, color, r)) = inline {
        if inline_done(st, a) {
            ui.c.fill_rrect(r, r.h / 2.0, color.with_alpha(0.20));
            ui.c.icon(Icon::Check, Rect::from_center(r.center(), 14.0, 14.0), color);
        } else {
            ui.pill(wid("notice.act"), r, &label, Some(icon), PillStyle::Primary(color));
        }
    }
}
