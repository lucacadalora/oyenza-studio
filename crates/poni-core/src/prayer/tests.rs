//! Checks against Oyen Umroh's engine (the reference this module ports) and
//! against Kemenag's own published schedule.

use super::*;
use std::io::Read;

const PLACES: &str = include_str!("../../../../addons/sholat/places.json");
/// Kemenag's published 2026 schedules (via api.myquran.com), one line per place and day.
const KEMENAG: &[u8] = include_bytes!("../../tests/fixtures/prayer/kemenag-2026.txt.zst");
/// Oyen Umroh's engine on the same places for 2026-2027 and a few later years.
const GOLDEN: &[u8] = include_bytes!("../../tests/fixtures/prayer/engine-golden.txt.zst");

fn unpack(packed: &[u8]) -> String {
    let mut out = String::new();
    ruzstd::decoding::StreamingDecoder::new(packed).unwrap().read_to_string(&mut out).unwrap();
    out
}

fn places() -> PlaceList {
    PlaceList::parse(PLACES).unwrap()
}

/// "HH:MM" -> minutes after the place's midnight.
fn minute_of_day(hhmm: &str) -> i64 {
    let (h, m) = hhmm.split_once(':').unwrap();
    h.parse::<i64>().unwrap() * 60 + m.parse::<i64>().unwrap()
}

/// Minutes after the place's midnight of `day` (negative or past 1440 if the
/// moment falls on another date).
fn local_minutes(site: &Site, day: i64, utc: i64) -> i64 {
    (utc + site.offset_min(utc) as i64 * 60 - day * 86_400).div_euclid(60)
}

fn parse_day(s: &str) -> i64 {
    let mut it = s.split('-').map(|x| x.parse::<i64>().unwrap());
    let (y, m, d) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
    day_number(y as i32, m as u8, d as u8)
}

/// (place, day, expected minutes of the 7 times) for every data line.
fn rows(text: &str) -> Vec<(String, i64, [i64; 7])> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            assert_eq!(f.len(), 9, "{l}");
            let mut t = [0; 7];
            for i in 0..7 {
                t[i] = minute_of_day(f[2 + i]);
            }
            (f[0].to_string(), parse_day(f[1]), t)
        })
        .collect()
}

fn computed(site: &Site, day: i64) -> [i64; 7] {
    let t = day_times(site, day).unwrap();
    Time::ALL.map(|x| local_minutes(site, day, t.get(x)))
}

#[test]
fn matches_oyen_umroh_engine_to_the_minute() {
    let list = places();
    let rows = rows(&unpack(GOLDEN));
    assert!(rows.len() > 18_000, "{}", rows.len());
    let mut wrong = Vec::new();
    for (id, day, want) in &rows {
        let site = list.find(id).unwrap_or_else(|| panic!("{id} missing")).site().unwrap();
        let got = computed(&site, *day);
        if got != *want {
            wrong.push(format!("{id} {} got {got:?} want {want:?}", civil_from_days(*day).0));
        }
    }
    assert!(wrong.is_empty(), "{} of {} days differ, first: {:?}", wrong.len(), rows.len(), &wrong[..wrong.len().min(5)]);
}

