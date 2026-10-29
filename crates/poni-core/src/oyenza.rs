//! Oyenza (oyenza.app), optional: what the island shows from a person's money
//! app once they sign in from the tray, and the words it uses.
//!
//! Signing in is a device code, the way a TV signs in: the island shows 8
//! letters, the person approves them in oyenza.app (where they are signed in
//! anyway), and the island gets a key of its own (`oyzd_…`). No password is
//! ever typed into Poni, and the key opens only the island's routes on
//! api.oyenza.app (catat-server `routes/island.ts`): money in and out that
//! arrived by itself in the last 2 hours (the notices), the last 7 days of
//! transactions by day (the Oyenza page's live view), bills due in the next
//! 30 days, saving a spend typed here (with a short Undo), and the daily step
//! totals the phone sent for the Health page, once the person turned that on
//! in Oyenza's Health (Sehat). It can't read balances, account numbers, notes
//! or receipts, change or delete anything else, or touch the account.
//!
//! Everything here is pure: request bodies and URLs, parsing the answers, the
//! words of notices and rows, how sums hide on screen, and how often to ask.
//! The HTTP and the sealed key live in the Windows crate
//! (`services/oyenza.rs`).

use crate::datetime::{days_from_civil, civil_from_days, weekday_from_days, LocalDateTime, MONTH_SHORT, WEEKDAY_SHORT};
use crate::health::HealthSnapshot;
use crate::urlenc::with_query;
use serde::{Deserialize, Serialize};

pub const API: &str = "https://api.oyenza.app";
pub const WEB: &str = "https://oyenza.app";
pub const APP: &str = "poni";

/// Money is asked for this often while the PC is in use on mains power...
pub const MONEY_EVERY_S: u64 = 60;
/// ...and this often on battery or in battery saver.
pub const MONEY_EVERY_BATTERY_S: u64 = 180;
/// Bills change rarely: every half hour, and when the Oyenza tab opens.
pub const BILLS_EVERY_S: u64 = 30 * 60;
/// While the Oyenza page is open, its day list is checked this often (an
/// answer with nothing new is a few bytes).
pub const ACTIVITY_EVERY_S: u64 = 15;
/// The steps from the phone are asked for this often while the PC is in use,
/// and when the Health page opens: the phone sends them only while Oyenza is
/// open on it.
pub const HEALTH_EVERY_S: u64 = 15 * 60;
/// Bills are asked for this many days ahead: Tomorrow and Later on the
/// Oyenza tab, and the five weeks on Today.
pub const BILLS_DAYS: u32 = 35;
/// A transaction that lands while the page is open glows this long.
pub const GLOW_S: f64 = 60.0;
/// Errors back off from a minute, doubling, up to this.
pub const MAX_BACKOFF_S: u64 = 15 * 60;
/// How long a money or bill notice stays (seconds), and how much longer once
/// its sum is shown.
pub const NOTICE_S: f64 = 8.0;
pub const SHOWN_S: f64 = 6.0;
/// The island's Undo for a saved spend (the server allows 2 minutes).
pub const UNDO_S: f64 = 10.0;
/// A notice held back by a full-screen app still shows within this time.
pub const HELD_MAX_S: f64 = 30.0 * 60.0;
/// Bill reminders wait for the morning: the first time the PC is in use from
/// this hour on, the day before the bill (or on the day, if missed).
pub const BILL_HOUR: u8 = 8;
/// Notes longer than this are cut (the server reads 300 characters at most).
pub const NOTE_MAX_CHARS: usize = 300;

// ---------------------------------------------------------------- the link

/// What the PC keeps after signing in, sealed with Windows (DPAPI) under
/// `%LOCALAPPDATA%\Poni\` (on a Mac, a file only its account can read, in
/// `~/Library/Application Support/Poni/`). The key is never logged or shown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub key: String,
    /// The account, masked ("a•••@example.com"), to show which one it joined.
    pub account: String,
    pub device_id: String,
    pub device_name: String,
}

pub fn parse_link(s: &str) -> Option<Link> {
    let l: Link = serde_json::from_str(s).ok()?;
    (l.key.starts_with("oyzd_") && l.key.len() == 48).then_some(l)
}

pub fn link_json(l: &Link) -> String {
    serde_json::to_string(l).unwrap_or_default()
}

// ---------------------------------------------------------------- requests

pub fn start_url() -> String {
    format!("{API}/connect/start")
}

pub fn token_url() -> String {
    format!("{API}/connect/token")
}

pub fn disconnect_url() -> String {
    format!("{API}/connect/disconnect")
}

/// Money in and out that arrived by itself since `since` (`kinds=all`: 0.1.9
/// asked without it and heard money in alone).
pub fn money_url(since: Option<&str>, tz: &str) -> String {
    with_query(&format!("{API}/island/money"), &[("since", since.unwrap_or("")), ("tz", tz), ("kinds", "all")])
}

/// The last 7 days of transactions; with `since`, only whether anything changed.
pub fn activity_url(tz: &str, since: Option<&str>) -> String {
    with_query(&format!("{API}/island/activity"), &[("tz", tz), ("days", "7"), ("since", since.unwrap_or(""))])
}

pub fn bills_url(tz: &str) -> String {
    with_query(&format!("{API}/island/bills"), &[("tz", tz), ("days", &BILLS_DAYS.to_string())])
}

/// The daily step totals the person's phone sent.
pub fn health_url() -> String {
    format!("{API}/island/health")
}

/// Whether the signed-in account has Oyenza Studio Pro: `{"pro": bool}`. The
/// server decides (who it is is never in the island, which only knows a
/// masked account).
pub fn pro_url() -> String {
    format!("{API}/island/pro")
}

pub fn parse_pro(body: &str) -> Option<bool> {
    serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v["pro"].as_bool())
}

pub fn note_url() -> String {
    format!("{API}/island/note")
}

pub fn undo_url(id: &str) -> String {
    format!("{API}/island/note/{}", crate::urlenc::encode(id))
}

/// `platform`: [`PLATFORM`], so oyenza.app shows a Mac as one (a server from
/// before 0.2.7's leaves it out and stores every PC as Windows).
pub fn start_body(pc_name: &str, version: &str, platform: &str) -> String {
    serde_json::json!({ "app": APP, "name": pc_name, "version": version, "platform": platform }).to_string()
}

/// The system this app runs on, as the sign-in names it.
pub const PLATFORM: &str = if cfg!(target_os = "macos") { "macos" } else { "windows" };

pub fn token_body(device_code: &str) -> String {
    serde_json::json!({ "deviceCode": device_code }).to_string()
}

pub fn note_body(text: &str, tz: &str) -> String {
    serde_json::json!({ "text": text, "tz": tz }).to_string()
}

/// What oyenza.app lists a computer as when the system gives no name.
const UNNAMED: &str = if cfg!(target_os = "macos") { "Mac" } else { "Windows PC" };

