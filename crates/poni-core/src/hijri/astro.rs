//! The sun and the moon as the Hijri calendar needs them: where each stands,
//! when the moon is new, when the sun sets at a place, and how high the young
//! moon is and how far from the sun at that sunset. Formulas from Jean Meeus,
//! "Astronomical Algorithms" (2nd ed.): the sun from chapter 25 (to about
//! 0.01°), the moon from chapter 47 with the larger terms of its tables 47.A
//! and 47.B (to about 0.01°, far inside what a 3° or 5° limit needs).
//!
//! Moments are Julian days in UT. The formulas run in dynamical time, which
//! is UT plus [`DELTA_T_S`].

use std::f64::consts::PI;

/// Dynamical time less UT, seconds: 69 in 2020-2026, growing slowly. A minute
/// out moves the moon by about 0.01°, so one number serves these years.
pub const DELTA_T_S: f64 = 69.0;
/// Julian day of 1970-01-01 00:00 UT.
pub const UNIX_EPOCH_JD: f64 = 2440587.5;
const EARTH_RADIUS_KM: f64 = 6378.14;
/// The sun's upper limb on the horizon, refraction included.
pub const SUNSET_ALT: f64 = -0.8333;

fn rad(d: f64) -> f64 {
    d * PI / 180.0
}

fn deg(r: f64) -> f64 {
    r * 180.0 / PI
}

fn wrap360(a: f64) -> f64 {
    a.rem_euclid(360.0)
}

/// -180..180.
fn wrap180(a: f64) -> f64 {
    let a = wrap360(a);
    if a > 180.0 {
        a - 360.0
    } else {
        a
    }
}

/// Julian centuries of dynamical time since J2000, for a UT Julian day.
fn centuries(jd_ut: f64) -> f64 {
    (jd_ut + DELTA_T_S / 86_400.0 - 2451545.0) / 36525.0
}

/// Where a body stands: ecliptic longitude and latitude (degrees, of date,
/// nutation in longitude included) and its distance (km).
#[derive(Clone, Copy, Debug)]
pub struct Ecliptic {
    pub lon: f64,
    pub lat: f64,
    pub dist_km: f64,
}

fn nutation_lon_and_obliquity(t: f64) -> (f64, f64) {
    let omega = rad(125.04452 - 1934.136261 * t);
    let l = rad(280.4665 + 36000.7698 * t);
    let lp = rad(218.3165 + 481267.8813 * t);
    let dpsi = (-17.20 * omega.sin() - 1.32 * (2.0 * l).sin() - 0.23 * (2.0 * lp).sin() + 0.21 * (2.0 * omega).sin()) / 3600.0;
    let deps = (9.20 * omega.cos() + 0.57 * (2.0 * l).cos() + 0.10 * (2.0 * lp).cos() - 0.09 * (2.0 * omega).cos()) / 3600.0;
    let eps0 = 23.0 + 26.0 / 60.0 + 21.448 / 3600.0 - (46.8150 * t + 0.00059 * t * t - 0.001813 * t * t * t) / 3600.0;
    (dpsi, eps0 + deps)
}

/// The sun's apparent place (Meeus 25), distance in km.
pub fn sun(jd_ut: f64) -> Ecliptic {
    let t = centuries(jd_ut);
    let l0 = 280.46646 + 36000.76983 * t + 0.0003032 * t * t;
    let m = rad(357.52911 + 35999.05029 * t - 0.0001537 * t * t);
    let e = 0.016708634 - 0.000042037 * t - 0.0000001267 * t * t;
    let c = (1.914602 - 0.004817 * t - 0.000014 * t * t) * m.sin() + (0.019993 - 0.000101 * t) * (2.0 * m).sin() + 0.000289 * (3.0 * m).sin();
    let v = m + rad(c);
    let r_au = 1.000001018 * (1.0 - e * e) / (1.0 + e * v.cos());
    let (dpsi, _) = nutation_lon_and_obliquity(t);
    // Aberration: -20.4898" / R.
    let lon = wrap360(l0 + c + dpsi - 20.4898 / 3600.0 / r_au);
    Ecliptic { lon, lat: 0.0, dist_km: r_au * 149_597_870.7 }
}