#[test]
fn matches_kemenag_published_2026_schedule() {
    let list = places();
    let rows = rows(&unpack(KEMENAG));
    let (mut total, mut exact, mut worst) = (0usize, 0usize, 0i64);
    let mut per_time = [[0usize; 2]; 7];
    // Per place: (times, exact, largest difference).
    let mut per_place: std::collections::BTreeMap<&str, (usize, usize, i64)> = Default::default();
    for (id, day, want) in &rows {
        let site = list.find(id).unwrap_or_else(|| panic!("{id} missing")).site().unwrap();
        let got = computed(&site, *day);
        let place = per_place.entry(id.as_str()).or_default();
        for i in 0..7 {
            let diff = (got[i] - want[i]).abs();
            total += 1;
            per_time[i][0] += 1;
            place.0 += 1;
            if diff == 0 {
                exact += 1;
                per_time[i][1] += 1;
                place.1 += 1;
            }
            worst = worst.max(diff);
            place.2 = place.2.max(diff);
        }
    }
    let rate = exact as f64 / total as f64;
    for (i, t) in Time::ALL.iter().enumerate() {
        eprintln!("{:8} {:6} times, {:5.1}% exact", t.name(), per_time[i][0], 100.0 * per_time[i][1] as f64 / per_time[i][0] as f64);
    }
    eprintln!("all      {total:6} times, {:5.1}% exact, largest difference {worst} min", 100.0 * rate);
    let (low_id, low) = per_place.iter().map(|(id, p)| (*id, p.1 as f64 / p.0 as f64)).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
    eprintln!("{} places; lowest exact rate {:.1}% ({low_id})", per_place.len(), 100.0 * low);
    // Every Indonesian place in the list is replayed.
    assert_eq!(per_place.len(), list.places.iter().filter(|p| p.kind != "abroad").count());
    assert_eq!(total, 227_367);
    assert!(rate >= 0.98, "{:.4}", rate);
    assert!(worst <= 1, "{worst}");
    assert!(low >= 0.95, "{low_id} {low:.3}");
}

#[test]
fn papua_highland_regencies_use_kemenags_wider_twilight_angles() {
    // Kemenag's Subuh and Isya there match 21 and 19 degrees below the
    // horizon, 4-5 minutes wider than the usual 20 and 18.
    let list = places();
    let own: Vec<&Place> = list.places.iter().filter(|p| p.subuh_angle != KEMENAG_SUBUH_ANGLE || p.isya_angle != KEMENAG_ISYA_ANGLE).collect();
    let mut ids: Vec<&str> = own.iter().map(|p| p.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["kab-intan-jaya", "kab-lanny-jaya", "kab-paniai", "kab-pegunungan-arfak", "kab-puncak", "kab-puncak-jaya"]);
    let day = parse_day("2026-06-01");
    for p in own {
        assert_eq!((p.subuh_angle, p.isya_angle), (21.0, 19.0), "{}", p.id);
        let site = p.site().unwrap();
        let usual = Site { subuh_angle: KEMENAG_SUBUH_ANGLE, isya_angle: KEMENAG_ISYA_ANGLE, ..site.clone() };
        let (t, u) = (day_times(&site, day).unwrap(), day_times(&usual, day).unwrap());
        assert!((240..=360).contains(&(u.subuh - t.subuh)), "{} subuh {}", p.id, u.subuh - t.subuh);
        assert!((240..=360).contains(&(t.isya - u.isya)), "{} isya {}", p.id, t.isya - u.isya);
        assert_eq!((t.terbit, t.dzuhur, t.ashar, t.maghrib), (u.terbit, u.dzuhur, u.ashar, u.maghrib), "{}", p.id);
    }
}

#[test]
fn matches_kemenag_jawa_tengah_ramadan_pdf_for_semarang() {
    // Kanwil Kemenag Jawa Tengah, Jadwal Imsakiyah Ramadan 1447 H, Kota Semarang (WIB).
    let pdf = [
        ("2026-02-19", ["04:16", "04:26", "05:40", "11:56", "15:05", "18:04", "19:14"]),
        ("2026-03-01", ["04:18", "04:28", "05:40", "11:54", "14:56", "18:01", "19:10"]),
        ("2026-03-19", ["04:18", "04:28", "05:39", "11:50", "14:59", "17:53", "19:01"]),
    ];
    let site = places().find("semarang").unwrap().site().unwrap();
    for (date, times) in pdf {
        let day = parse_day(date);
        let t = day_times(&site, day).unwrap();
        let got: Vec<String> = Time::ALL.iter().map(|&x| site.hhmm(t.get(x))).collect();
        assert_eq!(got, times, "{date}");
    }
}

