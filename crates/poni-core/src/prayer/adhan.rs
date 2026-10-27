//! The sun's position and the moments it reaches a given altitude: a Rust port
//! of the parts of Adhan (adhan-js 4.4.6, MIT, Copyright (c) Batoul Apps) that
//! Oyen Umroh's prayer engine runs on. The arithmetic follows the JavaScript
//! step for step, including where it truncates to whole seconds, so both
//! engines land on the same minute after Kemenag's rounding. Formulas are from
//! Jean Meeus, "Astronomical Algorithms". Licence text: THIRD-PARTY-NOTICES.txt.
//!
//! Times are UTC milliseconds since 1970 (whole seconds, like a JavaScript
//! `Date` built from Adhan's `TimeComponents`). `None` = the sun never reaches
//! that altitude that day.

use std::f64::consts::PI;

use crate::datetime::{civil_from_days, days_from_civil};

const DAY_MS: i64 = 86_400_000;

fn deg2rad(d: f64) -> f64 {
    d * PI / 180.0
}

fn rad2deg(r: f64) -> f64 {
    r * 180.0 / PI
}

fn normalize_to_scale(num: f64, max: f64) -> f64 {
    num - max * (num / max).floor()
}

fn unwind_angle(a: f64) -> f64 {
    normalize_to_scale(a, 360.0)
}

/// JavaScript's `Math.round`: halves go up (toward +infinity), not away from zero.
fn js_round(x: f64) -> f64 {
    let r = x.round();
    if (r - x).abs() == 0.5 {
        x.ceil()
    } else {
        r
    }
}

fn quadrant_shift_angle(a: f64) -> f64 {
    if (-180.0..=180.0).contains(&a) {
        a
    } else {
        a - 360.0 * js_round(a / 360.0)
    }
}

// ---------------------------------------------------------------- Meeus

fn julian_day(year: i32, month: u8, day: u8) -> f64 {
    let (y, m) = if month > 2 { (year as f64, month as f64) } else { (year as f64 - 1.0, month as f64 + 12.0) };
    let a = (y / 100.0).trunc();
    let b = (2.0 - a + (a / 4.0).trunc()).trunc();
    let i0 = (365.25 * (y + 4716.0)).trunc();
    let i1 = (30.6001 * (m + 1.0)).trunc();
    i0 + i1 + day as f64 + b - 1524.5
}

fn julian_century(jd: f64) -> f64 {
    (jd - 2451545.0) / 36525.0
}

fn mean_solar_longitude(t: f64) -> f64 {
    unwind_angle(280.4664567 + 36000.76983 * t + 0.0003032 * t.powi(2))
}

fn mean_lunar_longitude(t: f64) -> f64 {
    unwind_angle(218.3165 + 481267.8813 * t)
}

fn ascending_lunar_node_longitude(t: f64) -> f64 {
    unwind_angle(125.04452 - 1934.136261 * t + 0.0020708 * t.powi(2) + t.powi(3) / 450000.0)
}

fn mean_solar_anomaly(t: f64) -> f64 {
    unwind_angle(357.52911 + 35999.05029 * t - 0.0001537 * t.powi(2))
}

fn solar_equation_of_the_center(t: f64, mean_anomaly: f64) -> f64 {
    let m = deg2rad(mean_anomaly);
    let term1 = (1.914602 - 0.004817 * t - 0.000014 * t.powi(2)) * m.sin();
    let term2 = (0.019993 - 0.000101 * t) * (2.0 * m).sin();
    let term3 = 0.000289 * (3.0 * m).sin();
    term1 + term2 + term3
}

fn apparent_solar_longitude(t: f64, mean_longitude: f64) -> f64 {
    let longitude = mean_longitude + solar_equation_of_the_center(t, mean_solar_anomaly(t));
    let omega = 125.04 - 1934.136 * t;
    unwind_angle(longitude - 0.00569 - 0.00478 * deg2rad(omega).sin())
}

