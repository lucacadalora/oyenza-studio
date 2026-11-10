//! The Hijri date, by one of two calendars:
//!
//! - **Kemenag** (the government's): a month starts the evening the young
//!   moon, at sunset somewhere in Indonesia, stands at least 3° high and
//!   6.4° from the sun (MABIMS' criteria). Kemenag's sidang isbat has the
//!   last word for Ramadan, Syawal and Dzulhijjah, so the dates it announced
//!   are kept here ([`KEMENAG_DECIDED`]) and win over the reckoning.
//! - **KHGT**, Kalender Hijriah Global Tunggal (Muhammadiyah's, from 1
//!   Muharram 1447): one date for the whole world. A month starts the day
//!   after the first date on which, before 24:00 UTC, the moon at sunset
//!   anywhere on Earth stands at least 5° high and 8° from the sun; or when
//!   that happens only later the same night, on land in the Americas, while
//!   the new moon came before dawn in New Zealand.
//!
//! The reckoning runs on [`astro`] (Meeus) over a grid of places. One month
//! takes a few milliseconds and is kept once worked out; nothing runs unless
//! a date is asked for.

pub mod astro;

use std::collections::HashMap;
use std::sync::Mutex;

use astro::{crescent, day_to_jd, new_moon, sunset};

/// Which calendar's dates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Calendar {
    /// Kemenag RI (MABIMS, and the isbat's decisions).
    #[default]
    Kemenag,
    /// Kalender Hijriah Global Tunggal (Muhammadiyah).
    Khgt,
}

/// A Hijri date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HijriDate {
    pub year: i32,
    /// 1 = Muharram ... 12 = Dzulhijjah.
    pub month: u8,
    pub day: u8,
}

/// The months as Indonesia writes them.
pub const MONTHS: [&str; 12] = [
    "Muharram",
    "Safar",
    "Rabiul Awal",
    "Rabiul Akhir",
    "Jumadil Awal",
    "Jumadil Akhir",
    "Rajab",
    "Syakban",
    "Ramadan",
    "Syawal",
    "Zulkaidah",
    "Zulhijah",
];

impl HijriDate {
    /// "21 Jumadil Awal 1448 H".
    pub fn label(&self) -> String {
        format!("{} {} {} H", self.day, MONTHS[(self.month - 1) as usize], self.year)
    }
}

/// Lunation 315 (Meeus' count, new moon 25 June 2025) is Muharram 1447.
const MUHARRAM_1447_LUNATION: i64 = 315;

/// The Hijri month (years * 12 + month - 1, from 1 AH) a lunation begins.
fn month_index(k: i64) -> i64 {
    (1447 - 1) * 12 + (k - MUHARRAM_1447_LUNATION)
}

/// Month starts the government announced (sidang isbat, or Kemenag's
/// calendar for Muharram), which win over the reckoning: Hijri year, month,
/// the 1st. Add each new isbat's here.
pub const KEMENAG_DECIDED: &[(i32, u8, (i32, u8, u8))] = &[
    (1445, 9, (2024, 3, 12)),
    (1445, 10, (2024, 4, 10)),
    (1445, 12, (2024, 6, 8)),
    (1446, 9, (2025, 3, 1)),
    (1446, 10, (2025, 3, 31)),
    (1446, 12, (2025, 5, 28)),
    (1447, 1, (2025, 6, 27)),
    (1447, 9, (2026, 2, 19)),
    (1447, 10, (2026, 3, 21)),
    (1447, 12, (2026, 5, 18)),
    (1448, 1, (2026, 6, 16)),
];