#[test]
fn place_list_is_complete_and_consistent() {
    let list = places();
    let mut ids = std::collections::HashSet::new();
    let mut labels = std::collections::HashSet::new();
    for p in &list.places {
        assert!(ids.insert(p.id.as_str()), "duplicate id {}", p.id);
        assert!(labels.insert(p.label.as_str()), "duplicate label {}", p.label);
        assert!(p.site().is_some(), "{}: unknown zone {}", p.id, p.tz);
        match p.kind.as_str() {
            "kota" | "kab" | "pulau" => {
                assert_eq!(p.method, Method::Kemenag, "{}", p.id);
                assert!((-11.5..=6.5).contains(&p.lat) && (94.5..=141.5).contains(&p.lng), "{} outside Indonesia", p.id);
                assert!((-3.5..=-0.5).contains(&p.h0), "{} h0 {}", p.id, p.h0);
                match p.kind.as_str() {
                    "kota" => assert!(p.label.starts_with("Kota "), "{}", p.label),
                    "kab" => assert!(p.label.starts_with("Kab. "), "{}", p.label),
                    _ => assert!(p.label.contains(", Kab. "), "{}", p.label), // "Pulau Laut, Kab. Natuna"
                }
            }
            "abroad" => assert_eq!(p.method, Method::UmmAlQura, "{}", p.id),
            k => panic!("{}: kind {k}", p.id),
        }
    }
    // Every location in Kemenag's list, plus Makkah, Madinah and Jeddah.
    assert_eq!(list.places.len(), 520);
    for id in ["jakarta", "bandung", "kab-bandung", "semarang", "kota-sofifi", "kab-intan-jaya", "pulau-laut-kab-natuna", "makkah", "madinah", "jeddah"] {
        assert!(list.find(id).is_some(), "{id}");
    }
}

#[test]
fn imsak_is_ten_minutes_before_subuh_everywhere() {
    let day = parse_day("2026-10-24");
    for p in &places().places {
        let t = day_times(&p.site().unwrap(), day).unwrap();
        assert_eq!(t.subuh - t.imsak, 600, "{}", p.id);
        assert!(t.subuh < t.terbit && t.terbit < t.dzuhur && t.dzuhur < t.ashar && t.ashar < t.maghrib && t.maghrib < t.isya, "{}", p.id);
        assert!(t.subuh % 60 == 0 && t.isya % 60 == 0, "{}", p.id);
    }
}

fn jakarta() -> Site {
    places().find("jakarta").unwrap().site().unwrap()
}

/// UTC seconds for a wall-clock time at a site.
fn at(site: &Site, date: &str, hhmm: &str) -> i64 {
    parse_day(date) * 86_400 + minute_of_day(hhmm) * 60 - site.offset_min(0) as i64 * 60
}

#[test]
fn next_and_current_prayer_in_the_afternoon() {
    // Kota Jakarta, Saturday 24 Oct 2026: Dzuhur 11:40, Ashar 14:51, Maghrib 17:49.
    let s = jakarta();
    let now = at(&s, "2026-10-24", "14:43");
    let next = next_prayer(&s, now).unwrap();
    assert_eq!((next.prayer, s.hhmm(next.at)), (Prayer::Ashar, "14:51".to_string()));
    assert_eq!(minutes_until(now, next.at), 8);
    let cur = current_prayer(&s, now).unwrap();
    assert_eq!((cur.prayer, s.hhmm(cur.at), s.hhmm(cur.end)), (Prayer::Dzuhur, "11:40".into(), "14:51".into()));
    // At the minute itself the prayer is current, and the next one is Maghrib.
    let ashar = at(&s, "2026-10-24", "14:51");
    assert_eq!(current_prayer(&s, ashar).unwrap().prayer, Prayer::Ashar);
    assert_eq!(next_prayer(&s, ashar).unwrap().prayer, Prayer::Maghrib);
    assert_eq!(minutes_until(ashar - 1, ashar), 1);
}

#[test]
fn isya_lasts_until_the_next_subuh_across_midnight() {
    let s = jakarta();
    let late = at(&s, "2026-10-24", "23:30");
    let next = next_prayer(&s, late).unwrap();
    assert_eq!(next.prayer, Prayer::Subuh);
    assert_eq!(next.day, parse_day("2026-10-25"));
    let cur = current_prayer(&s, late).unwrap();
    assert_eq!((cur.prayer, cur.day, cur.end), (Prayer::Isya, parse_day("2026-10-24"), next.at));
    // Before dawn, yesterday's Isya is still the current prayer.
    let dawn = at(&s, "2026-10-25", "03:00");
    let cur = current_prayer(&s, dawn).unwrap();
    assert_eq!((cur.prayer, cur.day), (Prayer::Isya, parse_day("2026-10-24")));
    assert_eq!(next_prayer(&s, dawn).unwrap().prayer, Prayer::Subuh);
}

