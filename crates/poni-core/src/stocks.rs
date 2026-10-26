//! Stocks: the IHSG and up to ten stocks on the Indonesia Stock Exchange, for
//! the Stocks add-on's page, from oyenza.trade (Stockmap by Oyenza).
//!
//! oyenza.trade reads the whole exchange every 15 seconds while IDX trades,
//! 10 minutes behind it, as every free IDX feed is (real time needs IDX's data
//! licence through a vendor). The apps ask it three things, and only while
//! the Stocks page is on screen (`services::stocks`):
//!
//! * `watch?s=BBCA,TLKM`: the IHSG and the stocks asked for, each with its
//!   last price, the previous close, the day's open, high and low, volume and
//!   value, the year's range, market cap, and the day's line, a point
//!   every 5 minutes. A few KB, asked again when the answer says a fresher
//!   one is due (`every`: 15 s while IDX trades), with `If-None-Match`, so an
//!   unchanged answer is a 304 without a body.
//! * `names`: every listed stock's code and short name, for the search (once
//!   a run, when the search first opens).
//! * `chart?s=BBCA`: a year of daily closes, for the 1M, 6M and 1Y ranges
//!   (once per stock per run, when one of them is picked).
//!
//! This file reads those answers and keeps the watchlist, and does the page's
//! arithmetic (number formats, the search, where a line's points go), so all
//! of it is tested on any OS.

use crate::datetime::{civil_from_days, days_from_civil, weekday_from_days};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where the answers come from. The apps read `PONI_STOCKS_URL` to point a
/// test build at another copy of the server.
pub const BASE: &str = "https://oyenza.trade/dashboard/api/live/";
/// The most stocks a list holds, besides the IHSG.
pub const MAX: usize = 10;
/// The index's code in answers and on the page. It always comes first.
pub const INDEX: &str = "IHSG";
/// What a new list holds: big, well-known names, so the page shows a market
/// the first time it opens. Each is one click from gone.
pub const STARTERS: [&str; 5] = ["BBCA", "BBRI", "BMRI", "TLKM", "ASII"];
/// The exchange's clock: WIB, UTC+7.
const WIB_S: i64 = 7 * 3600;
/// The minus sign numbers carry (U+2212: as wide as the plus in tabular figures).
const MINUS: char = '\u{2212}';

/// True for an IDX stock code: four letters, upper case.
pub fn is_code(s: &str) -> bool {
    s.len() == 4 && s.bytes().all(|b| b.is_ascii_uppercase())
}

/// What a person typed as a code, if it is one: "bbca " → "BBCA".
pub fn code_of(s: &str) -> Option<String> {
    let c = s.trim().to_ascii_uppercase();
    is_code(&c).then_some(c)
}

// ---------------------------------------------------------------- the list

/// What each row's change pill shows; a click on any pill moves all of them
/// to the next, as in Apple's Stocks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    #[default]
    Percent,
    Points,
    Cap,
}

impl Show {
    pub fn next(self) -> Show {
        match self {
            Show::Percent => Show::Points,
            Show::Points => Show::Cap,
            Show::Cap => Show::Percent,
        }
    }
}

/// The chart's time range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Range {
    /// Today, from the opening bell.
    #[default]
    #[serde(rename = "1d")]
    Day,
    #[serde(rename = "1m")]
    Month,
    #[serde(rename = "6m")]
    HalfYear,
    #[serde(rename = "1y")]
    Year,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Day, Range::Month, Range::HalfYear, Range::Year];

    pub fn label(self) -> &'static str {
        match self {
            Range::Day => "1D",
            Range::Month => "1M",
            Range::HalfYear => "6M",
            Range::Year => "1Y",
        }
    }

    /// How many months back the range reaches (None: today only).
    fn months(self) -> Option<i64> {
        match self {
            Range::Day => None,
            Range::Month => Some(1),
            Range::HalfYear => Some(6),
            Range::Year => Some(12),
        }
    }
}

/// The person's list, kept in data.json (`store.stocks`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watchlist {
    /// The stocks, in the person's order: IDX codes, at most [`MAX`]. The
    /// IHSG isn't one of them; it always leads the page.
    pub codes: Vec<String>,
    /// What the right of the page shows: one of `codes`, or [`INDEX`].
    pub pick: String,
    pub show: Show,
    pub range: Range,
}

impl Default for Watchlist {
    fn default() -> Self {
        Self { codes: STARTERS.iter().map(|c| c.to_string()).collect(), pick: INDEX.to_string(), show: Show::default(), range: Range::default() }
    }
}

impl Watchlist {
    pub fn full(&self) -> bool {
        self.codes.len() >= MAX
    }

    pub fn has(&self, code: &str) -> bool {
        self.codes.iter().any(|c| c == code)
    }

    /// Puts `code` at the end of the list and shows it. False (and nothing
    /// changes) when it isn't a code, is there already, or the list is full.
    pub fn add(&mut self, code: &str) -> bool {
        let Some(code) = code_of(code) else { return false };
        if self.has(&code) || self.full() {
            return false;
        }
        self.pick = code.clone();
        self.codes.push(code);
        true
    }

