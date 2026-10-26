//! Prayer tab, while the Prayer times add-on is on. On the left, the next
//! prayer and a countdown to it in hours, minutes and seconds, with its
//! time beside it. On the right, today on one arc: the five prayers as dots
//! from Subuh to Isya, the sun (Terbit to Maghrib) or the moon where the day
//! stands now, the part of the day gone brighter than the part to come, and
//! each prayer's name and time under its dot (past ones dim, the next one
//! lit). Above the arc: Imsak, Terbit and the qibla. Then what happens at
//! prayer time. "Change" swaps in the place picker: type a kab/kota or any
//! city in the world (looked up online as you type, two letters' worth at a
//! time), or go back to following the location. Outside Indonesia the
//! place's name carries the method its times follow.
//!
//! The countdown ticks once a second while the page is open ([`next_wake`])
//! and someone is at the PC; nothing runs for it otherwise.
//!
//! Times come from `poni_core::prayer`; the place from `settings.prayer`,
//! found by Windows' or macOS's location (the app) or picked here.

use crate::commands::Command;
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::state::{AppState, CityFile, Locate};
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::prayer::{self, world, DayTimes, Method, Place, Prayer, PrayerMoment, Site, World};
use poni_core::settings::PrayerSound;
use poni_core::svgpath::Seg;
use std::f32::consts::PI;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const HEIGHT: f32 = 146.0;
/// The next prayer's column; the arc takes the rest, at least [`SLOT_MIN`]
/// for each prayer's time under its dot (the column narrows for that on a
/// narrow island, down to [`LEFT_MIN`], where the countdown still fits).
const LEFT_W: f32 = 232.0;
const LEFT_MIN: f32 = 190.0;
const SLOT_MIN: f32 = 50.0;
const COL_GAP: f32 = 12.0;
/// The left column's rows: the prayer's name, the countdown (its time and
/// day beside it, [`AT_GAP`] after it), the place.
const NAME_H: f32 = 22.0;
const COUNT_Y: f32 = 22.0;
const COUNT_H: f32 = 38.0;
const AT_GAP: f32 = 14.0;
const PLACE_Y: f32 = 70.0;
/// The arc, from the area's top: Imsak, Terbit and the qibla in the first
/// row, the arc's ends and its top under them, then each prayer's name and
/// time under the dots.
const INFO_H: f32 = 15.0;
const ARC_END_Y: f32 = 56.0;
const ARC_TOP_Y: f32 = 28.0;
const LABEL_Y: f32 = 64.0;
const TIME_Y: f32 = 78.0;
/// The prayers sit at the middles of five equal slots along the arc; the
/// night before Subuh and after Isya runs out to its ends.
const SLOTS: [f32; 5] = [0.1, 0.3, 0.5, 0.7, 0.9];
/// The dotted line: a dot this often (DIPs), kept this clear of the
/// prayers' dots and of the sun or moon.
const LINE_STEP: f32 = 6.0;
const LINE_DOT: f32 = 1.15;
const DOT_CLEAR: f32 = 11.0;
const MARK_CLEAR: f32 = 12.0;
const DOT_R: f32 = 3.5;
/// The next prayer: a ring in the accent around a smaller dot, so the sun
/// reads as arriving in it as the time comes (the ring clears its glow).
const NEXT_RING: f32 = 9.5;
const NEXT_DOT: f32 = 3.0;
const SUN_R: f32 = 5.0;
const SUN_HALO: f32 = 8.0;
const MOON_R: f32 = 8.0;
const MOON_ICON: f32 = 12.0;
/// The day gone and the day to come along the line; the prayers' dots,
/// past and later; the words of past prayers.
const GONE: Color = Color::white(0.42);
const TO_COME: Color = Color::white(0.16);
const PAST_DOT: Color = Color::white(0.30);
const LATER_DOT: Color = Color::white(0.62);
const PAST_TEXT: Color = Color::white(0.40);
const SUN: Color = palette::ORANGE;
/// The moon's lavender (the night side of the arc).
const MOON: Color = Color::hex(0xB9AEF7);
const FOOT_Y: f32 = 114.0;
const FOOT_H: f32 = 28.0;
/// The picker: the search row, then two columns of results.
const SEARCH_H: f32 = 32.0;
const RESULT_H: f32 = 26.0;
const RESULTS: usize = 8;
const QURAN: &str = "https://quran.oyenza.com";
/// Where location is allowed for Oyenza Studio: Windows' Settings › Privacy &
/// security › Location, or the Mac's System Settings › Privacy & Security ›
/// Location Services.
const LOCATION_SETTINGS: &str =
    if cfg!(windows) { "ms-settings:privacy-location" } else { "x-apple.systempreferences:com.apple.preference.security?Privacy_LocationServices" };