#[test]
fn prayers_between_lists_each_once_in_order() {
    let s = jakarta();
    let from = at(&s, "2026-10-24", "00:00");
    let day = prayers_between(&s, from, from + 86_400);
    assert_eq!(day.iter().map(|m| m.prayer).collect::<Vec<_>>(), Prayer::ALL.to_vec());
    assert!(day.windows(2).all(|w| w[0].at < w[1].at));
    // (from, to]: a prayer exactly at `from` is left out, one exactly at `to` is in.
    let ashar = at(&s, "2026-10-24", "14:51");
    assert!(prayers_between(&s, ashar, ashar + 60).is_empty());
    assert_eq!(prayers_between(&s, ashar - 60, ashar).len(), 1);
    assert!(prayers_between(&s, ashar, ashar).is_empty());
}

#[test]
fn umm_al_qura_isya_moves_to_two_hours_in_ramadan() {
    let makkah = places().find("makkah").unwrap().site().unwrap();
    let gap = |date: &str| {
        let t = day_times(&makkah, parse_day(date)).unwrap();
        (t.isya - t.maghrib) / 60
    };
    assert_eq!(gap("2026-02-17"), 90);
    assert_eq!(gap("2026-02-18"), 120); // 1 Ramadan 1447
    assert_eq!(gap("2026-03-19"), 120); // 30 Ramadan
    assert_eq!(gap("2026-03-20"), 90); // 1 Syawal
    assert!(is_ramadan_umm_al_qura(parse_day("2027-02-08")) && !is_ramadan_umm_al_qura(parse_day("2027-02-07")));
}

#[test]
fn clock_helpers_use_the_place_zone() {
    let s = jakarta();
    let t = at(&s, "2026-10-24", "00:05");
    assert_eq!(s.hhmm(t), "00:05");
    assert_eq!(s.local_day(t), parse_day("2026-10-24"));
    assert_eq!(s.local_day(t - 600), parse_day("2026-10-23"));
    assert_eq!(utc_offset_min("Asia/Jayapura"), Some(540));
    assert_eq!(utc_offset_min("Europe/London"), None);
    assert_eq!(minutes_until(0, 0), 0);
    assert_eq!(minutes_until(0, 61), 2);
}

#[test]
fn nearest_place_to_a_point() {
    let list = places();
    // Gedung Sate, in the city of Bandung.
    let (p, km) = list.nearest(-6.9025, 107.6188).unwrap();
    assert_eq!(p.id, "bandung");
    assert!(km < 5.0, "{km}");
    // Masjidil Haram.
    let (p, km) = list.nearest(21.4225, 39.8262).unwrap();
    assert_eq!((p.id.as_str(), km < 1.0), ("makkah", true));
    // Mid-Pacific: nothing within reach.
    assert!(list.nearest(0.0, -160.0).unwrap().1 > NEAREST_MAX_KM);
    assert!((distance_km(-6.2, 106.8, -6.9, 107.6) - 117.0).abs() < 3.0);
}

#[test]
fn search_puts_names_that_start_with_the_text_first() {
    let list = places();
    let labels = |q: &str| list.search(q, 8).into_iter().map(|p| p.label.as_str()).collect::<Vec<_>>();
    assert_eq!(labels("band"), ["Kota Banda Aceh", "Kota Bandar Lampung", "Kab. Bandung", "Kota Bandung", "Kab. Bandung Barat"]);
    assert_eq!(labels("BANDUNG barat"), ["Kab. Bandung Barat"]);
    assert_eq!(labels("kab. bandung")[0], "Kab. Bandung");
    assert_eq!(labels("makkah"), ["Makkah"]);
    assert!(labels("  ").is_empty());
    assert_eq!(list.search("a", 6).len(), 6);
}