/// Meeus table 47.A, the larger terms: D, M, M', F, then longitude (1e-6 °)
/// and distance (1e-3 km).
const MOON_LR: [(i8, i8, i8, i8, i32, i32); 40] = [
    (0, 0, 1, 0, 6288774, -20905355),
    (2, 0, -1, 0, 1274027, -3699111),
    (2, 0, 0, 0, 658314, -2955968),
    (0, 0, 2, 0, 213618, -569925),
    (0, 1, 0, 0, -185116, 48888),
    (0, 0, 0, 2, -114332, -3149),
    (2, 0, -2, 0, 58793, 246158),
    (2, -1, -1, 0, 57066, -152138),
    (2, 0, 1, 0, 53322, -170733),
    (2, -1, 0, 0, 45758, -204586),
    (0, 1, -1, 0, -40923, -129620),
    (1, 0, 0, 0, -34720, 108743),
    (0, 1, 1, 0, -30383, 104755),
    (2, 0, 0, -2, 15327, 10321),
    (0, 0, 1, 2, -12528, 0),
    (0, 0, 1, -2, 10980, 79661),
    (4, 0, -1, 0, 10675, -34782),
    (0, 0, 3, 0, 10034, -23210),
    (4, 0, -2, 0, 8548, -21636),
    (2, 1, -1, 0, -7888, 24208),
    (2, 1, 0, 0, -6766, 30824),
    (1, 0, -1, 0, -5163, -8379),
    (1, 1, 0, 0, 4987, -16675),
    (2, -1, 1, 0, 4036, -12831),
    (2, 0, 2, 0, 3994, -10445),
    (4, 0, 0, 0, 3861, -11650),
    (2, 0, -3, 0, 3665, 14403),
    (0, 1, -2, 0, -2689, -7003),
    (2, 0, -1, 2, -2602, 0),
    (2, -1, -2, 0, 2390, 10056),
    (1, 0, 1, 0, -2348, 6322),
    (2, -2, 0, 0, 2236, -9884),
    (0, 1, 2, 0, -2120, 5751),
    (0, 2, 0, 0, -2069, 0),
    (2, -2, -1, 0, 2048, -4950),
    (2, 0, 1, -2, -1773, 4130),
    (2, 0, 0, 2, -1595, 0),
    (4, -1, -1, 0, 1215, -3958),
    (0, 0, 2, 2, -1110, 0),
    (3, 0, -1, 0, -892, 3258),
];

/// Meeus table 47.B, the larger terms: D, M, M', F, latitude (1e-6 °).
const MOON_B: [(i8, i8, i8, i8, i32); 30] = [
    (0, 0, 0, 1, 5128122),
    (0, 0, 1, 1, 280602),
    (0, 0, 1, -1, 277693),
    (2, 0, 0, -1, 173237),
    (2, 0, -1, 1, 55413),
    (2, 0, -1, -1, 46271),
    (2, 0, 0, 1, 32573),
    (0, 0, 2, 1, 17198),
    (2, 0, 1, -1, 9266),
    (0, 0, 2, -1, 8822),
    (2, -1, 0, -1, 8216),
    (2, 0, -2, -1, 4324),
    (2, 0, 1, 1, 4200),
    (2, 1, 0, -1, -3359),
    (2, -1, -1, 1, 2463),
    (2, -1, 0, 1, 2211),
    (2, -1, -1, -1, 2065),
    (0, 1, -1, -1, -1870),
    (4, 0, -1, -1, 1828),
    (0, 1, 0, 1, -1794),
    (0, 0, 0, 3, -1749),
    (0, 1, -1, 1, -1565),
    (1, 0, 0, 1, -1491),
    (0, 1, 1, 1, -1475),
    (0, 1, 1, -1, -1410),
    (0, 1, 0, -1, -1344),
    (1, 0, 0, -1, -1335),
    (0, 0, 3, 1, 1107),
    (4, 0, 0, -1, 1021),
    (4, 0, -1, 1, 833),
];