/// The picker's line when location is off, or when the system couldn't say.
const DENIED_HINT: &str = if cfg!(windows) {
    "Windows' location is off for Oyenza Studio. Turn it on to find your place, or type it."
} else {
    "Location Services is off for Oyenza Studio. Turn it on to find your place, or type it."
};
const UNKNOWN_HINT: &str =
    if cfg!(windows) { "Windows couldn't tell where this PC is. Type your city." } else { "macOS couldn't tell where this Mac is. Type your city." };
const SOUNDS: [(PrayerSound, &str); 3] = [(PrayerSound::Adzan, "Adzan"), (PrayerSound::Tone, "Tone"), (PrayerSound::Silent, "Silent")];

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// The countdown's next second, on the app's clock (mono seconds). The app
/// asks only while the page is open and someone is at the PC, so the page
/// draws once a second then, and never otherwise.
pub fn next_wake(st: &AppState) -> Option<f64> {
    if st.prayer.picking || st.prayer.site().is_none() {
        return None;
    }
    // Right after the wall clock's next whole second, so each frame shows a new one.
    let into = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| f64::from(d.subsec_micros()) / 1e6);
    Some(crate::clock::mono() + 1.0 - into)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if st.prayer.picking {
        picker(ui, st, area);
        return;
    }
    let Some(site) = st.prayer.site() else {
        super::empty_state(ui, area, Icon::Moon, "No place yet", "Pick your city to see its times");
        let w = ui.pill_width("Pick a place", false, true);
        let r = Rect::new(area.center().x - w / 2.0, area.bottom() - PILL_SMALL, w, PILL_SMALL);
        if ui.pill(wid("prayer.pick"), r, "Pick a place", None, PillStyle::Utility).clicked {
            st.prayer.picking = true;
        }
        return;
    };
    let now = st.prayer_now();
    let day = site.local_day(now);
    let Some(times) = prayer::day_times(&site, day) else { return };
    let next = prayer::next_prayer(&site, now);
    let accent = st.accent;
    let left_w = left_width(area.w);

    // ---- Left: the next prayer, the time to it, its time and day.
    let x = area.x;
    if let Some(n) = next {
        ui.c.text(n.prayer.name(), Rect::new(x, area.y, left_w, NAME_H), &TextStyle::value().color(accent));
        let count = countdown(n.at - now);
        let cs = TextStyle::display().weight(Weight::Medium).tabular();
        let cw = ui.c.measure(&count, &cs, left_w).0.ceil();
        ui.c.text(&count, Rect::new(x, area.y + COUNT_Y, cw + 2.0, COUNT_H), &cs);
        let ax = x + cw + AT_GAP;
        let aw = (x + left_w - ax).max(0.0);
        let when = if n.day > day { "tomorrow" } else { "today" };
        ui.c.text(&site.hhmm(n.at), Rect::new(ax, area.y + COUNT_Y + 2.0, aw, 19.0), &TextStyle::headline().tabular());
        ui.c.text(when, Rect::new(ax, area.y + COUNT_Y + 21.0, aw, 15.0), &TextStyle::caption().color(palette::TEXT_DIM));
    }
    let place = st.prayer.place.as_ref().map_or("", |p| p.label.as_str());
    let name_style = TextStyle::footnote().weight(Weight::Semibold);
    ui.c.text(place, Rect::new(x, area.y + PLACE_Y, left_w, 17.0), &name_style);
    // Who sets the times, after the name, where it isn't Kemenag.
    if let Some(tag) = st.prayer.place.as_ref().and_then(method_tag) {
        let tx = x + ui.c.measure(place, &name_style, left_w).0.ceil() + 6.0;
        if tx < x + left_w - 40.0 {
            ui.c.text(&tag, Rect::new(tx, area.y + PLACE_Y, x + left_w - tx, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
        }
    }
    ui.c.text(&place_line(st), Rect::new(x, area.y + PLACE_Y + 17.0, left_w, 16.0), &TextStyle::caption().color(palette::TEXT_DIM));

    // ---- Right: Imsak, Terbit and the qibla, then today on its arc.
    let gx = x + left_w + COL_GAP;
    let arc = Rect::new(gx, area.y, area.right() - gx, FOOT_Y);
    let place_pt = st.prayer.place.as_ref().map(|p| (p.lat, p.lng)).unwrap_or_default();
    let q = prayer::qibla(place_pt.0, place_pt.1);
    let info = format!("Imsak {} \u{b7} Terbit {} \u{b7} Qibla {q:.0}\u{b0} {}", site.hhmm(times.imsak), site.hhmm(times.terbit), compass(q));
    ui.c.text(&info, Rect::new(gx, area.y, arc.w, INFO_H), &TextStyle::caption().color(palette::TEXT_DIM).right());
    draw_day(ui, arc, &site, &times, day, now, next, accent);

    // ---- Footer. Left: Change place, Read Quran. Right, under the arc:
    // what plays at prayer time, and the countdown before it.
    let fy = area.y + FOOT_Y;
    let py = fy + (FOOT_H - PILL_SMALL) / 2.0;
    let change_w = ui.pill_width("Change place", false, true);
    if ui.pill(wid("prayer.change"), Rect::new(x, py, change_w, PILL_SMALL), "Change place", None, PillStyle::Utility).clicked {
        st.prayer.picking = true;
        st.prayer.query.clear();
    }
    let read_w = ui.pill_width("Read Quran", true, true);
    if ui.pill(wid("prayer.quran"), Rect::new(x + change_w + 8.0, py, read_w, PILL_SMALL), "Read Quran", Some(Icon::Open), PillStyle::Utility).clicked {
        st.cmd(Command::OpenUrl(QURAN.into()));
    }

    let names = SOUNDS.map(|s| s.1);
    let sel = SOUNDS.iter().position(|s| s.0 == st.settings.prayer.sound).unwrap_or(1);
    // Never over the two pills when the column narrows.
    let sx = gx.max(x + change_w + 8.0 + read_w + COL_GAP);
    if let Some(i) = super::settings::picker(ui, "prayer.sound", sx, fy + 2.0, 24.0, &names, sel) {
        st.settings.prayer.sound = SOUNDS[i].0;
        st.cmd(Command::SaveSettings);
    }
    let before = "10 min before";
    let bw = ui.c.measure(before, &TextStyle::body(), 200.0).0.ceil() + 10.0 + 30.0;
    let on = st.settings.prayer.remind_before_min > 0;
    if super::settings::switch_row(ui, wid("prayer.before"), Rect::new(area.right() - bw, fy, bw, FOOT_H), before, Some(on), accent) {
        st.settings.prayer.remind_before_min = if on { 0 } else { 10 };
        st.cmd(Command::SaveSettings);
    }
}

/// The left column's width on an island `w` wide: [`LEFT_W`], less on a
/// narrow island so each prayer keeps [`SLOT_MIN`] on the arc.
fn left_width(w: f32) -> f32 {
    (w - COL_GAP - 5.0 * SLOT_MIN).clamp(LEFT_MIN, LEFT_W)
}

/// Today along an arc in `r`: the line dotted (the day gone brighter), the
/// five prayers on it, the sun or the moon where `now` is, and each
/// prayer's name and time under its dot.
#[allow(clippy::too_many_arguments)]
fn draw_day(ui: &mut Ui, r: Rect, site: &Site, times: &DayTimes, day: i64, now: i64, next: Option<PrayerMoment>, accent: Color) {
    let midnight = site.midnight(day);
    let at = Prayer::ALL.map(|p| times.prayer(p));
    let here = day_pos(now, midnight, at);
    let point = |pos: f32| Point::new(r.x + pos * r.w, r.y + ARC_END_Y - (ARC_END_Y - ARC_TOP_Y) * (PI * pos).sin());

    // The dotted line as two fills (the day gone, the day to come), not one
    // draw a dot. Each dot is a tiny hexagon: at this size it reads as round,
    // and a filled shape draws the same everywhere (Wine's Direct2D leaves
    // out the round ends of a hair-long line).
    let n = ((r.w / LINE_STEP).round() as usize).max(2);
    let (mut gone, mut to_come) = (Vec::new(), Vec::new());
    for i in 0..=n {
        let pos = i as f32 / n as f32;
        let near = |p: f32, d: f32| ((p - pos) * r.w).abs() < d;
        if SLOTS.iter().any(|&s| near(s, DOT_CLEAR)) || near(here, MARK_CLEAR) {
            continue;
        }
        let p = point(pos);
        let segs = if pos <= here { &mut gone } else { &mut to_come };
        for k in 0..6 {
            let a = k as f32 * PI / 3.0;
            let (x, y) = (p.x + LINE_DOT * a.cos(), p.y + LINE_DOT * a.sin());
            segs.push(if k == 0 { Seg::MoveTo(x, y) } else { Seg::LineTo(x, y) });
        }
        segs.push(Seg::Close);
    }
    for (segs, c) in [(gone, GONE), (to_come, TO_COME)] {
        if !segs.is_empty() {
            ui.c.path(&segs, Some(c), None);
        }
    }

    // The next prayer may be tomorrow's (Subuh, after Isya): its time then.
    let next_i = next.and_then(|m| Prayer::ALL.iter().position(|&p| p == m.prayer));
    let slot_w = r.w / SLOTS.len() as f32;
    for (i, p) in Prayer::ALL.iter().enumerate() {
        let c = point(SLOTS[i]);
        let is_next = next_i == Some(i);
        let t = match next {
            Some(m) if is_next => m.at,
            _ => at[i],
        };
        let past = !is_next && t <= now;
        if !is_next {
            ui.c.fill_circle(c, DOT_R, if past { PAST_DOT } else { LATER_DOT });
        }
        let (name_c, time_c) = match (is_next, past) {
            (true, _) => (accent, palette::TEXT),
            (false, true) => (PAST_TEXT, PAST_TEXT),
            (false, false) => (palette::TEXT_DIM, palette::TEXT),
        };
        let weight = if is_next { Weight::Semibold } else { Weight::Regular };
        let x0 = r.x + SLOTS[i] * r.w - slot_w / 2.0;
        ui.c.text(p.name(), Rect::new(x0, r.y + LABEL_Y, slot_w, 14.0), &TextStyle::caption().weight(weight).color(name_c).center());
        ui.c.text(&site.hhmm(t), Rect::new(x0, r.y + TIME_Y, slot_w, 16.0), &TextStyle::footnote().weight(weight).tabular().color(time_c).center());
    }

    // Where the day stands: the sun from Terbit to Maghrib, else the moon.
    let m = point(here);
    if (times.terbit..times.maghrib).contains(&now) {
        ui.c.fill_circle(m, SUN_HALO, SUN.with_alpha(0.22));
        ui.c.fill_circle(m, SUN_R, SUN);
    } else {
        ui.c.fill_circle(m, MOON_R, MOON.with_alpha(0.22));
        ui.c.icon(Icon::Moon, Rect::from_center(m, MOON_ICON, MOON_ICON), MOON);
    }
    // The next prayer's ring goes over the sun, which arrives in it.
    if let Some(i) = next_i {
        let c = point(SLOTS[i]);
        ui.c.stroke_circle(c, NEXT_RING, accent, 1.5);
        if (c.x - m.x).abs() > NEXT_RING {
            ui.c.fill_circle(c, NEXT_DOT, accent);
        }
    }
}

/// Where `now` sits along the day's arc, 0 (the place's midnight, the left
/// end) to 1 (the next midnight, the right end): each prayer at the middle
/// of its slot ([`SLOTS`]), the time between two prayers spread evenly
/// between their dots, the night before Subuh and after Isya out to the ends.
fn day_pos(now: i64, midnight: i64, at: [i64; 5]) -> f32 {
    let mut marks = [(midnight, 0.0f32); 7];
    for (i, &t) in at.iter().enumerate() {
        marks[i + 1] = (t, SLOTS[i]);
    }
    marks[6] = (midnight + 86_400, 1.0);
    for w in marks.windows(2) {
        let ((t0, p0), (t1, p1)) = (w[0], w[1]);
        if now < t1 {
            let k = ((now - t0) as f32 / (t1 - t0).max(1) as f32).clamp(0.0, 1.0);
            return p0 + (p1 - p0) * k;
        }
    }
    1.0
}

/// "01:24:59": hours, minutes and seconds to go.
fn countdown(secs: i64) -> String {
    let s = secs.max(0);
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// N, NNE, NE … for a bearing in degrees.
fn compass(deg: f64) -> &'static str {
    const POINTS: [&str; 16] = ["N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW"];
    POINTS[(((deg.rem_euclid(360.0) + 11.25) / 22.5) as usize) % 16]
}

/// The method a place's times follow, where it isn't Kemenag's: "Moonsighting
/// Committee", "Karachi, Hanafi".
fn method_tag(p: &Place) -> Option<String> {
    match p.method {
        Method::Kemenag => None,
        m if p.hanafi => Some(format!("{}, Hanafi", m.name())),
        m => Some(m.name().to_string()),
    }
}

/// How the place was chosen, under its name.
fn place_line(st: &AppState) -> String {
    let p = &st.prayer;
    if st.settings.prayer.follow_location {
        match p.locate {
            Locate::Found { km } if km > 30.0 => format!("Nearest to you, {km:.0} km away"),
            Locate::Found { .. } => "From your location".into(),
            Locate::Asking => "Finding your location".into(),
            Locate::Denied if p.suggested => "Location is off: from your time zone".into(),
            _ if p.suggested => "From your time zone".into(),
            _ => "Your last location".into(),
        }
    } else if p.suggested {
        "From your time zone".into()
    } else {
        "Picked by you".into()
    }
}

// ---------------------------------------------------------------- picker

fn picker(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let accent = st.accent;
    let done_w = ui.pill_width("Done", false, true);
    let done = Rect::new(area.right() - done_w, area.y + (SEARCH_H - PILL_SMALL) / 2.0, done_w, PILL_SMALL);
    let loc_label = "Use my location";
    let loc_w = ui.pill_width(loc_label, true, true);
    let loc = Rect::new(done.x - 8.0 - loc_w, done.y, loc_w, PILL_SMALL);
    let input = Rect::new(area.x, area.y, loc.x - 12.0 - area.x, SEARCH_H);
    let resp = ui.text_input_row(wid("prayer.search"), input, &mut st.prayer.query, "Type a city, like Bandung or London", 40, accent, Some(Icon::Moon));
    // Cities abroad come from the website: the file of the first two letters
    // typed, asked for once while the picker is open.
    let file = world::file_for(&world::fold(&st.prayer.query));
    let typed = st.prayer.found.as_ref().is_some_and(|(q, _)| *q != st.prayer.query);
    if let Some(f) = &file {
        // Offline last time: ask again once something new is typed.
        if typed && matches!(st.prayer.cities.get(f), Some(CityFile::Failed)) {
            st.prayer.cities.remove(f);
        }
        if !st.prayer.cities.contains_key(f) {
            st.prayer.cities.insert(f.clone(), CityFile::Asking);
            st.cmd(Command::GetCities(f.clone()));
        }
    }
    let city_file = file.as_ref().and_then(|f| st.prayer.cities.get(f)).cloned();
    // Searched again when the text changes or the file comes.
    if st.prayer.found.as_ref().is_none_or(|(q, _)| *q != st.prayer.query) {
        let world = match &city_file {
            Some(CityFile::Here(w)) => Some(w.clone()),
            _ => None,
        };
        let found = match &st.prayer.places {
            Some(list) => prayer::search(list, world.as_deref(), &st.prayer.query, RESULTS),
            None => Vec::new(),
        };
        st.prayer.found = Some((st.prayer.query.clone(), found));
    }
    let found = |st: &AppState, i: usize| st.prayer.found.as_ref().and_then(|f| f.1.get(i)).cloned();
    let count = st.prayer.found.as_ref().map_or(0, |f| f.1.len());
    if resp.submitted {
        if let Some(p) = found(st, 0) {
            choose(st, p);
            return;
        }
    }
    if ui.pill(wid("prayer.locate"), loc, loc_label, None, PillStyle::UtilityAccent(accent)).clicked {
        st.settings.prayer.follow_location = true;
        st.cmd(Command::SaveSettings);
        close(st);
        return;
    }
    if ui.pill(wid("prayer.done"), done, "Done", None, PillStyle::Utility).clicked || resp.cancelled {
        close(st);
        return;
    }

    let top = area.y + SEARCH_H + 12.0;
    if st.prayer.query.trim().is_empty() {
        let hint = match st.prayer.locate {
            Locate::Denied => DENIED_HINT,
            Locate::Unknown => UNKNOWN_HINT,
            Locate::Far { .. } => "No city in the list is near you. Type the nearest one.",
            Locate::Offline => "Couldn't look up the cities near you. Type yours, or try again online.",
            _ => "Every kab/kota in Kemenag's schedule, and cities all over the world from GeoNames.",
        };
        ui.c.text(hint, Rect::new(area.x, top, area.w, 18.0), &TextStyle::footnote().color(palette::TEXT_DIM));
        if st.prayer.locate == Locate::Denied {
            let w = ui.pill_width("Location settings", true, true);
            let r = Rect::new(area.x, top + 26.0, w, PILL_SMALL);
            if ui.pill(wid("prayer.location-settings"), r, "Location settings", Some(Icon::Open), PillStyle::Utility).clicked {
                st.cmd(Command::OpenUrl(LOCATION_SETTINGS.into()));
            }
        }
        return;
    }
    if count == 0 {
        let line = match city_file {
            Some(CityFile::Asking) => "Looking up cities\u{2026}",
            Some(CityFile::Failed) => "Couldn't reach the city list. Check the connection and type again.",
            _ => "No city with that name",
        };
        ui.c.text(line, Rect::new(area.x, top, area.w, 18.0), &TextStyle::footnote().color(palette::TEXT_DIM));
        return;
    }
    let col_w = (area.w - 16.0) / 2.0;
    // The bottom row's hover wash meets the Pill card's rounded corners.
    let card = super::card::pill_card(st, area);
    let mut clicked = None;
    for (i, place) in st.prayer.found.iter().flat_map(|f| f.1.iter()).enumerate() {
        let (label, prov) = (&place.label, &place.prov);
        let r = Rect::new(area.x + (i / 4) as f32 * (col_w + 16.0), top + (i % 4) as f32 * RESULT_H, col_w, RESULT_H);
        let b = ui.button(wid_n("prayer.result", i as u64), Rect::new(r.x - 8.0, r.y, r.w + 8.0, r.h));
        if b.hovered || b.pressed {
            super::card::fill_box(ui, card, Rect::new(r.x - 8.0, r.y, r.w + 8.0, r.h), 8.0, Color::white(if b.pressed { 0.10 } else { 0.06 }));
        }
        let lw = ui.c.measure(label, &TextStyle::body(), 400.0).0.ceil();
        ui.c.text(label, Rect::new(r.x, r.y, lw.min(r.w) + 2.0, r.h), &TextStyle::body());
        if lw + 12.0 < r.w {
            ui.c.text(prov, Rect::new(r.x + lw + 10.0, r.y, r.w - lw - 10.0, r.h), &TextStyle::footnote().color(palette::TEXT_DIM));
        }
        if b.clicked {
            clicked = Some(i);
        }
    }
    if let Some(p) = clicked.and_then(|i| found(st, i)) {
        choose(st, p);
    }
}

/// A place picked by hand: it stays until changed (no following the location).
fn choose(st: &mut AppState, place: Place) {
    st.settings.prayer.follow_location = false;
    set_place(st, place);
    close(st);
}

fn close(st: &mut AppState) {
    st.prayer.picking = false;
    st.prayer.query.clear();
    st.prayer.found = None;
    // The picker's city files go; a location's squares stay until it's placed.
    st.prayer.cities.retain(|f, _| f.starts_with("near/"));
    st.cmd(Command::BlurText);
}

/// `place` becomes the place the times are for, as it is, and is saved by
/// its id (a city abroad also as its own few lines, so it comes back
/// without going online).
pub fn set_place(st: &mut AppState, place: Place) {
    if st.prayer.place.as_ref().map(|p| &p.id) != Some(&place.id) {
        crate::pinfo!("prayer: times for {}", place.id);
        // A new place: prayer times that passed there don't ring now.
        st.prayer.handled_until = Some(st.prayer_now());
    }
    st.settings.prayer.place = Some(place.id.clone());
    st.settings.prayer.world_place = place.world_lines.clone();
    st.prayer.place = Some(place);
    st.prayer.suggested = false;
    st.cmd(Command::SaveSettings);
}

/// Where Windows or macOS says the PC is, while following the location:
/// the nearest of Kemenag's places when one is close, else the nearest of
/// those and the world's cities in the squares around, once they're here
/// ([`cities_came`]). The point stays in memory only.
pub fn located(st: &mut AppState, lat: f64, lng: f64) {
    let Some(list) = st.prayer.places.clone() else { return };
    let Some((home, km)) = list.nearest(lat, lng).map(|(p, km)| (p.clone(), km)) else { return };
    if km <= prayer::KEMENAG_NEAR_KM {
        settle(st, Some((home, km)));
        return;
    }
    // The same spot as the last time it went online: the same place.
    if let Some((la, lo, found)) = st.prayer.last_fix.clone() {
        if prayer::distance_km(la, lo, lat, lng) < 1.0 {
            settle(st, found);
            return;
        }
    }
    let files = world::files_near(lat, lng, km.min(prayer::NEAREST_MAX_KM));
    for f in &files {
        if !st.prayer.cities.contains_key(f) {
            st.prayer.cities.insert(f.clone(), CityFile::Asking);
            st.cmd(Command::GetCities(f.clone()));
        }
    }
    st.prayer.fix = Some((lat, lng, files));
    place_fix(st);
}

/// One of the online city files came (or couldn't): the picker searches
/// again with it, and a location waiting for its squares may be placed.
/// A file nobody wants any more (the picker closed) is dropped.
pub fn cities_came(st: &mut AppState, file: String, got: Result<World, String>) {
    let Some(slot) = st.prayer.cities.get_mut(&file) else { return };
    *slot = match got {
        Ok(w) => CityFile::Here(Arc::new(w)),
        Err(_) => CityFile::Failed,
    };
    st.prayer.found = None;
    place_fix(st);
}

/// The waiting location's place, once all its squares are here; offline,
/// the place stays as it was (the next location asks again).
fn place_fix(st: &mut AppState) {
    let Some((lat, lng, files)) = st.prayer.fix.clone() else { return };
    let got: Vec<Option<CityFile>> = files.iter().map(|f| st.prayer.cities.get(f).cloned()).collect();
    if got.iter().any(|c| matches!(c, Some(CityFile::Asking))) {
        return;
    }
    st.prayer.fix = None;
    for f in &files {
        st.prayer.cities.remove(f);
    }
    if got.iter().any(|c| !matches!(c, Some(CityFile::Here(_)))) {
        st.prayer.locate = Locate::Offline;
        return;
    }
    let Some(list) = st.prayer.places.clone() else { return };
    let squares: Vec<&World> = got.iter().filter_map(|c| if let Some(CityFile::Here(w)) = c { Some(&**w) } else { None }).collect();
    let found = prayer::nearest(&list, &squares, lat, lng);
    st.prayer.last_fix = Some((lat, lng, found.clone()));
    settle(st, found);
}

/// The nearest place becomes the place while following the location, unless
/// it's too far away to use.
fn settle(st: &mut AppState, nearest: Option<(Place, f64)>) {
    let Some((place, km)) = nearest else { return };
    st.prayer.locate = if km > prayer::NEAREST_MAX_KM {
        crate::pinfo!("prayer: the nearest listed place is {km:.0} km away");
        Locate::Far { km }
    } else {
        if st.settings.prayer.follow_location && st.settings.prayer.place.as_deref() != Some(place.id.as_str()) {
            set_place(st, place);
        }
        Locate::Found { km }
    };
}

/// The prayers' names as the island says them (`It's time for Ashar`).
pub fn moment_title(p: Prayer) -> String {
    format!("It's time for {}", p.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_down_in_hours_minutes_and_seconds() {
        assert_eq!(countdown(8 * 60), "00:08:00");
        assert_eq!(countdown(2 * 3600 + 37 * 60 + 27), "02:37:27");
        assert_eq!(countdown(59), "00:00:59");
        assert_eq!(countdown(-3), "00:00:00");
    }

    #[test]
    fn the_day_runs_along_the_arc_through_each_prayers_dot() {
        // Jakarta-like times, minutes after the place's midnight.
        let mid = 1_000_000;
        let at = [251, 700, 891, 1068, 1138].map(|m| mid + m * 60);
        let pos = |min: i64| day_pos(mid + min * 60, mid, at);
        assert_eq!(pos(0), 0.0, "midnight: the left end");
        assert_eq!(pos(251), SLOTS[0], "Subuh on its dot");
        assert_eq!(pos(891), SLOTS[2], "Ashar on its dot");
        assert_eq!(pos(1138), SLOTS[4], "Isya on its dot");
        assert_eq!(pos(24 * 60), 1.0, "the next midnight: the right end");
        // Halfway between Dzuhur and Ashar is halfway between their dots.
        assert!((pos((700 + 891) / 2) - 0.4).abs() < 0.002);
        // It only ever moves on through the day.
        let mut last = -1.0;
        for m in 0..24 * 60 {
            let p = pos(m);
            assert!(p >= last && (0.0..=1.0).contains(&p), "{m} min: {p}");
            last = p;
        }
    }

    #[test]
    fn a_narrow_island_keeps_room_for_every_prayer_on_the_arc() {
        // The default island (620 wide, 584 inside): the full column.
        assert_eq!(left_width(584.0), LEFT_W);
        // The narrowest (480): the column gives way, down to its floor.
        let w = 444.0;
        let left = left_width(w);
        assert!(left < LEFT_W && left >= LEFT_MIN);
        assert!((w - left - COL_GAP) / 5.0 >= SLOT_MIN - 2.0, "{left}");
    }

    #[test]
    fn names_compass_points() {
        assert_eq!(compass(295.1), "WNW");
        assert_eq!(compass(0.0), "N");
        assert_eq!(compass(359.0), "N");
        assert_eq!(compass(90.0), "E");
    }
}