#[test]
fn time_zones_suggest_a_listed_place() {
    let list = places();
    for off in [420, 480, 540, 180] {
        let id = suggested_place_id(off).unwrap();
        assert!(list.find(id).is_some(), "{id}");
    }
    assert_eq!(suggested_place_id(60), None);
}

#[test]
fn countdown_runs_in_the_minutes_before_a_prayer() {
    let s = jakarta();
    let (m, left) = countdown(&s, at(&s, "2026-10-24", "14:43"), 10).unwrap();
    assert_eq!((m.prayer, left), (Prayer::Ashar, 8));
    assert_eq!(countdown(&s, at(&s, "2026-10-24", "14:43") - 30, 10).unwrap().1, 9);
    assert!(countdown(&s, at(&s, "2026-10-24", "14:40"), 10).is_none());
    assert!(countdown(&s, at(&s, "2026-10-24", "14:43"), 0).is_none());
    // At the minute itself the countdown has moved on to Maghrib (not yet in reach).
    assert!(countdown(&s, at(&s, "2026-10-24", "14:51"), 10).is_none());
}

#[test]
fn a_passed_time_rings_only_when_on_time() {
    assert_eq!(due(1000, 1000), Due::Ring);
    assert_eq!(due(1000, 1120), Due::Ring);
    assert_eq!(due(1000, 1121), Due::Quiet);
    assert_eq!(due(1000, 1000 + 1800), Due::Quiet);
    assert_eq!(due(1000, 1000 + 1801), Due::Skip);
    assert_eq!(due(1000, 999), Due::Skip);
}

// ---------------------------------------------------------------- the world list

/// adhan-js 4.4.6 itself on 47 places of the world list, every 9 days of
/// 2026 and 2027 plus the solstices, equinoxes and 2026's summer-time
/// changes: each country's method, Hanafi Ashar where the list says so,
/// `HighLatitudeRule.recommended`, `PolarCircleResolution.AqrabBalad`, and
/// Isya +30 minutes in Ramadan for Umm al-Qura. UTC minutes since 1970.
const WORLD_GOLDEN: &[u8] = include_bytes!("../../tests/fixtures/prayer/world-golden.txt.zst");

/// The website's folder of the world's cities (tools/make_world_places.py).
fn site_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/places/1")
}

/// One of the website's files as the app reads it: a file the website
/// doesn't have is no places.
fn file(name: &str) -> World {
    match std::fs::read_to_string(site_dir().join(format!("{name}.txt"))) {
        Ok(text) => World::parse(text).unwrap_or_else(|e| panic!("{name}: {e}")),
        Err(_) => World::empty(),
    }
}

/// What the picker has once the file for what was typed came.
fn typed(q: &str) -> World {
    file(&world::file_for(&world::fold(q)).unwrap())
}

/// The squares Use my location reads around a point.
fn squares(lat: f64, lng: f64) -> Vec<World> {
    world::files_near(lat, lng, NEAREST_MAX_KM).iter().map(|f| file(f)).collect()
}

fn near(list: &PlaceList, lat: f64, lng: f64) -> Option<(Place, f64)> {
    let squares = squares(lat, lng);
    nearest(list, &squares.iter().collect::<Vec<_>>(), lat, lng)
}

/// A world place by its id, from the square it sits in.
fn world_place(id: &str) -> Place {
    let (lat, lng) = id.strip_prefix(world::ID_PREFIX).and_then(|p| p.split_once(',')).unwrap();
    file(&world::files_near(lat.parse().unwrap(), lng.parse().unwrap(), 0.0)[0]).find(id).unwrap_or_else(|| panic!("{id}"))
}