/// KHGT's month starts as Muhammadiyah publishes them
/// (khgt.muhammadiyah.or.id, 1447-1451 H), which win over the reckoning:
/// three of these 60 come a day off from it (Rajab 1450, Muharram and
/// Syakban 1451), where the moon only just meets the limits and the
/// published rule doesn't say enough to tell. Add new years here as they
/// come out.
pub const KHGT_PUBLISHED: &[(i32, u8, (i32, u8, u8))] = &[
    (1447, 1, (2025, 6, 26)),
    (1447, 2, (2025, 7, 26)),
    (1447, 3, (2025, 8, 24)),
    (1447, 4, (2025, 9, 23)),
    (1447, 5, (2025, 10, 23)),
    (1447, 6, (2025, 11, 21)),
    (1447, 7, (2025, 12, 21)),
    (1447, 8, (2026, 1, 20)),
    (1447, 9, (2026, 2, 18)),
    (1447, 10, (2026, 3, 20)),
    (1447, 11, (2026, 4, 18)),
    (1447, 12, (2026, 5, 18)),
    (1448, 1, (2026, 6, 16)),
    (1448, 2, (2026, 7, 15)),
    (1448, 3, (2026, 8, 14)),
    (1448, 4, (2026, 9, 12)),
    (1448, 5, (2026, 10, 12)),
    (1448, 6, (2026, 11, 10)),
    (1448, 7, (2026, 12, 10)),
    (1448, 8, (2027, 1, 9)),
    (1448, 9, (2027, 2, 8)),
    (1448, 10, (2027, 3, 9)),
    (1448, 11, (2027, 4, 8)),
    (1448, 12, (2027, 5, 7)),
    (1449, 1, (2027, 6, 6)),
    (1449, 2, (2027, 7, 5)),
    (1449, 3, (2027, 8, 3)),
    (1449, 4, (2027, 9, 2)),
    (1449, 5, (2027, 10, 1)),
    (1449, 6, (2027, 10, 31)),
    (1449, 7, (2027, 11, 29)),
    (1449, 8, (2027, 12, 29)),
    (1449, 9, (2028, 1, 28)),
    (1449, 10, (2028, 2, 26)),
    (1449, 11, (2028, 3, 27)),
    (1449, 12, (2028, 4, 26)),
    (1450, 1, (2028, 5, 25)),
    (1450, 2, (2028, 6, 24)),
    (1450, 3, (2028, 7, 23)),
    (1450, 4, (2028, 8, 21)),
    (1450, 5, (2028, 9, 20)),
    (1450, 6, (2028, 10, 19)),
    (1450, 7, (2028, 11, 17)),
    (1450, 8, (2028, 12, 17)),
    (1450, 9, (2029, 1, 16)),
    (1450, 10, (2029, 2, 14)),
    (1450, 11, (2029, 3, 16)),
    (1450, 12, (2029, 4, 15)),
    (1451, 1, (2029, 5, 15)),
    (1451, 2, (2029, 6, 13)),
    (1451, 3, (2029, 7, 13)),
    (1451, 4, (2029, 8, 11)),
    (1451, 5, (2029, 9, 9)),
    (1451, 6, (2029, 10, 9)),
    (1451, 7, (2029, 11, 7)),
    (1451, 8, (2029, 12, 6)),
    (1451, 9, (2030, 1, 5)),
    (1451, 10, (2030, 2, 4)),
    (1451, 11, (2030, 3, 5)),
    (1451, 12, (2030, 4, 4)),
];

/// What a calendar's months need of the young moon at sunset.
#[derive(Clone, Copy, Debug)]
struct Criterion {
    alt: f64,
    elong: f64,
    /// Altitude and elongation seen from the place (parallax taken off)
    /// rather than from the earth's centre.
    alt_topo: bool,
    elong_topo: bool,
    /// Grid step (degrees) over the places looked at.
    step: f64,
}

/// Kemenag reckons the altitude from the place and the elongation from the
/// earth's centre (its 28 Feb 2025 report for Aceh: 4°40' high, 6°24' apart).
const MABIMS: Criterion = Criterion { alt: 3.0, elong: 6.4, alt_topo: true, elong_topo: false, step: 0.5 };
const KHGT: Criterion = Criterion { alt: 5.0, elong: 8.0, alt_topo: false, elong_topo: false, step: 2.0 };

impl Criterion {
    fn met(&self, jd: f64, lat: f64, lng: f64) -> bool {
        let c = crescent(jd, lat, lng);
        let alt = if self.alt_topo { c.alt_topo } else { c.alt_geo };
        let el = if self.elong_topo { c.elong_topo } else { c.elong_geo };
        alt >= self.alt && el >= self.elong
    }
}