fn mean_obliquity_of_the_ecliptic(t: f64) -> f64 {
    23.439291 - 0.013004167 * t - 0.0000001639 * t.powi(2) + 0.0000005036 * t.powi(3)
}

fn apparent_obliquity_of_the_ecliptic(t: f64, mean_obliquity: f64) -> f64 {
    mean_obliquity + 0.00256 * deg2rad(125.04 - 1934.136 * t).cos()
}

fn mean_sidereal_time(t: f64) -> f64 {
    let jd = t * 36525.0 + 2451545.0;
    unwind_angle(280.46061837 + 360.98564736629 * (jd - 2451545.0) + 0.000387933 * t.powi(2) - t.powi(3) / 38710000.0)
}

fn nutation_in_longitude(l0: f64, lp: f64, omega: f64) -> f64 {
    let term1 = -17.2 / 3600.0 * deg2rad(omega).sin();
    let term2 = 1.32 / 3600.0 * (2.0 * deg2rad(l0)).sin();
    let term3 = 0.23 / 3600.0 * (2.0 * deg2rad(lp)).sin();
    let term4 = 0.21 / 3600.0 * (2.0 * deg2rad(omega)).sin();
    term1 - term2 - term3 + term4
}

fn nutation_in_obliquity(l0: f64, lp: f64, omega: f64) -> f64 {
    let term1 = 9.2 / 3600.0 * deg2rad(omega).cos();
    let term2 = 0.57 / 3600.0 * (2.0 * deg2rad(l0)).cos();
    let term3 = 0.1 / 3600.0 * (2.0 * deg2rad(lp)).cos();
    let term4 = 0.09 / 3600.0 * (2.0 * deg2rad(omega)).cos();
    term1 + term2 + term3 - term4
}

fn altitude_of_celestial_body(latitude: f64, declination: f64, hour_angle: f64) -> f64 {
    let term1 = deg2rad(latitude).sin() * deg2rad(declination).sin();
    let term2 = deg2rad(latitude).cos() * deg2rad(declination).cos() * deg2rad(hour_angle).cos();
    rad2deg((term1 + term2).asin())
}

fn interpolate(y2: f64, y1: f64, y3: f64, n: f64) -> f64 {
    let a = y2 - y1;
    let b = y3 - y2;
    let c = b - a;
    y2 + n / 2.0 * (a + b + n * c)
}

fn interpolate_angles(y2: f64, y1: f64, y3: f64, n: f64) -> f64 {
    let a = unwind_angle(y2 - y1);
    let b = unwind_angle(y3 - y2);
    let c = b - a;
    y2 + n / 2.0 * (a + b + n * c)
}

/// The sun's coordinates at 0h UT of a Julian day.
#[derive(Clone, Copy, Debug)]
struct SolarCoordinates {
    /// Degrees.
    declination: f64,
    /// Degrees.
    right_ascension: f64,
    /// Degrees.
    apparent_sidereal_time: f64,
}

impl SolarCoordinates {
    fn new(jd: f64) -> Self {
        let t = julian_century(jd);
        let l0 = mean_solar_longitude(t);
        let lp = mean_lunar_longitude(t);
        let omega = ascending_lunar_node_longitude(t);
        let lambda = deg2rad(apparent_solar_longitude(t, l0));
        let theta0 = mean_sidereal_time(t);
        let d_psi = nutation_in_longitude(l0, lp, omega);
        let d_epsilon = nutation_in_obliquity(l0, lp, omega);
        let epsilon0 = mean_obliquity_of_the_ecliptic(t);
        let epsilon = deg2rad(apparent_obliquity_of_the_ecliptic(t, epsilon0));
        SolarCoordinates {
            declination: rad2deg((epsilon.sin() * lambda.sin()).asin()),
            right_ascension: unwind_angle(rad2deg((epsilon.cos() * lambda.sin()).atan2(lambda.cos()))),
            apparent_sidereal_time: theta0 + d_psi * 3600.0 * deg2rad(epsilon0 + d_epsilon).cos() / 3600.0,
        }
    }
}

