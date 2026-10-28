//! Steps from the person's phone, as Oyenza sends them.
//!
//! Poni has no health data of its own: Oyenza on the person's iPhone or
//! Android phone reads daily totals from Apple Health or Health Connect and,
//! once the person turns on Show on Oyenza Studio there, sends the last 7
//! days to their account, where a signed-in Oyenza Studio reads them
//! (`/island/health`). Only daily totals travel, never samples, places,
//! sleep, heart rate or weight, and the PC keeps them in memory only.

use crate::datetime::LocalDateTime;
use serde::{Deserialize, Serialize};

/// Where the phone read the numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthSource {
    AppleHealth,
    HealthConnect,
    /// A health app this version doesn't know (a newer Oyenza): the numbers still show.
    #[serde(other)]
    Other,
}

impl HealthSource {
    pub fn name(self) -> &'static str {
        match self {
            HealthSource::AppleHealth => "Apple Health",
            HealthSource::HealthConnect => "Health Connect",
            HealthSource::Other => "your phone",
        }
    }
}

/// Numbers the phone read longer ago than this are old, and the page asks the
/// person to open Oyenza on the phone. Oyenza also sends with the app closed
/// (on an iPhone when Apple Health wakes it for new steps, about once an hour
/// at most; on Android every 15 minutes or so), but not always: an iPhone that
/// was locked, an Oyenza that iOS closed, an older Oyenza. Two hours is past
/// a missed wake or two, so a reading that has fallen behind says so while
/// the day is still going, not only the next day.
pub const STALE_AFTER_S: i64 = 2 * 3600;

/// The phone read the numbers more than two hours before `now` (unix seconds).
pub fn is_stale(updated_at: i64, now: i64) -> bool {
    now.saturating_sub(updated_at) > STALE_AFTER_S
}

/// One day's totals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthDay {
    /// Local day `YYYY-MM-DD` on the phone.
    pub date: String,
    pub steps: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_kcal: Option<u32>,
}

/// What Oyenza last sent: the step goal and up to the last week of days.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    /// Daily step goal, as set in Oyenza.
    pub goal: u32,
    pub source: HealthSource,
    /// When the phone last read the numbers (unix seconds).
    pub updated_at: i64,
    pub days: Vec<HealthDay>,
}

impl HealthSnapshot {
    pub fn day(&self, key: &str) -> Option<&HealthDay> {
        self.days.iter().find(|d| d.date == key)
    }

    /// Steps on `key`, 0 when the phone sent nothing for that day.
    pub fn steps_on(&self, key: &str) -> u32 {
        self.day(key).map_or(0, |d| d.steps)
    }

    /// The seven days ending today, oldest first: (day, steps) with `None`
    /// for days the phone sent nothing.
    pub fn week(&self, today: &LocalDateTime) -> [(LocalDateTime, Option<u32>); 7] {
        std::array::from_fn(|i| {
            let d = today.add_days(i as i64 - 6);
            let steps = self.day(&d.day_key()).map(|x| x.steps);
            (d, steps)
        })
    }

    pub fn goal(&self) -> u32 {
        self.goal.max(1)
    }
}

/// True when `next` puts today over the goal and `prev` did not: the moment
/// for a one-time "goal reached" notice.
pub fn goal_reached(prev: Option<&HealthSnapshot>, next: &HealthSnapshot, today: &str) -> bool {
    let goal = next.goal();
    let before = prev.map_or(0, |p| p.steps_on(today));
    before < goal && next.steps_on(today) >= goal
}

/// [`goal_reached`] for numbers that arrive while Poni runs: the first ones
/// after start, sign-in or a pause (`prev` None) only set the baseline, as
/// today may have passed the goal hours before. A restart never repeats the notice.
pub fn goal_notice(prev: Option<&HealthSnapshot>, next: &HealthSnapshot, today: &str) -> bool {
    prev.is_some() && goal_reached(prev, next, today)
}