fn grid(step: f64, lat0: f64, lat1: f64, lng0: f64, lng1: f64) -> impl Iterator<Item = (f64, f64)> {
    let nlat = ((lat1 - lat0) / step).round() as i64;
    let nlng = ((lng1 - lng0) / step).round() as i64;
    (0..=nlat).flat_map(move |i| (0..=nlng).map(move |j| (lat0 + i as f64 * step, lng0 + j as f64 * step)))
}


/// The earliest month start a criterion gives after the new moon `conj`:
/// for every sunset after it, in `area` (on land where `land` says), that
/// meets it, the day after that sunset's UTC date. `exception` maps a sunset
/// (its JD) to the start it gives instead, when it should count earlier.
fn earliest_start(cr: &Criterion, conj: f64, area: (f64, f64, f64, f64), land: Option<fn(f64, f64) -> bool>, exception: Option<&dyn Fn(f64) -> i64>) -> Option<i64> {
    let conj_day = (conj - astro::UNIX_EPOCH_JD).floor() as i64;
    let mut best: Option<i64> = None;
    for d in conj_day - 1..=conj_day + 2 {
        let day_jd = day_to_jd(d);
        for (lat, lng) in grid(cr.step, area.0, area.1, area.2, area.3) {
            if land.is_some_and(|f| !f(lat, lng)) {
                continue;
            }
            let Some(t) = sunset(day_jd, lat, lng) else { continue };
            if t <= conj || !cr.met(t, lat, lng) {
                continue;
            }
            let start = exception.map_or_else(|| (t - astro::UNIX_EPOCH_JD).floor() as i64 + 1, |f| f(t));
            best = Some(best.map_or(start, |b| b.min(start)));
        }
        // Every later evening's sunset is on day d or after.
        if best.is_some_and(|b| b <= d + 1) {
            break;
        }
    }
    best
}

/// Indonesia, Sabang to Merauke, Miangas to Rote.
const INDONESIA: (f64, f64, f64, f64) = (-11.0, 6.0, 95.0, 141.0);

/// Kemenag's reckoning: the first Indonesian evening after the new moon that
/// meets MABIMS; the month starts the next day. Istikmal (the month before
/// runs to 30 days) caps it two days after the new moon's.
fn kemenag_start(conj: f64) -> i64 {
    let conj_day = (conj - astro::UNIX_EPOCH_JD).floor() as i64;
    earliest_start(&MABIMS, conj, INDONESIA, None, None).unwrap_or(conj_day + 2).min(conj_day + 2)
}

/// The parts of the Americas that are land, roughly: boxes over the
/// continents, from Alaska to Tierra del Fuego.
fn americas_land(lat: f64, lng: f64) -> bool {
    const BOXES: &[(f64, f64, f64, f64)] = &[
        (54.0, 71.0, -168.0, -141.0), // Alaska
        (49.0, 70.0, -141.0, -60.0),  // Canada
        (25.0, 49.0, -124.0, -67.0),  // the United States
        (15.0, 32.0, -117.0, -87.0),  // Mexico
        (7.0, 18.0, -92.0, -77.0),    // Central America
        (-5.0, 12.0, -80.0, -35.0),   // northern South America
        (-23.0, -5.0, -81.0, -35.0),
        (-40.0, -23.0, -73.0, -48.0),
        (-55.0, -40.0, -75.0, -63.0), // Patagonia
    ];
    BOXES.iter().any(|&(a, b, c, d)| (a..=b).contains(&lat) && (c..=d).contains(&lng))
}

