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
//! Nothing new runs at rest. The picture is painted when the minute tick
//! redraws the pill (the app already wakes for it, for the clock) and kept
//! as an image until something it shows changes. Twice a day the glass ball
//! rolls, at sunset to the tree and at sunrise back to the house, and once
//! when the theme is picked: about a second and a half of frames, asked
//! for only while the pill is on screen to show them.
//!
//! The sun rises at six and sets at six, the house's own day, unless the
//! Prayer times add-on has a place: then it rises and sets when it does
//! there (terbit and maghrib).

use crate::gfx::{Image, ImageData};
use crate::island::Geom;
use crate::state::AppState;
use crate::ui::Ui;
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

/// Everything the painting shows: it is painted again when one changes.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    moment: Moment,
    view: View,
}

/// The glass ball's way: from `from` to `to`, started at `t0` (mono seconds).
#[derive(Clone, Copy)]
struct Ball {
    from: f32,
    to: f32,
    t0: f64,
}

impl Ball {
    /// Where it is at `now`, and whether it is still rolling.
    fn at(&self, now: f64) -> (f32, bool) {
        let t = ((now - self.t0) / ROLL).clamp(0.0, 1.0) as f32;
        // Sets off gently and comes to rest gently.
        let eased = t * t * t * (t * (6.0 * t - 15.0) + 10.0);
        (self.from + (self.to - self.from) * eased, t < 1.0 && self.from != self.to)
    }
}

thread_local! {
    /// The last painting and what it shows.
    static KEPT: RefCell<Option<(Key, Image)>> = const { RefCell::new(None) };
    static BALL: Cell<Option<Ball>> = const { Cell::new(None) };
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
}

/// Put the ball where the time has it, with no roll on the way (the
/// self-test and the Mac's renders, whose pictures must not depend on what
/// was drawn before them).
pub fn settle() {
    BALL.with(|b| b.set(None));
    PICKED.with(|p| p.set(false));
}

/// The resting pill is the picture: in the Diorama theme, whatever the
/// idle look, unless it is Hidden.
pub fn shows(st: &AppState) -> bool {
    st.settings.theme == Theme::Diorama && st.settings.idle_style != IdleStyle::Hidden
}

/// The theme shows its night: the glass ball rests on the tree, where the
/// sun being down puts it. The open card, the pop-ups and every other pill
/// of the theme take its night look then, and its day look otherwise.
pub fn night(st: &AppState) -> bool {
    moment(st).night()
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

/// When the sun rises and sets, in minutes of this computer's day: at the
/// Prayer times add-on's place when it has one, else six to six.
fn sun(st: &AppState) -> (f32, f32) {
    if !st.prayer_on() {
        return HOUSE_DAY;
    }
    // This computer's clock against UTC, to the minute.
    let local = st.local.hour as i64 * 3600 + st.local.minute as i64 * 60 + st.local.second as i64;
    let ahead = (((local - st.unix.rem_euclid(86_400)) as f64 / 60.0).round() as i64 * 60 + 43_200).rem_euclid(86_400) - 43_200;
    st.prayer.site().and_then(|site| sun_at(&site, st.prayer_now(), ahead)).unwrap_or(HOUSE_DAY)
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
    };
    m.lens = m.rest_lens();
    m
}

/// Where the ball is at `now` on its way to `rest`, and whether it rolls.
fn ball(rest: f32, now: f64) -> (f32, bool) {
    let picked = PICKED.with(|p| p.replace(false));
    let b = match BALL.with(|b| b.get()) {
        // Just picked: it rolls in from the other end, and the day (or the
        // night) comes with it.
        _ if picked => Ball { from: 1.0 - rest, to: rest, t0: now },
        Some(b) if b.to == rest => b,
        // The sun set or rose: on its way from wherever it is.
        Some(b) => Ball { from: b.at(now).0, to: rest, t0: now },
        None => Ball { from: rest, to: rest, t0: now },
    };
    BALL.with(|c| c.set(Some(b)));
    b.at(now)
}

/// Draw the resting pill as the picture. `false` = it isn't (another theme
/// or look): Pill draws it.
pub fn draw(ui: &mut Ui, st: &mut AppState, body: Rect) -> bool {
    let Some(rest) = geom(st) else { return false };
    let mut m = moment(st);
    let (lens, rolling) = ball(m.rest_lens(), st.now);
    // In thousandths: the eye sees no finer, and a settled ball is exact.
    m.lens = (lens * 1000.0).round() / 1000.0;
    if rolling {
        ui.request_anim();
    }

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
        assert_eq!(geom(&st), Some(Geom::new(100.0, 38.0, 19.0)));
        st.settings.use_24h = false;
        assert_eq!(geom(&st), Some(Geom::new(112.0, 38.0, 19.0)));
        // The status bar is the Pill theme's: here it is the clock.
        st.settings.idle_style = IdleStyle::Status;
        assert_eq!(geom(&st), Some(Geom::new(112.0, 38.0, 19.0)));
        // Minimal is the place alone, and so is a setting left on Eyes.
        for style in [IdleStyle::Minimal, IdleStyle::Eyes] {
            st.settings.idle_style = style;
            assert_eq!(geom(&st), Some(Geom::new(80.0, 38.0, 19.0)), "{style:?}");
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
        // It makes room for itself beside the time.
        assert_eq!(geom(&st), Some(Geom::new(112.0, 38.0, 19.0)));
        st.settings.idle_style = IdleStyle::Minimal;
        assert_eq!(badge(&st), None);
    }

    #[test]
    fn the_ball_rolls_when_the_sun_sets_and_when_the_theme_is_picked() {
        settle();
        // First shown: where the time has it, at rest.
        assert_eq!(ball(0.0, 10.0), (0.0, false));
        assert_eq!(ball(0.0, 70.0), (0.0, false));
        // Sunset: it sets off, is halfway at half time, and arrives.
        assert_eq!(ball(1.0, 100.0), (0.0, true));
        let (mid, rolling) = ball(1.0, 100.0 + ROLL / 2.0);
        assert!((mid - 0.5).abs() < 1e-4 && rolling, "{mid}");
        assert_eq!(ball(1.0, 100.0 + ROLL), (1.0, false));
        // Turned back on its way, it starts from where it is.
        ball(0.0, 200.0);
        let (part, _) = ball(0.0, 200.0 + ROLL / 2.0);
        let (from, rolling) = ball(1.0, 200.0 + ROLL / 2.0);
        assert!((from - part).abs() < 1e-6 && rolling);
        // Another theme drew, then Diorama again: it rolls in from the far end.
        settle();
        ball(0.0, 300.0);
        note_theme(Theme::Classic);
        note_theme(Theme::Diorama);
        assert_eq!(ball(0.0, 310.0), (1.0, true));
        assert_eq!(ball(0.0, 310.0 + ROLL), (0.0, false));
        settle();
    }
}