/// One day's sun at one place: Adhan's `SolarTime`. Moments are hours after
/// 0h UT of that date (negative = the evening before, in UTC).
#[derive(Clone, Copy, Debug)]
pub struct SolarTime {
    lat: f64,
    lng: f64,
    solar: SolarCoordinates,
    prev: SolarCoordinates,
    next: SolarCoordinates,
    approx_transit: f64,
    /// Hours (UT) of the sun's highest point.
    pub transit: f64,
    /// Hours (UT) with the sun at -50' (Adhan's sunrise).
    pub sunrise: f64,
    /// Hours (UT) with the sun at -50' (Adhan's sunset).
    pub sunset: f64,
}

impl SolarTime {
    pub fn new(year: i32, month: u8, day: u8, lat: f64, lng: f64) -> Self {
        let jd = julian_day(year, month, day);
        let solar = SolarCoordinates::new(jd);
        let prev = SolarCoordinates::new(jd - 1.0);
        let next = SolarCoordinates::new(jd + 1.0);
        let m0 = approximate_transit(lng, solar.apparent_sidereal_time, solar.right_ascension);
        let mut st = SolarTime { lat, lng, solar, prev, next, approx_transit: m0, transit: 0.0, sunrise: 0.0, sunset: 0.0 };
        st.transit = st.corrected_transit();
        st.sunrise = st.hour_angle(-50.0 / 60.0, false);
        st.sunset = st.hour_angle(-50.0 / 60.0, true);
        st
    }

    fn corrected_transit(&self) -> f64 {
        let m0 = self.approx_transit;
        let lw = -self.lng;
        let theta = unwind_angle(self.solar.apparent_sidereal_time + 360.985647 * m0);
        let a = unwind_angle(interpolate_angles(self.solar.right_ascension, self.prev.right_ascension, self.next.right_ascension, m0));
        let h = quadrant_shift_angle(theta - lw - a);
        let dm = h / -360.0;
        (m0 + dm) * 24.0
    }

    /// Hours (UT) when the sun's centre is at `angle` degrees (negative =
    /// below the horizon), before or after transit. NaN when it never is.
    pub fn hour_angle(&self, angle: f64, after_transit: bool) -> f64 {
        let m0 = self.approx_transit;
        let (lat, lw) = (self.lat, -self.lng);
        let d2 = self.solar.declination;
        let term1 = deg2rad(angle).sin() - deg2rad(lat).sin() * deg2rad(d2).sin();
        let term2 = deg2rad(lat).cos() * deg2rad(d2).cos();
        let h0 = rad2deg((term1 / term2).acos());
        let m = if after_transit { m0 + h0 / 360.0 } else { m0 - h0 / 360.0 };
        let theta = unwind_angle(self.solar.apparent_sidereal_time + 360.985647 * m);
        let a = unwind_angle(interpolate_angles(self.solar.right_ascension, self.prev.right_ascension, self.next.right_ascension, m));
        let delta = interpolate(d2, self.prev.declination, self.next.declination, m);
        let h = theta - lw - a;
        let alt = altitude_of_celestial_body(lat, delta, h);
        let term3 = alt - angle;
        let term4 = 360.0 * deg2rad(delta).cos() * deg2rad(lat).cos() * deg2rad(h).sin();
        let dm = term3 / term4;
        (m + dm) * 24.0
    }

    /// Hours (UT) when a shadow is `shadow_length` times the object plus its
    /// noon shadow (1 = Syafi'i Ashar, 2 = Hanafi).
    pub fn afternoon(&self, shadow_length: f64) -> f64 {
        let tangent = (self.lat - self.solar.declination).abs();
        let inverse = shadow_length + deg2rad(tangent).tan();
        let angle = rad2deg((1.0 / inverse).atan());
        self.hour_angle(angle, true)
    }
}

