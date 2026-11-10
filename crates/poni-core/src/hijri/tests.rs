use super::*;
use crate::datetime::days_from_civil;

/// Kemenag's month starts: the isbat's (and Kemenag's calendar's) for the
/// four big months, the others from Bimas Islam's 1447-1448 calendar as
/// detik.com quotes it, and its forecast for Ramadan 1448.
const KEMENAG_PUBLISHED: &[(i32, u8, (i32, u8, u8))] = &[
    (1445, 9, (2024, 3, 12)),
    (1445, 10, (2024, 4, 10)),
    (1445, 12, (2024, 6, 8)),
    (1446, 9, (2025, 3, 1)),
    (1446, 10, (2025, 3, 31)),
    (1446, 12, (2025, 5, 28)),
    (1447, 1, (2025, 6, 27)),
    (1447, 7, (2025, 12, 21)),
    (1447, 8, (2026, 1, 20)),
    (1447, 9, (2026, 2, 19)),
    (1447, 10, (2026, 3, 21)),
    (1447, 11, (2026, 4, 19)),
    (1447, 12, (2026, 5, 18)),
    (1448, 1, (2026, 6, 16)),
    (1448, 2, (2026, 7, 16)),
    (1448, 3, (2026, 8, 14)),
    (1448, 4, (2026, 9, 13)),
    (1448, 5, (2026, 10, 12)),
    (1448, 6, (2026, 11, 11)),
    (1448, 9, (2027, 2, 8)),
];

fn lunation_of(year: i32, month: u8) -> i64 {
    (year as i64 - 1) * 12 + (month as i64 - 1) - month_index(0)
}

/// The months the reckoning alone gets wrong against a table.
fn reckoning_misses(cal: Calendar, table: &[(i32, u8, (i32, u8, u8))]) -> Vec<String> {
    let mut wrong = Vec::new();
    for &(y, m, (gy, gm, gd)) in table {
        let want = days_from_civil(gy, gm, gd);
        let got = reckon(cal, lunation_of(y, m));
        if got != want {
            wrong.push(format!("1 {} {y}: {:?}, published {gy}-{gm:02}-{gd:02}", MONTHS[m as usize - 1], crate::datetime::civil_from_days(got)));
        }
        // Whatever the reckoning says, the date shown is the published one.
        assert_eq!(date(cal, want), HijriDate { year: y, month: m, day: 1 }, "{cal:?} {y}/{m}");
        if want > days_from_civil(2024, 1, 1) {
            assert_eq!(date(cal, want - 1).month, if m == 1 { 12 } else { m - 1 });
        }
    }
    wrong
}

#[test]
fn khgt_reckoning_matches_57_of_muhammadiyahs_60_published_months() {
    let wrong = reckoning_misses(Calendar::Khgt, KHGT_PUBLISHED);
    assert_eq!(
        wrong,
        [
            "1 Rajab 1450: (2028, 11, 18), published 2028-11-17",
            "1 Muharram 1451: (2029, 5, 14), published 2029-05-15",
            "1 Syakban 1451: (2029, 12, 7), published 2029-12-06",
        ]
    );
}

#[test]
fn kemenag_reckoning_matches_every_government_month() {
    let wrong = reckoning_misses(Calendar::Kemenag, KEMENAG_PUBLISHED);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn dates_count_through_the_month() {
    let d = |y, m, dd| days_from_civil(y, m, dd);
    // 10 Nov 2026: KHGT 1 Jumadil Akhir 1448, Kemenag still 30 Jumadil Awal.
    assert_eq!(date(Calendar::Khgt, d(2026, 11, 10)).label(), "1 Jumadil Akhir 1448 H");
    assert_eq!(date(Calendar::Kemenag, d(2026, 11, 10)).label(), "30 Jumadil Awal 1448 H");
    // 18 Feb 2026: KHGT's first day of fasting, Kemenag's last of Syakban.
    assert!(is_ramadan(Calendar::Khgt, d(2026, 2, 18)) && !is_ramadan(Calendar::Kemenag, d(2026, 2, 18)));
    assert_eq!(date(Calendar::Khgt, d(2026, 3, 19)), HijriDate { year: 1447, month: 9, day: 30 });
    assert_eq!(date(Calendar::Kemenag, d(2026, 3, 20)), HijriDate { year: 1447, month: 9, day: 30 });
}

#[test]
fn a_month_is_29_or_30_days() {
    for k in 280..400 {
        for cal in [Calendar::Kemenag, Calendar::Khgt] {
            let len = month_start(cal, k + 1) - month_start(cal, k);
            assert!((29..=30).contains(&len), "{cal:?} lunation {k}: {len} days");
        }
    }
}

