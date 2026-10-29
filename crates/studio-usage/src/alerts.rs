//! Heads-up notices from the readings of one service: a limit getting close,
//! running out, or coming back after its reset, and a balance running low.
//! Pure, like the readers: the worker keeps a [`Memory`] per service while
//! the person has notices on, hands it each new reading, and sends what comes
//! back to the island. Oyen never reacts to any of it (no sad Oyen at 100%).
//!
//! Each reset period says each thing once. A reading that dips back under
//! the line (a log line written before the server's rounder figure) changes
//! nothing, because [`Memory`] keeps the most each window has used until its
//! reset passes. Several things at once make one notice per service, the
//! most urgent: out, then a low balance, then near, then back.

use super::{micros, Money, ProviderId, Snapshot, Window};
use poni_core::datetime::{civil_from_days, fmt_clock, fmt_date_short, LocalDateTime, WEEKDAY_SHORT};

/// What the person chose on the AI tab, and the app's clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlertCfg {
    /// Say so once a window passes this much used (0.0..=1.0).
    pub near: f32,
    /// Say so once a balance falls under this many US dollars (0: never).
    pub floor_usd: f64,
    /// Local time minus UTC, in minutes, for the reset time
    /// (`oyenza::offset_minutes(&st.local, st.unix)`).
    pub offset_min: i32,
    /// "14:20" or "2:20 PM", as the app's clock (`settings.use_24h`).
    pub use_24h: bool,
    /// Oyenza Inference is connected: a plan that runs out says it can carry
    /// on there ([`Alert::nudge`]).
    pub inference: bool,
}

impl Default for AlertCfg {
    fn default() -> Self {
        AlertCfg { near: 0.8, floor_usd: 1.0, offset_min: 0, use_24h: true, inference: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    /// Passed `near` (percent used).
    Near(u8),
    /// All used.
    Out,
    /// Was out or near, and the window reset.
    Back,
    /// A balance fell under the floor.
    Low,
}

impl AlertKind {
    /// Which to show first when several services speak at once.
    pub fn urgency(self) -> u8 {
        match self {
            AlertKind::Out => 3,
            AlertKind::Low => 2,
            AlertKind::Near(_) => 1,
            AlertKind::Back => 0,
        }
    }
}

/// One notice for the island.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub provider: ProviderId,
    pub kind: AlertKind,
    /// "Claude Code: 90% of 5-hour".
    pub title: String,
    /// "Resets 14:20".
    pub subtitle: String,
    /// One more line when a plan ran out and Oyenza Inference is connected
    /// ([`AlertCfg::inference`]).
    pub nudge: Option<String>,
}

/// Placeholder words until the Oyenza lane writes the real ones.
/// The Oyenza Inference card's own words for it.
const NUDGE: &str = super::oyenza::NUDGE;

/// The longest title that keeps "of <window>" on its line: about what
/// Classic's notice fits at 15 DIP (Pill's is wider), measured on the
/// services' names and labels in the tests.
const TITLE_MAX: usize = 34;

/// What the notices remember of one service between its readings, kept by
/// the worker while notices are on and dropped when they go off (the next
/// reading then starts afresh, saying nothing). Each window keeps the most
/// it has used until its reset passes, and a balance under the floor stays
/// said until a top-up takes it past twice the floor.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    seen: Option<Snapshot>,
}

impl Memory {
    /// The notice `after` deserves (at most one), then remember it.
    pub fn changes(&mut self, after: &Snapshot, now: i64, cfg: &AlertCfg) -> Vec<Alert> {
        let (alert, seen) = step(self.seen.as_ref(), after, now, cfg);
        self.seen = Some(seen);
        alert.into_iter().collect()
    }
}

/// The notices `after` deserves, compared with `before` (None: the first
/// reading this run, which never alerts on its own): at most one, the most
/// urgent. `before` is what a [`Memory`] keeps; given the bare last reading
/// instead, a reading that dips under the line and back says it again.
pub fn changes(before: Option<&Snapshot>, after: &Snapshot, now: i64, cfg: &AlertCfg) -> Vec<Alert> {
    step(before, after, now, cfg).0.into_iter().collect()
}

/// One window's way from the last reading to this one.
struct Turn {
    /// Where it is in the remembered windows.
    i: usize,
    /// Percent used before, in its own period.
    was: u32,
    /// Its reset passed in between.
    reset: bool,
}