/// "6,420": thousands with commas.
pub fn fmt_steps(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "4.7 km", or "850 m" under a kilometre.
pub fn fmt_distance(m: u32) -> String {
    if m < 1000 {
        format!("{m} m")
    } else {
        let tenths = (m + 50) / 100;
        format!("{}.{} km", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(goal: u32, days: &[(&str, u32)]) -> HealthSnapshot {
        HealthSnapshot {
            goal,
            source: HealthSource::AppleHealth,
            updated_at: 0,
            days: days.iter().map(|(d, s)| HealthDay { date: (*d).into(), steps: *s, distance_m: None, active_kcal: None }).collect(),
        }
    }

    #[test]
    fn reads_the_feed_shape() {
        let j = r#"{"goal":8000,"source":"health_connect","updated_at":1793000000,
                    "days":[{"date":"2026-10-24","steps":6420,"distance_m":4710,"active_kcal":312},
                            {"date":"2026-10-23","steps":9120}]}"#;
        let s: HealthSnapshot = serde_json::from_str(j).unwrap();
        assert_eq!(s.source, HealthSource::HealthConnect);
        assert_eq!(s.steps_on("2026-10-24"), 6420);
        assert_eq!(s.day("2026-10-23").unwrap().distance_m, None);
        assert_eq!(s.steps_on("2026-10-01"), 0);
    }

    #[test]
    fn week_ends_today_oldest_first() {
        let today = LocalDateTime { year: 2026, month: 10, day: 24, hour: 9, minute: 0, second: 0 };
        let w = snap(8000, &[("2026-10-24", 100), ("2026-10-18", 7)]).week(&today);
        assert_eq!(w[0].0.day_key(), "2026-10-18");
        assert_eq!(w[0].1, Some(7));
        assert_eq!(w[3].1, None);
        assert_eq!(w[6].0.day_key(), "2026-10-24");
        assert_eq!(w[6].1, Some(100));
    }

    #[test]
    fn goal_reached_once() {
        let day = "2026-10-24";
        let below = snap(8000, &[(day, 7900)]);
        let above = snap(8000, &[(day, 8010)]);
        assert!(goal_reached(Some(&below), &above, day));
        assert!(goal_reached(None, &above, day));
        assert!(!goal_reached(Some(&above), &above, day));
        assert!(!goal_reached(Some(&below), &below, day));
        // A goal of 0 is treated as 1, so a day with no steps never counts.
        assert!(!goal_reached(None, &snap(0, &[(day, 0)]), day));
    }

    #[test]
    fn the_first_numbers_are_only_a_baseline() {
        let day = "2026-10-24";
        let below = snap(8000, &[(day, 7900)]);
        let above = snap(8000, &[(day, 8010)]);
        // Over the goal already when Poni starts or signs in: no notice.
        assert!(!goal_notice(None, &above, day));
        assert!(goal_notice(Some(&below), &above, day));
        assert!(!goal_notice(Some(&above), &above, day));
        // A new day: yesterday's numbers count as none for today.
        assert!(goal_notice(Some(&snap(8000, &[("2026-10-23", 9000)])), &above, day));
    }

    #[test]
    fn stale_after_two_hours() {
        let now = 1_793_000_000;
        assert!(!is_stale(now - 8 * 60, now));
        assert!(!is_stale(now - 3600, now));
        assert!(is_stale(now - 3 * 3600, now));
        assert!(!is_stale(now - STALE_AFTER_S, now));
        assert!(is_stale(now - STALE_AFTER_S - 1, now));
        assert!(is_stale(now - 3 * 86_400, now));
        // A phone clock a little ahead is not old.
        assert!(!is_stale(now + 120, now));
        assert!(is_stale(i64::MIN, now));
    }

    #[test]
    fn an_unknown_health_app_still_reads() {
        let s: HealthSnapshot = serde_json::from_str(r#"{"goal":8000,"source":"samsung_health","updated_at":1,"days":[{"date":"2026-10-24","steps":12}]}"#).unwrap();
        assert_eq!(s.source, HealthSource::Other);
        assert_eq!(s.source.name(), "your phone");
        assert_eq!(s.steps_on("2026-10-24"), 12);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_steps(0), "0");
        assert_eq!(fmt_steps(999), "999");
        assert_eq!(fmt_steps(6420), "6,420");
        assert_eq!(fmt_steps(1_234_567), "1,234,567");
        assert_eq!(fmt_distance(850), "850 m");
        assert_eq!(fmt_distance(4710), "4.7 km");
        assert_eq!(fmt_distance(4960), "5.0 km");
    }
}