/// The PC's name as oyenza.app will list it: one printable line, 40 characters
/// at most ([`UNNAMED`] when the system gives none).
pub fn pc_name(raw: &str) -> String {
    let clean: String = raw.chars().filter(|c| !c.is_control()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = clean.chars().take(40).collect();
    if cut.trim().is_empty() {
        UNNAMED.into()
    } else {
        cut.trim().to_string()
    }
}

/// The local zone as a UTC offset, the way the server reads days in it
/// ("+07:00", "-03:30"). Windows names zones its own way; an offset is
/// unambiguous for "today" and is all the server needs.
pub fn tz_param(offset_minutes: i32) -> String {
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let m = offset_minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", m / 60, m % 60)
}

/// Local wall time minus UTC, in minutes, rounded to the quarter hour.
pub fn offset_minutes(local: &LocalDateTime, unix: i64) -> i32 {
    let local_s = days_from_civil(local.year, local.month, local.day) * 86_400
        + local.hour as i64 * 3600
        + local.minute as i64 * 60
        + local.second as i64;
    let raw = (local_s - unix) as f64 / 60.0;
    ((raw / 15.0).round() * 15.0) as i32
}

// ---------------------------------------------------------------- answers

/// The server's error code (`{"error": "…"}`), if the body has one.
pub fn error_code(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct E {
        error: String,
    }
    serde_json::from_str::<E>(body).ok().map(|e| e.error)
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    pub device_code: String,
    pub user_code: String,
    pub verify_url_complete: String,
    pub expires_in: u64,
    #[serde(default = "five")]
    pub interval: u64,
}

fn five() -> u64 {
    5
}

pub fn parse_started(body: &str) -> Option<Started> {
    serde_json::from_str(body).ok()
}

/// What asking for the key answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Claim {
    Waiting,
    Connected(Link),
    SlowDown,
    Expired,
    Denied,
    /// The server no longer knows the code (used, or never made).
    Invalid,
}

pub fn parse_claim(status: u16, body: &str) -> Option<Claim> {
    #[derive(Deserialize)]
    struct Device {
        id: String,
        name: String,
    }
    #[derive(Deserialize)]
    struct Ok {
        status: String,
        key: Option<String>,
        device: Option<Device>,
        #[serde(default)]
        account: String,
    }
    if status == 200 {
        let ok: Ok = serde_json::from_str(body).ok()?;
        return match (ok.status.as_str(), ok.key, ok.device) {
            ("waiting", _, _) => Some(Claim::Waiting),
            ("connected", Some(key), Some(d)) => {
                let link = Link { key, account: ok.account, device_id: d.id, device_name: d.name };
                parse_link(&link_json(&link)).map(Claim::Connected)
            }
            _ => None,
        };
    }
    match error_code(body).as_deref() {
        Some("slow_down") => Some(Claim::SlowDown),
        Some("expired") => Some(Claim::Expired),
        Some("denied") => Some(Claim::Denied),
        Some("invalid_code") => Some(Claim::Invalid),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WalletRef {
    pub name: String,
    #[serde(default)]
    pub color: String,
}

/// Money that arrived by itself (Gmail's check or a bank notification).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Arrival {
    pub id: String,
    /// "income", or "expense" for spending that arrived by itself.
    #[serde(rename = "type", default = "income")]
    pub kind: String,
    pub amount: f64,
    #[serde(default)]
    pub currency: String,
    /// The sum written out in the account's language ("Rp 1.250.000").
    pub text: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub wallet: Option<WalletRef>,
    #[serde(default)]
    pub at: String,
}

fn income() -> String {
    "income".into()
}

impl Arrival {
    pub fn is_out(&self) -> bool {
        self.kind == "expense"
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MoneyFeed {
    pub cursor: String,
    #[serde(default)]
    pub arrived: Vec<Arrival>,
}

pub fn parse_money(body: &str) -> Option<MoneyFeed> {
    serde_json::from_str(body).ok()
}

/// One transaction of the live view (the server's `/island/activity` rows).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Txn {
    pub id: String,
    /// "income", "expense" or "transfer" (between the person's own wallets).
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub amount: f64,
    /// The sum in the account's language ("Rp 25.000").
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub wallet: Option<WalletRef>,
    /// A transfer's other wallet.
    #[serde(default, rename = "toWallet")]
    pub to_wallet: Option<WalletRef>,
    /// When it happened ("2026-10-24T12:40:00.000Z").
    #[serde(default)]
    pub at: String,
    /// Logged by Oyenza itself, from a bank's email or notification.
    #[serde(default)]
    pub auto: bool,
}

/// Which way a transaction moved money.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    In,
    Out,
    /// Between the person's own wallets: no money in or out.
    Move,
}

impl Txn {
    pub fn flow(&self) -> Flow {
        match self.kind.as_str() {
            "income" => Flow::In,
            "transfer" => Flow::Move,
            _ => Flow::Out,
        }
    }
}

/// One day of the live view, newest rows first; `count` and the sums cover
/// the whole day even when the server sent only its newest rows.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    /// YYYY-MM-DD in the PC's zone.
    pub date: String,
    #[serde(default)]
    pub count: usize,
    #[serde(default, rename = "in")]
    pub in_sum: f64,
    #[serde(default)]
    pub out: f64,
    #[serde(default)]
    pub in_text: String,
    #[serde(default)]
    pub out_text: String,
    #[serde(default)]
    pub rows: Vec<Txn>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    /// The space's change number: asked again with it, the server says only
    /// whether anything changed.
    pub cursor: String,
    /// The server's today in the PC's zone (YYYY-MM-DD).
    pub today: String,
    /// Today first, then each day before it.
    pub days: Vec<Day>,
}

