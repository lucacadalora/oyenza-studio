//! Opt-in usage counts (docs/usage-counts.md): which tabs and features this
//! copy used this week, off until the person says yes. This is the whole of
//! what is kept and sent: no ID, account, name, time of day, song, file, link
//! or typed text fits in it. A week is Monday to Sunday, UTC.

use crate::datetime::civil_from_days;

/// Where a finished week's report goes. `server/counts.mjs` adds it to that
/// week's totals and throws it away.
pub const SEND_URL: &str = "https://drop.oyenza.com/api/counts";
/// The island asks this many days after this copy first ran, not sooner.
pub const ASK_AFTER_DAYS: u64 = 3;
/// The features a report can name. `server/counts.mjs` lists the same ones.
pub const FEATURES: [&str; 9] =
    ["media_controls", "phone_drop", "paired_phone", "ielts", "notes", "downloader", "record", "pin", "advanced_settings"];

const SCHEMA: u32 = 1;
const DAY_S: u64 = 86_400;

/// The week so far, in `counts.txt` in the data folder. `days` and each tab's
/// number hold one bit for each day of the week it was used on; `features`
/// one bit for each of [`FEATURES`]. Read and written by hand, here and in
/// [`Counts::report`]: serde's code for it cost more than the rest of this
/// feature, and poni.exe has a budget.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub week: u64,
    pub days: u8,
    tabs: Vec<(String, u8)>,
    features: u16,
}

fn week_of(unix: u64) -> u64 {
    (unix / DAY_S + 3) / 7
}

/// 0 is Monday.
fn weekday(unix: u64) -> u32 {
    ((unix / DAY_S + 3) % 7) as u32
}

impl Counts {
    /// Move on to the week of `now`. The finished week comes back if anything
    /// was counted in it. True in `.1` when `self` changed and needs saving.
    pub fn roll(&mut self, now: u64) -> (Option<Counts>, bool) {
        let week = week_of(now);
        if self.week == week {
            return (None, false);
        }
        let done = std::mem::take(self);
        self.week = week;
        (Some(done).filter(|d| d.days != 0), true)
    }

    /// `tab` or `feature` was used at `now`. True when something changed.
    pub fn mark(&mut self, now: u64, tab: Option<&str>, feature: Option<&str>) -> bool {
        let bit = 1u8 << weekday(now);
        let before = (self.days, self.features);
        self.days |= bit;
        let mut changed = false;
        if let Some(tab) = tab {
            match self.tabs.iter_mut().find(|t| t.0 == tab) {
                Some(t) => {
                    changed = t.1 & bit == 0;
                    t.1 |= bit;
                }
                None => {
                    self.tabs.push((tab.to_string(), bit));
                    changed = true;
                }
            }
        }
        if let Some(i) = feature.and_then(|f| FEATURES.iter().position(|n| *n == f)) {
            self.features |= 1 << i;
        }
        changed || before != (self.days, self.features)
    }

    /// As kept in `counts.txt`: "week days features", then a line per tab.
    pub fn to_text(&self) -> String {
        let mut s = format!("{} {} {}\n", self.week, self.days, self.features);
        for (tab, days) in &self.tabs {
            s.push_str(&format!("{tab} {days}\n"));
        }
        s
    }

    /// Back from [`Counts::to_text`]. A damaged file starts the week again.
    pub fn from_text(text: &str) -> Counts {
        let mut lines = text.lines();
        let mut head = lines.next().unwrap_or("").split(' ').map(|n| n.parse::<u64>().ok());
        let (Some(Some(week)), Some(Some(days)), Some(Some(features))) = (head.next(), head.next(), head.next()) else {
            return Counts::default();
        };
        let tabs = lines
            .filter_map(|l| {
                let (tab, days) = l.split_once(' ')?;
                Some((tab.to_string(), days.parse().ok()?))
            })
            .collect();
        Counts { week, days: days as u8, tabs, features: features as u16 }
    }

