//! The days on the Today page, once signed in to Oyenza: what `GET
//! /island/day` answers (catat-server `island/day.ts`), from the apps the
//! person switched on for it in oyenza.app (Google Calendar's events, and the
//! flights Oyenza's Gmail reader kept), with the bills due that the island
//! already has from Oyenza (`oyenza::Bill`). Studio asks for today and the
//! next five weeks ([`ASK_DAYS`]); a server from before the weeks, or a person
//! who hasn't agreed to them in oyenza.app yet, answers the day alone.
//!
//! Pure: the address, reading the answer, and what the page draws (the
//! meeting on now, the days that hold something, and what each date holds
//! for the five-week grid). The HTTP is the Oyenza worker's
//! (`services/oyenza.rs`), which asks only while the Today page is on screen:
//! when it opens (at most every [`OPEN_GAP_S`]) and every [`EVERY_S`] while it
//! stays.

use crate::datetime::{civil_from_days, days_from_civil, weekday_from_days, MONTH_SHORT, WEEKDAY_SHORT};
use crate::oyenza::{self, Bill, API};
use crate::urlenc::with_query;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Asked again this often while the Today page stays open.
pub const EVERY_S: u64 = 5 * 60;
/// Opening the page asks again only after this.
pub const OPEN_GAP_S: u64 = 30;
/// A server without the day yet (404), or with it paused (503), is asked
/// again only after this.
pub const UNAVAILABLE_S: u64 = 30 * 60;
/// The next event becomes the card this long before it starts.
const NEXT_WITHIN_MIN: i32 = 60;
/// Today and the next five weeks: what Studio asks for.
pub const ASK_DAYS: u32 = 35;
/// The grid: five weeks from this week's Monday.
pub const GRID_DAYS: i64 = 35;

/// `day`: the PC's own date (YYYY-MM-DD); `tz` as `oyenza::tz_param`.
pub fn url(tz: &str, day: &str) -> String {
    with_query(&format!("{API}/island/day"), &[("tz", tz), ("day", day), ("days", &ASK_DAYS.to_string())])
}

// ---------------------------------------------------------------- answer

/// How one app went: switched off in oyenza.app, not connected to the
/// account there, read, or not readable just now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    On,
    #[default]
    Off,
    NotConnected,
    Failed,
    /// A state this version doesn't know.
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Sources {
    #[serde(default)]
    pub googlecalendar: Source,
    #[serde(default)]
    pub gmail: Source,
}

impl Sources {
    pub fn any_on(&self) -> bool {
        self.googlecalendar != Source::Off || self.gmail != Source::Off
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Event {
    pub title: String,
    /// "YYYY-MM-DD HH:MM" in the PC's zone; "YYYY-MM-DD" when `all_day`
    /// (the end is then the day after the last one).
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub join: Option<String>,
    /// The event's own page in Google Calendar, opened when it's clicked.
    #[serde(default)]
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Flight {
    /// The day it leaves (YYYY-MM-DD); a server from before the weeks sends
    /// the answer's day's flights only, without it.
    #[serde(default)]
    pub day: Option<String>,
    #[serde(default)]
    pub flight: Option<String>,
    #[serde(default)]
    pub airline: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub from_city: String,
    #[serde(default)]
    pub to_city: String,
    /// HH:MM at each airport, as the booking prints them.
    #[serde(default)]
    pub depart: Option<String>,
    #[serde(default)]
    pub arrive: Option<String>,
    #[serde(default)]
    pub arrive_day: Option<String>,
    #[serde(default)]
    pub from_terminal: Option<String>,
}

fn one_day() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Answer {
    /// The day it starts on (YYYY-MM-DD).
    pub day: String,
    /// Days from `day` it covers: 1 from a server before the weeks, or until
    /// the person agrees to them in oyenza.app.
    #[serde(default = "one_day")]
    pub days: u32,
    /// The last day the events are complete through: earlier than the end of
    /// `days` when Google's answer was full.
    #[serde(default)]
    pub through: Option<String>,
    /// Whether the person agreed to the weeks (None: a server from before them).
    #[serde(default)]
    pub weeks: Option<bool>,
    #[serde(default)]
    pub apps: Sources,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub flights: Vec<Flight>,
}

impl Answer {
    /// The day with no app switched on for it.
    pub fn none(day: &str) -> Self {
        Answer { day: day.into(), days: 1, through: None, weeks: Some(true), apps: Sources::default(), events: Vec::new(), flights: Vec::new() }
    }

    /// The last date it speaks for (days since 1970-01-01): `through`, else
    /// the end of its days; never before its first day.
    pub fn last_day(&self) -> Option<i64> {
        let first = day_num(&self.day)?;
        let end = first + i64::from(self.days.clamp(1, ASK_DAYS)) - 1;
        let through = self.through.as_deref().and_then(day_num).unwrap_or(end);
        Some(through.clamp(first, end))
    }

    /// The server answered the day alone because the person hasn't agreed to
    /// the weeks in oyenza.app yet.
    pub fn weeks_to_allow(&self) -> bool {
        self.weeks == Some(false) && self.days <= 1
    }
}

pub fn parse(body: &str) -> Option<Answer> {
    serde_json::from_str::<Answer>(body).ok().filter(|a| day_num(&a.day).is_some())
}

/// A meeting link Join may open: https, on Google Meet, Zoom, Microsoft Teams
/// or Webex, with no name, password or port. The server keeps only such links
/// (catat-server `joinLink`); this checks again, so no other link can ever get
/// behind the button.
pub fn join_ok(url: &str) -> bool {
    let Some((host, _)) = https_parts(url) else { return false };
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    host == "meet.google.com" || under("zoom.us") || host == "teams.microsoft.com" || host == "teams.live.com" || under("webex.com")
}

/// An event's page a click may open: https on Google Calendar's own pages
/// (`www.google.com/calendar/…` or `calendar.google.com`), with no name,
/// password or port. The server keeps only such links (catat-server
/// `calendarLink`); this checks again.
pub fn open_ok(url: &str) -> bool {
    let Some((host, path)) = https_parts(url) else { return false };
    (host == "www.google.com" && path.starts_with("/calendar/")) || host == "calendar.google.com"
}

/// An https URL's host (lowercase) and the rest after it, when it has no
/// name, password, port, space or control character and isn't too long.
fn https_parts(url: &str) -> Option<(String, &str)> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains(['@', ':']) || url.len() > 500 || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    Some((authority.to_ascii_lowercase(), &rest[authority.len()..]))
}

/// The service a Join link is on, said instead of a missing place.
pub fn meeting_service(url: &str) -> &'static str {
    let host = url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("").to_ascii_lowercase();
    if host == "meet.google.com" {
        "Google Meet"
    } else if host == "zoom.us" || host.ends_with(".zoom.us") {
        "Zoom"
    } else if host.starts_with("teams.") {
        "Microsoft Teams"
    } else if host.ends_with("webex.com") {
        "Webex"
    } else {
        "Online"
    }
}

// ---------------------------------------------------------------- rows

/// Where a row came from: its logo on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Calendar,
    Gmail,
    /// Oyenza's own: a bill due.
    Bill,
}