impl Activity {
    pub fn day(&self, date: &str) -> Option<&Day> {
        self.days.iter().find(|d| d.date == date)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActivityAnswer {
    /// Nothing changed in the space since the cursor.
    Unchanged { cursor: String, today: String },
    Full(Activity),
}

pub fn parse_activity(body: &str) -> Option<ActivityAnswer> {
    #[derive(Deserialize)]
    struct A {
        cursor: String,
        #[serde(default)]
        today: String,
        #[serde(default)]
        changed: Option<bool>,
        #[serde(default)]
        days: Option<Vec<Day>>,
    }
    let a: A = serde_json::from_str(body).ok()?;
    match (a.changed, a.days) {
        (Some(false), _) => Some(ActivityAnswer::Unchanged { cursor: a.cursor, today: a.today }),
        (_, Some(days)) => Some(ActivityAnswer::Full(Activity { cursor: a.cursor, today: a.today, days })),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Bill {
    /// "subscription" or "card".
    pub kind: String,
    pub name: String,
    /// YYYY-MM-DD.
    pub due: String,
    #[serde(default)]
    pub amount: Option<f64>,
    #[serde(default)]
    pub text: Option<String>,
}

impl Bill {
    pub fn is_card(&self) -> bool {
        self.kind == "card"
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct BillsFeed {
    #[serde(default)]
    pub today: String,
    #[serde(default)]
    pub bills: Vec<Bill>,
}

pub fn parse_bills(body: &str) -> Option<BillsFeed> {
    serde_json::from_str(body).ok()
}

/// The steps answer, `{"health": {…} | null}`: `Some(None)` when there are none
/// to show (the switch is off in Oyenza, the server paused it, or nothing is
/// kept), `None` when the body doesn't read. Keys this version doesn't know are
/// left out.
pub fn parse_health(body: &str) -> Option<Option<HealthSnapshot>> {
    let mut v: serde_json::Value = serde_json::from_str(body).ok()?;
    match v.get_mut("health")?.take() {
        serde_json::Value::Null => Some(None),
        h => serde_json::from_value(h).ok().map(Some),
    }
}

/// A spend the server saved from the island's text.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Saved {
    pub id: String,
    pub title: String,
    pub amount: f64,
    pub text: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub wallet: String,
}

pub fn parse_saved(body: &str) -> Option<Vec<Saved>> {
    #[derive(Deserialize)]
    struct S {
        saved: Vec<Saved>,
    }
    serde_json::from_str::<S>(body).ok().map(|s| s.saved).filter(|v| !v.is_empty())
}

// ---------------------------------------------------------------- cadence

/// Seconds until the next money check. Battery saver counts as battery.
pub fn money_every_s(on_ac: bool, saver: bool) -> u64 {
    if on_ac && !saver {
        MONEY_EVERY_S
    } else {
        MONEY_EVERY_BATTERY_S
    }
}

/// Wait after `fails` failed calls in a row: 1, 2, 4 … 15 minutes, never
/// shorter than the server's `Retry-After`.
pub fn backoff_s(fails: u32, retry_after_s: Option<u64>) -> u64 {
    let doubling = 60u64.saturating_mul(1u64 << fails.saturating_sub(1).min(10));
    doubling.min(MAX_BACKOFF_S).max(retry_after_s.unwrap_or(0).min(MAX_BACKOFF_S))
}

// ---------------------------------------------------------------- words

/// A sum as it stays on a shared screen until asked: the currency mark kept,
/// the digits gone ("Rp 1.250.000" -> "Rp •••").
pub fn masked(text: &str) -> String {
    match text.trim().rsplit_once(' ') {
        Some((mark, _)) if !mark.trim().is_empty() => format!("{} •••", mark.trim()),
        _ => "•••".into(),
    }
}

/// Title and subtitles of the notice for money that just arrived or went
/// out: the subtitle with the sums and who, and the one shown until "Show".
pub struct MoneyNotice {
    pub title: String,
    pub shown: String,
    pub hidden: String,
    /// What the notice is about, for its mark: money in, out, or both.
    pub flow: Option<Flow>,
}

pub fn money_notice(arrived: &[Arrival]) -> Option<MoneyNotice> {
    let last = arrived.last()?;
    let outs = arrived.iter().filter(|a| a.is_out()).count();
    if arrived.len() == 1 {
        let (word, hidden, flow) = if last.is_out() { ("Money out", "Amount and payee hidden", Flow::Out) } else { ("Money in", "Amount and sender hidden", Flow::In) };
        let title = match last.wallet.as_ref().map(|w| w.name.trim()).filter(|n| !n.is_empty()) {
            Some(w) => format!("{word} · {w}"),
            None => word.to_string(),
        };
        let shown = if last.title.trim().is_empty() { last.text.clone() } else { format!("{} · {}", last.text, last.title.trim()) };
        return Some(MoneyNotice { title, shown, hidden: hidden.into(), flow: Some(flow) });
    }
    let n = arrived.len();
    if outs == 0 || outs == n {
        let sums: Vec<&str> = arrived.iter().map(|a| a.text.as_str()).collect();
        let (title, hidden, flow) =
            if outs == 0 { (format!("Money in · {n} new"), "Amounts and senders hidden", Flow::In) } else { (format!("Money out · {n} new"), "Amounts and payees hidden", Flow::Out) };
        return Some(MoneyNotice { title, shown: sums.join(" · "), hidden: hidden.into(), flow: Some(flow) });
    }
    // Both ways at once: each sum with its direction.
    let sums: Vec<String> = arrived.iter().map(|a| format!("{}{}", if a.is_out() { MINUS } else { "+" }, a.text)).collect();
    Some(MoneyNotice { title: format!("Oyenza · {n} new"), shown: sums.join(" · "), hidden: "Amounts hidden".into(), flow: None })
}

/// The minus sign, a dash's width like "+".
const MINUS: &str = "\u{2212}";

// ---------------------------------------------------------------- the day strip

/// A day on the Oyenza page's strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DayPick {
    /// `n` days ago: 1 is yesterday, 6 the oldest the page shows.
    Past(u8),
    #[default]
    Today,
    Tomorrow,
    /// Bills after tomorrow, up to 30 days ahead.
    Later,
}

/// The strip, oldest first: the five days before yesterday by weekday ("Sun"
/// … "Thu": each weekday comes once in a week, so the name is enough),
/// Yesterday, Today, Tomorrow and Later.
pub fn strip(today: &str) -> Vec<(DayPick, String)> {
    let t = day_number(today);
    let mut out: Vec<(DayPick, String)> = (2..=6u8)
        .rev()
        .map(|n| (DayPick::Past(n), t.map(|d| WEEKDAY_SHORT[weekday_from_days(d - n as i64) as usize].to_string()).unwrap_or_default()))
        .collect();
    out.extend([(DayPick::Past(1), "Yesterday".into()), (DayPick::Today, "Today".into()), (DayPick::Tomorrow, "Tomorrow".into()), (DayPick::Later, "Later".into())]);
    out
}

/// The date a past day or Today stands for (YYYY-MM-DD); None for the days ahead.
pub fn pick_date(pick: DayPick, today: &str) -> Option<String> {
    let n = match pick {
        DayPick::Today => 0,
        DayPick::Past(n) => n as i64,
        DayPick::Tomorrow | DayPick::Later => return None,
    };
    let (y, m, d) = civil_from_days(day_number(today)? - n);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// The heading over a day: "TODAY · 5 TRANSACTIONS", "YESTERDAY", "THU 22 OCT · 1 TRANSACTION",
/// "TOMORROW · SUN 25 OCT", "LATER · THE NEXT 30 DAYS".
pub fn day_heading(pick: DayPick, today: &str, count: usize) -> String {
    let name = match pick {
        DayPick::Today => "TODAY".to_string(),
        DayPick::Past(1) => "YESTERDAY".to_string(),
        DayPick::Past(_) => pick_date(pick, today).and_then(|d| short_date(&d)).unwrap_or_default().to_uppercase(),
        DayPick::Tomorrow => return format!("TOMORROW · {}", short_date(&next_day(today)).unwrap_or_default().to_uppercase()),
        DayPick::Later => return "LATER · THE NEXT 30 DAYS".into(),
    };
    match count {
        0 => name,
        1 => format!("{name} · 1 TRANSACTION"),
        n => format!("{name} · {n} TRANSACTIONS"),
    }
}

/// A day's money in and out beside its heading ("in Rp ••• · out Rp •••");
/// a side with nothing is left out, and a day with neither says nothing.
pub fn day_totals(day: &Day, show: bool) -> String {
    let sum = |t: &str| if show { t.to_string() } else { masked(t) };
    let mut parts = Vec::new();
    if day.in_sum > 0.0 {
        parts.push(format!("in {}", sum(&day.in_text)));
    }
    if day.out > 0.0 {
        parts.push(format!("out {}", sum(&day.out_text)));
    }
    parts.join("  ·  ")
}

/// What an empty day says.
pub fn empty_day_text(pick: DayPick) -> &'static str {
    match pick {
        DayPick::Today => "Nothing logged today yet",
        DayPick::Past(_) => "Nothing logged that day",
        DayPick::Tomorrow => "Nothing due tomorrow",
        DayPick::Later => "Nothing due in the next 30 days",
    }
}

// ---------------------------------------------------------------- rows

fn wallet_name(w: &Option<WalletRef>) -> Option<&str> {
    w.as_ref().map(|w| w.name.trim()).filter(|n| !n.is_empty())
}

/// A row's title as Oyenza wrote it; without one, a transfer's wallets, else
/// the category, else what it was.
pub fn txn_title(t: &Txn) -> String {
    let title = t.title.trim();
    if !title.is_empty() {
        return title.to_string();
    }
    let category = t.category.as_deref().map(str::trim).filter(|c| !c.is_empty());
    match (t.flow(), wallet_name(&t.wallet), wallet_name(&t.to_wallet)) {
        (Flow::Move, Some(a), Some(b)) => format!("{a} to {b}"),
        (Flow::Move, _, _) => "Transfer".into(),
        (Flow::In, ..) => category.unwrap_or("Money in").to_string(),
        (Flow::Out, ..) => category.unwrap_or("Spending").to_string(),
    }
}

/// The dim detail after a row's title: its wallet (a titled transfer: from
/// and to; an untitled one says it is your own) and the time. Words, not an
/// arrow: not every font the island falls back to has one.
pub fn txn_detail(t: &Txn, time: &str) -> String {
    let place = match t.flow() {
        Flow::Move if !t.title.trim().is_empty() => match (wallet_name(&t.wallet), wallet_name(&t.to_wallet)) {
            (Some(a), Some(b)) => format!("{a} to {b}"),
            _ => "Own transfer".into(),
        },
        Flow::Move => "Own transfer".into(),
        _ => wallet_name(&t.wallet).unwrap_or("").to_string(),
    };
    [place.as_str(), time].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ")
}

/// A row's sum with its direction: "+Rp 1.250.000", "−Rp 25.000", a transfer
/// plain; hidden, "+Rp •••".
pub fn signed_sum(t: &Txn, show: bool) -> String {
    let text = if show { t.text.clone() } else { masked(&t.text) };
    match t.flow() {
        Flow::In => format!("+{text}"),
        Flow::Out => format!("{MINUS}{text}"),
        Flow::Move => text,
    }
}

/// The bills a day of the strip lists: due that day, or after tomorrow for Later.
pub fn bills_for<'a>(bills: &'a [Bill], today: &str, pick: DayPick) -> Vec<&'a Bill> {
    let tomorrow = next_day(today);
    bills
        .iter()
        .filter(|b| match pick {
            DayPick::Today => b.due == today,
            DayPick::Tomorrow => b.due == tomorrow,
            DayPick::Later => b.due.as_str() > tomorrow.as_str(),
            DayPick::Past(_) => false,
        })
        .collect()
}

/// When each row on the page was first seen (monotonic seconds), so what
/// lands while the person looks glows for a minute. The first answer after
/// starting counts as seen long ago: nothing glows just for opening.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    at: Vec<(String, f64)>,
    primed: bool,
}

impl Seen {
    pub fn note(&mut self, days: &[Day], now: f64) {
        let ids: Vec<&str> = days.iter().flat_map(|d| d.rows.iter().map(|t| t.id.as_str())).collect();
        let when = if self.primed { now } else { f64::NEG_INFINITY };
        for id in &ids {
            if !self.at.iter().any(|(s, _)| s == id) {
                self.at.push((id.to_string(), when));
            }
        }
        self.at.retain(|(s, _)| ids.contains(&s.as_str()));
        self.primed = true;
    }