/// The moon's apparent geocentric place (Meeus 47).
pub fn moon(jd_ut: f64) -> Ecliptic {
    let t = centuries(jd_ut);
    let (t2, t3, t4) = (t * t, t * t * t, t * t * t * t);
    let lp = wrap360(218.3164477 + 481267.88123421 * t - 0.0015786 * t2 + t3 / 538841.0 - t4 / 65194000.0);
    let d = wrap360(297.8501921 + 445267.1114034 * t - 0.0018819 * t2 + t3 / 545868.0 - t4 / 113065000.0);
    let m = wrap360(357.5291092 + 35999.0502909 * t - 0.0001536 * t2 + t3 / 24490000.0);
    let mp = wrap360(134.9633964 + 477198.8675055 * t + 0.0087414 * t2 + t3 / 69699.0 - t4 / 14712000.0);
    let f = wrap360(93.2720950 + 483202.0175233 * t - 0.0036539 * t2 - t3 / 3526000.0 + t4 / 863310000.0);
    let a1 = rad(119.75 + 131.849 * t);
    let a2 = rad(53.09 + 479264.290 * t);
    let a3 = rad(313.45 + 481266.484 * t);
    let e = 1.0 - 0.002516 * t - 0.0000074 * t2;
    let ecc = |mm: i8| match mm.abs() {
        1 => e,
        2 => e * e,
        _ => 1.0,
    };
    let arg = |cd: i8, cm: i8, cmp: i8, cf: i8| rad(cd as f64 * d + cm as f64 * m + cmp as f64 * mp + cf as f64 * f);
    let (mut sl, mut sr, mut sb) = (0.0, 0.0, 0.0);
    for &(cd, cm, cmp, cf, l, r) in &MOON_LR {
        let a = arg(cd, cm, cmp, cf);
        sl += l as f64 * ecc(cm) * a.sin();
        sr += r as f64 * ecc(cm) * a.cos();
    }
    for &(cd, cm, cmp, cf, b) in &MOON_B {
        sb += b as f64 * ecc(cm) * arg(cd, cm, cmp, cf).sin();
    }
    let (lpr, fr, mpr) = (rad(lp), rad(f), rad(mp));
    sl += 3958.0 * a1.sin() + 1962.0 * (lpr - fr).sin() + 318.0 * a2.sin();
    sb += -2235.0 * lpr.sin() + 382.0 * a3.sin() + 175.0 * (a1 - fr).sin() + 175.0 * (a1 + fr).sin() + 127.0 * (lpr - mpr).sin()
        - 115.0 * (lpr + mpr).sin();
    let (dpsi, _) = nutation_lon_and_obliquity(t);
    Ecliptic { lon: wrap360(lp + sl / 1e6 + dpsi), lat: sb / 1e6, dist_km: 385000.56 + sr / 1000.0 }
}

/// Right ascension and declination (degrees) of an ecliptic place.
fn equatorial(p: &Ecliptic, eps: f64) -> (f64, f64) {
    let (l, b, e) = (rad(p.lon), rad(p.lat), rad(eps));
    let ra = (l.sin() * e.cos() - b.tan() * e.sin()).atan2(l.cos());
    let dec = (b.sin() * e.cos() + b.cos() * e.sin() * l.sin()).asin();
    (wrap360(deg(ra)), deg(dec))
}

/// Greenwich apparent sidereal time, degrees (Meeus 12.4, with the equation
/// of the equinoxes).
fn sidereal(jd_ut: f64) -> f64 {
    let t = (jd_ut - 2451545.0) / 36525.0;
    let mean = 280.46061837 + 360.98564736629 * (jd_ut - 2451545.0) + 0.000387933 * t * t - t * t * t / 38710000.0;
    let (dpsi, eps) = nutation_lon_and_obliquity(centuries(jd_ut));
    wrap360(mean + dpsi * rad(eps).cos())
}