#[test]
fn every_website_file_reads_and_holds_its_cities() {
    let (mut ids, mut by_name) = (std::collections::HashSet::new(), 0);
    for dir in [site_dir(), site_dir().join("near")] {
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_none_or(|x| x != "txt") {
                continue;
            }
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let w = World::parse(std::fs::read_to_string(&path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
            for i in 0..w.len() {
                let p = w.place(i).unwrap();
                if dir.ends_with("near") {
                    // Each city in the square of its point, once.
                    assert_eq!(world::files_near(p.lat, p.lng, 0.0)[0], format!("near/{name}"), "{}", p.label);
                    assert!(ids.insert(p.id.clone()), "{} twice", p.id);
                } else {
                    // Each name file holds cities with a word starting with its two letters.
                    let key = world::fold(&p.label);
                    assert!(key.split(' ').any(|w| w.starts_with(&name)), "{name}: {}", p.label);
                    by_name += 1;
                }
            }
        }
    }
    assert!(ids.len() > 33_000, "{}", ids.len());
    assert!(by_name >= ids.len(), "{by_name}");
}

#[test]
fn the_files_leave_indonesia_to_kemenag() {
    let list = places();
    // Indonesia comes from Kemenag's list only: no world place near a kab/kota point.
    let jakarta = squares(-6.1833, 106.8333).iter().filter_map(|w| w.nearest(-6.1833, 106.8333)).map(|n| n.1).fold(f64::MAX, f64::min);
    assert!(jakarta > 50.0, "{jakarta}");
    // Nor on Makkah, Madinah or Jeddah.
    for id in ["makkah", "madinah", "jeddah"] {
        let p = list.find(id).unwrap();
        let km = squares(p.lat, p.lng).iter().filter_map(|w| w.nearest(p.lat, p.lng)).map(|n| n.1).fold(f64::MAX, f64::min);
        assert!(km > 15.0, "{id} {km}");
    }
    let e = world_place("w:55.952,-3.196");
    assert_eq!((e.label.as_str(), e.prov.as_str(), e.method, e.hanafi), ("Edinburgh", "Scotland, United Kingdom", Method::MoonsightingCommittee, false));
    assert!(e.is_world() && e.site().is_some());
}

#[test]
fn what_is_typed_names_one_small_file() {
    let f = |q: &str| world::file_for(&world::fold(q));
    assert_eq!(f("edin").as_deref(), Some("ed"));
    assert_eq!(f("Edinburgh").as_deref(), Some("ed"));
    assert_eq!(f("new yo").as_deref(), Some("ne"));
    assert_eq!(f("A Coruña").as_deref(), Some("co"));
    assert_eq!(f("Düsseldorf").as_deref(), Some("du"));
    assert_eq!(f("e"), None);
    assert_eq!(f(""), None);
    assert_eq!(f("مكة"), None);
    // Every file a name can ask for is small.
    let biggest = std::fs::read_dir(site_dir()).unwrap().flatten().map(|e| e.metadata().unwrap().len()).max().unwrap();
    assert!(biggest < 128 * 1024, "{biggest}");
}

#[test]
fn a_location_reads_the_squares_around_it() {
    // Edinburgh: its own square, then the ones within 250 km.
    let e = world::files_near(55.95, -3.2, NEAREST_MAX_KM);
    assert_eq!(e[0], "near/55_-5");
    assert!(e.contains(&"near/55_0".to_string()) && e.contains(&"near/50_-5".to_string()) && e.len() <= 6, "{e:?}");
    // Mid-square on the equator: that square alone.
    assert_eq!(world::files_near(2.5, 102.5, 100.0), ["near/0_100"]);
    // Across 180 degrees: Fiji's squares on both sides.
    let fiji = world::files_near(-17.8, 179.9, 100.0);
    assert!(fiji.contains(&"near/-20_175".to_string()) && fiji.contains(&"near/-20_-180".to_string()), "{fiji:?}");
    assert_eq!(world::url("near/55_-5"), "https://studio.oyenza.com/places/1/near/55_-5.txt");
}

#[test]
fn a_place_keeps_as_its_own_lines() {
    let list = places();
    for q in ["edin", "new york", "karachi", "sydney", "kuala lumpur", "cairo"] {
        let w = typed(q);
        let p = w.place(w.search(&world::fold(q), 1)[0].order).unwrap();
        let lines = p.world_lines.clone().unwrap();
        assert!(lines.len() < 1024, "{q}: {} bytes", lines.len());
        let back = find(&list, Some(&lines), &p.id).unwrap();
        assert_eq!(back, p, "{q}");
    }
    assert_eq!(find(&list, None, "w:55.952,-3.196"), None);
    assert_eq!(find(&list, Some("P\tnope"), "w:55.952,-3.196"), None);
    assert_eq!(find(&list, None, "bandung").unwrap().label, "Kota Bandung");
}

#[test]
fn world_places_match_adhan_js_to_the_minute() {
    let (mut exact, mut polar) = (0, 0);
    let mut seen: std::collections::HashMap<String, Place> = std::collections::HashMap::new();
    for line in unpack(WORLD_GOLDEN).lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split(' ').collect();
        let place = seen.entry(f[0].to_string()).or_insert_with(|| world_place(f[0])).clone();
        assert_eq!(place.id, f[0]);
        let t = day_times(&place.site().unwrap(), parse_day(f[1])).unwrap();
        let ours = [t.subuh, t.terbit, t.dzuhur, t.ashar, t.maghrib, t.isya].map(|s| s.div_euclid(60));
        let theirs: Vec<i64> = f[2..].iter().map(|x| x.parse().unwrap()).collect();
        assert_eq!(t.imsak, t.subuh - 600);
        if theirs.windows(2).all(|p| p[0] <= p[1]) {
            assert_eq!(ours.to_vec(), theirs, "{} {line}", place.label);
            exact += 1;
        } else {
            // Adhan's polar winter days with Asr out of place: ours keep the order.
            assert!(ours.windows(2).all(|p| p[0] <= p[1]), "{} {line}: {ours:?}", place.label);
            assert!(place.lat.abs() > 66.0, "{}", place.label);
            polar += 1;
        }
    }
    assert!(exact > 4_200 && polar < 60, "{exact} exact, {polar} polar");
}