    /// 1 just after it landed, down to 0 over [`GLOW_S`].
    pub fn glow(&self, id: &str, now: f64) -> f32 {
        self.at.iter().find(|(s, _)| s == id).map_or(0.0, |(_, t)| (1.0 - (now - t) / GLOW_S).clamp(0.0, 1.0) as f32)
    }

    /// Something on `day` is still glowing (for the strip's dot), and until when.
    pub fn glowing_until(&self, day: &Day, now: f64) -> Option<f64> {
        day.rows.iter().filter_map(|t| self.at.iter().find(|(s, _)| s == &t.id)).map(|(_, at)| at + GLOW_S).filter(|&u| u > now).reduce(f64::max)
    }
}

/// "Today", "Tomorrow", or "Mon 26 Oct" for a YYYY-MM-DD day seen from `today`.
pub fn due_label(due: &str, today: &str) -> String {
    let (Some(d), Some(t)) = (day_number(due), day_number(today)) else { return due.to_string() };
    match d - t {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        _ => short_date(due).unwrap_or_else(|| due.to_string()),
    }
}

/// "Mon 26 Oct" for a YYYY-MM-DD day.
pub fn short_date(day: &str) -> Option<String> {
    let d = day_number(day)?;
    let (_, m, dom) = civil_from_days(d);
    Some(format!("{} {} {}", WEEKDAY_SHORT[weekday_from_days(d) as usize], dom, MONTH_SHORT[(m.clamp(1, 12) - 1) as usize]))
}

fn day_number(day: &str) -> Option<i64> {
    let mut it = day.split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: u8 = it.next()?.parse().ok()?;
    let d: u8 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// The day after `today` (YYYY-MM-DD).
pub fn next_day(today: &str) -> String {
    match day_number(today) {
        Some(n) => {
            let (y, m, d) = civil_from_days(n + 1);
            format!("{y:04}-{m:02}-{d:02}")
        }
        None => String::new(),
    }
}

/// The bill notice's title: "Spotify renews tomorrow", "Kartu Kredit BCA due today".
pub fn bill_title(b: &Bill, today: &str) -> String {
    let when = due_label(&b.due, today);
    let when = if when == "Today" || when == "Tomorrow" { when.to_lowercase() } else { format!("on {when}") };
    if b.is_card() {
        format!("{} due {when}", b.name)
    } else {
        format!("{} renews {when}", b.name)
    }
}

/// The bill notice's subtitle with the sum, and the one until "Show" (None:
/// nothing to hide, a card's amount stays in the app).
pub fn bill_subtitles(b: &Bill) -> (String, Option<String>) {
    match (&b.text, b.is_card()) {
        (Some(t), false) => (t.clone(), Some("Amount hidden".into())),
        _ => ("Open Oyenza for the amount".into(), None),
    }
}

/// Bills worth a notice now: due today or tomorrow, and not reminded of yet
/// (`noticed` holds [`notice_key`]s).
pub fn bills_to_remind<'a>(bills: &'a [Bill], today: &str, noticed: &[String]) -> Vec<&'a Bill> {
    let tomorrow = next_day(today);
    bills.iter().filter(|b| (b.due == today || b.due == tomorrow) && !noticed.contains(&notice_key(b))).collect()
}

/// One reminder per bill and due date.
pub fn notice_key(b: &Bill) -> String {
    format!("{}|{}|{}", b.due, b.kind, b.name)
}

/// Reminder keys still worth keeping: bills due from `today` on.
pub fn prune_noticed(noticed: &mut Vec<String>, today: &str) {
    noticed.retain(|k| k.get(..10).is_some_and(|d| d >= today));
    noticed.dedup();
    if noticed.len() > 200 {
        let cut = noticed.len() - 200;
        noticed.drain(..cut);
    }
}

/// The saved row: "Kopi · Rp 25.000 · Coffee · GoPay"; two or more show the
/// first and how many more.
pub fn saved_line(saved: &[Saved]) -> String {
    let Some(first) = saved.first() else { return String::new() };
    let mut parts = vec![first.title.trim().to_string(), first.text.clone()];
    if saved.len() == 1 {
        if let Some(c) = first.category.as_deref().filter(|c| !c.trim().is_empty()) {
            parts.push(c.trim().to_string());
        }
        if !first.wallet.trim().is_empty() {
            parts.push(first.wallet.trim().to_string());
        }
        return parts.join(" · ");
    }
    format!("{}, and {} more", parts.join(" · "), saved.len() - 1)
}