/// The event on now, or the next one starting within the hour.
#[derive(Clone, Debug, PartialEq)]
pub struct Focus {
    /// On now (else coming up).
    pub now: bool,
    pub title: String,
    /// "10:00–11:00".
    pub when: String,
    /// "48 min left", "in 25 min".
    pub left: String,
    /// The place, else the meeting service.
    pub place: Option<String>,
    pub join: Option<String>,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// "12:30", "All day", "Earlier", "Due".
    pub when: String,
    pub mark: Mark,
    pub title: String,
    pub detail: String,
    pub join: Option<String>,
    /// Its page in Google Calendar (checked with [`open_ok`]).
    pub link: Option<String>,
}

/// What a date holds: its dots in the grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kinds {
    pub calendar: bool,
    pub flight: bool,
    pub bill: bool,
}

impl Kinds {
    pub fn any(&self) -> bool {
        self.calendar || self.flight || self.bill
    }
}

/// One date of the list (days since 1970-01-01) and its rows: all-day events
/// first, then events and flights by time, then bills due.
#[derive(Clone, Debug, PartialEq)]
pub struct DayRows {
    pub day: i64,
    pub rows: Vec<Row>,
}

/// What the Today page draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Weeks {
    /// The meeting on now, or the next one within the hour (today's).
    pub focus: Option<Focus>,
    /// Today first, always (what's still to come), then each later date that
    /// holds something, up to `last`.
    pub days: Vec<DayRows>,
    /// What each date holds, from today to `last` (today's past events count).
    pub kinds: BTreeMap<i64, Kinds>,
    pub today: i64,
    /// The last date the answer speaks for.
    pub last: i64,
}

impl Weeks {
    /// Today held something already ("Nothing else today" rather than "Nothing today").
    pub fn today_had(&self) -> bool {
        self.kinds.get(&self.today).is_some_and(Kinds::any)
    }
}

fn date_parts(s: &str) -> Option<(i32, u8, u8)> {
    let mut it = s.get(..10)?.split('-');
    let y = it.next()?.parse().ok()?;
    let m: u8 = it.next()?.parse().ok()?;
    let d: u8 = it.next()?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d) && (s.len() == 10 || s.as_bytes().get(10) == Some(&b' '))).then_some((y, m, d))
}

/// "YYYY-MM-DD" (or "YYYY-MM-DD HH:MM") as days since 1970-01-01.
pub fn day_num(s: &str) -> Option<i64> {
    let (y, m, d) = date_parts(s)?;
    Some(days_from_civil(y, m, d))
}