fn approximate_transit(lng: f64, sidereal_time: f64, right_ascension: f64) -> f64 {
    let lw = -lng;
    let m0 = normalize_to_scale((right_ascension + lw - sidereal_time) / 360.0, 1.0);
    let expected = normalize_to_scale((12.0 - lng / 15.0) / 24.0, 1.0);
    if m0 - expected > 0.5 {
        m0 - 1.0
    } else if expected - m0 > 0.5 {
        m0 + 1.0
    } else {
        m0
    }
}

/// Adhan's `TimeComponents(hours).utcDate(...)`: hours after 0h UT of the
/// day, truncated to whole seconds, as UTC milliseconds.
fn utc_ms(day0_ms: i64, hours: f64) -> Option<i64> {
    if !hours.is_finite() {
        return None;
    }
    let h = hours.floor();
    let m = ((hours - h) * 60.0).floor();
    let s = ((hours - (h + m / 60.0)) * 60.0 * 60.0).floor();
    Some(day0_ms + (h as i64) * 3_600_000 + (m as i64) * 60_000 + (s as i64) * 1000)
}

/// A JavaScript `Date` moved by fractional seconds keeps whole milliseconds,
/// cut toward zero.
fn add_seconds(ms: i64, secs: f64) -> i64 {
    (ms as f64 + secs * 1000.0).trunc() as i64
}

// ---------------------------------------------------------------- Adhan's PrayerTimes

/// How Adhan bounds Fajr and Isha where twilight lasts all night.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighLatitudeRule {
    MiddleOfTheNight,
    SeventhOfTheNight,
}

impl HighLatitudeRule {
    /// Adhan's `HighLatitudeRule.recommended`.
    pub fn recommended(lat: f64) -> Self {
        if lat > 48.0 {
            HighLatitudeRule::SeventhOfTheNight
        } else {
            HighLatitudeRule::MiddleOfTheNight
        }
    }

    fn portion(self) -> f64 {
        match self {
            HighLatitudeRule::MiddleOfTheNight => 1.0 / 2.0,
            HighLatitudeRule::SeventhOfTheNight => 1.0 / 7.0,
        }
    }
}

/// Adhan's `CalculationParameters`, the parts the engine uses (no method
/// adjustments, no rounding: the caller rounds).
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Degrees below the horizon.
    pub fajr_angle: f64,
    /// Degrees below the horizon; unused when `isha_interval_min` > 0.
    pub isha_angle: f64,
    /// Isha this many minutes after sunset (Umm al-Qura), 0 = by angle.
    pub isha_interval_min: f64,
    /// 1 = Syafi'i, 2 = Hanafi.
    pub shadow_length: f64,
    pub high_latitude: HighLatitudeRule,
}

/// Adhan's `PrayerTimes` with rounding off: UTC milliseconds, whole seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moments {
    pub fajr: i64,
    pub sunrise: i64,
    pub dhuhr: i64,
    pub asr: i64,
    pub sunset: i64,
    pub isha: i64,
}

