//! The Diorama theme's resting pill: a small window onto a pixel-art place
//! that keeps the day's hours, with the time written in its sky. The
//! picture is `studio_diorama`'s; this is the app's side of it: what the
//! moment is (the clock, the sun's times, the battery), where the glass
//! ball is, and the painting kept between frames.
//!
//! Diorama is the Pill theme's island ([`super::on`]), so everything but
//! the resting pill is Pill's: music, timers, pop-ups and the open card.
//! Whatever idle look is picked, the theme rests as its picture: the clock
//! is the picture with the time in it, Minimal the picture alone. Only
//! Hidden hides it, as it hides every theme's island. The theme has no
//! eyes and no status bar of its own, so its pickers offer neither
//! ([`super::status::looks`]); a setting left on one of them from another
//! theme shows the picture alone, or with the clock.
//!
//! The glass ball can be slid by hand, like the day and night switch it
//! comes from: press it and the pointer carries it along the pane, while
//! the place runs on through the day with it, through the golden hour and
//! the sunset into the night (or on through the small hours into the
//! morning). Let go, it rolls on to the end it was nearer or thrown toward;
//! a tap sends it to the other end. Left on the far side of the day, the
//! theme shows that side, the open card and every other pill too
//! ([`night`]), until the clock comes round to it: by day, night falls 45
//! minutes after the next sunset anyway; at night, the morning comes 50
//! minutes after sunrise.
//!
//! Nothing new runs at rest. The picture is painted when the minute tick
//! redraws the pill (the app already wakes for it, for the clock) and kept
//! as an image until something it shows changes. Twice a day the glass ball
//! rolls, at sunset to the tree and at sunrise back to the house, and once
//! when the theme is picked: about a second and a half of frames, asked
//! for only while the pill is on screen to show them. A slide draws a frame
//! when the pointer moves, and the ball's way on to its end a few tenths of
//! a second more.
//!
//! The sun rises at six and sets at six, the house's own day, unless the
//! Prayer times add-on has a place: then it rises and sets when it does
//! there (terbit and maghrib).

use crate::gfx::{Image, ImageData};
use crate::island::Geom;
use crate::state::AppState;
use crate::ui::{wid, CursorKind, Ui};
use poni_core::datetime::days_from_civil;
use poni_core::geom::Rect;
use poni_core::settings::{IdleStyle, Theme};
use std::cell::{Cell, RefCell};
use studio_diorama::{Badge, Clock, Moment, View};

/// The glass ball's roll from one end of the pane to the other (seconds).
const ROLL: f64 = 1.5;
/// How much larger than the resting pill the picture gets with the body:
/// the swelling under the pointer ([`super::motion::HOVER_OUTSET`] on each
/// side) and room for its springs to overshoot.
const SWELL: f32 = 2.0 * super::motion::HOVER_OUTSET + 4.0;
/// The sun's hours when no place says otherwise: six to six.
const HOUSE_DAY: (f32, f32) = (360.0, 1080.0);
/// 1 January 2000 in days since 1970, where `Moment::day` counts from.
const DAY_ZERO: i64 = 10_957;

/// The glass ball as a widget: press it to slide it, or tap it.
pub const BALL_ID: &str = "diorama.ball";
/// A tap rolls the ball to the other end in this long (seconds).
const FLIP: f64 = 1.1;
/// How far the pointer goes (DIPs) before a press on the ball is a slide.
const SLIDE_SLOP: f32 = 2.0;
/// Let go at this speed or faster (ends a second), the ball carries on to
/// the end it was thrown toward, even the farther one.
const FLING: f32 = 2.5;
/// A pointer that rested this long (seconds) before letting go threw nothing.
const FLING_STALE: f64 = 0.08;

/// Everything the painting shows: it is painted again when one changes.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    moment: Moment,
    view: View,
}

/// The glass ball's way: from `from` to `to`, started at `t0` (mono
/// seconds), over `dur` seconds.
#[derive(Clone, Copy)]
struct Ball {
    from: f32,
    to: f32,
    t0: f64,
    dur: f64,
    /// Sent by hand from the clock's moment then: the place runs through
    /// the day with it (`Moment::slid`). The sun's own rolls (and the theme
    /// being picked) keep the clock's minute.
    hand: Option<Moment>,
    /// Let go while moving: it carries on and comes to rest gently. Else it
    /// sets off gently too.
    thrown: bool,
}