#[test]
fn world_clocks_follow_summer_time() {
    let utc = |date: &str, hhmm: &str| parse_day(date) * 86_400 + minute_of_day(hhmm) * 60;
    let edinburgh = world_place("w:55.952,-3.196").site().unwrap();
    // British Summer Time: 29 Mar 2026 01:00 UTC to 25 Oct 2026 01:00 UTC.
    assert_eq!(edinburgh.offset_min(utc("2026-03-29", "00:59")), 0);
    assert_eq!(edinburgh.offset_min(utc("2026-03-29", "01:00")), 60);
    assert_eq!(edinburgh.offset_min(utc("2026-10-25", "00:59")), 60);
    assert_eq!(edinburgh.offset_min(utc("2026-10-25", "01:00")), 0);
    assert_eq!(edinburgh.hhmm(utc("2026-10-26", "16:21")), "16:21");
    assert_eq!(edinburgh.hhmm(utc("2026-07-01", "16:21")), "17:21");
    assert_eq!(edinburgh.midnight(parse_day("2026-07-01")), utc("2026-06-30", "23:00"));
    let w = typed("new york");
    let ny = w.place(w.search(&world::fold("New York"), 1)[0].order).unwrap().site().unwrap();
    assert_eq!(ny.offset_min(utc("2026-03-08", "06:59")), -300);
    assert_eq!(ny.offset_min(utc("2026-03-08", "07:00")), -240);
    // Sydney's summer is the other half of the year.
    let w = typed("sydney");
    let sydney = w.place(w.search("sydney", 1)[0].order).unwrap().site().unwrap();
    assert_eq!(sydney.offset_min(utc("2026-01-15", "00:00")), 660);
    assert_eq!(sydney.offset_min(utc("2026-07-15", "00:00")), 600);
    // Edinburgh, 26 Oct 2026 (Moonsighting Committee, past 55° north).
    let t = day_times(&edinburgh, parse_day("2026-10-26")).unwrap();
    let times: Vec<String> = Time::ALL.iter().map(|&x| edinburgh.hhmm(t.get(x))).collect();
    assert_eq!(times, ["05:18", "05:28", "07:07", "12:02", "14:12", "16:49", "18:12"]);
}