/// Moments for one calendar date (the date of the place, not of UTC).
/// `None` when the sun doesn't rise or set that day.
pub fn moments(year: i32, month: u8, day: u8, lat: f64, lng: f64, p: &Params) -> Option<Moments> {
    let days = days_from_civil(year, month, day);
    let day0 = days * DAY_MS;
    let st = SolarTime::new(year, month, day, lat, lng);
    let dhuhr = utc_ms(day0, st.transit)?;
    let sunrise = utc_ms(day0, st.sunrise)?;
    let sunset = utc_ms(day0, st.sunset)?;
    let (ty, tm, td) = civil_from_days(days + 1);
    let tomorrow = SolarTime::new(ty, tm, td, lat, lng);
    let asr = utc_ms(day0, st.afternoon(p.shadow_length))?;
    // The night's length bounds Fajr and Isha; where tomorrow's sunrise is
    // missing Adhan's bound is an invalid date, so the angle's moment stands.
    let night = utc_ms(day0 + DAY_MS, tomorrow.sunrise).map(|r| (r - sunset) as f64 / 1000.0);
    let portion = p.high_latitude.portion();

    let safe_fajr = night.map(|n| add_seconds(sunrise, -(portion * n)));
    let fajr = match (utc_ms(day0, st.hour_angle(-p.fajr_angle, false)), safe_fajr) {
        (Some(f), Some(s)) => f.max(s),
        (f, s) => f.or(s)?,
    };
    let isha = if p.isha_interval_min > 0.0 {
        add_seconds(sunset, p.isha_interval_min * 60.0)
    } else {
        let safe_isha = night.map(|n| add_seconds(sunset, portion * n));
        match (utc_ms(day0, st.hour_angle(-p.isha_angle, true)), safe_isha) {
            (Some(i), Some(s)) => i.min(s),
            (i, s) => i.or(s)?,
        }
    };
    Some(Moments { fajr, sunrise, dhuhr, asr, sunset, isha })
}

// ---------------------------------------------------------------- Adhan's methods

/// One of Adhan's calculation methods, with the parts `PrayerTimes` reads:
/// for places outside Indonesia (the world list), whose times follow Adhan
/// itself, rounding and method adjustments included.
#[derive(Clone, Copy, Debug)]
pub struct Method {
    /// Degrees below the horizon.
    pub fajr_angle: f64,
    /// Degrees below the horizon; unused when `isha_interval_min` > 0.
    pub isha_angle: f64,
    /// Isha this many minutes after Maghrib, 0 = by angle.
    pub isha_interval_min: f64,
    /// Maghrib with the sun this far below the horizon (Tehran), 0 = sunset.
    pub maghrib_angle: f64,
    /// The Moonsighting Committee's seasonal twilight and its 1/7 rule at 55° and up.
    pub moonsighting: bool,
    /// Minutes added to Fajr, sunrise, Dhuhr, Asr, Maghrib and Isha.
    pub adjust: [i64; 6],
    /// Round to the next minute (`Rounding.Up`) instead of the nearest.
    pub round_up: bool,
}

impl Method {
    const fn angles(fajr: f64, isha: f64) -> Method {
        Method { fajr_angle: fajr, isha_angle: isha, isha_interval_min: 0.0, maghrib_angle: 0.0, moonsighting: false, adjust: [0; 6], round_up: false }
    }
    pub const MUSLIM_WORLD_LEAGUE: Method = Method { adjust: [0, 0, 1, 0, 0, 0], ..Method::angles(18.0, 17.0) };
    pub const EGYPTIAN: Method = Method { adjust: [0, 0, 1, 0, 0, 0], ..Method::angles(19.5, 17.5) };
    pub const KARACHI: Method = Method { adjust: [0, 0, 1, 0, 0, 0], ..Method::angles(18.0, 18.0) };
    pub const DUBAI: Method = Method { adjust: [0, -3, 3, 3, 3, 0], ..Method::angles(18.2, 18.2) };
    pub const MOONSIGHTING_COMMITTEE: Method = Method { moonsighting: true, adjust: [0, 0, 5, 0, 3, 0], ..Method::angles(18.0, 18.0) };
    pub const KUWAIT: Method = Method::angles(18.0, 17.5);
    pub const QATAR: Method = Method { isha_interval_min: 90.0, ..Method::angles(18.0, 0.0) };
    pub const SINGAPORE: Method = Method { adjust: [0, 0, 1, 0, 0, 0], round_up: true, ..Method::angles(20.0, 18.0) };
    pub const TURKEY: Method = Method { adjust: [0, -7, 5, 4, 7, 0], ..Method::angles(18.0, 17.0) };
    pub const TEHRAN: Method = Method { maghrib_angle: 4.5, ..Method::angles(17.7, 14.0) };
}