impl Ball {
    /// A roll from `from` to `to` starting at `now`: the sun's, at its pace.
    fn roll(from: f32, to: f32, now: f64) -> Ball {
        Ball { from, to, t0: now, dur: ROLL, hand: None, thrown: false }
    }

    /// Where it is at `now`, and whether it is still rolling.
    fn at(&self, now: f64) -> (f32, bool) {
        let t = ((now - self.t0) / self.dur.max(1e-3)).clamp(0.0, 1.0) as f32;
        let eased = if self.thrown { 1.0 - (1.0 - t).powi(3) } else { t * t * t * (t * (6.0 * t - 15.0) + 10.0) };
        (self.from + (self.to - self.from) * eased, t < 1.0 && self.from != self.to)
    }
}

/// The ball left at the other end from where the sun has it: the place
/// keeps that side of the day, as it is at `until` (minutes since 1 January
/// 2000), until the clock comes round to that minute.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Left {
    night: bool,
    until: f64,
}

impl Left {
    /// Still kept with the clock at `now`: not come to yet, and not more
    /// than a day off (a clock set back, another time zone).
    fn holds(&self, now: f64) -> bool {
        now < self.until && self.until - now <= 1440.0
    }
}

/// The ball in someone's hand.
#[derive(Clone, Copy)]
struct Held {
    /// The pointer where it pressed (DIPs across the window) and the ball then.
    from_x: f32,
    from_lens: f32,
    lens: f32,
    /// DIPs from one end to the other.
    travel: f32,
    /// Past [`SLIDE_SLOP`]: a slide, not a tap.
    moved: bool,
    /// When it last moved (mono seconds) and how fast (ends a second).
    t: f64,
    speed: f32,
    /// The clock's moment at the press, which the slide runs on from.
    from: Moment,
}

thread_local! {
    /// The last painting and what it shows.
    static KEPT: RefCell<Option<(Key, Image)>> = const { RefCell::new(None) };
    static BALL: Cell<Option<Ball>> = const { Cell::new(None) };
    /// The ball in hand, from the press to the letting go.
    static HELD: Cell<Option<Held>> = const { Cell::new(None) };
    /// Where it was left by hand, against the sun.
    static LEFT: Cell<Option<Left>> = const { Cell::new(None) };
    /// The Prayer times place's sunrise and sunset, worked out once a
    /// quarter of an hour: the whole theme's look asks for them every frame.
    static SUN: Cell<Option<(SunKey, (f32, f32))>> = const { Cell::new(None) };
    /// Another theme has drawn since the picture last showed: when it shows
    /// again, the theme was just picked, and the ball rolls into place.
    static PICKED: Cell<bool> = const { Cell::new(false) };
}

/// The theme the island draws in this frame (`pill::geom` says so for every
/// frame, open or closed), to know when Diorama was just picked.
pub fn note_theme(theme: Theme) {
    if theme != Theme::Diorama {
        PICKED.with(|p| p.set(true));
    }
}

/// Another theme draws the island: let the painting go, the renderer's
/// copy of it too.
pub fn put_away(ui: &mut Ui) {
    if let Some((_, image)) = KEPT.with(|kept| kept.borrow_mut().take()) {
        ui.c.forget_image(&image);
    }
    HELD.with(|h| h.set(None));
    LEFT.with(|l| l.set(None));
}

/// Put the ball where the time has it, with no roll on the way (the
/// self-test and the Mac's renders, whose pictures must not depend on what
/// was drawn before them).
pub fn settle() {
    BALL.with(|b| b.set(None));
    PICKED.with(|p| p.set(false));
    HELD.with(|h| h.set(None));
    LEFT.with(|l| l.set(None));
}

/// The resting pill is the picture: in the Diorama theme, whatever the
/// idle look, unless it is Hidden.
pub fn shows(st: &AppState) -> bool {
    st.settings.theme == Theme::Diorama && st.settings.idle_style != IdleStyle::Hidden
}

/// The theme shows its night: the glass ball rests on the tree, where the
/// sun being down puts it, or a hand left it (or holds it past halfway).
/// The open card, the pop-ups and every other pill of the theme take its
/// night look then, and its day look otherwise.
pub fn night(st: &AppState) -> bool {
    if let Some(h) = HELD.with(|h| h.get()) {
        return h.lens >= 0.5;
    }
    resting(&moment(st)).0 >= 0.5
}