/// Dawn (sun 18° below, rising) in Wellington on New Zealand's date `day`
/// + 1, which falls on the UTC date `day` (NZ is 12-13 hours ahead).
fn nz_dawn(day: i64) -> f64 {
    let (lat, lng) = (-41.29, 174.78);
    // Solar noon of NZ's day + 1 is just after 00:00 UTC of day + 1; dawn is
    // some 7 hours before.
    let mut t = day_to_jd(day + 1) + 0.5 - lng / 360.0 - 0.3;
    for _ in 0..8 {
        let alt = astro::sun_altitude(t, lat, lng);
        // The sun climbs about 0.18° a minute there at dawn.
        let dt = (-18.0 - alt) / (0.18 * 1440.0);
        t += dt;
        if dt.abs() < 1e-5 {
            break;
        }
    }
    t
}

/// The whole earth, as far as the young moon can be seen at sunset.
const WORLD: (f64, f64, f64, f64) = (-60.0, 60.0, -180.0, 179.0);

/// KHGT: the day after the first UTC date on which the moon meets 5° and 8°
/// at some sunset before 24:00 UTC; or the day after an evening in the
/// Americas that meets it on land past 24:00 UTC, when the new moon came
/// before dawn in New Zealand.
fn khgt_start(conj: f64) -> i64 {
    let conj_day = (conj - astro::UNIX_EPOCH_JD).floor() as i64;
    let regular = earliest_start(&KHGT, conj, WORLD, None, None).unwrap_or(conj_day + 2);
    // A sunset at UTC moment t past midnight belongs to the evening of the UTC
    // date before; that evening's month starts on t's own UTC date.
    let americas = |t: f64| {
        let utc_day = (t - astro::UNIX_EPOCH_JD).floor() as i64;
        let evening = utc_day - 1;
        if t - day_to_jd(utc_day) < 0.5 && conj < nz_dawn(evening) {
            utc_day
        } else {
            utc_day + 1
        }
    };
    let late = earliest_start(&KHGT, conj, (-56.0, 71.0, -168.0, -35.0), Some(americas_land), Some(&americas)).unwrap_or(i64::MAX);
    regular.min(late).min(conj_day + 2)
}

/// The month start by the reckoning alone.
fn reckon(cal: Calendar, k: i64) -> i64 {
    let conj = new_moon(k);
    match cal {
        Calendar::Kemenag => kemenag_start(conj),
        Calendar::Khgt => khgt_start(conj),
    }
}

static STARTS: Mutex<Option<HashMap<(Calendar, i64), i64>>> = Mutex::new(None);

/// The first day (days since 1970, as a civil date) of the month lunation
/// `k` begins, by `cal`.
pub fn month_start(cal: Calendar, k: i64) -> i64 {
    if let Some(&d) = STARTS.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&(cal, k)).copied())).as_ref() {
        return d;
    }
    let idx = month_index(k);
    let (year, month) = ((idx / 12 + 1) as i32, (idx % 12 + 1) as u8);
    let table = match cal {
        Calendar::Kemenag => KEMENAG_DECIDED,
        Calendar::Khgt => KHGT_PUBLISHED,
    };
    let decided = table.iter().find(|&&(y, m, _)| y == year && m == month).map(|&(_, _, (gy, gm, gd))| crate::datetime::days_from_civil(gy, gm, gd));
    let d = decided.unwrap_or_else(|| reckon(cal, k));
    if let Ok(mut g) = STARTS.lock() {
        g.get_or_insert_with(HashMap::new).insert((cal, k), d);
    }
    d
}

/// The Hijri date of the civil date `day` (days since 1970). In Indonesia
/// the new date is reckoned from the day, not from the evening before.
pub fn date(cal: Calendar, day: i64) -> HijriDate {
    // The lunation whose month holds `day`: the new moon before it, or the
    // one before that when this month hasn't started yet.
    let mut k = astro::lunation_at(day_to_jd(day) + 1.0);
    while month_start(cal, k) > day {
        k -= 1;
    }
    let idx = month_index(k);
    HijriDate { year: (idx / 12 + 1) as i32, month: (idx % 12 + 1) as u8, day: (day - month_start(cal, k) + 1) as u8 }
}

/// Is `day` in Ramadan by `cal`?
pub fn is_ramadan(cal: Calendar, day: i64) -> bool {
    date(cal, day).month == 9
}

#[cfg(test)]
mod tests;