    /// Takes `code` off the list; the IHSG shows if it was the one showing.
    pub fn remove(&mut self, code: &str) -> bool {
        let before = self.codes.len();
        self.codes.retain(|c| c != code);
        if self.pick == code {
            self.pick = INDEX.to_string();
        }
        self.codes.len() != before
    }

    /// Moves `code` to place `to` (0 = first after the IHSG).
    pub fn move_to(&mut self, code: &str, to: usize) {
        if let Some(i) = self.codes.iter().position(|c| c == code) {
            let c = self.codes.remove(i);
            self.codes.insert(to.min(self.codes.len()), c);
        }
    }

    /// The page's rows: the IHSG, then the stocks.
    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        std::iter::once(INDEX).chain(self.codes.iter().map(String::as_str))
    }

    /// A list read from an older or hand-edited file: codes made upper case,
    /// anything that isn't one dropped, no repeats, at most [`MAX`]; a pick
    /// that's gone becomes the IHSG.
    pub fn tidy(&mut self) {
        let mut seen = Vec::new();
        for c in self.codes.drain(..) {
            if let Some(c) = code_of(&c) {
                if !seen.contains(&c) && seen.len() < MAX {
                    seen.push(c);
                }
            }
        }
        self.codes = seen;
        if self.pick != INDEX && !self.has(&self.pick) {
            self.pick = INDEX.to_string();
        }
    }
}

// ---------------------------------------------------------------- answers

/// Where the exchange's day stands, as the answer says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Before the first session.
    Pre,
    Open,
    /// Between the two sessions.
    Break,
    /// The closing auction.
    Closing,
    #[default]
    Closed,
}

impl Phase {
    fn parse(s: &str) -> Phase {
        match s {
            "pre" => Phase::Pre,
            "open" => Phase::Open,
            "break" => Phase::Break,
            "closing" => Phase::Closing,
            _ => Phase::Closed,
        }
    }

    /// Prices move: the line grows while the page watches.
    pub fn trading(self) -> bool {
        matches!(self, Phase::Open | Phase::Closing)
    }
}

/// One row of an answer: the IHSG or a stock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quote {
    pub code: String,
    /// The short name oyenza.trade shows ("Bank BCA"); the code when it has none.
    pub name: String,
    pub price: f64,
    /// The previous close, which the day's change is from.
    pub prev: f64,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    /// Shares traded today (stocks only).
    pub volume: Option<f64>,
    /// Rupiah traded today (stocks only).
    pub value: Option<f64>,
    /// The last 52 weeks' low and high.
    pub w52: Option<(f64, f64)>,
    /// Market cap in rupiah at today's price (stocks only). The answers
    /// carry no P/E: they need no sign-in, and oyenza.trade keeps what it
    /// reads from companies' statements behind it.
    pub mcap: Option<f64>,
    /// The day's line: (minute of the exchange's day, WIB; price), oldest
    /// first, a point every 5 minutes and the last at the price's own time.
    pub spark: Vec<(u16, f64)>,
}

/// Which way a price went since the previous close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trend {
    Up,
    Down,
    Flat,
}

impl Quote {
    pub fn is_index(&self) -> bool {
        self.code == INDEX
    }

    pub fn change(&self) -> f64 {
        self.price - self.prev
    }

    pub fn change_pct(&self) -> Option<f64> {
        (self.prev > 0.0).then(|| (self.price / self.prev - 1.0) * 100.0)
    }

    pub fn trend(&self) -> Trend {
        trend_of(self.price, self.prev)
    }

    /// What the change pill says for `show`. The IHSG has no market cap, so
    /// its pill keeps the change in points there.
    pub fn pill(&self, show: Show) -> String {
        match show {
            Show::Percent => self.change_pct().map_or_else(|| "\u{2014}".to_string(), fmt_pct),
            Show::Points => fmt_change(self.change(), self.is_index()),
            Show::Cap => match self.mcap {
                Some(m) => fmt_cap(m),
                None if self.is_index() => fmt_change(self.change(), true),
                None => "\u{2014}".to_string(),
            },
        }
    }
}

/// Which way `now` went from `base`, to the price's own precision.
pub fn trend_of(now: f64, base: f64) -> Trend {
    let d = now - base;
    if d.abs() < 0.005 {
        Trend::Flat
    } else if d > 0.0 {
        Trend::Up
    } else {
        Trend::Down
    }
}

/// One `watch` answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// The answer's version, sent back as `If-None-Match`.
    pub tag: String,
    /// When oyenza.trade read the prices (Unix seconds)...
    pub at: i64,
    /// ...and the exchange's time they are from (10 minutes earlier).
    pub t: i64,
    /// The trading day they are from, "2026-10-27".
    pub day: String,
    pub phase: Phase,
    /// How far behind the exchange the prices run (seconds).
    pub delay_s: i64,
    /// Seconds until a fresher answer is due.
    pub every_s: u64,
    /// The feed has paused (oyenza.trade hasn't read the exchange for a while).
    pub stale: bool,
    /// The day's sessions, first open to last close (minutes of the day, WIB):
    /// the day line's width.
    pub span: (u16, u16),
    /// The IHSG first, then the stocks in the order asked.
    pub rows: Vec<Quote>,
    /// Codes asked for that aren't listed.
    pub missing: Vec<String>,
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn pair(v: &Value) -> Option<(f64, f64)> {
    let a = v.as_array()?;
    Some((num(a.first()?)?, num(a.get(1)?)?))
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().trim().to_string()
}