/// The time is written in the picture's sky: for the clock (and the status
/// bar, which is the Pill theme's own); not for Minimal (or Eyes).
fn timed(st: &AppState) -> bool {
    matches!(st.settings.idle_style, IdleStyle::Clock | IdleStyle::Status)
}

/// The resting pill's size, or `None` where it isn't the picture.
pub fn geom(st: &AppState) -> Option<Geom> {
    shows(st).then(|| Geom::new(studio_diorama::width(clock(st), badge(st)), studio_diorama::H, studio_diorama::H / 2.0))
}

/// The time in the sky; Minimal is the place alone.
fn clock(st: &AppState) -> Option<Clock> {
    timed(st).then_some(Clock { hour: st.local.hour, minute: st.local.minute, h24: st.settings.use_24h })
}

/// Beside the time, as Classic's clock has it: a bolt while the battery
/// charges, the battery in red at 20 % or less.
fn badge(st: &AppState) -> Option<Badge> {
    let p = &st.power;
    if !p.has_battery || !timed(st) {
        None
    } else if p.charging {
        Some(Badge::Charging)
    } else if p.percent <= 20 {
        Some(Badge::Low(p.percent))
    } else {
        None
    }
}

/// What the Prayer place's sun times were worked out for: the place, whose
/// schedule it follows, the quarter of an hour and this computer's clock
/// against UTC.
#[derive(Clone, Copy, PartialEq)]
struct SunKey {
    lat: u64,
    lng: u64,
    follow: poni_core::settings::PrayerFollow,
    quarter: i64,
    ahead: i64,
}

/// When the sun rises and sets, in minutes of this computer's day: at the
/// Prayer times add-on's place when it has one, else six to six.
fn sun(st: &AppState) -> (f32, f32) {
    let Some(place) = st.prayer.place.as_ref().filter(|_| st.prayer_on()) else { return HOUSE_DAY };
    // This computer's clock against UTC, to the minute.
    let local = st.local.hour as i64 * 3600 + st.local.minute as i64 * 60 + st.local.second as i64;
    let ahead = (((local - st.unix.rem_euclid(86_400)) as f64 / 60.0).round() as i64 * 60 + 43_200).rem_euclid(86_400) - 43_200;
    let key = SunKey { lat: place.lat.to_bits(), lng: place.lng.to_bits(), follow: st.settings.prayer.follow, quarter: st.prayer_now().div_euclid(900), ahead };
    if let Some((_, times)) = SUN.with(|s| s.get()).filter(|(k, _)| *k == key) {
        return times;
    }
    let times = st.prayer_site().and_then(|site| sun_at(&site, st.prayer_now(), ahead)).unwrap_or(HOUSE_DAY);
    SUN.with(|s| s.set(Some((key, times))));
    times
}

/// Sunrise and sunset (the schedule's terbit and maghrib) at `site` on its
/// own date at `now` (UTC seconds), as minutes of the day on a clock
/// `ahead` seconds east of UTC: the place's sun on the clock the picture's
/// time comes from. `None` where the day has no sunrise or sunset, or one
/// the picture can't show (the sun up or down for less than two hours).
fn sun_at(site: &poni_core::prayer::Site, now: i64, ahead: i64) -> Option<(f32, f32)> {
    let t = poni_core::prayer::day_times(site, site.local_day(now))?;
    let minute = |utc: i64| ((utc + ahead).div_euclid(60).rem_euclid(1440)) as f32;
    let (rise, set) = (minute(t.terbit), minute(t.maghrib));
    (set - rise >= 120.0 && set - rise <= 1320.0).then_some((rise, set))
}

/// The moment the pill shows, the ball where the time has it.
fn moment(st: &AppState) -> Moment {
    let (sunrise, sunset) = sun(st);
    let l = &st.local;
    let mut m = Moment {
        minute: (l.hour as u32 * 60 + l.minute as u32) as f32,
        sunrise,
        sunset,
        day: (days_from_civil(l.year, l.month, l.day) - DAY_ZERO) as i32,
        lens: 0.0,
        clock: clock(st),
        badge: badge(st),
        dark: None,
    };
    m.lens = m.rest_lens();
    m
}

/// A moment's minute counted from 1 January 2000.
fn minutes(m: &Moment) -> f64 {
    m.day as f64 * 1440.0 + m.minute as f64
}