/// A typed note has an amount the server can read: a digit, or the Jakarta
/// words for one ("goceng" is 5.000), as Oyenza's own parser reads them.
pub fn has_amount(text: &str) -> bool {
    const WORDS: [&str; 7] = ["cepek", "gopek", "seceng", "noceng", "goceng", "ceban", "goban"];
    text.chars().any(|c| c.is_ascii_digit())
        || text.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| WORDS.contains(&w))
}

/// What the island says when a note couldn't be saved (the text stays in the field).
pub fn note_error_text(code: &str) -> &'static str {
    match code {
        "no_amount" | "empty_text" => "Add an amount, like kopi 25rb",
        "no_wallet" => "Add a wallet in Oyenza first",
        "no_space" => "Make a space in Oyenza first",
        "no_rate" => "That currency can't be converted yet",
        "ai_busy" | "rate_limited" => "Oyenza is busy. Try again in a minute",
        "offline" => "Couldn't reach Oyenza. Your text is still here",
        _ => "Couldn't save it. Your text is still here",
    }
}

/// Title and subtitle when signing in stops without a key.
pub fn connect_failure_text(code: &str) -> (&'static str, &'static str) {
    match code {
        "expired" | "invalid_code" => ("Sign-in code expired", "Start again from the tray"),
        "denied" => ("Not connected", "The code was refused in oyenza.app"),
        "offline" => ("Can't reach Oyenza", "Check the connection and try again"),
        _ => ("Couldn't sign in to Oyenza", "Try again from the tray"),
    }
}