/// Adhan's `PolarCircleResolution.AqrabBalad`: where the sun doesn't rise or
/// set that day (inside the polar circles), the sun of the nearest latitude,
/// in 0.5° steps toward the equator, where it does. `None` where it does.
fn aqrab_balad(year: i32, month: u8, day: u8, lat: f64, lng: f64) -> Option<(SolarTime, SolarTime)> {
    let valid = |s: &SolarTime| !s.sunrise.is_nan() && !s.sunset.is_nan();
    let (ty, tm, td) = civil_from_days(days_from_civil(year, month, day) + 1);
    let mut l = lat - lat.signum() * 0.5;
    loop {
        let (st, tomorrow) = (SolarTime::new(year, month, day, l, lng), SolarTime::new(ty, tm, td, l, lng));
        if valid(&st) && valid(&tomorrow) {
            return Some((st, tomorrow));
        }
        if l.abs() < 65.0 {
            return None;
        }
        l -= l.signum() * 0.5;
    }
}

/// Days since the winter solstice for the Moonsighting Committee's seasons.
fn days_since_solstice(day_of_year: i64, year: i32, lat: f64) -> i64 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_year = if leap { 366 } else { 365 };
    if lat >= 0.0 {
        let d = day_of_year + 10;
        if d >= days_in_year {
            d - days_in_year
        } else {
            d
        }
    } else {
        let d = day_of_year - if leap { 173 } else { 172 };
        if d < 0 {
            d + days_in_year
        } else {
            d
        }
    }
}

/// The Moonsighting Committee's twilight in minutes from four seasonal values.
fn seasonal(a: f64, b: f64, c: f64, d: f64, dyy: i64) -> f64 {
    let dyy = dyy as f64;
    if dyy < 91.0 {
        a + (b - a) / 91.0 * dyy
    } else if dyy < 137.0 {
        b + (c - b) / 46.0 * (dyy - 91.0)
    } else if dyy < 183.0 {
        c + (d - c) / 46.0 * (dyy - 137.0)
    } else if dyy < 229.0 {
        d + (c - d) / 46.0 * (dyy - 183.0)
    } else if dyy < 275.0 {
        c + (b - c) / 46.0 * (dyy - 229.0)
    } else {
        b + (a - b) / 91.0 * (dyy - 275.0)
    }
}

/// `Astronomical.seasonAdjustedMorningTwilight`: the earliest Fajr.
fn season_adjusted_morning(lat: f64, day_of_year: i64, year: i32, sunrise: i64) -> i64 {
    let l = lat.abs();
    let (a, b, c, d) = (75.0 + 28.65 / 55.0 * l, 75.0 + 19.44 / 55.0 * l, 75.0 + 32.74 / 55.0 * l, 75.0 + 48.1 / 55.0 * l);
    let adjustment = seasonal(a, b, c, d, days_since_solstice(day_of_year, year, lat));
    add_seconds(sunrise, js_round(adjustment * -60.0))
}

/// `Astronomical.seasonAdjustedEveningTwilight` with `Shafaq.General`: the latest Isha.
fn season_adjusted_evening(lat: f64, day_of_year: i64, year: i32, sunset: i64) -> i64 {
    let l = lat.abs();
    let (a, b, c, d) = (75.0 + 25.6 / 55.0 * l, 75.0 + 2.05 / 55.0 * l, 75.0 - 9.21 / 55.0 * l, 75.0 + 6.14 / 55.0 * l);
    let adjustment = seasonal(a, b, c, d, days_since_solstice(day_of_year, year, lat));
    add_seconds(sunset, js_round(adjustment * 60.0))
}