/// The notice, and what to remember for the next reading.
fn step(before: Option<&Snapshot>, after: &Snapshot, now: i64, cfg: &AlertCfg) -> (Option<Alert>, Snapshot) {
    let line = pct(cfg.near).clamp(1, 100);
    // Another account starts over, as the first reading does.
    let before = before.filter(|b| b.provider == after.provider && !(b.account.is_some() && after.account.is_some() && b.account != after.account));
    let mut seen = Snapshot { windows: Vec::with_capacity(after.windows.len()), ..after.clone() };
    if seen.account.is_none() {
        seen.account = before.and_then(|b| b.account.clone());
    }
    let mut turns = Vec::new();
    for a in &after.windows {
        let Some(b) = before.and_then(|b| b.windows.iter().find(|w| w.kind == a.kind && w.label == a.label)) else {
            // A window seen for the first time only sets where it stands.
            seen.windows.push(a.at(now));
            continue;
        };
        let mut a = a.clone();
        a.length_s = a.length_s.or(b.length_s);
        // Numbers from before the reset we know of, without one of their
        // own, belong to that period: empty once it has passed.
        if a.resets_at.is_none() && b.resets_at.is_some_and(|t| after.as_of < t) {
            a.resets_at = b.resets_at;
        }
        let mut w = a.at(now);
        let was = pct(b.used);
        let reset = match b.resets_at {
            Some(t) => t <= now,
            // Without a reset time, only a clear drop from the line says so.
            None => was >= line && pct(w.used) < line / 2,
        };
        if !reset {
            // Same period: the most it has used stands (a reading can lag),
            // and so does the reset time first seen (a reading can drift).
            if b.used > w.used {
                w.used = b.used;
                w.count = b.count;
            }
            w.resets_at = b.resets_at.or(w.resets_at);
        }
        turns.push(Turn { i: seen.windows.len(), was, reset });
        seen.windows.push(w);
    }
    if let Some(b) = before {
        // Windows this reading left out keep their place, so one that comes
        // and goes starts nothing over, and one whose reset passes is back.
        for w in &b.windows {
            if !seen.windows.iter().any(|s| s.kind == w.kind && s.label == w.label) {
                turns.push(Turn { i: seen.windows.len(), was: pct(w.used), reset: w.resets_at.is_some_and(|t| t <= now) });
                seen.windows.push(w.at(now));
            }
        }
    }

    let floor = if cfg.floor_usd > 0.0 { micros(cfg.floor_usd) } else { 0 };
    let usd = |m: &Option<Money>| m.as_ref().filter(|m| m.currency == "USD").map(|m| m.micros);
    let low = match (before.and_then(|b| usd(&b.balance)), usd(&after.balance)) {
        (Some(b), Some(a)) if floor > 0 && b >= floor && a < floor => Some(a),
        _ => None,
    };
    match (before.and_then(|b| usd(&b.balance)), usd(&after.balance)) {
        // Said once: a refund or a small top-up doesn't say it again.
        (Some(b), Some(a)) if floor > 0 && b < floor && a >= floor && a < floor.saturating_mul(2) => seen.balance = before.and_then(|b| b.balance.clone()),
        // Unknown is not zero: no balance this time forgets nothing.
        _ if after.balance.is_none() => seen.balance = before.and_then(|b| b.balance.clone()),
        _ => {}
    }

    let out = |w: &Window| pct(w.used) >= 100;
    let blocked_before = before.is_some_and(|b| b.windows.iter().any(|w| out(w) && w.resets_at.is_none_or(|t| t > now)));
    let blocked = seen.windows.iter().any(out);
    let is = |t: &Turn| pct(seen.windows[t.i].used);
    let from = |t: &Turn| if t.reset { 0 } else { t.was };
    let later = |w: &Window| w.resets_at.unwrap_or(i64::MAX);
    let name = after.provider.name();
    let alert = |kind, (title, subtitle): (String, String)| Alert { provider: after.provider, kind, title, subtitle, nudge: None };

    let said = if !blocked_before && turns.iter().any(|t| from(t) < 100 && is(t) >= 100) {
        // Out until the last of them resets: that one names the notice.
        seen.windows.iter().filter(|w| out(w)).max_by_key(|w| later(w)).map(|w| {
            let mut a = alert(AlertKind::Out, limit_words(format!("{name}: {} used up", w.label), format!("{name}: 100% used"), w, when(w, now, cfg, "None left for now")));
            a.nudge = (cfg.inference && after.provider != ProviderId::Oyenza).then(|| NUDGE.to_string());
            a
        })
    } else if let Some(left) = low {
        let left = Money { micros: left.max(0), currency: "USD".into() };
        Some(alert(AlertKind::Low, (format!("{name}: {} left", left.fmt()), "Top up to keep going".into())))
    } else if blocked {
        None
    } else if let Some(t) = turns.iter().filter(|t| from(t) < line && is(t) >= line && is(t) < 100).max_by_key(|t| (is(t), later(&seen.windows[t.i]))) {
        let (w, p) = (&seen.windows[t.i], is(t));
        let left = format!("{}% left", 100 - p);
        Some(alert(AlertKind::Near(p as u8), limit_words(format!("{name}: {p}% of {}", w.label), format!("{name}: {p}% used"), w, when(w, now, cfg, &left))))
    } else {
        turns.iter().filter(|t| t.reset && t.was >= line && is(t) < line).max_by_key(|t| t.was).map(|t| {
            let label = seen.windows[t.i].label.trim();
            alert(AlertKind::Back, (format!("{name} is back"), if label.is_empty() { "Limit reset".into() } else { format!("{label} reset") }))
        })
    };
    (said, seen)
}

