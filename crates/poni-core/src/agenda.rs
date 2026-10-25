//! The day on the Today page, once signed in to Oyenza: what `GET
//! /island/day` answers (catat-server `island/day.ts`), from the apps the
//! person switched on for it in oyenza.app (Google Calendar's events, and the
//! flights Oyenza's Gmail reader kept), with the bills due today the island
//! already has from Oyenza (`oyenza::Bill`).
//!
//! Pure: the address, reading the answer, and the rows the page draws. The
//! HTTP is the Oyenza worker's (`services/oyenza.rs`), which asks only while
//! the Today page is on screen: when it opens (at most every
//! [`OPEN_GAP_S`]) and every [`EVERY_S`] while it stays.

use crate::oyenza::{self, Bill, API};
use crate::urlenc::with_query;
use serde::Deserialize;

/// Asked again this often while the Today page stays open.
pub const EVERY_S: u64 = 5 * 60;
/// Opening the page asks again only after this.
pub const OPEN_GAP_S: u64 = 30;
/// A server without the day yet (404), or with it paused (503), is asked
/// again only after this.
pub const UNAVAILABLE_S: u64 = 30 * 60;
/// The next event becomes the card this long before it starts.
const NEXT_WITHIN_MIN: i32 = 60;

/// `day`: the PC's own date (YYYY-MM-DD); `tz` as `oyenza::tz_param`.
pub fn url(tz: &str, day: &str) -> String {
    with_query(&format!("{API}/island/day"), &[("tz", tz), ("day", day)])
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
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Flight {
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Answer {
    /// The day it is for (YYYY-MM-DD).
    pub day: String,
    #[serde(default)]
    pub apps: Sources,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub flights: Vec<Flight>,
}

pub fn parse(body: &str) -> Option<Answer> {
    serde_json::from_str::<Answer>(body).ok().filter(|a| date_parts(&a.day).is_some())
}

/// A meeting link Join may open: https, on Google Meet, Zoom, Microsoft Teams
/// or Webex, with no name, password or port. The server keeps only such links
/// (catat-server `joinLink`); this checks again, so no other link can ever get
/// behind the button.
pub fn join_ok(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains(['@', ':']) || url.len() > 500 || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let host = authority.to_ascii_lowercase();
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    host == "meet.google.com" || under("zoom.us") || host == "teams.microsoft.com" || host == "teams.live.com" || under("webex.com")
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
    /// Oyenza's own: a bill due today.
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// "12:30", "All day", "Due".
    pub when: String,
    pub mark: Mark,
    pub title: String,
    pub detail: String,
    pub join: Option<String>,
}

/// What the Today page draws for the day.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub focus: Option<Focus>,
    /// All-day events, then what's still to come by time, then bills due.
    pub rows: Vec<Row>,
}

impl View {
    pub fn is_empty(&self) -> bool {
        self.focus.is_none() && self.rows.is_empty()
    }
}

fn date_parts(s: &str) -> Option<(i32, u8, u8)> {
    let mut it = s.get(..10)?.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    (s.len() == 10 || s.as_bytes().get(10) == Some(&b' ')).then_some((y, m, d))
}

/// Minutes from `today`'s midnight to "YYYY-MM-DD HH:MM" (negative before it).
fn minutes_from(today: &str, at: &str) -> Option<i32> {
    let (y, m, d) = date_parts(at)?;
    let (ty, tm, td) = date_parts(today)?;
    let time = at.get(11..16)?;
    let (h, mi) = time.split_once(':')?;
    let (h, mi): (i32, i32) = (h.parse().ok()?, mi.parse().ok()?);
    let days = crate::datetime::days_from_civil(y, m, d) - crate::datetime::days_from_civil(ty, tm, td);
    Some(days as i32 * 1440 + h * 60 + mi)
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

/// The day as the page shows it at `now_min` (minutes since the PC's
/// midnight) on `today`. Past events and flights already gone are left out.
/// `bills` with `bills_today` (the server's today for them) add today's
/// bills; their sums stay masked unless `show_amounts`.
pub fn view(answer: &Answer, bills: &[Bill], bills_today: &str, today: &str, now_min: i32, show_amounts: bool) -> View {
    let mut all_day: Vec<Row> = Vec::new();
    let mut timed: Vec<(i32, i32, &Event)> = Vec::new();
    for e in &answer.events {
        let title = one_line(&e.title);
        let title = if title.is_empty() { "Busy".to_string() } else { title };
        if e.all_day {
            let (Some(start), Some(end)) = (date_parts(&e.start), date_parts(&e.end)) else { continue };
            let t = date_parts(today).unwrap_or(start);
            if start <= t && t < end.max((start.0, start.1, start.2 + 1)) {
                all_day.push(Row { when: "All day".into(), mark: Mark::Calendar, title, detail: e.place.as_deref().map(one_line).unwrap_or_default(), join: None });
            }
            continue;
        }
        let (Some(s), Some(en)) = (minutes_from(today, &e.start), minutes_from(today, &e.end)) else { continue };
        if en > now_min && s < 1440 {
            timed.push((s, en.max(s), e));
        }
    }
    timed.sort_by_key(|(s, en, _)| (*s, *en));

    let place_of = |e: &Event| -> Option<String> {
        let p = e.place.as_deref().map(one_line).filter(|p| !p.is_empty());
        p.or_else(|| e.join.as_deref().filter(|j| join_ok(j)).map(|j| meeting_service(j).to_string()))
    };
    // The card: what's on now (the latest to start), else the next within the hour.
    let on_now = timed.iter().rposition(|(s, en, _)| *s <= now_min && now_min < *en);
    let pick = on_now.or_else(|| timed.iter().position(|(s, _, _)| *s > now_min && *s - now_min <= NEXT_WITHIN_MIN));
    let focus = pick.map(|i| {
        let (s, en, e) = timed[i];
        let now = s <= now_min;
        Focus {
            now,
            title: one_line(&e.title),
            when: format!("{}–{}", clock(s.max(0)), clock(en.min(1440))),
            left: if now { format!("{} left", span(en - now_min)) } else { format!("in {}", span(s - now_min)) },
            place: place_of(e),
            join: e.join.clone().filter(|j| join_ok(j)),
        }
    });

    let mut later: Vec<(i32, Row)> = timed
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != pick)
        .map(|(_, (s, _, e))| {
            let when = if *s < 0 { "Earlier".to_string() } else { clock(*s) };
            let row = Row { when, mark: Mark::Calendar, title: one_line(&e.title), detail: place_of(e).unwrap_or_default(), join: e.join.clone().filter(|j| join_ok(j)) };
            (*s, row)
        })
        .collect();
    for f in &answer.flights {
        let at = f.depart.as_deref().and_then(|d| minutes_from(today, &format!("{today} {d}")));
        // A flight that left more than half an hour ago is behind you.
        if at.is_some_and(|m| m + 30 < now_min) {
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
        later.push((at.unwrap_or(1440), Row { when: f.depart.clone().unwrap_or_else(|| "Today".into()), mark: Mark::Gmail, title: format!("Flight to {city}"), detail, join: None }));
    }
    later.sort_by_key(|(at, _)| *at);

    let mut rows = all_day;
    rows.extend(later.into_iter().map(|(_, r)| r));
    if bills_today == today {
        for b in bills.iter().filter(|b| b.due == today) {
            let detail = match (&b.text, b.is_card()) {
                (_, true) => "Card bill".to_string(),
                (Some(t), false) if show_amounts => t.clone(),
                (Some(t), false) => oyenza::masked(t),
                (None, false) => "Renews today".to_string(),
            };
            rows.push(Row { when: "Due".into(), mark: Mark::Bill, title: one_line(&b.name), detail, join: None });
        }
    }
    View { focus, rows }
}

/// The day the self-tests draw (Windows' `selftest`, the Mac's `render`),
/// around the clock they run at: a meeting on Google Meet that started 12
/// minutes ago, lunch later, a call on Zoom, and a flight tonight. What falls
/// past midnight stays out, as it would.
pub fn sample(today: &str, now_min: i32) -> Answer {
    let at = |min: i32| (min < 1440).then(|| format!("{today} {:02}:{:02}", min / 60, min % 60));
    let event = |title: &str, start: i32, len: i32, place: Option<&str>, join: Option<&str>| {
        Some(Event {
            title: title.into(),
            start: at(start.max(0))?,
            end: at((start + len).min(1439))?,
            all_day: false,
            place: place.map(Into::into),
            join: join.map(Into::into),
        })
    };
    let events = [
        event("Product review", now_min - 12, 60, None, Some("https://meet.google.com/abc-defg-hij")),
        event("Lunch with the team", now_min + 138, 60, Some("Senopati"), None),
        event("Content plan check-in", now_min + 288, 30, None, Some("https://acme.zoom.us/j/123")),
    ];
    let depart = now_min + 533;
    let flight = (depart < 1440).then(|| Flight {
        flight: Some("GA408".into()),
        airline: Some("Garuda Indonesia".into()),
        from: "CGK".into(),
        to: "DPS".into(),
        from_city: "Jakarta".into(),
        to_city: "Bali".into(),
        depart: Some(format!("{:02}:{:02}", depart / 60, depart % 60)),
        arrive: None,
        arrive_day: None,
        from_terminal: Some("3".into()),
    });
    Answer {
        day: today.into(),
        apps: Sources { googlecalendar: Source::On, gmail: Source::On },
        events: events.into_iter().flatten().collect(),
        flights: flight.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(title: &str, start: &str, end: &str) -> Event {
        Event { title: title.into(), start: start.into(), end: end.into(), all_day: false, place: None, join: None }
    }

    fn answer(events: Vec<Event>, flights: Vec<Flight>) -> Answer {
        Answer { day: "2026-10-26".into(), apps: Sources { googlecalendar: Source::On, gmail: Source::On }, events, flights }
    }

    const TODAY: &str = "2026-10-26";
    // 10:12
    const NOW: i32 = 10 * 60 + 12;

    #[test]
    fn reads_the_answer() {
        let body = r#"{"day":"2026-10-26","apps":{"googlecalendar":"on","gmail":"not_connected"},
            "events":[{"title":"Product review","start":"2026-10-26 10:00","end":"2026-10-26 11:00","all_day":false,"join":"https://meet.google.com/abc"}],
            "flights":[{"flight":"GA408","airline":"Garuda Indonesia","from":"CGK","to":"DPS","from_city":"Jakarta","to_city":"Bali","depart":"19:05","arrive":"22:00","from_terminal":"3"}]}"#;
        let a = parse(body).expect("reads");
        assert_eq!(a.apps, Sources { googlecalendar: Source::On, gmail: Source::NotConnected });
        assert_eq!(a.events.len(), 1);
        assert_eq!(a.flights[0].to_city, "Bali");
        // Something new from a newer server doesn't break it.
        let a = parse(r#"{"day":"2026-10-26","apps":{"googlecalendar":"sleeping","notion":"on"},"events":[]}"#).expect("reads");
        assert_eq!(a.apps.googlecalendar, Source::Other);
        assert!(parse(r#"{"day":"soon"}"#).is_none());
        assert!(parse("<html>").is_none());
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
    fn the_card_is_the_meeting_on_now_then_the_rest_of_the_day() {
        let mut review = event("Product review", "2026-10-26 10:00", "2026-10-26 11:00");
        review.join = Some("https://meet.google.com/abc".into());
        let mut lunch = event("Lunch with the team", "2026-10-26 12:30", "2026-10-26 13:30");
        lunch.place = Some("Senopati".into());
        let standup = event("Standup", "2026-10-26 09:30", "2026-10-26 09:45");
        let flight = Flight {
            flight: Some("GA408".into()), airline: None, from: "CGK".into(), to: "DPS".into(), from_city: "Jakarta".into(), to_city: "Bali".into(),
            depart: Some("19:05".into()), arrive: Some("22:00".into()), arrive_day: None, from_terminal: Some("3".into()),
        };
        let bills = vec![
            Bill { kind: "subscription".into(), name: "Netflix".into(), due: TODAY.into(), amount: Some(186_000.0), text: Some("Rp 186.000".into()) },
            Bill { kind: "card".into(), name: "Kartu Kredit".into(), due: TODAY.into(), amount: None, text: None },
            Bill { kind: "subscription".into(), name: "Spotify".into(), due: "2026-10-27".into(), amount: None, text: Some("Rp 54.990".into()) },
        ];
        let v = view(&answer(vec![lunch, review, standup], vec![flight]), &bills, TODAY, TODAY, NOW, false);
        let focus = v.focus.expect("a card");
        assert!(focus.now);
        assert_eq!((focus.title.as_str(), focus.when.as_str(), focus.left.as_str()), ("Product review", "10:00–11:00", "48 min left"));
        assert_eq!(focus.place.as_deref(), Some("Google Meet"));
        assert_eq!(focus.join.as_deref(), Some("https://meet.google.com/abc"));
        let rows: Vec<(String, String, String)> = v.rows.iter().map(|r| (r.when.clone(), r.title.clone(), r.detail.clone())).collect();
        assert_eq!(
            rows,
            vec![
                ("12:30".into(), "Lunch with the team".into(), "Senopati".into()),
                ("19:05".into(), "Flight to Bali".into(), "GA408 · CGK T3".into()),
                ("Due".into(), "Netflix".into(), "Rp •••".into()),
                ("Due".into(), "Kartu Kredit".into(), "Card bill".into()),
            ]
        );
        // "Show amounts" shows the sum.
        let shown = view(&answer(vec![], vec![]), &bills, TODAY, TODAY, NOW, true);
        assert_eq!(shown.rows[0].detail, "Rp 186.000");
        // Bills read for another day don't count as today's.
        assert!(view(&answer(vec![], vec![]), &bills, "2026-10-25", TODAY, NOW, false).rows.is_empty());
    }

    #[test]
    fn nothing_on_now_the_next_within_the_hour_is_the_card() {
        let v = view(&answer(vec![event("Design crit", "2026-10-26 10:40", "2026-10-26 11:10")], vec![]), &[], "", TODAY, NOW, false);
        let f = v.focus.expect("a card");
        assert!(!f.now);
        assert_eq!(f.left, "in 28 min");
        assert!(v.rows.is_empty());
        // Further off than an hour: a row, no card.
        let v = view(&answer(vec![event("Dinner", "2026-10-26 19:00", "2026-10-26 21:00")], vec![]), &[], "", TODAY, NOW, false);
        assert!(v.focus.is_none());
        assert_eq!(v.rows[0].when, "19:00");
    }

    #[test]
    fn all_day_first_past_left_out_and_long_ones_cut_to_today() {
        let mut holiday = event("Libur", "2026-10-26", "2026-10-27");
        holiday.all_day = true;
        let mut trip = event("Bali trip", "2026-10-25", "2026-10-28");
        trip.all_day = true;
        let mut tomorrow = event("Tomorrow only", "2026-10-27", "2026-10-28");
        tomorrow.all_day = true;
        let gone = event("Breakfast", "2026-10-26 07:00", "2026-10-26 08:00");
        let overnight = event("Shift", "2026-10-25 22:00", "2026-10-26 11:30");
        let v = view(&answer(vec![gone, holiday, trip, tomorrow, overnight], vec![]), &[], "", TODAY, NOW, false);
        let f = v.focus.expect("the overnight one is on");
        assert_eq!((f.when.as_str(), f.left.as_str()), ("00:00–11:30", "1 h 18 min left"));
        assert_eq!(v.rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), vec!["Libur", "Bali trip"]);
        assert!(v.rows.iter().all(|r| r.when == "All day"));
    }

    #[test]
    fn a_flight_that_left_is_behind_you_and_odd_links_get_no_join() {
        let flight = |depart: &str| Flight {
            flight: None, airline: None, from: "CGK".into(), to: "SIN".into(), from_city: String::new(), to_city: String::new(),
            depart: Some(depart.into()), arrive: None, arrive_day: None, from_terminal: Some("2F".into()),
        };
        let v = view(&answer(vec![], vec![flight("06:00"), flight("09:50"), flight("21:00")]), &[], "", TODAY, NOW, false);
        assert_eq!(v.rows.iter().map(|r| r.when.as_str()).collect::<Vec<_>>(), vec!["09:50", "21:00"]);
        assert_eq!((v.rows[0].title.as_str(), v.rows[0].detail.as_str()), ("Flight to SIN", "CGK 2F"));
        let mut odd = event("Call", "2026-10-26 10:00", "2026-10-26 10:30");
        odd.join = Some("https://example.test/call".into());
        let f = view(&answer(vec![odd], vec![]), &[], "", TODAY, NOW, false).focus.expect("on now");
        assert_eq!((f.join, f.place), (None, None));
    }

    #[test]
    fn the_sample_day_has_a_meeting_on_now_until_late() {
        let v = view(&sample(TODAY, NOW), &[], "", TODAY, NOW, false);
        assert!(v.focus.as_ref().is_some_and(|f| f.now && f.join.is_some()));
        assert_eq!(v.rows.len(), 3);
        // At 22:00 the later ones fall past midnight: the meeting alone.
        let late = view(&sample(TODAY, 22 * 60), &[], "", TODAY, 22 * 60, false);
        assert!(late.focus.is_some() && late.rows.is_empty());
    }

    #[test]
    fn the_address_names_the_zone_and_the_pcs_date() {
        assert_eq!(url("+07:00", "2026-10-26"), "https://api.oyenza.app/island/day?tz=%2B07%3A00&day=2026-10-26");
    }
}