/// Put the place of `m` at `t` (minutes since 1 January 2000): its minute
/// and its date (the moon's), not the time written in its sky.
fn show_at(m: &mut Moment, t: f64) {
    let day = (t / 1440.0).floor();
    m.day = day as i32;
    m.minute = (t - day * 1440.0) as f32;
}

/// What the place shows with the ball slid `lens` of the way across from
/// the clock's moment `from`, in minutes since 1 January 2000.
fn slid_to(from: &Moment, lens: f32) -> f64 {
    minutes(&from.slid(lens))
}

/// Where the ball rests and the minute the place shows at rest, for the
/// clock's moment `m`: the sun's end and the clock's minute, or the end a
/// hand left it at and that side of the day, while it holds.
fn resting(m: &Moment) -> (f32, f64) {
    let now = minutes(m);
    match LEFT.with(|l| l.get()).filter(|l| l.holds(now)) {
        Some(l) => (if l.night { 1.0 } else { 0.0 }, l.until),
        None => (m.rest_lens(), now),
    }
}

/// Where the ball is at `now` on its way to `rest`, whether it rolls, and
/// the day a hand sent it with.
fn ball(rest: f32, now: f64) -> (f32, bool, Option<Moment>) {
    let picked = PICKED.with(|p| p.replace(false));
    let b = match BALL.with(|b| b.get()) {
        // Just picked: it rolls in from the other end, and the day (or the
        // night) comes with it.
        _ if picked => Ball::roll(1.0 - rest, rest, now),
        Some(b) if b.to == rest => b,
        // The sun set or rose: on its way from wherever it is.
        Some(b) => Ball::roll(b.at(now).0, rest, now),
        None => Ball::roll(rest, rest, now),
    };
    BALL.with(|c| c.set(Some(b)));
    let (lens, rolling) = b.at(now);
    (lens, rolling, b.hand)
}

/// A press at `x` (DIPs across the window) on the glass ball ([`BALL_ID`]):
/// the ball is in hand until [`let_go`], from wherever it was, resting or
/// rolling. Nothing where the resting pill isn't the picture.
pub fn grab(st: &AppState, x: f32) {
    let Some(rest) = geom(st) else { return };
    let m = moment(st);
    let now = st.now;
    let lens = match BALL.with(|b| b.take()) {
        Some(b) => b.at(now).0,
        None => resting(&m).0,
    };
    let travel = (rest.w - rest.h).max(1.0);
    HELD.with(|h| h.set(Some(Held { from_x: x, from_lens: lens, lens, travel, moved: false, t: now, speed: 0.0, from: m })));
}

/// The pointer at `x` (DIPs across the window) at `now` (mono seconds)
/// with the ball in hand: it follows the pointer along the pane. `false` =
/// no ball in hand.
pub fn slide(x: f32, now: f64) -> bool {
    HELD.with(|h| {
        let Some(mut held) = h.get() else { return false };
        let dx = x - held.from_x;
        if !held.moved && dx.abs() < SLIDE_SLOP {
            return true;
        }
        held.moved = true;
        let lens = (held.from_lens + dx / held.travel).clamp(0.0, 1.0);
        let dt = (now - held.t) as f32;
        if dt > 1e-3 {
            let v = (lens - held.lens) / dt;
            // A move after a rest starts the speed afresh; quick ones even it out.
            held.speed = if dt > 0.1 { v } else { 0.6 * held.speed + 0.4 * v };
            held.t = now;
        }
        held.lens = lens;
        h.set(Some(held));
        true
    })
}

/// Let go of the ball at `now` (mono seconds). A tap (it never moved) sends
/// it to the other end; a slide on to the end it was thrown toward, or
/// else the nearer one. Left at the other end from the sun's, the place
/// keeps that side of the day until the clock comes round to it. `false` =
/// no ball in hand.
pub fn let_go(st: &AppState, now: f64) -> bool {
    let Some(held) = HELD.with(|h| h.take()) else { return false };
    let speed = if now - held.t > FLING_STALE { 0.0 } else { held.speed };
    let to = if !held.moved {
        if held.from_lens >= 0.5 {
            0.0
        } else {
            1.0
        }
    } else if speed.abs() >= FLING {
        if speed > 0.0 {
            1.0
        } else {
            0.0
        }
    } else if held.lens >= 0.5 {
        1.0
    } else {
        0.0
    };
    let (dur, thrown) = if held.moved { (0.16 + 0.44 * (to - held.lens).abs() as f64, true) } else { (FLIP, false) };
    BALL.with(|b| b.set(Some(Ball { from: held.lens, to, t0: now, dur, hand: Some(held.from), thrown })));
    let night = to >= 0.5;
    let sun_night = moment(st).night();
    LEFT.with(|l| l.set((night != sun_night).then(|| Left { night, until: slid_to(&held.from, to) })));
    true
}