/// "Codex: 90% of 5-hour" over "Resets 14:20" while the title fits the
/// notice, else "GitHub Copilot: 90% used" over "Premium requests · resets
/// Tue 1 Dec".
fn limit_words(long: String, short: String, w: &Window, when: String) -> (String, String) {
    let label = w.label.trim();
    if label.is_empty() {
        (short, when)
    } else if long.chars().count() <= TITLE_MAX {
        (long, when)
    } else {
        let mut c = when.chars();
        let when: String = c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default();
        (short, format!("{label} · {when}"))
    }
}

/// "Resets 14:20", or `none` when the service gave no reset time.
fn when(w: &Window, now: i64, cfg: &AlertCfg, none: &str) -> String {
    match w.resets_at {
        Some(t) => format!("Resets {}", reset_time(t, now, cfg.offset_min, cfg.use_24h)),
        None => none.to_string(),
    }
}

/// When a window resets, on the person's clock: "14:20" today, "Thu 09:00"
/// within the week, "Tue 3 Nov" a week or more away; "2:20 PM" and "Thu
/// 9:00 AM" on a 12-hour clock. To the nearest minute, so a reset seconds
/// before the hour reads as the hour. Today's offset stands for the reset's
/// too, so one past a daylight-saving change reads an hour off.
pub fn reset_time(at: i64, now: i64, offset_min: i32, use_24h: bool) -> String {
    let (day, t) = local((at + 30).div_euclid(60) * 60, offset_min);
    let (today, _) = local(now, offset_min);
    match day - today {
        0 => fmt_clock(&t, use_24h),
        1..=6 => format!("{} {}", WEEKDAY_SHORT[t.weekday() as usize], fmt_clock(&t, use_24h)),
        _ => fmt_date_short(&t),
    }
}

/// Unix seconds on the person's wall clock: the day number and the time.
fn local(t: i64, offset_min: i32) -> (i64, LocalDateTime) {
    let s = t + offset_min as i64 * 60;
    let (day, rest) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    let (year, month, d) = civil_from_days(day);
    (day, LocalDateTime { year, month, day: d, hour: (rest / 3600) as u8, minute: (rest / 60 % 60) as u8, second: (rest % 60) as u8 })
}