impl Quote {
    fn parse(v: &Value) -> Option<Quote> {
        let code = text(&v["s"]);
        if code.is_empty() {
            return None;
        }
        let price = num(&v["p"])?;
        let name = Some(text(&v["n"])).filter(|n| !n.is_empty()).unwrap_or_else(|| code.clone());
        let mut spark: Vec<(u16, f64)> = v["spark"]
            .as_array()
            .map(|a| a.iter().filter_map(|p| Some((num(&p[0])?.clamp(0.0, 1439.0) as u16, num(&p[1])?))).collect())
            .unwrap_or_default();
        spark.sort_by_key(|p| p.0);
        Some(Quote {
            name,
            price,
            prev: num(&v["pc"]).unwrap_or(price),
            open: num(&v["o"]),
            high: num(&v["h"]),
            low: num(&v["l"]),
            volume: num(&v["v"]),
            value: num(&v["val"]),
            w52: pair(&v["w52"]),
            mcap: num(&v["mcap"]),
            spark,
            code,
        })
    }
}

impl Board {
    /// A `watch` answer; None for anything that isn't one.
    pub fn parse(json: &str) -> Option<Board> {
        let v: Value = serde_json::from_str(json).ok()?;
        let rows: Vec<Quote> = v["rows"].as_array()?.iter().filter_map(Quote::parse).collect();
        if rows.is_empty() {
            return None;
        }
        let span = pair(&v["span"]).map(|(a, b)| (a.clamp(0.0, 1439.0) as u16, b.clamp(0.0, 1439.0) as u16)).filter(|(a, b)| b > a).unwrap_or((540, 960));
        Some(Board {
            tag: text(&v["tag"]),
            at: v["at"].as_i64().unwrap_or(0),
            t: v["t"].as_i64().unwrap_or(0),
            day: text(&v["day"]),
            phase: Phase::parse(v["state"].as_str().unwrap_or_default()),
            delay_s: v["delay"].as_i64().unwrap_or(600),
            every_s: v["every"].as_u64().unwrap_or(15),
            stale: v["stale"].as_bool().unwrap_or(false),
            span,
            rows,
            missing: v["missing"].as_array().map(|a| a.iter().map(text).collect()).unwrap_or_default(),
        })
    }

    pub fn quote(&self, code: &str) -> Option<&Quote> {
        self.rows.iter().find(|q| q.code == code)
    }

    /// The line under the chart: where the exchange stands and how late the
    /// prices are. `now`: Unix seconds.
    pub fn status(&self, now: i64) -> String {
        let delay = (self.delay_s / 60).max(1);
        let today = wib_day(now);
        // Prices from an earlier day (overnight, a weekend) say which.
        let earlier = day_num(&self.day).filter(|d| *d != today).map(|d| {
            if today - d < 7 {
                WEEKDAYS[weekday_from_days(d) as usize].to_string()
            } else {
                let (_, m, dd) = civil_from_days(d);
                format!("{dd} {}", MONTHS[(m - 1) as usize])
            }
        });
        match (self.phase, earlier) {
            _ if self.stale => format!("Paused at {}", wib_clock(self.t)),
            (Phase::Open | Phase::Closing, _) => format!("Open \u{b7} {delay} min behind IDX"),
            (Phase::Break, _) => "Lunch break".to_string(),
            (Phase::Pre, None) => "Opens soon".to_string(),
            (Phase::Pre, Some(d)) => format!("Opens at 09:00 \u{b7} prices from {d}"),
            (Phase::Closed, None) => "Closed".to_string(),
            (Phase::Closed, Some(d)) => format!("Closed \u{b7} prices from {d}"),
        }
    }
}

/// "2026-10-27" as days since 1970-01-01 (an answer's `day`).
pub fn day_of(s: &str) -> Option<i64> {
    day_num(s)
}