/// A ball is in someone's hand.
pub fn held() -> bool {
    HELD.with(|h| h.get().is_some())
}

/// The side of the day a hand left the ball on against the sun (`true`:
/// the night), while the theme keeps it; `None` when it follows the sun.
pub fn left_by_hand(st: &AppState) -> Option<bool> {
    let now = minutes(&moment(st));
    LEFT.with(|l| l.get()).filter(|l| l.holds(now)).map(|l| l.night)
}

/// In thousandths: the eye sees no finer, and a settled ball is exact.
fn thousandths(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

/// Where the glass ball is in the picture drawn at `at` (the resting pill
/// `rest`, centred in it), `lens` of the way across: the square end it
/// fills, which takes the pointer.
fn ball_rect(at: Rect, rest: Geom, lens: f32) -> Rect {
    let (aw, ah) = (rest.w.round(), rest.h.round());
    let (x0, y0) = (at.x + (at.w - aw) / 2.0, at.y + (at.h - ah) / 2.0);
    let cx = x0 + ah / 2.0 + lens.clamp(0.0, 1.0) * (aw - ah);
    Rect::new(cx - ah / 2.0, y0, ah, ah)
}

/// Draw the resting pill as the picture. `false` = it isn't (another theme
/// or look): Pill draws it.
pub fn draw(ui: &mut Ui, st: &mut AppState, body: Rect) -> bool {
    let Some(rest) = geom(st) else { return false };
    let m = moment(st);
    // The clock came round to where the ball was left: it follows the sun again.
    let now = minutes(&m);
    LEFT.with(|l| {
        if l.get().is_some_and(|left| !left.holds(now)) {
            l.set(None)
        }
    });
    let (rest_lens, shown) = resting(&m);
    let mut pic = match HELD.with(|h| h.get()) {
        // In hand: it goes where the pointer takes it, and the day with it.
        Some(h) => h.from.slid(h.lens),
        None => {
            let (lens, rolling, hand) = ball(rest_lens, st.now);
            if rolling {
                ui.request_anim();
            }
            match hand.filter(|_| rolling) {
                // On its way to an end from a hand: the day goes on with it.
                Some(from) => from.slid(lens),
                // Resting (or the sun's roll): the minute it shows there.
                None => {
                    let mut p = m;
                    show_at(&mut p, shown);
                    p.lens = lens;
                    p
                }
            }
        }
    };
    // The time in its sky (and the badge) are the clock's, now.
    pic.clock = m.clock;
    pic.badge = m.badge;
    pic.lens = thousandths(pic.lens);
    pic.dark = pic.dark.map(thousandths);
    let m = pic;

    // Under the pointer the body swells and the frame with it; the pane
    // keeps its size. The picture is the body's size for as far as that
    // swelling goes (and its springs overshoot); a body still on its way
    // here from another shape is larger than that, and the picture waits
    // in its top middle, its frame at its thickest.
    let (w, h) = (body.w.clamp(rest.w, rest.w + SWELL), body.h.clamp(rest.h, rest.h + SWELL));
    let scale = ui.c.scale();
    let at = super::shape::snap(Rect::new(body.center().x - w / 2.0, body.y, w, h), scale);
    let key = Key { moment: m, view: View { w: at.w, h: at.h, rest_w: rest.w, rest_h: rest.h, scale } };

    let image = KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        match kept.as_ref() {
            Some((k, image)) if *k == key => image.clone(),
            _ => {
                let p = studio_diorama::render(&key.moment, &key.view);
                let image = ImageData::new(p.w, p.h, p.bgra);
                if let Some((_, old)) = kept.replace((key, image.clone())) {
                    ui.c.forget_image(&old);
                }
                image
            }
        }
    });
    ui.c.image_exact(&image, at, at.h / 2.0);
    // The ball takes the pointer: press it to slide it, or tap it.
    ui.interact(wid(BALL_ID), ball_rect(at, rest, m.lens), CursorKind::Hand);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PowerState;
    use poni_core::datetime::LocalDateTime;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    fn state(hour: u8, minute: u8) -> AppState {
        let mut st = AppState::new(Settings { theme: Theme::Diorama, ..Settings::default() }, Store::default());
        st.local = LocalDateTime { year: 2026, month: 11, day: 10, hour, minute, second: 30 };
        st.power = PowerState { has_battery: false, percent: 100, charging: false, on_ac: true, saver: false };
        st
    }

    #[test]
    fn the_picture_rests_in_its_theme_for_the_clock_and_minimal() {
        let mut st = state(10, 8);
        // As long as a notch, whatever the clock.
        assert_eq!(geom(&st), Some(Geom::new(190.0, 38.0, 19.0)));
        st.settings.use_24h = false;
        assert_eq!(geom(&st), Some(Geom::new(190.0, 38.0, 19.0)));
        // The status bar is the Pill theme's: here it is the clock.
        st.settings.idle_style = IdleStyle::Status;
        assert_eq!(moment(&st).clock.map(|c| c.h24), Some(false));
        // Minimal is the place alone, and so is a setting left on Eyes.
        for style in [IdleStyle::Minimal, IdleStyle::Eyes] {
            st.settings.idle_style = style;
            assert_eq!(geom(&st), Some(Geom::new(190.0, 38.0, 19.0)), "{style:?}");
            assert_eq!(moment(&st).clock, None, "{style:?}");
        }
        // Hidden hides it, and every other theme rests its own way.
        st.settings.idle_style = IdleStyle::Hidden;
        assert_eq!(geom(&st), None);
        st.settings.idle_style = IdleStyle::Clock;
        for theme in [Theme::Classic, Theme::Oyen, Theme::Glass, Theme::Pill] {
            st.settings.theme = theme;
            assert_eq!(geom(&st), None, "{theme:?}");
        }
    }

    #[test]
    fn the_moment_is_the_computers_clock_and_the_houses_day() {
        let m = moment(&state(17, 5));
        assert_eq!((m.minute, m.sunrise, m.sunset), (1025.0, 360.0, 1080.0));
        // 10 November 2026 is 9,810 days after 1 January 2000.
        assert_eq!(m.day, 9810);
        assert_eq!(m.clock, Some(Clock { hour: 17, minute: 5, h24: true }));
        assert_eq!((m.lens, moment(&state(18, 0)).lens, moment(&state(5, 59)).lens), (0.0, 1.0, 1.0));
    }

    #[test]
    fn with_a_prayer_place_the_sun_keeps_its_hours_there() {
        use poni_core::prayer::{Clock as Zone, Method, Site, KEMENAG_ISYA_ANGLE, KEMENAG_SUBUH_ANGLE};
        // Jakarta on 10 November 2026, 12:00 WIB (05:00 UTC), on a
        // computer keeping WIB: up about 05:26, down about 17:49.
        let jakarta = Site { lat: -6.2, lng: 106.816, clock: Zone::Fixed(420), h0: -1.0, subuh_angle: KEMENAG_SUBUH_ANGLE, isya_angle: KEMENAG_ISYA_ANGLE, method: Method::Kemenag, hanafi: false };
        let noon_utc = poni_core::datetime::days_from_civil(2026, 11, 10) * 86_400 + 5 * 3600;
        let (rise, set) = sun_at(&jakarta, noon_utc, 7 * 3600).expect("Jakarta has a sunrise");
        assert!((320.0..=335.0).contains(&rise), "sunrise at minute {rise}");
        assert!((1062.0..=1076.0).contains(&set), "sunset at minute {set}");
        // The same place seen from a computer an hour ahead (WITA): an hour later on its clock.
        let (rise_wita, set_wita) = sun_at(&jakarta, noon_utc, 8 * 3600).unwrap();
        assert_eq!((rise_wita, set_wita), (rise + 60.0, set + 60.0));
        // Without the add-on, the house's own day.
        assert_eq!(sun(&state(12, 0)), HOUSE_DAY);
    }

    #[test]
    fn the_badge_is_the_bolt_or_the_low_battery() {
        let mut st = state(10, 8);
        assert_eq!(badge(&st), None);
        st.power = PowerState { has_battery: true, percent: 64, charging: false, on_ac: false, saver: false };
        assert_eq!(badge(&st), None);
        st.power.percent = 20;
        assert_eq!(badge(&st), Some(Badge::Low(20)));
        st.power.charging = true;
        assert_eq!(badge(&st), Some(Badge::Charging));
        // The long pill has room for it beside the time.
        assert_eq!(geom(&st), Some(Geom::new(190.0, 38.0, 19.0)));
        st.settings.idle_style = IdleStyle::Minimal;
        assert_eq!(badge(&st), None);
    }

    #[test]
    fn the_ball_rolls_when_the_sun_sets_and_when_the_theme_is_picked() {
        settle();
        let at = |rest: f32, now: f64| {
            let (lens, rolling, _) = ball(rest, now);
            (lens, rolling)
        };
        // First shown: where the time has it, at rest.
        assert_eq!(at(0.0, 10.0), (0.0, false));
        assert_eq!(at(0.0, 70.0), (0.0, false));
        // Sunset: it sets off, is halfway at half time, and arrives.
        assert_eq!(at(1.0, 100.0), (0.0, true));
        let (mid, rolling) = at(1.0, 100.0 + ROLL / 2.0);
        assert!((mid - 0.5).abs() < 1e-4 && rolling, "{mid}");
        assert_eq!(at(1.0, 100.0 + ROLL), (1.0, false));
        // Turned back on its way, it starts from where it is.
        at(0.0, 200.0);
        let (part, _) = at(0.0, 200.0 + ROLL / 2.0);
        let (from, rolling) = at(1.0, 200.0 + ROLL / 2.0);
        assert!((from - part).abs() < 1e-6 && rolling);
        // Another theme drew, then Diorama again: it rolls in from the far end.
        settle();
        at(0.0, 300.0);
        note_theme(Theme::Classic);
        note_theme(Theme::Diorama);
        assert_eq!(at(0.0, 310.0), (1.0, true));
        assert_eq!(at(0.0, 310.0 + ROLL), (0.0, false));
        settle();
    }

    /// How far the ball goes from one end of the long pill to the other.
    const TRAVEL: f32 = 190.0 - 38.0;

    /// 10 November 2026 at `hour:minute`, as minutes since 1 January 2000.
    fn abs(hour: u32, minute: u32) -> f64 {
        9810.0 * 1440.0 + (hour * 60 + minute) as f64
    }

    #[test]
    fn a_slide_runs_through_the_sunset_into_the_night_and_back() {
        // By day: from the clock's minute, through the golden hour ten
        // minutes before sunset halfway, to the night 45 minutes after it.
        let day = moment(&state(10, 8));
        assert_eq!((slid_to(&day, 0.0), slid_to(&day, 0.5), slid_to(&day, 1.0)), (abs(10, 8), abs(17, 50), abs(18, 45)));
        // At night: on through the small hours to the glow ten minutes
        // after sunrise, and the day 50 minutes after it, the next morning.
        let night = moment(&state(22, 0));
        assert_eq!((slid_to(&night, 1.0), slid_to(&night, 0.5), slid_to(&night, 0.0)), (abs(22, 0), abs(30, 10), abs(30, 50)));
        // The place's date turns with it: 06:50 the next morning.
        let mut m = night;
        show_at(&mut m, slid_to(&night, 0.0));
        assert_eq!((m.day, m.minute), (9811, 410.0));
    }

    #[test]
    fn slid_past_halfway_the_ball_goes_on_and_the_theme_keeps_the_night() {
        settle();
        let mut st = state(10, 8);
        st.now = 50.0;
        // 190 DIPs long: the ball goes 152 DIPs from one end to the other.
        grab(&st, 500.0);
        assert!(held() && !night(&st));
        // A tremble is still a tap.
        assert!(slide(501.0, 50.05));
        assert_eq!(HELD.with(|h| h.get()).map(|h| (h.lens, h.moved)), Some((0.0, false)));
        // Two thirds of the way across, slowly: the look follows the hand.
        slide(500.0 + 0.4 * TRAVEL, 50.5);
        slide(500.0 + 0.66 * TRAVEL, 51.5);
        assert!(night(&st));
        // Held still, then let go: it goes on to the tree, the nearer end.
        assert!(let_go(&st, 51.7));
        assert!(!held() && night(&st));
        let (lens, rolling, hand) = ball(1.0, 51.7);
        assert!((lens - 0.66).abs() < 1e-3 && rolling && hand.is_some(), "{lens}");
        let (lens, rolling, _) = ball(1.0, 52.5);
        assert_eq!((lens, rolling), (1.0, false));
        // At rest it shows the night it was slid to, 18:45, the clock still 10:08.
        assert_eq!(resting(&moment(&st)), (1.0, abs(18, 45)));
        assert_eq!(left_by_hand(&st), Some(true));
        // At sunset the sun agrees; at 18:45 the clock has come round to it.
        st.local.hour = 18;
        st.local.minute = 0;
        assert_eq!(resting(&moment(&st)), (1.0, abs(18, 45)));
        st.local.minute = 45;
        assert_eq!(resting(&moment(&st)), (1.0, abs(18, 45)));
        assert!(night(&st));
        st.local.hour = 21;
        assert_eq!(resting(&moment(&st)), (1.0, abs(21, 45)));
        settle();
    }

    #[test]
    fn a_tap_flips_the_ball_and_a_tap_back_follows_the_sun_again() {
        settle();
        let mut st = state(14, 0);
        st.now = 10.0;
        grab(&st, 300.0);
        assert!(let_go(&st, 10.1));
        assert!(night(&st));
        // It rolls over (gently, the length of a tap's flip).
        let (half, rolling, _) = ball(1.0, 10.1 + FLIP / 2.0);
        assert!((half - 0.5).abs() < 1e-3 && rolling, "{half}");
        assert_eq!(resting(&moment(&st)), (1.0, abs(18, 45)));
        // Tapped again, it rolls back to the house, and the day is the clock's.
        st.now = 20.0;
        ball(1.0, 20.0);
        grab(&st, 300.0 + TRAVEL);
        assert!(let_go(&st, 20.1));
        assert!(!night(&st));
        assert_eq!(left_by_hand(&st), None);
        assert_eq!(resting(&moment(&st)), (0.0, abs(14, 0)));
        settle();
    }

    #[test]
    fn a_throw_carries_the_ball_to_the_end_it_was_thrown_toward() {
        settle();
        let mut st = state(9, 0);
        st.now = 5.0;
        grab(&st, 400.0);
        // A fifth of the way in 50 ms: four ends a second.
        slide(400.0 + 0.1 * TRAVEL, 5.025);
        slide(400.0 + 0.2 * TRAVEL, 5.05);
        assert!(let_go(&st, 5.06));
        assert!(night(&st));
        // The same, but held still before letting go: back to the house.
        st.now = 10.0;
        settle();
        grab(&st, 400.0);
        slide(400.0 + 0.1 * TRAVEL, 10.025);
        slide(400.0 + 0.2 * TRAVEL, 10.05);
        assert!(let_go(&st, 10.3));
        assert!(!night(&st));
        settle();
    }

    #[test]
    fn at_night_a_slide_brings_the_morning_until_the_clock_comes_to_it() {
        settle();
        let mut st = state(22, 0);
        st.now = 1.0;
        assert!(night(&st));
        // The ball rests on the tree: 152 DIPs back toward the house.
        grab(&st, 600.0);
        slide(600.0 - 0.8 * TRAVEL, 1.5);
        assert!(!night(&st));
        let_go(&st, 2.0);
        assert!(!night(&st));
        assert_eq!(resting(&moment(&st)), (0.0, abs(30, 50)));
        // After midnight, still the morning it was slid to.
        st.local.day = 11;
        st.local.hour = 3;
        st.local.minute = 0;
        assert!(!night(&st));
        // A clock set back more than a day drops it.
        st.local.day = 9;
        assert!(night(&st));
        settle();
    }

    #[test]
    fn the_ball_takes_the_pointer_where_it_is() {
        let rest = Geom::new(100.0, 38.0, 19.0);
        // At rest on the house end, then on the tree end.
        assert_eq!(ball_rect(Rect::new(200.0, 8.0, 100.0, 38.0), rest, 0.0), Rect::new(200.0, 8.0, 38.0, 38.0));
        assert_eq!(ball_rect(Rect::new(200.0, 8.0, 100.0, 38.0), rest, 1.0), Rect::new(262.0, 8.0, 38.0, 38.0));
        // Swollen under the pointer the place keeps its size, centred.
        assert_eq!(ball_rect(Rect::new(197.0, 8.0, 106.0, 44.0), rest, 0.0), Rect::new(200.0, 11.0, 38.0, 38.0));
    }
}