    /// The report for these counts, as it is sent. Every word in it is the
    /// app's own (versions, page keys, add-on ids), so nothing needs escaping.
    pub fn report(&self, version: &str, os: &str, installed: Option<&str>, addons: &[&str]) -> String {
        fn list(names: impl Iterator<Item = String>) -> String {
            names.collect::<Vec<_>>().join(", ")
        }
        let installed = installed.map_or("null".to_string(), |m| format!("\"{m}\""));
        let tabs = list(self.tabs.iter().map(|(k, v)| format!("\"{k}\": {}", v.count_ones())));
        let addons = list(addons.iter().map(|a| format!("\"{a}\"")));
        let features = list(FEATURES.iter().enumerate().filter(|(i, _)| self.features & (1 << i) != 0).map(|(_, f)| format!("\"{f}\"")));
        format!(
            "{{\n  \"schema\": {SCHEMA},\n  \"version\": \"{version}\",\n  \"os\": \"{os}\",\n  \"installed\": {installed},\n  \"days_used\": {},\n  \"tabs\": {{ {tabs} }},\n  \"addons\": [{addons}],\n  \"features\": [{features}]\n}}",
            self.days.count_ones()
        )
    }
}

/// "2026-11", the month of a Unix time.
pub fn month_of(unix: u64) -> String {
    let (y, m, _) = civil_from_days((unix / DAY_S) as i64);
    format!("{y}-{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONDAY: u64 = 1794182400; // Mon 9 Nov 2026 00:00 UTC

    #[test]
    fn a_day_counts_once() {
        let mut c = Counts::default();
        assert!(c.mark(MONDAY, Some("media"), None));
        assert!(!c.mark(MONDAY + 60, Some("media"), None));
        assert!(c.mark(MONDAY + 60, Some("shelf"), None));
        assert!(c.mark(MONDAY + DAY_S, Some("media"), None));
        let r: serde_json::Value = serde_json::from_str(&c.report("1.2.3", "windows", None, &[])).unwrap();
        assert_eq!(r["days_used"], 2);
        assert_eq!(r["tabs"]["media"], 2);
        assert_eq!(r["tabs"]["shelf"], 1);
    }

    #[test]
    fn only_listed_features_count() {
        let mut c = Counts::default();
        assert!(c.mark(MONDAY, None, Some("pin")));
        assert!(!c.mark(MONDAY, None, Some("something else")));
        assert_eq!(c.features.count_ones(), 1);
    }

    #[test]
    fn a_new_week_hands_back_the_last() {
        let mut c = Counts::default();
        assert_eq!(c.roll(MONDAY), (None, true), "nothing counted yet");
        assert_eq!(c.roll(MONDAY + 6 * DAY_S), (None, false), "Sunday is the same week");
        c.mark(MONDAY, Some("media"), None);
        let (done, changed) = c.roll(MONDAY + 7 * DAY_S);
        assert!(changed);
        assert_eq!(done.unwrap().days, 1);
        assert!(c.tabs.is_empty());
        assert_eq!(c.roll(MONDAY + 14 * DAY_S).0, None, "an unused week sends nothing");
    }

    #[test]
    fn the_report_holds_exactly_these_fields() {
        let mut c = Counts::default();
        c.mark(MONDAY, Some("media"), Some("pin"));
        let r: serde_json::Value = serde_json::from_str(&c.report("1.2.3", "mac", Some("2026-11"), &["sholat"])).unwrap();
        let mut keys: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["addons", "days_used", "features", "installed", "os", "schema", "tabs", "version"]);
    }

    #[test]
    fn the_report_reads_as_json_with_nothing_counted_too() {
        let r: serde_json::Value = serde_json::from_str(&Counts::default().report("1.2.3", "mac", None, &[])).unwrap();
        assert_eq!(r["days_used"], 0);
        assert!(r["installed"].is_null());
        assert_eq!(r["tabs"], serde_json::json!({}));
    }

    #[test]
    fn the_file_keeps_everything_and_a_damaged_one_starts_again() {
        let mut c = Counts::default();
        c.roll(MONDAY);
        c.mark(MONDAY, Some("media"), Some("pin"));
        c.mark(MONDAY + DAY_S, Some("shelf"), None);
        assert_eq!(Counts::from_text(&c.to_text()), c);
        assert_eq!(Counts::from_text("nonsense"), Counts::default());
        assert_eq!(Counts::from_text(""), Counts::default());
    }

    #[test]
    fn months_read_as_a_person_would() {
        assert_eq!(month_of(MONDAY), "2026-11");
        assert_eq!(month_of(0), "1970-01");
    }

    #[test]
    fn the_server_knows_every_name() {
        let server = include_str!("../../../server/counts.mjs");
        for name in FEATURES.iter().copied().chain(crate::addons::ALL.iter().map(|a| a.id)) {
            assert!(server.contains(&format!("'{name}'")), "server/counts.mjs doesn't list {name}");
        }
    }
}