#[test]
fn world_search_finds_any_city_and_keeps_kemenag_first_at_home() {
    let list = places();
    let labels = |q: &str| search(&list, Some(&typed(q)), q, 8).into_iter().map(|p| format!("{} | {}", p.label, p.prov)).collect::<Vec<_>>();
    let edin = labels("edin");
    assert_eq!(edin[0], "Edinburgh | Scotland, United Kingdom");
    assert!(edin.contains(&"Edinburg | Texas, United States".to_string()), "{edin:?}");
    let london = labels("london");
    assert_eq!(&london[..2], ["London | England, United Kingdom", "London | Ontario, Canada"]);
    assert_eq!(labels("vie")[0], "Vienna | Austria");
    // A word of the name, not only its start; the whole name first.
    let york = labels("york");
    assert_eq!(york[0], "York | England, United Kingdom");
    assert!(york.contains(&"New York City | New York, United States".to_string()), "{york:?}");
    assert_eq!(labels("mecca")[0], "Makkah | Arab Saudi");
    assert_eq!(labels("Madina")[0], "Madinah | Arab Saudi");
    // Typed with its accent or without.
    assert_eq!(labels("Düsseldorf")[0], labels("dusseldorf")[0]);
    assert!(labels("dusseldorf")[0].starts_with("Düsseldorf | "));
    // Kemenag's kab/kota before smaller cities abroad; big cities first.
    assert_eq!(&labels("bandung")[..3], ["Kab. Bandung | Jawa Barat", "Kota Bandung | Jawa Barat", "Kab. Bandung Barat | Jawa Barat"]);
    assert_eq!(labels("kuala")[0], "Kuala Lumpur | Malaysia");
    // Before the file comes (or offline) the picker still finds Kemenag's places.
    assert_eq!(search(&list, None, "bandung", 8).len(), 3);
    assert!(search(&list, None, "edin", 8).is_empty());
}

#[test]
fn location_picks_the_nearest_place_in_either_list() {
    let list = places();
    // Edinburgh Castle.
    let (p, km) = near(&list, 55.9486, -3.1999).unwrap();
    assert_eq!((p.label.as_str(), km < 2.0), ("Edinburgh", true));
    // Gedung Sate is next to Kota Bandung: no squares needed.
    assert!(list.nearest(-6.9025, 107.6188).unwrap().1 < KEMENAG_NEAR_KM);
    assert_eq!(near(&list, -6.9025, 107.6188).unwrap().0.id, "bandung");
    // Johor Bahru, across the strait from Batam: Malaysia's own times.
    assert!(list.nearest(1.4655, 103.7578).unwrap().1 > KEMENAG_NEAR_KM);
    let (p, _) = near(&list, 1.4655, 103.7578).unwrap();
    assert_eq!((p.method, p.is_world()), (Method::Singapore, true));
    // Singapore too.
    assert!(list.nearest(1.2897, 103.8501).unwrap().1 > KEMENAG_NEAR_KM);
    assert_eq!(near(&list, 1.2897, 103.8501).unwrap().0.method, Method::Singapore);
    // Mid-Atlantic: nothing within 250 km.
    assert!(near(&list, 40.0, -40.0).unwrap().1 > NEAREST_MAX_KM);
}

#[test]
fn hanafi_ashar_comes_later() {
    let w = typed("karachi");
    let karachi = w.place(w.search("karachi", 1)[0].order).unwrap();
    assert!(karachi.hanafi);
    let site = karachi.site().unwrap();
    let shafii = Site { hanafi: false, ..site.clone() };
    let day = parse_day("2026-06-01");
    let (h, s) = (day_times(&site, day).unwrap(), day_times(&shafii, day).unwrap());
    assert!((40 * 60..=90 * 60).contains(&(h.ashar - s.ashar)), "{}", h.ashar - s.ashar);
}

#[test]
fn the_self_tests_sample_is_a_piece_of_the_ed_file() {
    let (sample, w) = (World::parse(world::SAMPLE.to_string()).unwrap(), file("ed"));
    assert_eq!(sample.len(), 4);
    for i in 0..sample.len() {
        let p = sample.place(i).unwrap();
        let real = w.find(&p.id).unwrap();
        assert_eq!((&p.label, &p.prov, p.method), (&real.label, &real.prov, real.method));
        let at = parse_day("2026-07-01") * 86_400;
        assert_eq!(p.site().unwrap().offset_min(at), real.site().unwrap().offset_min(at), "{}", p.label);
    }
}