/// Adhan's `roundedMinute`: nearest uses the whole seconds only; `Up` always
/// moves on to the next minute, even from a whole one.
fn rounded_minute(ms: i64, up: bool) -> i64 {
    let secs = ms.div_euclid(1000).rem_euclid(60);
    let offset = if up || secs >= 30 { 60 - secs } else { -secs };
    ms + offset * 1000
}

/// Fajr, sunrise, Dhuhr, Asr, Maghrib and Isha in UTC milliseconds for one
/// calendar date of the place, after the method's adjustments and rounding:
/// Adhan's `PrayerTimes`, step for step ([`adhan_times`]). Near the poles,
/// where Adhan's day comes out of order (in deep winter the sun stays too low
/// for Asr's shadow, and Adhan's Asr lands after Maghrib or before Dhuhr),
/// the day of the nearest latitude, in 0.5° steps toward the equator, whose
/// times are in order. `None` only where a time can't be found at all.
pub fn method_times(year: i32, month: u8, day: u8, lat: f64, lng: f64, m: &Method, shadow_length: f64) -> Option<[i64; 6]> {
    let first = adhan_times(year, month, day, lat, lng, m, shadow_length);
    let in_order = |t: &[i64; 6]| t.windows(2).all(|w| w[0] <= w[1]);
    if first.as_ref().is_none_or(in_order) || lat.abs() < 60.0 {
        return first;
    }
    let mut l = lat;
    while l.abs() >= 60.0 {
        l -= lat.signum() * 0.5;
        if let Some(t) = adhan_times(year, month, day, l, lng, m, shadow_length).filter(in_order) {
            return Some(t);
        }
    }
    first
}

/// Adhan's `PrayerTimes` with `PolarCircleResolution.AqrabBalad`: inside
/// the polar circles, on days without a sunrise or sunset, the nearest
/// latitude's sun.
fn adhan_times(year: i32, month: u8, day: u8, lat: f64, lng: f64, m: &Method, shadow_length: f64) -> Option<[i64; 6]> {
    const HIGH_LATITUDE_THRESHOLD: f64 = 55.0;
    let days = days_from_civil(year, month, day);
    let day0 = days * DAY_MS;
    let mut st = SolarTime::new(year, month, day, lat, lng);
    let (ty, tm, td) = civil_from_days(days + 1);
    let mut tomorrow = SolarTime::new(ty, tm, td, lat, lng);
    if st.sunrise.is_nan() || st.sunset.is_nan() || tomorrow.sunrise.is_nan() {
        if let Some((s, t)) = aqrab_balad(year, month, day, lat, lng) {
            (st, tomorrow) = (s, t);
        }
    }
    let dhuhr = utc_ms(day0, st.transit)?;
    let sunrise = utc_ms(day0, st.sunrise)?;
    let sunset = utc_ms(day0, st.sunset)?;
    let asr = utc_ms(day0, st.afternoon(shadow_length))?;
    let night = utc_ms(day0 + DAY_MS, tomorrow.sunrise).map(|r| (r - sunset) as f64 / 1000.0);
    let portion = HighLatitudeRule::recommended(lat).portion();
    let day_of_year = days - days_from_civil(year, 1, 1) + 1;
    let moonsighting_high = m.moonsighting && lat >= HIGH_LATITUDE_THRESHOLD;

    let mut fajr = utc_ms(day0, st.hour_angle(-m.fajr_angle, false));
    if moonsighting_high {
        fajr = night.map(|n| add_seconds(sunrise, -(n / 7.0)));
    }
    let safe_fajr = if m.moonsighting { Some(season_adjusted_morning(lat, day_of_year, year, sunrise)) } else { night.map(|n| add_seconds(sunrise, -(portion * n))) };
    // JavaScript: a missing (NaN) side never wins the comparison.
    let fajr = match (fajr, safe_fajr) {
        (Some(f), Some(s)) => f.max(s),
        (None, s) => s?,
        (f, None) => f?,
    };
    let isha = if m.isha_interval_min > 0.0 {
        add_seconds(sunset, m.isha_interval_min * 60.0)
    } else {
        let mut isha = utc_ms(day0, st.hour_angle(-m.isha_angle, true));
        if moonsighting_high {
            isha = night.map(|n| add_seconds(sunset, n / 7.0));
        }
        let safe_isha = if m.moonsighting { Some(season_adjusted_evening(lat, day_of_year, year, sunset)) } else { night.map(|n| add_seconds(sunset, portion * n)) };
        match (isha, safe_isha) {
            (Some(i), Some(s)) => i.min(s),
            (None, s) => s?,
            (i, None) => i?,
        }
    };
    let mut maghrib = sunset;
    if m.maghrib_angle > 0.0 {
        if let Some(angle) = utc_ms(day0, st.hour_angle(-m.maghrib_angle, true)) {
            if sunset < angle && isha > angle {
                maghrib = angle;
            }
        }
    }
    let raw = [fajr, sunrise, dhuhr, asr, maghrib, isha];
    Some(std::array::from_fn(|i| rounded_minute(add_seconds(raw[i], (m.adjust[i] * 60) as f64), m.round_up)))
}