/// "2026-10-27" as days since 1970-01-01.
fn day_num(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let y = it.next()?.parse().ok()?;
    let m: u8 = it.next()?.parse().ok()?;
    let d: u8 = it.next()?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// The exchange's day at Unix time `t`.
fn wib_day(t: i64) -> i64 {
    (t + WIB_S).div_euclid(86_400)
}

/// The exchange's clock at Unix time `t`: "14:32".
pub fn wib_clock(t: i64) -> String {
    let s = (t + WIB_S).rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "24 Oct 2025": a date under the chart.
pub fn date_label(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// Minute `m` of the day as "14:05".
pub fn minute_label(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Every listed stock's code and short name: what the search looks through.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Names {
    pub tag: String,
    /// (code, name), by code.
    pub list: Vec<(String, String)>,
}

impl Names {
    pub fn parse(json: &str) -> Option<Names> {
        let v: Value = serde_json::from_str(json).ok()?;
        let list: Vec<(String, String)> = v["names"]
            .as_array()?
            .iter()
            .filter_map(|p| {
                let code = text(&p[0]);
                is_code(&code).then(|| {
                    let name = text(&p[1]);
                    (code.clone(), if name.is_empty() { code } else { name })
                })
            })
            .collect();
        (!list.is_empty()).then(|| Names { tag: text(&v["tag"]), list })
    }

    pub fn name(&self, code: &str) -> Option<&str> {
        self.list.iter().find(|(c, _)| c == code).map(|(_, n)| n.as_str())
    }

    /// Up to `n` stocks for what was typed, best first: the code itself,
    /// codes that start with it, names that start with it, a word of a name
    /// that does, then codes and names that hold it anywhere. Stocks in
    /// `skip` (already on the list) are left out.
    pub fn search(&self, query: &str, skip: &[String], n: usize) -> Vec<(String, String)> {
        let q = fold(query);
        if q.is_empty() {
            return Vec::new();
        }
        let upper = q.to_ascii_uppercase();
        let mut hits: Vec<(u8, usize)> = self
            .list
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| !skip.contains(c))
            .filter_map(|(i, (code, name))| {
                let name = fold(name);
                let rank = if *code == upper {
                    0
                } else if code.starts_with(&upper) {
                    1
                } else if name.starts_with(&q) {
                    2
                } else if name.split(' ').any(|w| w.starts_with(&q)) {
                    3
                } else if code.contains(&upper) {
                    4
                } else if name.contains(&q) {
                    5
                } else {
                    return None;
                };
                Some((rank, i))
            })
            .collect();
        hits.sort();
        hits.into_iter().take(n).map(|(_, i)| self.list[i].clone()).collect()
    }
}

/// Lower case, letters and digits, words split by single spaces.
fn fold(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' }).collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A year of one stock's (or the IHSG's) daily closes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub code: String,
    /// Trading days (days since 1970-01-01), oldest first, each with its close.
    pub days: Vec<i64>,
    pub closes: Vec<f64>,
}

impl History {
    pub fn parse(json: &str) -> Option<History> {
        let v: Value = serde_json::from_str(json).ok()?;
        let code = text(&v["s"]);
        let (d, c) = (v["d"].as_array()?, v["c"].as_array()?);
        let mut pts: Vec<(i64, f64)> = d.iter().zip(c).filter_map(|(d, c)| Some((day_num(d.as_str()?)?, num(c)?))).collect();
        pts.sort_by_key(|p| p.0);
        pts.dedup_by_key(|p| p.0);
        (!code.is_empty() && pts.len() >= 2).then(|| History { code, days: pts.iter().map(|p| p.0).collect(), closes: pts.iter().map(|p| p.1).collect() })
    }

    /// The closes `range` covers: back to the same date `range` months
    /// before the last one. Empty for [`Range::Day`].
    pub fn window(&self, range: Range) -> (&[i64], &[f64]) {
        let (Some(months), Some(&last)) = (range.months(), self.days.last()) else { return (&[], &[]) };
        let (y, m, d) = civil_from_days(last);
        let back = y as i64 * 12 + (m as i64 - 1) - months;
        let from = days_from_civil(back.div_euclid(12) as i32, (back.rem_euclid(12) + 1) as u8, d.min(28));
        let i = self.days.partition_point(|&x| x < from);
        (&self.days[i..], &self.closes[i..])
    }
}

// ---------------------------------------------------------------- numbers

/// `n` with commas between thousands: 1234567 → "1,234,567".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A price: stocks trade in whole rupiah ("4,940"), the IHSG to two places
/// ("5,373.09").
pub fn fmt_price(p: f64, index: bool) -> String {
    if !p.is_finite() {
        return "\u{2014}".to_string();
    }
    let neg = if p < 0.0 { MINUS.to_string() } else { String::new() };
    let p = p.abs();
    if index {
        let cents = (p * 100.0).round() as u64;
        format!("{neg}{}.{:02}", group(cents / 100), cents % 100)
    } else {
        format!("{neg}{}", group(p.round() as u64))
    }
}

fn sign(d: f64, zero: f64) -> String {
    if d.abs() < zero {
        String::new()
    } else if d > 0.0 {
        "+".to_string()
    } else {
        MINUS.to_string()
    }
}

/// A change in price: "+40", "−65.16".
pub fn fmt_change(d: f64, index: bool) -> String {
    let zero = if index { 0.005 } else { 0.5 };
    format!("{}{}", sign(d, zero), fmt_price(d.abs(), index))
}

/// A change in percent: "+0.82%", "−1.21%", "0.00%".
pub fn fmt_pct(pct: f64) -> String {
    format!("{}{:.2}%", sign(pct, 0.005), pct.abs())
}

/// A large number the short way: "607T", "26.7T", "850B", "94.1M", "8.2K".
/// T, B, M and K are trillion, billion, million and thousand.
pub fn fmt_big(v: f64) -> String {
    if !v.is_finite() {
        return "\u{2014}".to_string();
    }
    let a = v.abs();
    let (x, unit) = match a {
        a if a >= 1e12 => (a / 1e12, "T"),
        a if a >= 1e9 => (a / 1e9, "B"),
        a if a >= 1e6 => (a / 1e6, "M"),
        a if a >= 1e3 => (a / 1e3, "K"),
        a => (a, ""),
    };
    let x = if x >= 100.0 { format!("{x:.0}") } else if x >= 10.0 { format!("{x:.1}") } else { format!("{x:.2}") };
    // "26.0T" reads better as "26T".
    let x = if x.contains('.') { x.trim_end_matches('0').trim_end_matches('.').to_string() } else { x };
    format!("{}{x}{unit}", if v < 0.0 { MINUS.to_string() } else { String::new() })
}

/// Rupiah the short way: "Rp607T".
pub fn fmt_cap(v: f64) -> String {
    format!("Rp{}", fmt_big(v))
}

// ---------------------------------------------------------------- pacing

/// Seconds until the watch is asked again: when the last answer said a
/// fresher one is due, within 15 s and 10 minutes; after `failures` failed
/// asks in a row, 15 s doubled each time, up to 10 minutes.
pub fn next_ask(every_s: Option<u64>, failures: u32) -> u64 {
    if failures > 0 {
        return (15u64 << (failures - 1).min(6)).min(600);
    }
    every_s.unwrap_or(15).clamp(15, 600)
}

pub fn watch_url(base: &str, codes: &[String]) -> String {
    format!("{base}watch?s={}", codes.join(","))
}

pub fn names_url(base: &str) -> String {
    format!("{base}names")
}

pub fn chart_url(base: &str, code: &str) -> String {
    format!("{base}chart?s={code}")
}

// ---------------------------------------------------------------- lines

/// The values a line is drawn between: its lowest and highest, the previous
/// close (`base`) included, with room above and below so the line never
/// touches the edges. A flat line gets a band around it.
pub fn bounds(values: impl IntoIterator<Item = f64>, base: Option<f64>) -> Option<(f64, f64)> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in values.into_iter().chain(base).filter(|v| v.is_finite()) {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if lo > hi {
        return None;
    }
    let pad = ((hi - lo) * 0.08).max(hi.abs() * 0.0015).max(1e-6);
    Some((lo - pad, hi + pad))
}

/// Where minute `m` of the exchange's day sits across the day's sessions, 0..1.
pub fn day_x(m: u16, span: (u16, u16)) -> f32 {
    let (a, b) = (f32::from(span.0), f32::from(span.1));
    ((f32::from(m) - a) / (b - a).max(1.0)).clamp(0.0, 1.0)
}

/// `n` points along the line through `pts` (sorted by x), evenly spaced from
/// its first x to its last: the same number for every line, so the page can
/// morph one into the next point for point.
pub fn resample(pts: &[(f32, f32)], n: usize) -> Vec<(f32, f32)> {
    match pts {
        [] => Vec::new(),
        [p] => vec![*p; n],
        _ => {
            let (x0, x1) = (pts[0].0, pts[pts.len() - 1].0);
            let mut j = 0;
            (0..n)
                .map(|i| {
                    let x = x0 + (x1 - x0) * i as f32 / (n - 1).max(1) as f32;
                    while j + 2 < pts.len() && pts[j + 1].0 < x {
                        j += 1;
                    }
                    let (a, b) = (pts[j], pts[j + 1]);
                    let t = if b.0 > a.0 { ((x - a.0) / (b.0 - a.0)).clamp(0.0, 1.0) } else { 1.0 };
                    (x, a.1 + (b.1 - a.1) * t)
                })
                .collect()
        }
    }
}

// ---------------------------------------------------------------- samples

/// A made-up answer for the self-test's and the Mac renders' pictures: the
/// IHSG and the starter stocks at minute `now_min` of a trading day, each
/// with the day's line so far. The prices are invented, near the levels of
/// late October 2026.
pub fn sample(day: &str, now_min: u16) -> Board {
    let now_min = now_min.clamp(545, 960);
    let rows: [(&str, &str, f64, f64, f64); 6] = [
        // code, name, previous close, close now, market cap at that close
        (INDEX, "IDX Composite", 5_439.25, 5_473.09, 0.0),
        ("BBCA", "Bank BCA", 4_900.0, 4_940.0, 606.8e12),
        ("BBRI", "Bank BRI", 2_910.0, 2_950.0, 438.5e12),
        ("BMRI", "Bank Mandiri", 3_900.0, 3_870.0, 364.0e12),
        ("TLKM", "Telkom", 2_240.0, 2_250.0, 221.8e12),
        ("ASII", "Astra", 4_550.0, 4_490.0, 181.8e12),
    ];
    let rows = rows
        .iter()
        .enumerate()
        .map(|(i, &(code, name, prev, last, mcap))| {
            let index = code == INDEX;
            let tick = if index { 0.01 } else if prev < 5_000.0 { 10.0 } else { 25.0 };
            // From the previous close to `last`, wandering as a day's line
            // does: each step keeps most of the last one's lean (seeded by the row).
            let mut seed = 0x9E37_79B9u32.wrapping_mul(i as u32 + 7);
            let mut spark = Vec::new();
            let steps = (now_min - 540) / 5;
            let swing = (last - prev).abs().max(prev * 0.004);
            let mut lean = 0.0;
            for k in 0..=steps {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                lean = lean * 0.82 + (f64::from(seed % 1000) / 1000.0 - 0.5) * swing * 0.35;
                let f = f64::from(k) / f64::from(steps.max(1));
                let p = prev + (last - prev) * f.powf(0.8) + if k == steps { 0.0 } else { lean * (1.0 - f * 0.5) };
                let m = if k == steps { now_min } else { 540 + k * 5 };
                spark.push((m, (p / tick).round() * tick));
            }
            let (lo, hi) = spark.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
            Quote {
                code: code.to_string(),
                name: name.to_string(),
                price: last,
                prev,
                open: spark.first().map(|p| p.1),
                high: Some(hi),
                low: Some(lo),
                volume: (!index).then_some(48_200_000.0 * (i as f64 + 1.0)),
                value: (!index).then_some(48_200_000.0 * (i as f64 + 1.0) * last),
                w52: Some(if index { (4_907.37, 9_174.47) } else { (prev * 0.78, prev * 1.31) }),
                mcap: (!index).then_some(mcap),
                spark,
            }
        })
        .collect();
    Board {
        tag: "sample".into(),
        at: 0,
        t: 0,
        day: day.to_string(),
        phase: if now_min < 960 { Phase::Open } else { Phase::Closed },
        delay_s: 600,
        every_s: 15,
        stale: false,
        span: (540, 960),
        rows,
        missing: Vec::new(),
    }
}

/// A made-up year of daily closes for `code`, ending at `last` on day `today`.
pub fn sample_history(code: &str, today: i64, last: f64) -> History {
    let mut seed = code.bytes().fold(0x2545_F491u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let (mut days, mut closes) = (Vec::new(), Vec::new());
    let mut d = today - 365;
    let mut p = last * 0.86;
    while d <= today {
        if !matches!(weekday_from_days(d), 0 | 6) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            p *= 1.0 + (f64::from(seed % 1000) / 1000.0 - 0.47) * 0.03;
            days.push(d);
            closes.push(p);
        }
        d += 1;
    }
    // Ends on today's close.
    let k = last / closes.last().copied().unwrap_or(last);
    let n = closes.len().max(1) as f64;
    for (i, c) in closes.iter_mut().enumerate() {
        *c *= 1.0 + (k - 1.0) * (i as f64 / n);
    }
    if let Some(c) = closes.last_mut() {
        *c = last;
    }
    History { code: code.to_string(), days, closes }
}

/// A few listed stocks, for the search's pictures.
pub fn sample_names() -> Names {
    let list = [
        ("ADRO", "Alamtri Resources"),
        ("AMMN", "Amman Mineral"),
        ("ANTM", "Antam"),
        ("ASII", "Astra"),
        ("BBCA", "Bank BCA"),
        ("BBNI", "Bank BNI"),
        ("BBRI", "Bank BRI"),
        ("BBTN", "Bank BTN"),
        ("BMRI", "Bank Mandiri"),
        ("BRIS", "Bank Syariah Indonesia"),
        ("GOTO", "GoTo Gojek Tokopedia"),
        ("ICBP", "Indofood CBP"),
        ("TLKM", "Telkom"),
        ("UNVR", "Unilever Indonesia"),
    ];
    Names { tag: "sample".into(), list: list.iter().map(|(c, n)| (c.to_string(), n.to_string())).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WATCH: &str = r#"{"v":1,"tag":"9f2c","at":1793045721,"t":1793045121,"day":"2026-10-26","state":"open","delay":600,"every":15,"stale":false,"span":[540,969],
        "rows":[
          {"s":"IHSG","n":"IDX Composite","p":5373.09,"pc":5439.25,"o":5442.3,"h":5453.65,"l":5349.75,"w52":[4907.37,9174.47],"spark":[[540,5440.59],[545,5432.8],[600,5373.09]]},
          {"s":"BBCA","n":"Bank BCA","p":4940,"pc":4900,"o":5025,"h":5075,"l":4940,"v":353641200,"val":1740086927264,"w52":[4380,6550],"mcap":606838278062080,"spark":[[545,5000],[540,5025]]},
          {"s":"GOTO","p":25,"pc":25,"spark":[]},
          {"n":"no code","p":1}
        ],"missing":["ZZZZ"]}"#;

    #[test]
    fn reads_a_watch_answer() {
        let b = Board::parse(WATCH).expect("an answer");
        assert_eq!((b.tag.as_str(), b.phase, b.every_s, b.span), ("9f2c", Phase::Open, 15, (540, 969)));
        assert_eq!(b.rows.len(), 3, "a row without a code is left out");
        let ihsg = &b.rows[0];
        assert!(ihsg.is_index());
        assert_eq!(ihsg.w52, Some((4907.37, 9174.47)));
        assert_eq!(ihsg.mcap, None);
        let bbca = b.quote("BBCA").expect("BBCA");
        assert_eq!(bbca.spark, vec![(540, 5025.0), (545, 5000.0)], "points sorted by minute");
        assert_eq!(bbca.trend(), Trend::Up);
        assert_eq!(bbca.pill(Show::Percent), "+0.82%");
        assert_eq!(bbca.pill(Show::Points), "+40");
        assert_eq!(bbca.pill(Show::Cap), "Rp607T");
        assert_eq!(ihsg.pill(Show::Cap), "\u{2212}66.16", "the IHSG has no market cap: its change instead");
        let goto = b.quote("GOTO").expect("GOTO");
        assert_eq!((goto.name.as_str(), goto.trend(), goto.pill(Show::Percent).as_str()), ("GOTO", Trend::Flat, "0.00%"));
        assert_eq!(b.missing, vec!["ZZZZ".to_string()]);
        assert_eq!(Board::parse("{}"), None);
        assert_eq!(Board::parse(r#"{"rows":[]}"#), None);
        assert_eq!(Board::parse("not json"), None);
    }

    #[test]
    fn status_says_where_the_exchange_stands() {
        let mut b = Board::parse(WATCH).expect("an answer");
        // 26 Oct 2026, 13:15 WIB.
        let now = days_from_civil(2026, 10, 26) * 86_400 + 6 * 3600 + 15 * 60;
        assert_eq!(b.status(now), "Open \u{b7} 10 min behind IDX");
        b.phase = Phase::Break;
        assert_eq!(b.status(now), "Lunch break");
        b.phase = Phase::Closed;
        assert_eq!(b.status(now), "Closed");
        // The next morning, before the bell: the prices are yesterday's.
        assert_eq!(b.status(now + 86_400 - 6 * 3600), "Closed \u{b7} prices from Mon");
        b.phase = Phase::Pre;
        assert_eq!(b.status(now + 86_400 - 6 * 3600), "Opens at 09:00 \u{b7} prices from Mon");
        assert_eq!(b.status(now + 12 * 86_400), "Opens at 09:00 \u{b7} prices from 26 Oct", "after a long holiday");
        b.stale = true;
        b.phase = Phase::Open;
        assert_eq!(b.status(now), format!("Paused at {}", wib_clock(b.t)));
        assert_eq!(wib_clock(days_from_civil(2026, 10, 26) * 86_400 + 2 * 3600 + 5 * 60), "09:05");
    }

    #[test]
    fn numbers_read_like_the_exchange_writes_them() {
        assert_eq!(fmt_price(4940.0, false), "4,940");
        assert_eq!(fmt_price(179_975.0, false), "179,975");
        assert_eq!(fmt_price(25.0, false), "25");
        assert_eq!(fmt_price(5373.094, true), "5,373.09");
        assert_eq!(fmt_price(12_345_678.9, true), "12,345,678.90");
        assert_eq!(fmt_change(-65.16, true), "\u{2212}65.16");
        assert_eq!(fmt_change(40.0, false), "+40");
        assert_eq!(fmt_change(0.2, false), "0");
        assert_eq!(fmt_pct(0.8163), "+0.82%");
        assert_eq!(fmt_pct(-1.2149), "\u{2212}1.21%");
        assert_eq!(fmt_pct(0.001), "0.00%");
        assert_eq!(fmt_big(606.8e12), "607T");
        assert_eq!(fmt_big(26.66e12), "26.7T");
        assert_eq!(fmt_big(26.0e12), "26T");
        assert_eq!(fmt_big(850e9), "850B");
        assert_eq!(fmt_big(1.74e12), "1.74T");
        assert_eq!(fmt_big(353_641_200.0), "354M");
        assert_eq!(fmt_big(8_200.0), "8.2K");
        assert_eq!(fmt_big(512.0), "512");
        assert_eq!(fmt_cap(221.8e12), "Rp222T");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(999), "999");
    }

    #[test]
    fn the_list_holds_ten_stocks_each_once() {
        let mut w = Watchlist::default();
        assert_eq!(w.codes, STARTERS.map(String::from).to_vec());
        assert_eq!(w.pick, INDEX);
        assert!(w.add(" goto "), "typed in lower case with spaces");
        assert_eq!(w.pick, "GOTO", "a new stock shows");
        assert!(!w.add("GOTO"), "already there");
        assert!(!w.add("IHSG1") && !w.add("BB") && !w.add("BB CA"), "not codes");
        for c in ["ANTM", "BBNI", "ADRO", "ICBP"] {
            assert!(w.add(c));
        }
        assert!(w.full());
        assert!(!w.add("UNVR"), "ten is the most");
        assert_eq!(w.symbols().next(), Some(INDEX));
        assert_eq!(w.symbols().count(), 11);
        w.move_to("ICBP", 0);
        assert_eq!(w.codes[0], "ICBP");
        w.move_to("ICBP", 99);
        assert_eq!(w.codes[9], "ICBP");
        w.pick = "ANTM".into();
        assert!(w.remove("ANTM"));
        assert_eq!(w.pick, INDEX, "the IHSG shows once the stock showing is gone");
        assert!(!w.remove("ANTM"));
        assert!(!w.full());
    }

    #[test]
    fn a_hand_edited_list_is_tidied() {
        let mut w: Watchlist = serde_json::from_str(r#"{"codes":["bbca","BBCA","x","TLKM","ab cd"],"pick":"UNVR","show":"cap","range":"6m"}"#).expect("a list");
        w.tidy();
        assert_eq!(w.codes, vec!["BBCA".to_string(), "TLKM".to_string()]);
        assert_eq!((w.pick.as_str(), w.show, w.range), (INDEX, Show::Cap, Range::HalfYear));
        let w: Watchlist = serde_json::from_str("{}").expect("an empty object");
        assert_eq!(w, Watchlist::default(), "a file from before the add-on starts with the starters");
        assert_eq!(serde_json::to_string(&Range::Year).expect("json"), "\"1y\"");
        assert_eq!(Show::Cap.next(), Show::Percent);
    }

    #[test]
    fn search_finds_codes_then_names() {
        let n = sample_names();
        let codes = |v: Vec<(String, String)>| v.into_iter().map(|p| p.0).collect::<Vec<_>>();
        assert_eq!(codes(n.search("bbca", &[], 5)), vec!["BBCA"]);
        assert_eq!(codes(n.search("BB", &[], 3)), vec!["BBCA", "BBNI", "BBRI"]);
        assert_eq!(codes(n.search("bank", &[], 9)), vec!["BBCA", "BBNI", "BBRI", "BBTN", "BMRI", "BRIS"]);
        assert_eq!(codes(n.search("mandiri", &[], 9)), vec!["BMRI"], "a word of the name");
        assert_eq!(codes(n.search("tokopedia", &[], 9)), vec!["GOTO"]);
        assert_eq!(codes(n.search("BB", &["BBCA".to_string()], 2)), vec!["BBNI", "BBRI"], "already on the list");
        assert!(n.search("  ", &[], 5).is_empty());
        assert_eq!(n.name("TLKM"), Some("Telkom"));
        let parsed = Names::parse(r#"{"v":1,"tag":"a1","names":[["BBCA","Bank BCA"],["bad",""],["TLKM",""]]}"#).expect("names");
        assert_eq!(parsed.list, vec![("BBCA".into(), "Bank BCA".into()), ("TLKM".into(), "TLKM".into())]);
        assert_eq!(Names::parse(r#"{"names":[]}"#), None);
    }

    #[test]
    fn a_year_of_closes_is_cut_to_the_range() {
        let h = History::parse(r#"{"s":"BBCA","d":["2025-10-27","2026-04-27","2026-09-25","2026-09-28","2026-10-26","2026-10-26"],"c":[6500,5800,5000,4980,4940,1]}"#).expect("history");
        assert_eq!(h.days.len(), 5, "a repeated day counts once");
        assert_eq!(h.window(Range::Year).1.len(), 5);
        assert_eq!(h.window(Range::HalfYear).1, &[5800.0, 5000.0, 4980.0, 4940.0]);
        assert_eq!(h.window(Range::Month).1, &[4980.0, 4940.0]);
        assert!(h.window(Range::Day).0.is_empty());
        assert_eq!(History::parse(r#"{"s":"BBCA","d":["2026-10-26"],"c":[1]}"#), None, "one close is no line");
        let s = sample_history("BBCA", days_from_civil(2026, 10, 26), 4940.0);
        assert_eq!(s.closes.last(), Some(&4940.0));
        assert!(s.days.len() > 250 && s.days.iter().all(|d| !matches!(weekday_from_days(*d), 0 | 6)));
    }

    #[test]
    fn asks_as_often_as_the_answer_says() {
        assert_eq!(next_ask(Some(15), 0), 15);
        assert_eq!(next_ask(Some(5), 0), 15, "never more than 4 a minute");
        assert_eq!(next_ask(Some(600), 0), 600);
        assert_eq!(next_ask(Some(86_400), 0), 600);
        assert_eq!(next_ask(None, 0), 15);
        assert_eq!(next_ask(Some(15), 1), 15);
        assert_eq!(next_ask(Some(15), 3), 60);
        assert_eq!(next_ask(Some(15), 30), 600);
        assert_eq!(watch_url(BASE, &["BBCA".into(), "TLKM".into()]), "https://oyenza.trade/dashboard/api/live/watch?s=BBCA,TLKM");
        assert_eq!(chart_url(BASE, "IHSG"), "https://oyenza.trade/dashboard/api/live/chart?s=IHSG");
    }

    #[test]
    fn lines_fit_their_box() {
        let (lo, hi) = bounds([10.0, 12.0], Some(9.0)).expect("bounds");
        assert!(lo < 9.0 && hi > 12.0, "the previous close is inside");
        let (lo, hi) = bounds([5.0, 5.0], None).expect("a flat line");
        assert!(lo < 5.0 && hi > 5.0);
        assert_eq!(bounds([f64::NAN], None), None);
        assert_eq!(day_x(540, (540, 960)), 0.0);
        assert_eq!(day_x(750, (540, 960)), 0.5);
        assert_eq!(day_x(1000, (540, 960)), 1.0);
        let r = resample(&[(0.0, 0.0), (1.0, 10.0), (3.0, 10.0)], 4);
        assert_eq!(r, vec![(0.0, 0.0), (1.0, 10.0), (2.0, 10.0), (3.0, 10.0)]);
        assert_eq!(resample(&[(2.0, 7.0)], 3), vec![(2.0, 7.0); 3]);
        assert!(resample(&[], 3).is_empty());
    }

    #[test]
    fn the_sample_is_a_trading_day() {
        let b = sample("2026-10-26", 13 * 60 + 40);
        assert_eq!(b.rows.len(), 6);
        assert_eq!(b.rows[0].code, INDEX);
        for q in &b.rows {
            assert_eq!(q.spark.last().map(|p| p.1), Some(q.price), "{}: the line ends at the price", q.code);
            assert_eq!(q.spark.first().map(|p| p.0), Some(540));
        }
        assert_eq!(b.phase, Phase::Open);
        assert_eq!(sample("2026-10-26", 17 * 60).phase, Phase::Closed);
    }
}