/// Geocentric altitude (degrees, no refraction) and the hour angle of a body.
fn altitude(p: &Ecliptic, jd_ut: f64, lat: f64, lng: f64) -> f64 {
    let (_, eps) = nutation_lon_and_obliquity(centuries(jd_ut));
    let (ra, dec) = equatorial(p, eps);
    let h = rad(sidereal(jd_ut) + lng - ra);
    let (phi, d) = (rad(lat), rad(dec));
    deg((phi.sin() * d.sin() + phi.cos() * d.cos() * h.cos()).asin())
}

/// The sun's geocentric altitude (degrees, no refraction) at a place.
pub fn sun_altitude(jd_ut: f64, lat: f64, lng: f64) -> f64 {
    altitude(&sun(jd_ut), jd_ut, lat, lng)
}

/// Sun minus moon in longitude is zero: the new moon (conjunction) nearest
/// `jd_ut`, UT Julian day.
pub fn new_moon_near(jd_ut: f64) -> f64 {
    let mut t = jd_ut;
    for _ in 0..8 {
        let gap = wrap180(moon(t).lon - sun(t).lon);
        // The moon gains about 12.19° a day on the sun.
        let step = gap / 12.1907;
        t -= step;
        if step.abs() < 1e-6 {
            break;
        }
    }
    t
}

/// The new moon of lunation `k` (Meeus 49: k = 0 is the one of 6 Jan 2000).
pub fn new_moon(k: i64) -> f64 {
    new_moon_near(2451550.09766 + 29.530588861 * k as f64)
}

/// The lunation whose new moon falls on or before `jd_ut`.
pub fn lunation_at(jd_ut: f64) -> i64 {
    let mut k = ((jd_ut - 2451550.09766) / 29.530588861).floor() as i64;
    while new_moon(k + 1) <= jd_ut {
        k += 1;
    }
    while new_moon(k) > jd_ut {
        k -= 1;
    }
    k
}

/// When the sun sets at a place on the UTC date that starts at `day_jd`
/// (a Julian day at 0h UT), as seen at the place: its evening that falls
/// in `[day_jd + (−lng/360 − 0.5) .. +1)`, the local civil day roughly.
/// `None` where it doesn't set (polar day or night).
pub fn sunset(day_jd: f64, lat: f64, lng: f64) -> Option<f64> {
    // Local apparent noon is near 12h - lng/15 UT; sunset a quarter day after.
    let mut t = day_jd + 0.5 - lng / 360.0 + 0.25;
    for _ in 0..6 {
        let s = sun(t);
        let (_, eps) = nutation_lon_and_obliquity(centuries(t));
        let (ra, dec) = equatorial(&s, eps);
        let (phi, d) = (rad(lat), rad(dec));
        let cos_h0 = (rad(SUNSET_ALT).sin() - phi.sin() * d.sin()) / (phi.cos() * d.cos());
        if !(-1.0..=1.0).contains(&cos_h0) {
            return None;
        }
        let h0 = deg(cos_h0.acos());
        let h = wrap180(sidereal(t) + lng - ra);
        let dt = wrap180(h0 - h) / 360.98564736629;
        t += dt;
        if dt.abs() < 1e-6 {
            break;
        }
    }
    Some(t)
}

/// The young moon at one sunset: how high it is and how far from the sun.
#[derive(Clone, Copy, Debug)]
pub struct Crescent {
    /// Altitude of the moon's centre, degrees: geocentric, or seen from the
    /// place (parallax taken off), no refraction either way.
    pub alt_geo: f64,
    pub alt_topo: f64,
    /// Sun-moon elongation, degrees, geocentric and seen from the place.
    pub elong_geo: f64,
    pub elong_topo: f64,
}

pub fn crescent(jd_ut: f64, lat: f64, lng: f64) -> Crescent {
    let (m, s) = (moon(jd_ut), sun(jd_ut));
    let alt_geo = altitude(&m, jd_ut, lat, lng);
    // Parallax in altitude: asin(sin π cos h).
    let sin_pi = EARTH_RADIUS_KM / m.dist_km;
    let alt_topo = alt_geo - deg((sin_pi * rad(alt_geo).cos()).asin());
    let elong_geo = deg((rad(m.lat).cos() * rad(m.lon - s.lon).cos()).clamp(-1.0, 1.0).acos());
    // Seen from the place, the moon drops by the parallax toward the horizon;
    // at sunset the sun is on that horizon, so the gap closes by about the
    // same amount along the vertical. Taken as the angle between the two
    // topocentric directions.
    let elong_topo = topo_elongation(&m, &s, jd_ut, lat, lng);
    Crescent { alt_geo, alt_topo, elong_geo, elong_topo }
}