/// Whole percent used, as the notice says it: a hair under a whole number
/// counts as it (0.8 is 80, whatever the float did). NaN casts to 0.
fn pct(used: f32) -> u32 {
    (used.clamp(0.0, 10.0) * 100.0 + 0.01) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_time, Source, WindowKind};
    use ProviderId::*;

    /// Tue 27 Oct 2026 05:00 UTC: noon in Jakarta.
    const NOW: i64 = 1_793_077_200;
    const WIB: i32 = 420;
    const H: i64 = 3600;
    /// 14:20 in Jakarta.
    const FIVE_RESET: i64 = NOW + 2 * H + 20 * 60;
    /// Thu 29 Oct 09:00 in Jakarta.
    const WEEK_RESET: i64 = NOW + 45 * H;

    fn cfg() -> AlertCfg {
        AlertCfg { offset_min: WIB, ..AlertCfg::default() }
    }

    fn snap(provider: ProviderId, windows: Vec<Window>) -> Snapshot {
        let mut s = Snapshot::new(provider, NOW, Source::Online);
        s.windows = windows;
        s
    }

    fn five(used: f32) -> Window {
        Window::new(WindowKind::Session, "5-hour", used).resets(Some(FIVE_RESET)).length(Some(18_000))
    }

    fn weekly(used: f32) -> Window {
        Window::new(WindowKind::Weekly, "Weekly", used).resets(Some(WEEK_RESET)).length(Some(7 * 24 * 3600))
    }

    fn balance(provider: ProviderId, usd: f64) -> Snapshot {
        let mut s = snap(provider, Vec::new());
        s.balance = Some(Money::usd(usd));
        s
    }

    /// Feeds readings a few minutes apart; what each said.
    fn feed(m: &mut Memory, readings: &[Snapshot], cfg: &AlertCfg) -> Vec<Vec<Alert>> {
        readings.iter().enumerate().map(|(i, s)| m.changes(s, NOW + i as i64 * 180, cfg)).collect()
    }

    fn words(a: &[Alert]) -> Vec<(AlertKind, &str, &str)> {
        a.iter().map(|a| (a.kind, a.title.as_str(), a.subtitle.as_str())).collect()
    }

    #[test]
    fn the_first_reading_only_sets_where_things_stand() {
        let mut s = snap(Claude, vec![five(1.0), weekly(0.9)]);
        s.balance = Some(Money::usd(0.2));
        assert!(changes(None, &s, NOW, &cfg()).is_empty());
        let mut m = Memory::default();
        assert!(m.changes(&s, NOW, &cfg()).is_empty());
        assert!(m.changes(&s, NOW + 180, &cfg()).is_empty());
    }

    #[test]
    fn near_says_so_once_with_the_local_reset_time() {
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Claude, vec![five(0.7)]), snap(Claude, vec![five(0.9)]), snap(Claude, vec![five(0.95)])], &cfg());
        assert!(said[0].is_empty() && said[2].is_empty());
        assert_eq!(said[1], vec![Alert { provider: Claude, kind: AlertKind::Near(90), title: "Claude Code: 90% of 5-hour".into(), subtitle: "Resets 14:20".into(), nudge: None }]);
        // Exactly on the line counts, whatever the float did.
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Codex, vec![five(0.79)]), snap(Codex, vec![five(160.0 / 200.0)])], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Near(80), "Codex: 80% of 5-hour", "Resets 14:20")]);
    }

    #[test]
    fn a_reading_wobbling_around_the_line_says_it_once() {
        // The server's figure, then an older log line under it, and so on.
        let readings: Vec<Snapshot> = [0.78, 0.81, 0.79, 0.80, 0.795, 0.83, 0.799, 0.81, 0.7999].iter().map(|&u| snap(Codex, vec![five(u)])).collect();
        let mut m = Memory::default();
        let said: Vec<Alert> = feed(&mut m, &readings, &cfg()).concat();
        assert_eq!(words(&said), [(AlertKind::Near(81), "Codex: 81% of 5-hour", "Resets 14:20")]);
        // The same wobble at the top: one Out.
        let readings: Vec<Snapshot> = [0.9, 1.0, 0.99, 1.0, 0.98, 1.01].iter().map(|&u| snap(Codex, vec![five(u)])).collect();
        let mut m = Memory::default();
        let said: Vec<Alert> = feed(&mut m, &readings, &cfg()).concat();
        assert_eq!(words(&said), [(AlertKind::Out, "Codex: 5-hour used up", "Resets 14:20")]);
    }

    #[test]
    fn running_out_outranks_near() {
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Codex, vec![five(0.5)]), snap(Codex, vec![five(1.0)])], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Out, "Codex: 5-hour used up", "Resets 14:20")]);
        assert_eq!(said[1][0].nudge, None);
    }

    #[test]
    fn back_once_the_reset_passes_between_readings() {
        let mut m = Memory::default();
        assert!(m.changes(&snap(Codex, vec![five(1.0)]), NOW, &cfg()).is_empty());
        // A minute before the reset: still out.
        assert!(m.changes(&snap(Codex, vec![five(1.0)]), FIVE_RESET - 60, &cfg()).is_empty());
        // The next reading is from the new period.
        let fresh = snap(Codex, vec![Window::new(WindowKind::Session, "5-hour", 0.02).resets(Some(FIVE_RESET + 18_000)).length(Some(18_000))]);
        assert_eq!(words(&m.changes(&fresh, FIVE_RESET + 120, &cfg())), [(AlertKind::Back, "Codex is back", "5-hour reset")]);
        assert!(m.changes(&fresh, FIVE_RESET + 300, &cfg()).is_empty());
    }

    #[test]
    fn back_comes_at_the_reset_even_from_numbers_older_than_it() {
        // Nobody used Codex since: its log's last line, read again after the reset.
        let mut old = snap(Codex, vec![five(1.0)]);
        old.source = Source::Local;
        let mut m = Memory::default();
        m.changes(&old, NOW, &cfg());
        assert_eq!(words(&m.changes(&old, FIVE_RESET + 60, &cfg())), [(AlertKind::Back, "Codex is back", "5-hour reset")]);
        assert!(m.changes(&old, FIVE_RESET + 360, &cfg()).is_empty());
        // The same line without a reset time of its own takes the one known.
        let mut m = Memory::default();
        m.changes(&snap(Codex, vec![five(0.97)]), NOW, &cfg());
        let mut bare = snap(Codex, vec![Window::new(WindowKind::Session, "5-hour", 0.97)]);
        bare.as_of = NOW;
        assert_eq!(words(&m.changes(&bare, FIVE_RESET + 60, &cfg())), [(AlertKind::Back, "Codex is back", "5-hour reset")]);
    }

    #[test]
    fn near_then_the_reset_is_back_too() {
        let mut m = Memory::default();
        m.changes(&snap(Claude, vec![five(0.7)]), NOW, &cfg());
        assert_eq!(m.changes(&snap(Claude, vec![five(0.85)]), NOW + 60, &cfg()).len(), 1);
        let fresh = snap(Claude, vec![Window::new(WindowKind::Session, "5-hour", 0.0).resets(Some(FIVE_RESET + 18_000))]);
        assert_eq!(words(&m.changes(&fresh, FIVE_RESET + 60, &cfg())), [(AlertKind::Back, "Claude Code is back", "5-hour reset")]);
        // Under the line all period long, a reset says nothing.
        let mut m = Memory::default();
        m.changes(&snap(Claude, vec![five(0.5)]), NOW, &cfg());
        assert!(m.changes(&fresh, FIVE_RESET + 60, &cfg()).is_empty());
    }

    #[test]
    fn a_new_period_used_past_the_line_before_the_next_reading_says_so() {
        let mut m = Memory::default();
        m.changes(&snap(Codex, vec![five(1.0)]), NOW, &cfg());
        let busy = snap(Codex, vec![Window::new(WindowKind::Session, "5-hour", 0.86).resets(Some(FIVE_RESET + 18_000))]);
        assert_eq!(words(&m.changes(&busy, FIVE_RESET + 3 * H, &cfg())), [(AlertKind::Near(86), "Codex: 86% of 5-hour", "Resets 19:20")]);
    }

    #[test]
    fn back_waits_while_another_window_is_still_out() {
        let mut m = Memory::default();
        m.changes(&snap(Codex, vec![five(0.9), weekly(0.9)]), NOW, &cfg());
        assert_eq!(words(&m.changes(&snap(Codex, vec![five(1.0), weekly(1.0)]), NOW + 60, &cfg())), [(AlertKind::Out, "Codex: Weekly used up", "Resets Thu 09:00")]);
        // The 5-hour comes back, the week is still out: Codex isn't back.
        let five_fresh = Window::new(WindowKind::Session, "5-hour", 0.0).resets(Some(FIVE_RESET + 18_000)).length(Some(18_000));
        assert!(m.changes(&snap(Codex, vec![five_fresh.clone(), weekly(1.0)]), FIVE_RESET + 60, &cfg()).is_empty());
        // Nor when the week's window is missing from a reading.
        assert!(m.changes(&snap(Codex, vec![five_fresh.clone()]), FIVE_RESET + 120, &cfg()).is_empty());
        // The week resets: now it is.
        let week_fresh = Window::new(WindowKind::Weekly, "Weekly", 0.0).resets(Some(WEEK_RESET + 7 * 24 * H));
        assert_eq!(words(&m.changes(&snap(Codex, vec![five_fresh, week_fresh]), WEEK_RESET + 60, &cfg())), [(AlertKind::Back, "Codex is back", "Weekly reset")]);
    }

    #[test]
    fn two_windows_at_once_make_one_notice() {
        // Both pass the line: the fuller one.
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Codex, vec![five(0.7), weekly(0.75)]), snap(Codex, vec![five(0.85), weekly(0.82)])], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Near(85), "Codex: 85% of 5-hour", "Resets 14:20")]);
        // One runs out as the other passes the line: out.
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Codex, vec![five(0.9), weekly(0.7)]), snap(Codex, vec![five(1.0), weekly(0.85)])], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Out, "Codex: 5-hour used up", "Resets 14:20")]);
        // Out of the week already, the 5-hour running out too says nothing new.
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Codex, vec![five(0.5), weekly(1.0)]), snap(Codex, vec![five(1.0), weekly(1.0)])], &cfg());
        assert!(said.concat().is_empty());
    }

    #[test]
    fn a_twelve_hour_clock() {
        let twelve = AlertCfg { use_24h: false, ..cfg() };
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Claude, vec![five(0.5), weekly(0.5)]), snap(Claude, vec![five(0.9), weekly(0.5)])], &twelve);
        assert_eq!(words(&said[1]), [(AlertKind::Near(90), "Claude Code: 90% of 5-hour", "Resets 2:20 PM")]);
        assert_eq!(reset_time(WEEK_RESET, NOW, WIB, false), "Thu 9:00 AM");
        assert_eq!(reset_time(parse_time("2026-10-27T17:00:00Z").unwrap(), NOW, WIB, false), "Wed 12:00 AM");
        assert_eq!(reset_time(NOW, NOW - 60, WIB, false), "12:00 PM");
    }

    #[test]
    fn the_reset_line_names_the_day_once_the_local_date_changes() {
        // 23:30 in Jakarta, a reset at 01:00: Wednesday there, though still
        // Tuesday in UTC.
        let (now, at) = (parse_time("2026-10-27T16:30:00Z").unwrap(), parse_time("2026-10-27T18:00:00Z").unwrap());
        assert_eq!(reset_time(at, now, WIB, true), "Wed 01:00");
        assert_eq!(reset_time(at, now, 0, true), "18:00");
        // West of UTC: 22:00 on Tuesday in Bogotá, the reset at 01:00.
        let (now, at) = (parse_time("2026-10-28T03:00:00Z").unwrap(), parse_time("2026-10-28T06:00:00Z").unwrap());
        assert_eq!(reset_time(at, now, -300, true), "Wed 01:00");
        // A week or more away: the date, never a weekday that reads as this one.
        assert_eq!(reset_time(NOW + 7 * 24 * H - 3 * H, NOW, WIB, true), "Tue 3 Nov");
        assert_eq!(reset_time(NOW + 6 * 24 * H, NOW, WIB, true), "Mon 12:00");
        // Seconds before the minute read as the minute.
        assert_eq!(reset_time(FIVE_RESET - 1, NOW, WIB, true), "14:20");
        assert_eq!(reset_time(parse_time("2026-10-27T16:59:45Z").unwrap(), NOW, WIB, true), "Wed 00:00");
        // And in a notice.
        let mut m = Memory::default();
        let late = |used| snap(Codex, vec![Window::new(WindowKind::Session, "5-hour", used).resets(Some(at))]);
        m.changes(&late(0.5), now - 60, &AlertCfg { offset_min: -300, ..cfg() });
        assert_eq!(words(&m.changes(&late(0.9), now, &AlertCfg { offset_min: -300, ..cfg() })), [(AlertKind::Near(90), "Codex: 90% of 5-hour", "Resets Wed 01:00")]);
    }

    #[test]
    fn a_balance_under_the_floor_says_so_once() {
        let mut m = Memory::default();
        let said = feed(&mut m, &[balance(Oyenza, 4.2), balance(Oyenza, 1.5), balance(Oyenza, 0.8), balance(Oyenza, 0.6)], &cfg());
        assert_eq!(said.iter().map(Vec::len).collect::<Vec<_>>(), [0, 0, 1, 0]);
        assert_eq!(words(&said[2]), [(AlertKind::Low, "Oyenza Inference: $0.80 left", "Top up to keep going")]);
        assert_eq!(said[2][0].nudge, None);
        // A refund or a small top-up, then under again: nothing new.
        let said = feed(&mut m, &[balance(Oyenza, 1.2), balance(Oyenza, 0.9)], &cfg());
        assert!(said.concat().is_empty());
        // Unknown is not zero: a reading without a balance says nothing and forgets nothing.
        assert!(m.changes(&snap(Oyenza, Vec::new()), NOW, &cfg()).is_empty());
        assert!(m.changes(&balance(Oyenza, 0.7), NOW, &cfg()).is_empty());
        // A real top-up, spent down again: said again.
        let said = feed(&mut m, &[balance(Oyenza, 10.0), balance(Oyenza, 0.5)], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Low, "Oyenza Inference: $0.50 left", "Top up to keep going")]);
        // Overdrawn reads as nothing left.
        let mut m = Memory::default();
        let said = feed(&mut m, &[balance(OpenRouter, 3.0), balance(OpenRouter, -0.25)], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Low, "OpenRouter: $0.00 left", "Top up to keep going")]);
        // No floor, no notice; and euros aren't dollars.
        let mut m = Memory::default();
        assert!(feed(&mut m, &[balance(Oyenza, 4.2), balance(Oyenza, 0.1)], &AlertCfg { floor_usd: 0.0, ..cfg() }).concat().is_empty());
        let eur = |v: f64| {
            let mut s = balance(MiniMax, v);
            s.balance.as_mut().unwrap().currency = "EUR".into();
            s
        };
        let mut m = Memory::default();
        assert!(feed(&mut m, &[eur(4.2), eur(0.1)], &cfg()).concat().is_empty());
    }

    #[test]
    fn windows_match_by_kind_and_label() {
        let opus = |used| Window::new(WindowKind::Weekly, "Weekly · Opus", used).resets(Some(WEEK_RESET));
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Claude, vec![weekly(0.5), opus(0.7)]), snap(Claude, vec![opus(0.85), weekly(0.5)])], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Near(85), "Claude Code: 85% of Weekly · Opus", "Resets Thu 09:00")]);
        // The same label on another kind is another window: its first reading says nothing.
        let mut m = Memory::default();
        let monthly = Window::new(WindowKind::Monthly, "Weekly", 0.9);
        assert!(feed(&mut m, &[snap(Claude, vec![weekly(0.5)]), snap(Claude, vec![weekly(0.5), monthly])], &cfg()).concat().is_empty());
    }

    #[test]
    fn a_window_that_comes_and_goes_keeps_its_place() {
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Claude, vec![five(0.5), weekly(0.7)]), snap(Claude, vec![five(0.5), weekly(0.85)]), snap(Claude, vec![five(0.5)]), snap(Claude, vec![five(0.5), weekly(0.79)]), snap(Claude, vec![five(0.5), weekly(0.86)])], &cfg());
        assert_eq!(said.iter().map(Vec::len).collect::<Vec<_>>(), [0, 1, 0, 0, 0]);
        // Out when it left, and its reset passed meanwhile: back all the same, once.
        let mut m = Memory::default();
        m.changes(&snap(Codex, vec![five(0.5), weekly(1.0)]), NOW, &cfg());
        assert!(m.changes(&snap(Codex, vec![five(0.5)]), WEEK_RESET - 60, &cfg()).is_empty());
        assert_eq!(words(&m.changes(&snap(Codex, vec![five(0.5)]), WEEK_RESET + 60, &cfg())), [(AlertKind::Back, "Codex is back", "Weekly reset")]);
        let week_fresh = Window::new(WindowKind::Weekly, "Weekly", 0.01).resets(Some(WEEK_RESET + 7 * 24 * H));
        assert!(m.changes(&snap(Codex, vec![five(0.5), week_fresh]), WEEK_RESET + 120, &cfg()).is_empty());
    }

    #[test]
    fn another_account_starts_over() {
        let acct = |used, who: &str| {
            let mut s = snap(Codex, vec![five(used)]);
            s.account = Some(who.into());
            s
        };
        let mut m = Memory::default();
        let said = feed(&mut m, &[acct(0.5, "a•••@example.com"), acct(0.9, "b•••@example.com"), acct(0.95, "b•••@example.com")], &cfg());
        assert!(said.concat().is_empty());
        // A reading that doesn't say who is the same account.
        let said = feed(&mut m, &[snap(Codex, vec![five(1.0)]), acct(1.0, "b•••@example.com")], &cfg());
        assert_eq!(said.iter().map(Vec::len).collect::<Vec<_>>(), [1, 0]);
    }

    #[test]
    fn a_plan_running_out_points_to_oyenza_inference_when_connected() {
        let on = AlertCfg { inference: true, ..cfg() };
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Claude, vec![five(0.7)]), snap(Claude, vec![five(0.9)]), snap(Claude, vec![five(1.0)])], &on);
        assert_eq!(said[1][0].nudge, None);
        assert_eq!(said[2][0].nudge.as_deref(), Some(NUDGE));
        // Never for Oyenza Inference itself.
        let mut m = Memory::default();
        let w = |used| Window::new(WindowKind::Daily, "Daily", used).resets(Some(FIVE_RESET));
        let said = feed(&mut m, &[snap(Oyenza, vec![w(0.5)]), snap(Oyenza, vec![w(1.0)])], &on);
        assert_eq!((said[1][0].kind, said[1][0].nudge.as_deref()), (AlertKind::Out, None));
    }

    #[test]
    fn a_long_name_moves_the_window_to_the_second_line() {
        let dec1 = parse_time("2026-11-30T17:00:00Z").unwrap();
        let premium = |used| Window::new(WindowKind::Monthly, "Premium requests", used).resets(Some(dec1));
        let mut m = Memory::default();
        let said = feed(&mut m, &[snap(Copilot, vec![premium(0.5)]), snap(Copilot, vec![premium(0.9)]), snap(Copilot, vec![premium(1.0)])], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Near(90), "GitHub Copilot: 90% used", "Premium requests · resets Tue 1 Dec")]);
        assert_eq!(words(&said[2]), [(AlertKind::Out, "GitHub Copilot: 100% used", "Premium requests · resets Tue 1 Dec")]);
    }

    #[test]
    fn without_a_reset_time() {
        let bare = |used| snap(Zai, vec![Window::new(WindowKind::Session, "5-hour", used)]);
        let mut m = Memory::default();
        let said = feed(&mut m, &[bare(0.5), bare(0.9), bare(0.7), bare(1.0), bare(0.5), bare(0.1), bare(0.5)], &cfg());
        assert_eq!(words(&said[1]), [(AlertKind::Near(90), "Z.ai GLM: 90% of 5-hour", "10% left")]);
        assert_eq!(words(&said[3]), [(AlertKind::Out, "Z.ai GLM: 5-hour used up", "None left for now")]);
        // A dip that stays above half the line is not a reset; a clear drop is.
        assert!(said[2].is_empty() && said[4].is_empty() && said[6].is_empty());
        assert_eq!(words(&said[5]), [(AlertKind::Back, "Z.ai GLM is back", "5-hour reset")]);
    }

    /// About what the UI font's letters take, erring wide (as
    /// `pill::notice::guess` in the Windows crate).
    fn em(s: &str) -> f32 {
        s.chars()
            .map(|c| match c {
                ' ' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' | '\u{b7}' | 'i' | 'j' | 'l' | 'I' | 'f' | 't' | 'r' => 0.30,
                'm' | 'w' | 'M' | 'W' | '%' | '@' | '\u{2014}' => 0.85,
                'A'..='Z' => 0.64,
                _ => 0.56,
            })
            .sum()
    }

    #[test]
    fn every_notice_fits_the_island() {
        // Classic's notice leaves its words 292 DIP: the title at 15 DIP
        // semibold, the line under it at 12 (Pill's are smaller, as wide).
        let labels = ["5-hour", "Weekly", "Weekly · Opus", "Weekly · Sonnet", "Daily", "Monthly", "Premium requests", "Included usage", "5-hour prompts", "Requests"];
        let mut all = Vec::new();
        for id in ProviderId::ALL {
            for label in labels {
                let w = |used| Window::new(WindowKind::Other, label, used).resets(Some(WEEK_RESET));
                let fresh = Window::new(WindowKind::Other, label, 0.0).resets(Some(WEEK_RESET + 7 * 24 * H));
                let mut m = Memory::default();
                let twelve = AlertCfg { use_24h: false, inference: true, ..cfg() };
                all.extend(feed(&mut m, &[snap(id, vec![w(0.5)]), snap(id, vec![w(0.99)]), snap(id, vec![w(1.0)])], &twelve).concat());
                all.extend(m.changes(&snap(id, vec![fresh]), WEEK_RESET + 60, &twelve));
            }
            let mut m = Memory::default();
            all.extend(feed(&mut m, &[balance(id, 150.0), balance(id, 99.99)], &AlertCfg { floor_usd: 100.0, ..cfg() }).concat());
        }
        assert_eq!(all.len(), ProviderId::ALL.len() * (labels.len() * 3 + 1));
        for a in &all {
            assert!(em(&a.title) * 15.0 * 1.05 <= 292.0, "too wide: {:?}", a.title);
            for line in [Some(&a.subtitle), a.nudge.as_ref()].into_iter().flatten() {
                assert!(em(line) * 12.0 <= 292.0, "too wide: {line:?}");
                assert!(!line.contains('\u{2014}') && !a.title.contains('\u{2014}'));
            }
        }
    }
}