/// Initial great-circle bearing from a place to the Kaaba, degrees clockwise
/// from true north (0..360). Oyen Umroh's formula and Kaaba point.
pub fn qibla(lat: f64, lng: f64) -> f64 {
    const KAABA: (f64, f64) = (21.4225, 39.8262);
    let (p1, p2, dl) = (deg2rad(lat), deg2rad(KAABA.0), deg2rad(KAABA.1 - lng));
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    (rad2deg(y.atan2(x)) + 360.0) % 360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn julian_day_matches_meeus() {
        // Meeus example 7.a: 1957 October 4.81 = JD 2436116.31.
        assert!((julian_day(1957, 10, 4) + 0.81 - 2436116.31).abs() < 1e-9);
        assert_eq!(julian_day(2000, 1, 1), 2451544.5);
    }

    #[test]
    fn solar_coordinates_match_meeus_example_25a() {
        // 1992 October 13.0 TD: apparent declination -7.78507, right ascension 198.38083 (Meeus 25.a).
        let s = SolarCoordinates::new(2448908.5);
        assert!((s.declination - -7.78507).abs() < 0.0001, "{}", s.declination);
        assert!((s.right_ascension - 198.38083).abs() < 0.0001, "{}", s.right_ascension);
    }

    #[test]
    fn js_round_rounds_halves_up() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(-2.6), -3.0);
        // 540 / 360 = 1.5 rounds to 2 and -1.5 to -1, as in JavaScript: both land on -180.
        assert_eq!(quadrant_shift_angle(540.0), -180.0);
        assert_eq!(quadrant_shift_angle(-540.0), -180.0);
        assert_eq!(quadrant_shift_angle(190.0), -170.0);
    }

    #[test]
    fn time_components_truncate_like_adhan() {
        // -2.8 h on 2026-10-24 = 21:12:00 UTC the day before (seconds truncated, not rounded).
        let day0 = days_from_civil(2026, 10, 24) * DAY_MS;
        let ms = utc_ms(day0, -2.8).unwrap();
        assert_eq!((ms - day0) / 1000, -2 * 3600 - 48 * 60);
        assert_eq!(utc_ms(day0, f64::NAN), None);
        assert_eq!(add_seconds(1000, -0.0015), 998);
    }

    #[test]
    fn qibla_from_jakarta_points_west_north_west() {
        let q = qibla(-6.1667, 106.8333);
        assert!((q - 295.1).abs() < 0.05, "{q}");
    }

    #[test]
    fn high_latitude_rule_switches_above_48() {
        assert_eq!(HighLatitudeRule::recommended(48.0), HighLatitudeRule::MiddleOfTheNight);
        assert_eq!(HighLatitudeRule::recommended(51.5), HighLatitudeRule::SeventhOfTheNight);
    }
}