/// "9:41" left on a sign-in code.
pub fn code_time_left(secs: f64) -> String {
    let s = secs.max(0.0).ceil() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// "#0060AF" as 0..=255 channels, for a wallet's colour in a row.
pub fn hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// An instant from the server ("2026-10-24T02:12:00.000Z") as local wall time,
/// `offset_minutes` east of UTC.
pub fn local_clock(iso: &str, offset_minutes: i32) -> Option<LocalDateTime> {
    let b = iso.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || !iso.ends_with('Z') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| iso.get(r).and_then(|v| v.parse::<i64>().ok());
    let (y, mo, d, h, mi, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let t = days_from_civil(y as i32, mo as u8, d as u8) * 86_400 + h * 3600 + mi * 60 + sec + offset_minutes as i64 * 60;
    let (ly, lm, ld) = civil_from_days(t.div_euclid(86_400));
    let rest = t.rem_euclid(86_400);
    Some(LocalDateTime { year: ly, month: lm, day: ld, hour: (rest / 3600) as u8, minute: (rest % 3600 / 60) as u8, second: (rest % 60) as u8 })
}

/// The arrivals not shown as a notice yet, now remembered as shown (the last 50). The server sends each arrival to a
/// PC once; this keeps a notice from ever repeating, whatever the server does.
pub fn unseen(shown: &mut Vec<String>, arrived: &[Arrival]) -> Vec<Arrival> {
    let fresh: Vec<Arrival> = arrived.iter().filter(|a| !shown.contains(&a.id)).cloned().collect();
    shown.extend(fresh.iter().map(|a| a.id.clone()));
    if shown.len() > 50 {
        let cut = shown.len() - 50;
        shown.drain(..cut);
    }
    fresh
}


#[cfg(test)]
mod tests {

    #[test]
    fn pro_answers() {
        assert_eq!(pro_url(), "https://api.oyenza.app/island/pro");
        assert_eq!(parse_pro(r#"{"pro":true}"#), Some(true));
        assert_eq!(parse_pro(r#"{"pro":false}"#), Some(false));
        assert_eq!(parse_pro(r#"{"error":"invalid_key"}"#), None);
        assert_eq!(parse_pro("<html>"), None);
    }
    use super::*;

    fn arrival(id: &str, text: &str, title: &str, wallet: Option<&str>) -> Arrival {
        Arrival {
            id: id.into(),
            kind: "income".into(),
            amount: 1.0,
            currency: "IDR".into(),
            text: text.into(),
            title: title.into(),
            wallet: wallet.map(|w| WalletRef { name: w.into(), color: "#0060AF".into() }),
            at: String::new(),
        }
    }

    fn bill(kind: &str, name: &str, due: &str, text: Option<&str>) -> Bill {
        Bill { kind: kind.into(), name: name.into(), due: due.into(), amount: text.map(|_| 1.0), text: text.map(Into::into) }
    }

    #[test]
    fn urls_and_bodies() {
        assert_eq!(money_url(None, "+07:00"), "https://api.oyenza.app/island/money?tz=%2B07%3A00&kinds=all");
        assert_eq!(money_url(Some("123"), "+07:00"), "https://api.oyenza.app/island/money?since=123&tz=%2B07%3A00&kinds=all");
        assert_eq!(activity_url("+07:00", None), "https://api.oyenza.app/island/activity?tz=%2B07%3A00&days=7");
        assert_eq!(activity_url("+07:00", Some("9")), "https://api.oyenza.app/island/activity?tz=%2B07%3A00&days=7&since=9");
        assert_eq!(bills_url("-03:30"), "https://api.oyenza.app/island/bills?tz=-03%3A30&days=35");
        assert_eq!(undo_url("abc 1/2"), "https://api.oyenza.app/island/note/abc%201%2F2");
        let b: serde_json::Value = serde_json::from_str(&start_body("DESKTOP-7Q2K", "0.1.5", "windows")).unwrap();
        assert_eq!(b, serde_json::json!({ "app": "poni", "name": "DESKTOP-7Q2K", "version": "0.1.5", "platform": "windows" }));
        assert_eq!(PLATFORM, if cfg!(target_os = "macos") { "macos" } else { "windows" });
        let n: serde_json::Value = serde_json::from_str(&note_body("kopi \"25rb\"", "+07:00")).unwrap();
        assert_eq!(n["text"], "kopi \"25rb\"");
        assert_eq!(pc_name("  DESKTOP-7Q2K\u{0}\n "), "DESKTOP-7Q2K");
        assert_eq!(pc_name(""), UNNAMED);
        assert_eq!(pc_name(&"x".repeat(60)).len(), 40);
    }

    #[test]
    fn zone_offsets() {
        assert_eq!(tz_param(420), "+07:00");
        assert_eq!(tz_param(-210), "-03:30");
        assert_eq!(tz_param(0), "+00:00");
        let local = LocalDateTime { year: 2026, month: 10, day: 24, hour: 14, minute: 38, second: 20 };
        // 2026-10-24 07:38:20 UTC
        let unix = days_from_civil(2026, 10, 24) * 86_400 + 7 * 3600 + 38 * 60 + 20;
        assert_eq!(offset_minutes(&local, unix), 420);
        // A second off either way still rounds to the zone.
        assert_eq!(offset_minutes(&local, unix + 1), 420);
        let india = LocalDateTime { hour: 13, minute: 8, ..local };
        assert_eq!(offset_minutes(&india, unix), 330);
    }

    #[test]
    fn claims() {
        assert_eq!(parse_claim(200, r#"{"status":"waiting"}"#), Some(Claim::Waiting));
        let key = format!("oyzd_{}", "A".repeat(43));
        let body = format!(r#"{{"status":"connected","key":"{key}","device":{{"id":"d1","name":"DESKTOP-7Q2K"}},"account":"a•••@example.com"}}"#);
        assert_eq!(
            parse_claim(200, &body),
            Some(Claim::Connected(Link { key, account: "a•••@example.com".into(), device_id: "d1".into(), device_name: "DESKTOP-7Q2K".into() }))
        );
        assert_eq!(parse_claim(200, r#"{"status":"connected","key":"nope","device":{"id":"d","name":"x"}}"#), None);
        assert_eq!(parse_claim(429, r#"{"error":"slow_down"}"#), Some(Claim::SlowDown));
        assert_eq!(parse_claim(410, r#"{"error":"expired"}"#), Some(Claim::Expired));
        assert_eq!(parse_claim(403, r#"{"error":"denied"}"#), Some(Claim::Denied));
        assert_eq!(parse_claim(404, r#"{"error":"invalid_code"}"#), Some(Claim::Invalid));
        assert_eq!(parse_claim(500, "oops"), None);
        assert_eq!(parse_link(r#"{"key":"oyzc_x","account":"","device_id":"","device_name":""}"#), None);
    }

    #[test]
    fn feeds_parse_the_server_shapes() {
        let m = parse_money(
            r##"{"cursor":"42","currency":"IDR","arrived":[{"id":"t1","amount":1250000,"currency":"IDR","text":"Rp 1.250.000","title":"Gaji Oktober","wallet":{"name":"BCA","color":"#0060AF"},"at":"2026-10-24T02:12:00.000Z"}]}"##,
        )
        .unwrap();
        assert_eq!(m.cursor, "42");
        assert_eq!(m.arrived[0].wallet.as_ref().unwrap().name, "BCA");
        // 0.1.9's server answer has no type: money in.
        assert!(!m.arrived[0].is_out());
        let out = parse_money(r#"{"cursor":"43","arrived":[{"id":"t2","type":"expense","amount":89000,"text":"Rp 89.000"}]}"#).unwrap();
        assert!(out.arrived[0].is_out());
        let b = parse_bills(r#"{"currency":"IDR","today":"2026-10-24","bills":[{"kind":"subscription","name":"Spotify","due":"2026-10-25","amount":54990,"text":"Rp 54.990"},{"kind":"card","name":"Kartu Kredit BCA","due":"2026-10-26"}]}"#).unwrap();
        assert_eq!(b.bills.len(), 2);
        assert!(b.bills[1].is_card() && b.bills[1].text.is_none());
        let s = parse_saved(r#"{"saved":[{"id":"x","type":"expense","title":"Kopi","amount":25000,"currency":"IDR","text":"Rp 25.000","category":null,"wallet":"GoPay"}]}"#).unwrap();
        assert_eq!(s[0].category, None);
        assert_eq!(parse_saved(r#"{"saved":[]}"#), None);
        assert_eq!(error_code(r#"{"error":"no_wallet"}"#).as_deref(), Some("no_wallet"));
        assert_eq!(error_code("<html>"), None);
    }

    #[test]
    fn cadence() {
        assert_eq!(money_every_s(true, false), 60);
        assert_eq!(money_every_s(false, false), 180);
        assert_eq!(money_every_s(true, true), 180);
        assert_eq!((1..=6).map(|n| backoff_s(n, None)).collect::<Vec<_>>(), [60, 120, 240, 480, 900, 900]);
        assert_eq!(backoff_s(1, Some(300)), 300);
        assert_eq!(backoff_s(1, Some(99_999)), 900);
        assert_eq!(backoff_s(40, None), 900);
    }

    #[test]
    fn sums_hide_behind_their_mark() {
        assert_eq!(masked("Rp 1.250.000"), "Rp •••");
        assert_eq!(masked("S$ 12.50"), "S$ •••");
        assert_eq!(masked("-Rp 5.000"), "-Rp •••");
        assert_eq!(masked("1250000"), "•••");
        assert_eq!(masked(""), "•••");
    }

    #[test]
    fn money_notices() {
        let one = money_notice(&[arrival("a", "Rp 1.250.000", "Gaji Oktober", Some("BCA"))]).unwrap();
        assert_eq!((one.title.as_str(), one.shown.as_str(), one.hidden.as_str()), ("Money in · BCA", "Rp 1.250.000 · Gaji Oktober", "Amount and sender hidden"));
        let bare = money_notice(&[arrival("a", "Rp 50.000", " ", None)]).unwrap();
        assert_eq!((bare.title.as_str(), bare.shown.as_str()), ("Money in", "Rp 50.000"));
        let two = money_notice(&[arrival("a", "Rp 1.250.000", "Gaji", Some("BCA")), arrival("b", "Rp 50.000", "Uang masuk", Some("GoPay"))]).unwrap();
        assert_eq!((two.title.as_str(), two.shown.as_str(), two.hidden.as_str()), ("Money in · 2 new", "Rp 1.250.000 · Rp 50.000", "Amounts and senders hidden"));
        assert!(money_notice(&[]).is_none());
    }

    #[test]
    fn bills_and_their_days() {
        let today = "2026-10-24"; // a Saturday
        assert_eq!(due_label("2026-10-24", today), "Today");
        assert_eq!(due_label("2026-10-25", today), "Tomorrow");
        assert_eq!(due_label("2026-10-26", today), "Mon 26 Oct");
        assert_eq!(due_label("2027-01-01", "2026-12-31"), "Tomorrow");
        assert_eq!(due_label("nonsense", today), "nonsense");
        assert_eq!(next_day("2026-02-28"), "2026-03-01");
        let spotify = bill("subscription", "Spotify", "2026-10-25", Some("Rp 54.990"));
        let card = bill("card", "Kartu Kredit BCA", "2026-10-24", None);
        let later = bill("subscription", "Cloud", "2026-10-29", Some("Rp 29.000"));
        assert_eq!(bill_title(&spotify, today), "Spotify renews tomorrow");
        assert_eq!(bill_title(&card, today), "Kartu Kredit BCA due today");
        assert_eq!(bill_title(&later, today), "Cloud renews on Thu 29 Oct");
        assert_eq!(bill_subtitles(&spotify), ("Rp 54.990".to_string(), Some("Amount hidden".to_string())));
        assert_eq!(bill_subtitles(&card), ("Open Oyenza for the amount".to_string(), None));

        let all = vec![spotify.clone(), card.clone(), later.clone()];
        let due: Vec<&str> = bills_to_remind(&all, today, &[]).iter().map(|b| b.name.as_str()).collect();
        assert_eq!(due, ["Spotify", "Kartu Kredit BCA"]);
        let noticed = vec![notice_key(&spotify)];
        assert_eq!(bills_to_remind(&all, today, &noticed).len(), 1);
        let mut keys = vec![notice_key(&card), "2026-10-20|card|Old".into(), notice_key(&spotify)];
        prune_noticed(&mut keys, today);
        assert_eq!(keys, [notice_key(&card), notice_key(&spotify)]);
    }

    #[test]
    fn saved_rows_and_notes() {
        let kopi = Saved { id: "1".into(), title: "Kopi".into(), amount: 25000.0, text: "Rp 25.000".into(), category: Some("Coffee".into()), wallet: "GoPay".into() };
        assert_eq!(saved_line(std::slice::from_ref(&kopi)), "Kopi · Rp 25.000 · Coffee · GoPay");
        let parkir = Saved { id: "2".into(), title: "Parkir".into(), amount: 5000.0, text: "Rp 5.000".into(), category: None, wallet: "Bank".into() };
        assert_eq!(saved_line(&[kopi, parkir]), "Kopi · Rp 25.000, and 1 more");
        assert_eq!(saved_line(&[]), "");
        assert!(has_amount("kopi 25rb"));
        assert!(has_amount("parkir goceng"));
        assert!(!has_amount("kopi enak"));
        assert!(!has_amount("gocengan")); // only the whole word
        assert_eq!(note_error_text("no_wallet"), "Add a wallet in Oyenza first");
        assert_eq!(note_error_text("whatever"), "Couldn't save it. Your text is still here");
        assert_eq!(connect_failure_text("expired").0, "Sign-in code expired");
        assert_eq!(code_time_left(581.2), "9:42");
        assert_eq!(code_time_left(-3.0), "0:00");
        assert_eq!(hex_color("#0060AF"), Some((0, 0x60, 0xAF)));
        assert_eq!(hex_color("0060AF"), None);
        assert_eq!(hex_color("#GG0000"), None);
    }

    #[test]
    fn server_times_in_local_time() {
        let t = local_clock("2026-10-24T02:12:00.000Z", 420).unwrap();
        assert_eq!((t.year, t.month, t.day, t.hour, t.minute), (2026, 10, 24, 9, 12));
        let late = local_clock("2026-10-24T20:30:00Z", 420).unwrap();
        assert_eq!((late.day, late.hour, late.minute), (25, 3, 30));
        let west = local_clock("2026-01-01T01:00:00.5Z", -300).unwrap();
        assert_eq!((west.year, west.month, west.day, west.hour), (2025, 12, 31, 20));
        assert_eq!(local_clock("2026-10-24 02:12:00", 420), None);
        assert_eq!(local_clock("", 0), None);
    }

    #[test]
    fn a_notice_never_repeats() {
        let mut shown = Vec::new();
        let first = unseen(&mut shown, &[arrival("a", "Rp 1", "", None)]);
        assert_eq!(first.len(), 1);
        let again = unseen(&mut shown, &[arrival("a", "Rp 1", "", None), arrival("b", "Rp 2", "", None)]);
        assert_eq!(again.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["b"]);
        for i in 0..60 {
            unseen(&mut shown, &[arrival(&format!("n{i}"), "Rp 1", "", None)]);
        }
        assert_eq!(shown.len(), 50);
        assert_eq!(shown.last().map(String::as_str), Some("n59"));
    }

    fn spent(id: &str, text: &str, title: &str, wallet: Option<&str>) -> Arrival {
        Arrival { kind: "expense".into(), ..arrival(id, text, title, wallet) }
    }

    fn txn(id: &str, kind: &str, title: &str, text: &str) -> Txn {
        Txn {
            id: id.into(),
            kind: kind.into(),
            title: title.into(),
            amount: 1.0,
            text: text.into(),
            category: None,
            wallet: Some(WalletRef { name: "BCA".into(), color: String::new() }),
            to_wallet: None,
            at: String::new(),
            auto: false,
        }
    }

    fn day(date: &str, ids: &[&str]) -> Day {
        Day { date: date.into(), count: ids.len(), in_sum: 0.0, out: 0.0, in_text: String::new(), out_text: String::new(), rows: ids.iter().map(|i| txn(i, "expense", "x", "Rp 1")).collect() }
    }

    #[test]
    fn money_out_notices() {
        let one = money_notice(&[spent("a", "Rp 89.000", "Tokopedia", Some("BCA"))]).unwrap();
        assert_eq!((one.title.as_str(), one.shown.as_str(), one.hidden.as_str(), one.flow), ("Money out · BCA", "Rp 89.000 · Tokopedia", "Amount and payee hidden", Some(Flow::Out)));
        let two = money_notice(&[spent("a", "Rp 89.000", "", None), spent("b", "Rp 25.000", "", None)]).unwrap();
        assert_eq!((two.title.as_str(), two.hidden.as_str()), ("Money out · 2 new", "Amounts and payees hidden"));
        let both = money_notice(&[arrival("a", "Rp 1.250.000", "Gaji", Some("BCA")), spent("b", "Rp 89.000", "", None)]).unwrap();
        assert_eq!((both.title.as_str(), both.shown.as_str(), both.hidden.as_str(), both.flow), ("Oyenza · 2 new", "+Rp 1.250.000 · \u{2212}Rp 89.000", "Amounts hidden", None));
        assert_eq!(money_notice(&[arrival("a", "Rp 1", "", None)]).unwrap().flow, Some(Flow::In));
    }

    #[test]
    fn activity_parses_both_answers() {
        let full = parse_activity(
            r##"{"cursor":"77","today":"2026-10-24","currency":"IDR","days":[{"date":"2026-10-24","count":2,"in":1250000,"out":25000,"inText":"Rp 1.250.000","outText":"Rp 25.000","rows":[
              {"id":"t1","type":"income","title":"Gaji Oktober","amount":1250000,"text":"Rp 1.250.000","category":"Gaji","wallet":{"name":"BCA","color":"#0060AF"},"at":"2026-10-24T12:40:00.000Z","auto":true},
              {"id":"t2","type":"transfer","title":"","amount":500000,"text":"Rp 500.000","category":null,"wallet":{"name":"BCA","color":""},"toWallet":{"name":"GoPay","color":""},"at":"2026-10-24T02:00:00.000Z","auto":false}]},
              {"date":"2026-10-23","count":0,"in":0,"out":0,"inText":"Rp 0","outText":"Rp 0","rows":[]}]}"##,
        )
        .unwrap();
        let ActivityAnswer::Full(a) = full else { panic!("a full answer") };
        assert_eq!((a.cursor.as_str(), a.today.as_str(), a.days.len()), ("77", "2026-10-24", 2));
        let d = a.day("2026-10-24").unwrap();
        assert_eq!((d.count, d.in_sum, d.out_text.as_str()), (2, 1_250_000.0, "Rp 25.000"));
        assert_eq!((d.rows[0].flow(), d.rows[0].auto, d.rows[1].flow()), (Flow::In, true, Flow::Move));
        assert_eq!(d.rows[1].to_wallet.as_ref().map(|w| w.name.as_str()), Some("GoPay"));
        assert_eq!(
            parse_activity(r#"{"cursor":"77","today":"2026-10-24","changed":false}"#),
            Some(ActivityAnswer::Unchanged { cursor: "77".into(), today: "2026-10-24".into() })
        );
        assert_eq!(parse_activity(r#"{"cursor":"77"}"#), None);
        assert_eq!(parse_activity("<html>"), None);
    }

    #[test]
    fn health_parses_the_server_shapes() {
        use crate::health::HealthSource;
        assert_eq!(health_url(), "https://api.oyenza.app/island/health");
        assert_eq!(HEALTH_EVERY_S, 900);
        // Off in Oyenza, paused, or nothing kept.
        assert_eq!(parse_health(r#"{"health":null}"#), Some(None));
        // The contract's example: the second day has no distance or energy.
        let full = parse_health(
            r#"{"health":{"goal":8000,"source":"apple_health","updated_at":1761293520,"days":[
              {"date":"2026-10-23","steps":9120,"distance_m":6400,"active_kcal":388},{"date":"2026-10-24","steps":8412}]}}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!((full.goal, full.source, full.updated_at, full.days.len()), (8000, HealthSource::AppleHealth, 1_761_293_520, 2));
        let first = full.day("2026-10-23").unwrap();
        assert_eq!((first.steps, first.distance_m, first.active_kcal), (9120, Some(6400), Some(388)));
        let last = full.day("2026-10-24").unwrap();
        assert_eq!((last.steps, last.distance_m, last.active_kcal), (8412, None, None));
        // Keys a newer server adds are left out, and so is a health app this version doesn't know.
        let newer = parse_health(r#"{"v":2,"health":{"goal":6000,"source":"health_connect","updated_at":1,"phone":"x","days":[{"date":"2026-10-24","steps":5,"floors":3}]}}"#);
        assert_eq!(newer.unwrap().unwrap().source, HealthSource::HealthConnect);
        let other = parse_health(r#"{"health":{"goal":8000,"source":"garmin","updated_at":1,"days":[]}}"#).unwrap().unwrap();
        assert_eq!(other.source, HealthSource::Other);
        // What doesn't read is an error, never "off".
        let bad = [
            "<html>",
            "{}",
            "[]",
            r#"{"error":"rate_limited"}"#,
            r#"{"health":{"goal":8000}}"#,
            r#"{"health":{"goal":8000,"source":"apple_health","updated_at":1,"days":[{"date":"2026-10-24","steps":-1}]}}"#,
        ];
        for body in bad {
            assert_eq!(parse_health(body), None, "{body}");
        }
    }

    #[test]
    fn the_strip_and_its_headings() {
        let today = "2026-10-24"; // a Saturday
        let labels: Vec<String> = strip(today).into_iter().map(|(_, l)| l).collect();
        assert_eq!(labels, ["Sun", "Mon", "Tue", "Wed", "Thu", "Yesterday", "Today", "Tomorrow", "Later"]);
        assert_eq!(strip(today)[0].0, DayPick::Past(6));
        assert_eq!(pick_date(DayPick::Past(6), today).as_deref(), Some("2026-10-18"));
        assert_eq!(pick_date(DayPick::Today, "2026-03-01").as_deref(), Some("2026-03-01"));
        assert_eq!(pick_date(DayPick::Past(1), "2026-03-01").as_deref(), Some("2026-02-28"));
        assert_eq!(pick_date(DayPick::Tomorrow, today), None);
        assert_eq!(day_heading(DayPick::Today, today, 5), "TODAY · 5 TRANSACTIONS");
        assert_eq!(day_heading(DayPick::Past(1), today, 1), "YESTERDAY · 1 TRANSACTION");
        assert_eq!(day_heading(DayPick::Past(2), today, 0), "THU 22 OCT");
        assert_eq!(day_heading(DayPick::Tomorrow, today, 3), "TOMORROW · SUN 25 OCT");
        assert_eq!(day_heading(DayPick::Later, today, 3), "LATER · THE NEXT 30 DAYS");
        assert_eq!(short_date("2026-10-26").as_deref(), Some("Mon 26 Oct"));
        assert_eq!(empty_day_text(DayPick::Later), "Nothing due in the next 30 days");
    }

    #[test]
    fn day_totals_hide_like_every_sum() {
        let mut d = day("2026-10-24", &[]);
        assert_eq!(day_totals(&d, false), "");
        d.in_sum = 1_250_000.0;
        d.in_text = "Rp 1.250.000".into();
        assert_eq!(day_totals(&d, true), "in Rp 1.250.000");
        d.out = 25_000.0;
        d.out_text = "Rp 25.000".into();
        assert_eq!(day_totals(&d, false), "in Rp •••  ·  out Rp •••");
        assert_eq!(day_totals(&d, true), "in Rp 1.250.000  ·  out Rp 25.000");
    }

    #[test]
    fn rows_say_what_and_which_way() {
        let coffee = txn("c", "expense", "Kopi", "Rp 25.000");
        assert_eq!((txn_title(&coffee), txn_detail(&coffee, "19:40"), signed_sum(&coffee, true)), ("Kopi".to_string(), "BCA · 19:40".to_string(), "\u{2212}Rp 25.000".to_string()));
        assert_eq!(signed_sum(&coffee, false), "\u{2212}Rp •••");
        let pay = txn("p", "income", "", "Rp 1.250.000");
        assert_eq!((txn_title(&pay), signed_sum(&pay, false)), ("Money in".to_string(), "+Rp •••".to_string()));
        let lunch = Txn { category: Some("Makan & Minum".into()), ..txn("l", "expense", " ", "Rp 40.000") };
        assert_eq!(txn_title(&lunch), "Makan & Minum");
        let mut mv = txn("m", "transfer", "", "Rp 500.000");
        mv.to_wallet = Some(WalletRef { name: "GoPay".into(), color: String::new() });
        assert_eq!((txn_title(&mv), txn_detail(&mv, "09:00"), signed_sum(&mv, true)), ("BCA to GoPay".to_string(), "Own transfer · 09:00".to_string(), "Rp 500.000".to_string()));
        mv.title = "Top up".into();
        assert_eq!(txn_detail(&mv, "09:00"), "BCA to GoPay · 09:00");
        let no_wallet = Txn { wallet: None, ..txn("n", "expense", "Parkir", "Rp 5.000") };
        assert_eq!(txn_detail(&no_wallet, "08:10"), "08:10");
    }

    #[test]
    fn bills_by_day_of_the_strip() {
        let today = "2026-10-24";
        let all = vec![
            bill("card", "Kartu Kredit BCA", "2026-10-24", None),
            bill("subscription", "Spotify", "2026-10-25", Some("Rp 54.990")),
            bill("subscription", "Netflix", "2026-10-29", Some("Rp 54.000")),
            bill("subscription", "Cloud", "2026-11-20", Some("Rp 29.000")),
        ];
        let names = |p: DayPick| bills_for(&all, today, p).iter().map(|b| b.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(DayPick::Today), ["Kartu Kredit BCA"]);
        assert_eq!(names(DayPick::Tomorrow), ["Spotify"]);
        assert_eq!(names(DayPick::Later), ["Netflix", "Cloud"]);
        assert!(names(DayPick::Past(1)).is_empty());
    }

    #[test]
    fn only_what_lands_while_open_glows() {
        let mut seen = Seen::default();
        let first = vec![day("2026-10-24", &["a", "b"])];
        seen.note(&first, 100.0);
        assert_eq!((seen.glow("a", 100.0), seen.glowing_until(&first[0], 100.0)), (0.0, None));
        let next = vec![day("2026-10-24", &["c", "a", "b"])];
        seen.note(&next, 130.0);
        assert_eq!(seen.glow("c", 130.0), 1.0);
        assert!((seen.glow("c", 160.0) - 0.5).abs() < 1e-6);
        assert_eq!(seen.glow("c", 200.0), 0.0);
        assert_eq!(seen.glowing_until(&next[0], 140.0), Some(130.0 + GLOW_S));
        // A row that went (deleted in the app) is forgotten.
        seen.note(&[day("2026-10-24", &["c"])], 150.0);
        assert_eq!(seen.glow("a", 150.0), 0.0);
        assert_eq!(seen.at.len(), 1);
    }
}