/// Angle between the sun and the moon as seen from a point on the ground.
fn topo_elongation(m: &Ecliptic, s: &Ecliptic, jd_ut: f64, lat: f64, lng: f64) -> f64 {
    let (_, eps) = nutation_lon_and_obliquity(centuries(jd_ut));
    let lst = rad(sidereal(jd_ut) + lng);
    let obs = [EARTH_RADIUS_KM * rad(lat).cos() * lst.cos(), EARTH_RADIUS_KM * rad(lat).cos() * lst.sin(), EARTH_RADIUS_KM * rad(lat).sin()];
    let vec = |p: &Ecliptic| {
        let (ra, dec) = equatorial(p, eps);
        let (ra, dec) = (rad(ra), rad(dec));
        [p.dist_km * dec.cos() * ra.cos() - obs[0], p.dist_km * dec.cos() * ra.sin() - obs[1], p.dist_km * dec.sin() - obs[2]]
    };
    let (a, b) = (vec(m), vec(s));
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let na = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    let nb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
    deg((dot / (na * nb)).clamp(-1.0, 1.0).acos())
}

/// A UT Julian day as UTC seconds since 1970.
pub fn jd_to_unix(jd: f64) -> i64 {
    ((jd - UNIX_EPOCH_JD) * 86_400.0).round() as i64
}

/// Days since 1970-01-01 at 0h UT as a Julian day.
pub fn day_to_jd(day: i64) -> f64 {
    UNIX_EPOCH_JD + day as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moon_matches_meeus_example_47a() {
        // 1992 April 12, 0h TD: λ 133.162655°, β -3.229126°, Δ 368409.7 km
        // (apparent λ with nutation: 133.167265°).
        let jd_td = 2448724.5;
        let m = moon(jd_td - DELTA_T_S / 86_400.0);
        assert!((m.lon - 133.167265).abs() < 0.01, "λ {}", m.lon);
        assert!((m.lat - -3.229126).abs() < 0.005, "β {}", m.lat);
        assert!((m.dist_km - 368409.7).abs() < 30.0, "Δ {}", m.dist_km);
    }

    #[test]
    fn sun_matches_meeus_example_25a() {
        // 1992 October 13, 0h TD: apparent λ 199.90895° (high accuracy 199.90599°).
        let s = sun(2448908.5 - DELTA_T_S / 86_400.0);
        assert!((s.lon - 199.906).abs() < 0.01, "λ {}", s.lon);
    }

    #[test]
    fn finds_known_new_moons() {
        // 25 June 2025 10:31 UTC; 17 Feb 2026 12:01 UTC (annular eclipse).
        let nm = |y, mo, d| new_moon_near(day_to_jd(crate::datetime::days_from_civil(y, mo, d)));
        let a = jd_to_unix(nm(2025, 6, 25));
        let b = jd_to_unix(nm(2026, 2, 17));
        let at = |y: i32, mo: u8, d: u8, h: i64, mi: i64| crate::datetime::days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60;
        assert!((a - at(2025, 6, 25, 10, 31)).abs() < 180, "off by {} s", a - at(2025, 6, 25, 10, 31));
        assert!((b - at(2026, 2, 17, 12, 1)).abs() < 180, "off by {} s", b - at(2026, 2, 17, 12, 1));
    }

    #[test]
    fn jakarta_sunset() {
        // Jakarta, 17 Feb 2026: sunset about 18:17 WIB = 11:17 UTC.
        let day = crate::datetime::days_from_civil(2026, 2, 17);
        let t = sunset(day_to_jd(day), -6.2, 106.8).unwrap();
        let u = jd_to_unix(t) - day * 86_400;
        assert!((u - (11 * 3600 + 17 * 60)).abs() < 240, "sunset {} UTC s", u);
    }
}