/// Days since 1970-01-01 as "YYYY-MM-DD".
pub fn day_key(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// "Today · Mon 26 Oct", "Tomorrow · Tue 27 Oct", "Wed 28 Oct".
pub fn day_label(day: i64, today: i64) -> String {
    let (_, m, d) = civil_from_days(day);
    let base = format!("{} {d} {}", WEEKDAY_SHORT[weekday_from_days(day) as usize % 7], MONTH_SHORT[(m as usize).clamp(1, 12) - 1]);
    match day - today {
        0 => format!("Today · {base}"),
        1 => format!("Tomorrow · {base}"),
        _ => base,
    }
}

/// The grid's first date: the Monday of `today`'s week.
pub fn grid_start(today: i64) -> i64 {
    today - (i64::from(weekday_from_days(today)) + 6) % 7
}

/// Minutes from `today`'s midnight to "YYYY-MM-DD HH:MM" (negative before it).
fn minutes_from(today: &str, at: &str) -> Option<i32> {
    let days = day_num(at)? - day_num(today)?;
    Some(days as i32 * 1440 + hm(at.get(11..16)?)?)
}

/// "HH:MM" as minutes.
fn hm(s: &str) -> Option<i32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

fn clock(min: i32) -> String {
    let m = min.rem_euclid(1440);
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn span(min: i32) -> String {
    let min = min.max(1);
    match (min / 60, min % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The place, else the service of a meeting link.
fn place_of(e: &Event) -> Option<String> {
    let p = e.place.as_deref().map(one_line).filter(|p| !p.is_empty());
    p.or_else(|| e.join.as_deref().filter(|j| join_ok(j)).map(|j| meeting_service(j).to_string()))
}

/// The days as the page shows them at `now_min` (minutes since the PC's
/// midnight) on `today`, through the answer's last day: what's on now (the
/// card), what's still to come today, and each later date that holds
/// something. Past events and flights already gone are left out of today's
/// list (they still count for its dot). `bills` with `bills_today` (the
/// server's today for them) add the bills due; their sums stay masked unless
/// `show_amounts`. `pick`, a date clicked in the grid, gets a place in the
/// list even with nothing on it.
pub fn weeks(answer: &Answer, bills: &[Bill], bills_today: &str, today: &str, now_min: i32, show_amounts: bool, pick: Option<i64>) -> Weeks {
    let Some(t0) = day_num(today) else { return Weeks::default() };
    let last = answer.last_day().unwrap_or(t0).max(t0);
    let mut kinds: BTreeMap<i64, Kinds> = BTreeMap::new();
    // Rows by date, each with its place in the day: all-day -1, a time in minutes, bills after everything.
    let mut by_day: BTreeMap<i64, Vec<(i32, Row)>> = BTreeMap::new();
    let mut timed_today: Vec<(i32, i32, &Event)> = Vec::new();

    for e in &answer.events {
        let title = one_line(&e.title);
        let title = if title.is_empty() { "Busy".to_string() } else { title };
        let link = e.link.clone().filter(|l| open_ok(l));
        if e.all_day {
            let (Some(start), Some(end)) = (day_num(&e.start), day_num(&e.end)) else { continue };
            let detail = e.place.as_deref().map(one_line).unwrap_or_default();
            for d in start.max(t0)..end.max(start + 1).min(last + 1) {
                kinds.entry(d).or_default().calendar = true;
                let row = Row { when: "All day".into(), mark: Mark::Calendar, title: title.clone(), detail: detail.clone(), join: None, link: link.clone() };
                by_day.entry(d).or_default().push((-1, row));
            }
            continue;
        }
        let (Some(s), Some(en)) = (minutes_from(today, &e.start), minutes_from(today, &e.end)) else { continue };
        let en = en.max(s);
        if s < 1440 && en > 0 {
            // Today's, or one that began before today and runs into it.
            kinds.entry(t0).or_default().calendar = true;
            if en > now_min {
                timed_today.push((s, en, e));
            }
        } else if s >= 1440 && t0 + i64::from(s / 1440) <= last {
            let d = t0 + i64::from(s / 1440);
            kinds.entry(d).or_default().calendar = true;
            let row = Row { when: clock(s), mark: Mark::Calendar, title, detail: place_of(e).unwrap_or_default(), join: e.join.clone().filter(|j| join_ok(j)), link };
            by_day.entry(d).or_default().push((s % 1440, row));
        }
    }

    // The card: what's on now (the latest to start), else the next within the hour.
    timed_today.sort_by_key(|(s, en, _)| (*s, *en));
    let on_now = timed_today.iter().rposition(|(s, en, _)| *s <= now_min && now_min < *en);
    let pick_i = on_now.or_else(|| timed_today.iter().position(|(s, _, _)| *s > now_min && *s - now_min <= NEXT_WITHIN_MIN));
    let focus = pick_i.map(|i| {
        let (s, en, e) = timed_today[i];
        let now = s <= now_min;
        Focus {
            now,
            title: one_line(&e.title),
            when: format!("{}–{}", clock(s.max(0)), clock(en.min(1440))),
            left: if now { format!("{} left", span(en - now_min)) } else { format!("in {}", span(s - now_min)) },
            place: place_of(e),
            join: e.join.clone().filter(|j| join_ok(j)),
            link: e.link.clone().filter(|l| open_ok(l)),
        }
    });
    for (i, (s, _, e)) in timed_today.iter().enumerate() {
        if Some(i) == pick_i {
            continue;
        }
        let when = if *s < 0 { "Earlier".to_string() } else { clock(*s) };
        let title = one_line(&e.title);
        let title = if title.is_empty() { "Busy".to_string() } else { title };
        let row = Row { when, mark: Mark::Calendar, title, detail: place_of(e).unwrap_or_default(), join: e.join.clone().filter(|j| join_ok(j)), link: e.link.clone().filter(|l| open_ok(l)) };
        by_day.entry(t0).or_default().push(((*s).max(0), row));
    }

    for f in &answer.flights {
        let Some(d) = day_num(f.day.as_deref().unwrap_or(&answer.day)) else { continue };
        if d < t0 || d > last {
            continue;
        }
        kinds.entry(d).or_default().flight = true;
        let at = f.depart.as_deref().and_then(hm);
        // A flight that left more than half an hour ago is behind you.
        if d == t0 && at.is_some_and(|m| m + 30 < now_min) {
            continue;
        }
        let city = if f.to_city.trim().is_empty() { f.to.clone() } else { one_line(&f.to_city) };
        let from = match f.from_terminal.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) if t.chars().all(|c| c.is_ascii_digit()) => format!("{} T{t}", f.from),
            Some(t) => format!("{} {t}", f.from),
            None => f.from.clone(),
        };
        let detail = match f.flight.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
            Some(n) => format!("{n} · {from}"),
            None => from,
        };
        let row = Row { when: f.depart.clone().unwrap_or_default(), mark: Mark::Gmail, title: format!("Flight to {city}"), detail, join: None, link: None };
        by_day.entry(d).or_default().push((at.unwrap_or(1440), row));
    }

    if bills_today == today {
        for b in bills {
            let Some(d) = day_num(&b.due) else { continue };
            if d < t0 || d > last {
                continue;
            }
            kinds.entry(d).or_default().bill = true;
            let detail = match (&b.text, b.is_card()) {
                (_, true) => "Card bill".to_string(),
                (Some(t), false) if show_amounts => t.clone(),
                (Some(t), false) => oyenza::masked(t),
                (None, false) => "Renews".to_string(),
            };
            by_day.entry(d).or_default().push((2000, Row { when: "Due".into(), mark: Mark::Bill, title: one_line(&b.name), detail, join: None, link: None }));
        }
    }

    let rows_of = |mut v: Vec<(i32, Row)>| {
        v.sort_by_key(|(at, _)| *at);
        v.into_iter().map(|(_, r)| r).collect::<Vec<_>>()
    };
    let mut days = vec![DayRows { day: t0, rows: rows_of(by_day.remove(&t0).unwrap_or_default()) }];
    days.extend(by_day.into_iter().filter(|(d, _)| *d > t0).map(|(day, v)| DayRows { day, rows: rows_of(v) }));
    if let Some(p) = pick.filter(|p| *p > t0 && *p <= last && !days.iter().any(|d| d.day == *p)) {
        let at = days.iter().position(|d| d.day > p).unwrap_or(days.len());
        days.insert(at, DayRows { day: p, rows: Vec::new() });
    }
    Weeks { focus, days, kinds, today: t0, last }
}

/// The days the self-tests draw (Windows' `selftest`, the Mac's `render`),
/// around the clock they run at: today a meeting on Google Meet that started
/// 12 minutes ago, lunch later, a call on Zoom and a flight tonight (what
/// falls past midnight stays out, as it would), then the weeks: standups, a
/// two-day offsite, the flight back, reviews, all with their Google Calendar
/// pages.
pub fn sample(today: &str, now_min: i32) -> Answer {
    let t0 = day_num(today).unwrap_or(0);
    let on = |days: i64| day_key(t0 + days);
    let at = |days: i64, min: i32| (min < 1440).then(|| format!("{} {:02}:{:02}", on(days), min / 60, min % 60));
    let mut n = 0;
    let mut event = |title: &str, days: i64, start: i32, len: i32, place: Option<&str>, join: Option<&str>| {
        n += 1;
        Some(Event {
            title: title.into(),
            start: at(days, start.max(0))?,
            end: at(days, (start + len).min(1439))?,
            all_day: false,
            place: place.map(Into::into),
            join: join.map(Into::into),
            link: Some(format!("https://www.google.com/calendar/event?eid=c2FtcGxl{n}")),
        })
    };
    let meet = Some("https://meet.google.com/abc-defg-hij");
    let zoom = Some("https://acme.zoom.us/j/123");
    let mut events: Vec<Event> = [
        event("Product review", 0, now_min - 12, 60, None, meet),
        event("Lunch with the team", 0, now_min + 138, 60, Some("Senopati"), None),
        event("Content plan check-in", 0, now_min + 288, 30, None, zoom),
        event("Standup", 1, 9 * 60 + 30, 15, None, meet),
        event("Planning review", 1, 14 * 60, 60, None, zoom),
        event("Standup", 2, 9 * 60 + 30, 15, None, meet),
        event("Design sync", 3, 10 * 60, 45, None, meet),
        event("Weekly wrap", 4, 15 * 60, 45, None, zoom),
        event("Standup", 7, 9 * 60 + 30, 15, None, meet),
        event("Product review", 9, 10 * 60, 60, None, meet),
        event("Quarter planning", 16, 12 * 60, 90, Some("Office"), None),
        event("Weekly wrap", 18, 15 * 60, 45, None, zoom),
        event("Product review", 23, 10 * 60, 60, None, meet),
    ]
    .into_iter()
    .flatten()
    .collect();
    events.push(Event {
        title: "Team offsite".into(),
        start: on(10),
        end: on(12),
        all_day: true,
        place: Some("Bandung".into()),
        join: None,
        link: Some("https://www.google.com/calendar/event?eid=b2Zmc2l0ZQ".into()),
    });
    let flight = |days: i64, no: &str, from: &str, to: &str, from_city: &str, to_city: &str, depart: i32, terminal: Option<&str>| Flight {
        day: Some(on(days)),
        flight: Some(no.into()),
        airline: Some("Garuda Indonesia".into()),
        from: from.into(),
        to: to.into(),
        from_city: from_city.into(),
        to_city: to_city.into(),
        depart: Some(format!("{:02}:{:02}", depart / 60, depart % 60)),
        arrive: None,
        arrive_day: None,
        from_terminal: terminal.map(Into::into),
    };
    let tonight = now_min + 533;
    let mut flights: Vec<Flight> = (tonight < 1440).then(|| flight(0, "GA408", "CGK", "DPS", "Jakarta", "Bali", tonight, Some("3"))).into_iter().collect();
    flights.push(flight(6, "GA417", "DPS", "CGK", "Bali", "Jakarta", 17 * 60 + 40, None));
    Answer {
        day: today.into(),
        days: ASK_DAYS,
        through: None,
        weeks: Some(true),
        apps: Sources { googlecalendar: Source::On, gmail: Source::On },
        events,
        flights,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(title: &str, start: &str, end: &str) -> Event {
        Event { title: title.into(), start: start.into(), end: end.into(), all_day: false, place: None, join: None, link: None }
    }

    fn all_day(title: &str, start: &str, end: &str) -> Event {
        Event { all_day: true, ..event(title, start, end) }
    }

    /// One day, as a server from before the weeks answers.
    fn answer(events: Vec<Event>, flights: Vec<Flight>) -> Answer {
        Answer { events, flights, apps: Sources { googlecalendar: Source::On, gmail: Source::On }, ..Answer::none("2026-10-26") }
    }

    /// The five weeks.
    fn five(events: Vec<Event>, flights: Vec<Flight>) -> Answer {
        Answer { days: 35, ..answer(events, flights) }
    }

    fn flight(day: Option<&str>, depart: &str) -> Flight {
        Flight {
            day: day.map(Into::into),
            flight: None,
            airline: None,
            from: "CGK".into(),
            to: "SIN".into(),
            from_city: String::new(),
            to_city: String::new(),
            depart: Some(depart.into()),
            arrive: None,
            arrive_day: None,
            from_terminal: Some("2F".into()),
        }
    }

    const TODAY: &str = "2026-10-26";
    // 10:12
    const NOW: i32 = 10 * 60 + 12;

    fn today_rows(w: &Weeks) -> Vec<(String, String, String)> {
        w.days[0].rows.iter().map(|r| (r.when.clone(), r.title.clone(), r.detail.clone())).collect()
    }

    fn d(s: &str) -> i64 {
        day_num(s).expect("a date")
    }

    #[test]
    fn reads_the_answer() {
        let body = r#"{"day":"2026-10-26","apps":{"googlecalendar":"on","gmail":"not_connected"},
            "events":[{"title":"Product review","start":"2026-10-26 10:00","end":"2026-10-26 11:00","all_day":false,"join":"https://meet.google.com/abc"}],
            "flights":[{"flight":"GA408","airline":"Garuda Indonesia","from":"CGK","to":"DPS","from_city":"Jakarta","to_city":"Bali","depart":"19:05","arrive":"22:00","from_terminal":"3"}]}"#;
        let a = parse(body).expect("reads");
        assert_eq!(a.apps, Sources { googlecalendar: Source::On, gmail: Source::NotConnected });
        assert_eq!(a.events.len(), 1);
        assert_eq!(a.flights[0].to_city, "Bali");
        // A server from before the weeks: one day, no say about them.
        assert_eq!((a.days, a.through.as_deref(), a.weeks), (1, None, None));
        assert_eq!(a.last_day(), Some(d(TODAY)));
        assert!(!a.weeks_to_allow());
        // Something new from a newer server doesn't break it.
        let a = parse(r#"{"day":"2026-10-26","apps":{"googlecalendar":"sleeping","notion":"on"},"events":[]}"#).expect("reads");
        assert_eq!(a.apps.googlecalendar, Source::Other);
        assert!(parse(r#"{"day":"soon"}"#).is_none());
        assert!(parse(r#"{"day":"2026-13-01"}"#).is_none());
        assert!(parse("<html>").is_none());
    }

    #[test]
    fn reads_the_weeks() {
        let body = r#"{"day":"2026-10-26","days":35,"through":"2026-11-29","weeks":true,"apps":{"googlecalendar":"on","gmail":"on"},
            "events":[{"title":"Standup","start":"2026-10-27 09:30","end":"2026-10-27 09:45","all_day":false,"link":"https://www.google.com/calendar/event?eid=a"}],
            "flights":[{"day":"2026-10-28","flight":"GA408","from":"CGK","to":"DPS","depart":"19:05"}]}"#;
        let a = parse(body).expect("reads");
        assert_eq!(a.last_day(), Some(d("2026-11-29")));
        assert_eq!(a.events[0].link.as_deref(), Some("https://www.google.com/calendar/event?eid=a"));
        assert_eq!(a.flights[0].day.as_deref(), Some("2026-10-28"));
        // A full answer is complete through an earlier day; nonsense stays inside the days.
        let cut = Answer { through: Some("2026-11-05".into()), ..a.clone() };
        assert_eq!(cut.last_day(), Some(d("2026-11-05")));
        let odd = Answer { through: Some("2027-01-01".into()), ..a.clone() };
        assert_eq!(odd.last_day(), Some(d("2026-11-29")));
        // The person hasn't agreed to the weeks yet: the day alone, and the page offers the agreement.
        let one = parse(r#"{"day":"2026-10-26","days":1,"through":"2026-10-26","weeks":false}"#).expect("reads");
        assert!(one.weeks_to_allow());
        assert_eq!(url("+07:00", "2026-10-26"), "https://api.oyenza.app/island/day?tz=%2B07%3A00&day=2026-10-26&days=35");
    }

    #[test]
    fn join_only_on_meeting_services() {
        for ok in ["https://meet.google.com/abc-defg-hij", "https://us02web.zoom.us/j/1?pwd=x", "https://zoom.us/j/1", "https://teams.microsoft.com/l/meetup-join/1", "https://teams.live.com/meet/1", "https://acme.webex.com/meet/x"] {
            assert!(join_ok(ok), "{ok}");
        }
        for bad in ["http://meet.google.com/abc", "https://zoom.us.example.test/j/1", "https://evilzoom.us/j/1", "https://user@meet.google.com/abc", "https://meet.google.com:8443/abc", "https://meet.google.com/a b", "javascript:alert(1)", ""] {
            assert!(!join_ok(bad), "{bad}");
        }
        assert_eq!(meeting_service("https://acme.zoom.us/j/1"), "Zoom");
        assert_eq!(meeting_service("https://meet.google.com/x"), "Google Meet");
    }

    #[test]
    fn opens_only_google_calendar_pages() {
        for ok in ["https://www.google.com/calendar/event?eid=abc", "https://calendar.google.com/calendar/event?eid=abc", "https://WWW.google.com/calendar/r"] {
            assert!(open_ok(ok), "{ok}");
        }
        for bad in [
            "http://www.google.com/calendar/event?eid=abc",
            "https://www.google.com/search?q=calendar",
            "https://www.google.com.example.test/calendar/event",
            "https://user@calendar.google.com/x",
            "https://calendar.google.com:444/x",
            "https://example.test/calendar/event",
            "",
        ] {
            assert!(!open_ok(bad), "{bad}");
        }
    }

    #[test]
    fn the_card_is_the_meeting_on_now_then_the_rest_of_the_day() {
        let mut review = event("Product review", "2026-10-26 10:00", "2026-10-26 11:00");
        review.join = Some("https://meet.google.com/abc".into());
        review.link = Some("https://www.google.com/calendar/event?eid=r".into());
        let mut lunch = event("Lunch with the team", "2026-10-26 12:30", "2026-10-26 13:30");
        lunch.place = Some("Senopati".into());
        let standup = event("Standup", "2026-10-26 09:30", "2026-10-26 09:45");
        let flight = Flight {
            day: None,
            flight: Some("GA408".into()), airline: None, from: "CGK".into(), to: "DPS".into(), from_city: "Jakarta".into(), to_city: "Bali".into(),
            depart: Some("19:05".into()), arrive: Some("22:00".into()), arrive_day: None, from_terminal: Some("3".into()),
        };
        let bills = vec![
            Bill { kind: "subscription".into(), name: "Netflix".into(), due: TODAY.into(), amount: Some(186_000.0), text: Some("Rp 186.000".into()) },
            Bill { kind: "card".into(), name: "Kartu Kredit".into(), due: TODAY.into(), amount: None, text: None },
            Bill { kind: "subscription".into(), name: "Spotify".into(), due: "2026-10-27".into(), amount: None, text: Some("Rp 54.990".into()) },
        ];
        let w = weeks(&answer(vec![lunch, review, standup], vec![flight]), &bills, TODAY, TODAY, NOW, false, None);
        let focus = w.focus.clone().expect("a card");
        assert!(focus.now);
        assert_eq!((focus.title.as_str(), focus.when.as_str(), focus.left.as_str()), ("Product review", "10:00–11:00", "48 min left"));
        assert_eq!(focus.place.as_deref(), Some("Google Meet"));
        assert_eq!(focus.join.as_deref(), Some("https://meet.google.com/abc"));
        assert_eq!(focus.link.as_deref(), Some("https://www.google.com/calendar/event?eid=r"));
        assert_eq!(
            today_rows(&w),
            vec![
                ("12:30".into(), "Lunch with the team".into(), "Senopati".into()),
                ("19:05".into(), "Flight to Bali".into(), "GA408 · CGK T3".into()),
                ("Due".into(), "Netflix".into(), "Rp •••".into()),
                ("Due".into(), "Kartu Kredit".into(), "Card bill".into()),
            ]
        );
        // A one-day answer stops at today: tomorrow's bill waits for the weeks.
        assert_eq!(w.days.len(), 1);
        assert_eq!(w.kinds.get(&d(TODAY)), Some(&Kinds { calendar: true, flight: true, bill: true }));
        // "Show amounts" shows the sum.
        let shown = weeks(&answer(vec![], vec![]), &bills, TODAY, TODAY, NOW, true, None);
        assert_eq!(shown.days[0].rows[0].detail, "Rp 186.000");
        // Bills read for another day don't count.
        assert!(weeks(&answer(vec![], vec![]), &bills, "2026-10-25", TODAY, NOW, false, None).days[0].rows.is_empty());
    }

    #[test]
    fn nothing_on_now_the_next_within_the_hour_is_the_card() {
        let w = weeks(&answer(vec![event("Design crit", "2026-10-26 10:40", "2026-10-26 11:10")], vec![]), &[], "", TODAY, NOW, false, None);
        let f = w.focus.expect("a card");
        assert!(!f.now);
        assert_eq!(f.left, "in 28 min");
        assert!(w.days[0].rows.is_empty());
        // Further off than an hour: a row, no card.
        let w = weeks(&answer(vec![event("Dinner", "2026-10-26 19:00", "2026-10-26 21:00")], vec![]), &[], "", TODAY, NOW, false, None);
        assert!(w.focus.is_none());
        assert_eq!(w.days[0].rows[0].when, "19:00");
    }

    #[test]
    fn all_day_first_past_left_out_and_long_ones_on_each_of_their_days() {
        let gone = event("Breakfast", "2026-10-26 07:00", "2026-10-26 08:00");
        let overnight = event("Shift", "2026-10-25 22:00", "2026-10-26 11:30");
        let events = vec![gone, all_day("Libur", "2026-10-26", "2026-10-27"), all_day("Bali trip", "2026-10-25", "2026-10-28"), all_day("Tomorrow only", "2026-10-27", "2026-10-28"), overnight];
        let w = weeks(&answer(events.clone(), vec![]), &[], "", TODAY, NOW, false, None);
        let f = w.focus.clone().expect("the overnight one is on");
        assert_eq!((f.when.as_str(), f.left.as_str()), ("00:00–11:30", "1 h 18 min left"));
        assert_eq!(w.days[0].rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), vec!["Libur", "Bali trip"]);
        assert!(w.days[0].rows.iter().all(|r| r.when == "All day"));
        assert_eq!(w.days.len(), 1, "one day answered");
        // With the weeks, the trip shows on each of its days, and tomorrow's on tomorrow.
        let w = weeks(&five(events, vec![]), &[], "", TODAY, NOW, false, None);
        let titles = |i: usize| w.days[i].rows.iter().map(|r| r.title.clone()).collect::<Vec<_>>();
        assert_eq!(w.days.iter().map(|x| x.day).collect::<Vec<_>>(), vec![d(TODAY), d("2026-10-27")]);
        assert_eq!(titles(1), vec!["Bali trip", "Tomorrow only"]);
    }

    #[test]
    fn the_weeks_today_first_then_the_dates_that_hold_something() {
        let mut standup = event("Standup", "2026-10-27 09:30", "2026-10-27 09:45");
        standup.join = Some("https://meet.google.com/x".into());
        standup.link = Some("https://www.google.com/calendar/event?eid=s".into());
        let mut odd = event("Odd link", "2026-10-29 10:00", "2026-10-29 11:00");
        odd.link = Some("https://example.test/e".into());
        let gone = event("Breakfast", "2026-10-26 07:00", "2026-10-26 08:00");
        let late = event("Beyond", "2026-12-15 10:00", "2026-12-15 11:00");
        let flights = vec![flight(Some("2026-10-28"), "19:05"), flight(Some("2026-10-26"), "06:00")];
        let bills = vec![Bill { kind: "card".into(), name: "Kartu Kredit".into(), due: "2026-10-30".into(), amount: None, text: None }];
        let w = weeks(&five(vec![odd, standup, gone, late], flights), &bills, TODAY, TODAY, NOW, false, None);
        assert_eq!(w.days.iter().map(|x| day_key(x.day)).collect::<Vec<_>>(), vec!["2026-10-26", "2026-10-27", "2026-10-28", "2026-10-29", "2026-10-30"]);
        // Today: nothing still to come, but it held a breakfast and a flight that left.
        assert!(w.days[0].rows.is_empty());
        assert!(w.today_had());
        let tue = &w.days[1].rows[0];
        assert_eq!((tue.when.as_str(), tue.detail.as_str()), ("09:30", "Google Meet"));
        assert_eq!(tue.link.as_deref(), Some("https://www.google.com/calendar/event?eid=s"));
        assert_eq!(w.days[3].rows[0].link, None, "a link off Google Calendar never opens");
        assert_eq!((w.days[4].rows[0].when.as_str(), w.days[4].rows[0].mark), ("Due", Mark::Bill));
        assert_eq!(w.kinds.get(&d("2026-10-28")), Some(&Kinds { flight: true, ..Kinds::default() }));
        assert_eq!(w.kinds.get(&d("2026-10-30")), Some(&Kinds { bill: true, ..Kinds::default() }));
        assert!(!w.kinds.contains_key(&d("2026-12-15")), "past the weeks");
        assert_eq!(w.last, d("2026-11-29"));
        // Nothing at all today: "Nothing today".
        let quiet = weeks(&five(vec![], vec![]), &[], "", TODAY, NOW, false, None);
        assert!(!quiet.today_had());
        assert_eq!(quiet.days.len(), 1);
    }

    #[test]
    fn a_clicked_date_gets_its_place_even_when_empty() {
        let tue = event("Standup", "2026-10-27 09:30", "2026-10-27 09:45");
        let fri = event("Weekly wrap", "2026-10-30 15:00", "2026-10-30 15:45");
        let a = five(vec![tue, fri], vec![]);
        let w = weeks(&a, &[], "", TODAY, NOW, false, Some(d("2026-10-29")));
        assert_eq!(w.days.iter().map(|x| day_key(x.day)).collect::<Vec<_>>(), vec!["2026-10-26", "2026-10-27", "2026-10-29", "2026-10-30"]);
        assert!(w.days[2].rows.is_empty());
        // A date with something already has its place; one past the weeks, or gone, gets none.
        for pick in ["2026-10-27", "2026-12-30", "2026-10-20", TODAY] {
            assert_eq!(weeks(&a, &[], "", TODAY, NOW, false, Some(d(pick))).days.len(), 3, "{pick}");
        }
    }

    #[test]
    fn a_full_answer_stops_where_google_did() {
        let a = Answer { through: Some("2026-10-27".into()), ..five(vec![event("Standup", "2026-10-27 09:30", "2026-10-27 09:45"), event("Wrap", "2026-10-30 15:00", "2026-10-30 16:00")], vec![]) };
        let w = weeks(&a, &[], "", TODAY, NOW, false, None);
        assert_eq!(w.last, d("2026-10-27"));
        assert_eq!(w.days.len(), 2);
        assert!(!w.kinds.contains_key(&d("2026-10-30")));
    }

    #[test]
    fn a_flight_that_left_is_behind_you_and_odd_links_get_no_join() {
        let w = weeks(&answer(vec![], vec![flight(None, "06:00"), flight(None, "09:50"), flight(None, "21:00")]), &[], "", TODAY, NOW, false, None);
        assert_eq!(w.days[0].rows.iter().map(|r| r.when.as_str()).collect::<Vec<_>>(), vec!["09:50", "21:00"]);
        assert_eq!((w.days[0].rows[0].title.as_str(), w.days[0].rows[0].detail.as_str()), ("Flight to SIN", "CGK 2F"));
        let mut odd = event("Call", "2026-10-26 10:00", "2026-10-26 10:30");
        odd.join = Some("https://example.test/call".into());
        let f = weeks(&answer(vec![odd], vec![]), &[], "", TODAY, NOW, false, None).focus.expect("on now");
        assert_eq!((f.join, f.place), (None, None));
    }

    #[test]
    fn labels_and_the_grid_start_on_a_monday() {
        let t = d(TODAY);
        assert_eq!(day_label(t, t), "Today · Mon 26 Oct");
        assert_eq!(day_label(t + 1, t), "Tomorrow · Tue 27 Oct");
        assert_eq!(day_label(t + 6, t), "Sun 1 Nov");
        assert_eq!(grid_start(t), t);
        assert_eq!(day_key(grid_start(d("2026-10-29"))), TODAY);
        assert_eq!(day_key(grid_start(d("2026-11-01"))), TODAY);
        assert_eq!(day_key(t + GRID_DAYS - 1), "2026-11-29");
    }

    #[test]
    fn the_sample_has_a_meeting_on_now_and_five_weeks_after_it() {
        let w = weeks(&sample(TODAY, NOW), &[], "", TODAY, NOW, false, None);
        assert!(w.focus.as_ref().is_some_and(|f| f.now && f.join.is_some() && f.link.is_some()));
        assert_eq!(w.days[0].rows.len(), 3);
        assert!(w.days.len() > 8);
        assert!(w.kinds.values().any(|k| k.flight) && w.days.iter().flat_map(|x| &x.rows).all(|r| r.link.as_deref().is_none_or(open_ok)));
        // At 22:00 the later ones fall past midnight: the meeting alone today.
        let late = weeks(&sample(TODAY, 22 * 60), &[], "", TODAY, 22 * 60, false, None);
        assert!(late.focus.is_some() && late.days[0].rows.is_empty());
    }
}
