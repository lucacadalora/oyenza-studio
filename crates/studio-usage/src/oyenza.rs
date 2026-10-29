//! Oyenza Inference (inference.oyenza.com): the balance and spend of the
//! person's account on Oyenza's own API once they connect it, and the words
//! its card and the notices use.
//!
//! Connecting is a device code, the way the island signs in to oyenza.app
//! (`poni_core::oyenza`): the AI tab shows 8 letters, the person approves them in
//! the inference dashboard (where they are signed in anyway), and Studio gets
//! a read-only key of its own (`oyis_…`), kept sealed under [`SECRET`]. The
//! key reads the summary below and can disconnect itself; it can't call a
//! model, spend credit, or see prompts, keys, payments or settings. Nothing
//! about the person's other plans goes to Oyenza.
//!
//! Everything here is pure: the requests (`/v1/studio/*`), reading the
//! answers, when to ask again while a code is on screen, and the words. The
//! AI tab's worker (`services/usage.rs`) does the HTTP and keeps the key.

use super::{mask_email, num, reply_json, sort_small_by, time, Ctx, DayUse, Dirs, Got, Money, NetFail, Os, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Spend, Step, Trouble, Want};
use serde_json::Value;

/// Where Studio asks. A test build points [`URL_VAR`] at another copy of the
/// server, the way `PONI_STOCKS_URL` does for Stocks.
pub const BASE: &str = "https://inference.oyenza.com";
pub const URL_VAR: &str = "PONI_INFERENCE_URL";
/// The name the key is kept under ([`Want::Secret`]).
pub const SECRET: &str = "oyenza-inference";
/// What the start request calls this app.
const APP: &str = "studio";
/// The spend the summary carries, as the card labels it: the UTC day so far,
/// and the last 7 and 30 days (rolling).
const SPEND: [(&str, &str); 3] = [("Today", "today_usd"), ("7 days", "week_usd"), ("30 days", "month_usd")];

/// The card. No `takes_key`: the key comes from Connect, never from a field
/// (the dashboard never shows it), so the tab offers Connect for
/// [`Trouble::NeedsKey`].
pub struct Oyenza;

impl Provider for Oyenza {
    fn id(&self) -> ProviderId {
        ProviderId::Oyenza
    }

    fn env(&self) -> &'static [&'static str] {
        &[URL_VAR]
    }

    fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
        vec![Want::Secret(SECRET)]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let key = match cx.got(0) {
            Got::Denied => return Step::Done(Err(Trouble::Denied)),
            got => got.text().map(str::trim).filter(|k| key_ok(k)),
        };
        let Some(key) = key else { return Step::Done(Err(Trouble::NeedsKey)) };
        if cx.reach == Reach::Local {
            return Step::Done(Err(Trouble::NeedsOnline));
        }
        match replies.last() {
            None => Step::Fetch(summary_request(&base(cx.dirs.var(URL_VAR)), key)),
            Some(r) => Step::Done(read_summary(r, cx.now)),
        }
    }

    /// The server's numbers are a ledger read; it notes the key's last use at
    /// most once a minute, and a minute is plenty for a balance.
    fn min_gap_s(&self) -> u64 {
        60
    }
}

/// The server to ask: [`BASE`], or the address in [`URL_VAR`] (without its
/// trailing slash). The key goes wherever this points, so the variable takes
/// `https://` anywhere but plain `http://` only on this computer (a test
/// server); anything else is ignored.
pub fn base(var: Option<&str>) -> String {
    let ok = |u: &str| match u.split_once("://") {
        // No user info ("http://localhost:1@evil.example/" goes to evil.example).
        Some((scheme, rest)) if !rest.contains(char::is_whitespace) && !rest.contains('@') => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            // The host without its port: "[::1]:9000" -> "[::1]", "localhost:8080" -> "localhost".
            let host = match authority.find(']') {
                Some(end) if authority.starts_with('[') => &authority[..=end],
                _ => authority.split(':').next().unwrap_or(""),
            };
            !host.is_empty() && (scheme == "https" || (scheme == "http" && matches!(host, "127.0.0.1" | "localhost" | "[::1]")))
        }
        _ => false,
    };
    match var.map(str::trim) {
        Some(u) if ok(u) => u.trim_end_matches('/').to_string(),
        _ => BASE.to_string(),
    }
}

/// A key the device connect gave: `oyis_` and URL-safe letters. Anything else
/// kept under [`SECRET`] is never sent.
pub fn key_ok(key: &str) -> bool {
    key.len() <= 256 && key.strip_prefix("oyis_").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

// ---------------------------------------------------------------- the summary

pub fn summary_request(base: &str, key: &str) -> Request {
    Request::get(format!("{base}/v1/studio/summary")).bearer(key)
}

/// The summary as the card's snapshot, or why not. A 401 (the key was removed
/// in the dashboard, by a password change or by an 11th computer) is
/// [`Trouble::Refused`] ("Connect again"); a 403 `account_disabled` is
/// [`Trouble::Disabled`], where connecting again wouldn't help.
pub fn read_summary(r: &Result<Reply, NetFail>, now: i64) -> Outcome {
    if let Ok(reply) = r {
        if refusal(reply) == Some(Refusal::Disabled) {
            return Err(Trouble::Disabled);
        }
    }
    let v = reply_json(r)?;
    parse_summary(&v, now).ok_or_else(|| Trouble::Unreadable("summary without a balance or spend".into()))
}

fn parse_summary(v: &Value, now: i64) -> Option<Snapshot> {
    let mut s = Snapshot::new(ProviderId::Oyenza, v.get("as_of").and_then(time).unwrap_or(now), Source::Online);
    s.plan = v.get("tier").and_then(Value::as_str).and_then(plan_name).map(String::from);
    s.account = v.get("account").and_then(Value::as_str).map(mask_email).filter(|a| !a.is_empty());
    s.balance = v.get("balance_usd").and_then(num).map(Money::usd);
    for (label, field) in SPEND {
        if let Some(usd) = v.get("spend").and_then(|sp| sp.get(field)).and_then(num) {
            s.spend.push(Spend { label: label.into(), money: Money::usd(usd), cap: None });
        }
    }
    s.days = v.get("days").and_then(Value::as_array).map(|d| d.iter().filter_map(day_use).collect()).unwrap_or_default();
    sort_small_by(&mut s.days, |a, b| a.day > b.day);
    (s.balance.is_some() || !s.spend.is_empty()).then_some(s)
}

/// One of the 14 days (UTC, zeros included).
fn day_use(d: &Value) -> Option<DayUse> {
    let day = d.get("day")?.as_str()?.trim();
    let requests = d.get("requests").and_then(num).filter(|n| *n >= 0.0).map(|n| n as u64);
    (day.len() == 10).then(|| DayUse { day: day.into(), money: d.get("usd").and_then(num).map(Money::usd), requests, tokens: None })
}

/// The plan as the card says it, from the server's tier. A tier this version
/// doesn't know says nothing rather than something wrong.
fn plan_name(tier: &str) -> Option<&'static str> {
    match tier.trim().to_ascii_lowercase().as_str() {
        "trial" => Some("Trial"),
        "paid" => Some("Pay as you go"),
        _ => None,
    }
}

/// Why the server refused the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 401: removed in the dashboard, by a password change or by an 11th
    /// computer (or never valid). Connecting again gives a new one.
    Key,
    /// 403 `account_disabled`: connecting again wouldn't help.
    Disabled,
}

/// The reason behind a [`Trouble::Refused`], from the answer that said it.
pub fn refusal(r: &Reply) -> Option<Refusal> {
    match r.status {
        403 if poni_core::oyenza::error_code(&r.body).as_deref() == Some("account_disabled") => Some(Refusal::Disabled),
        401 | 403 => Some(Refusal::Key),
        _ => None,
    }
}

// ---------------------------------------------------------------- connecting

/// `device` is the computer's name as the system gives it (cleaned to one
/// line of 40 characters, as oyenza.app lists it), `version` this app's.
pub fn start_request(base: &str, device: &str, os: Os, version: &str) -> Request {
    let os = match os {
        Os::Windows => "windows",
        Os::Mac => "mac",
    };
    let body = serde_json::json!({ "device": poni_core::oyenza::pc_name(device), "os": os, "app": APP, "version": version });
    Request::post_json(format!("{base}/v1/studio/connect/start"), body.to_string())
}

pub fn token_request(base: &str, poll: &str) -> Request {
    Request::post_json(format!("{base}/v1/studio/connect/token"), serde_json::json!({ "poll": poll }).to_string())
}

/// "Disconnect": the server forgets this computer's key.
pub fn disconnect_request(base: &str, key: &str) -> Request {
    Request::post_json(format!("{base}/v1/studio/disconnect"), "{}").bearer(key)
}

/// The disconnect reached the server, or the key was gone already (401).
pub fn disconnected(r: &Result<Reply, NetFail>) -> bool {
    matches!(r, Ok(a) if a.status == 200 || a.status == 401)
}

/// The code as the card shows it ("KPJX-TMNQ"), however the server wrote it:
/// it matches codes without case, spaces or dashes.
pub fn show_code(raw: &str) -> Option<String> {
    let c: String = raw.chars().filter(|c| !matches!(c, ' ' | '-')).map(|c| c.to_ascii_uppercase()).collect();
    (c.len() == 8 && c.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| format!("{}-{}", &c[..4], &c[4..]))
}

/// Asks about a code come every `interval` seconds (the server says 5), kept
/// between these as the island does; each slow-down adds 5, up to the slowest.
const EVERY_MIN_S: u64 = 2;
const EVERY_MAX_S: u64 = 30;
const SLOWEST_S: u64 = 60;
/// A code is kept at most this long, whatever `expires_in` says (the server's
/// last 10 minutes).
const CODE_MAX_S: u64 = 30 * 60;

/// A code on screen, waiting for the person to approve it.
#[derive(Clone, PartialEq, Eq)]
pub struct Pending {
    /// "KPJX-TMNQ".
    pub code: String,
    /// The dashboard page that approves it (the tab's link).
    pub url: String,
    /// When the code stops working, unix seconds.
    pub expires_at: i64,
    /// When to ask next, unix seconds.
    pub next_at: i64,
    /// Seconds between asks.
    pub every_s: u64,
    /// What the asks carry. As private as the key: it claims the key once the
    /// code is approved.
    poll: String,
}

/// Everything but `poll`, should a worker's state ever be printed.
impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending").field("code", &self.code).field("expires_at", &self.expires_at).field("next_at", &self.next_at).field("every_s", &self.every_s).finish_non_exhaustive()
    }
}

/// The start request's answer as a code to show, or why connecting can't
/// start (a code for [`connect_failure_text`]).
pub fn started(r: &Result<Reply, NetFail>, base: &str, now: i64) -> Result<Pending, &'static str> {
    let reply = match r {
        Err(NetFail::Timeout | NetFail::Network(_)) => return Err(OFFLINE),
        Err(NetFail::Other(_)) => return Err(OTHER),
        Ok(a) if a.status == 429 => return Err(BUSY),
        Ok(a) if a.status >= 500 => return Err(OFFLINE),
        Ok(a) if a.status == 404 => return Err(SOON),
        Ok(a) if a.status != 200 => return Err(OTHER),
        Ok(a) => a,
    };
    let v: Value = serde_json::from_str(&reply.body).map_err(|_| OTHER)?;
    let code = v.get("code").and_then(Value::as_str).and_then(show_code).ok_or(OTHER)?;
    let poll = v.get("poll").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).ok_or(OTHER)?;
    // The server's link, if it is the same server's page; else the page the
    // contract names.
    let url = match v.get("url").and_then(Value::as_str) {
        Some(u) if u.starts_with(&format!("{base}/")) => u.to_string(),
        _ => format!("{base}/app/connect?code={}", code.replace('-', "")),
    };
    let secs = |k: &str, default: u64| v.get(k).and_then(num).filter(|n| *n > 0.0).map_or(default, |n| n as u64);
    let every_s = secs("interval", 5).clamp(EVERY_MIN_S, EVERY_MAX_S);
    let lasts_s = secs("expires_in", 600).clamp(30, CODE_MAX_S);
    Ok(Pending { code, url, expires_at: now + lasts_s as i64, next_at: now + every_s as i64, every_s, poll: poll.to_string() })
}

/// The key, on its way from an approval to the worker's sealed store.
/// Printed (`{:?}`), it shows only its kind, so it can't reach a log.
#[derive(Clone, PartialEq, Eq)]
pub struct Key(String);

impl Key {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("oyis_…")
    }
}

/// What asking about a code answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    Pending,
    /// Approved: the key (given exactly once) and the account, masked.
    Approved { key: Key, account: String },
    Denied,
    /// Ran out, already used, or unknown to the server.
    Expired,
    /// Asked faster than `interval` (429).
    SlowDown { retry_after_s: Option<u64> },
}

/// None when the answer says nothing this version reads: no answer, a server
/// error, or a shape it doesn't know.
pub fn parse_claim(r: &Result<Reply, NetFail>) -> Option<Claim> {
    let r = r.as_ref().ok()?;
    if r.status == 429 {
        return Some(Claim::SlowDown { retry_after_s: r.retry_after_s });
    }
    if r.status != 200 {
        return None;
    }
    let v: Value = serde_json::from_str(&r.body).ok()?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim);
    match text("status")? {
        "pending" => Some(Claim::Pending),
        "approved" => {
            let key = text("key").filter(|k| key_ok(k))?;
            Some(Claim::Approved { key: Key(key.to_string()), account: mask_email(text("account").unwrap_or("")) })
        }
        "denied" => Some(Claim::Denied),
        "expired" => Some(Claim::Expired),
        _ => None,
    }
}

/// What the worker does after an answer about a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    /// Keep the code on screen and ask again after [`Pending::wait_s`].
    Wait,
    /// Keep `key` sealed under [`SECRET`] and say [`connected_text`].
    Approved { key: Key, account: String },
    /// It ended without a key: say [`connect_failure_text`] of this.
    Failed(&'static str),
}

impl Pending {
    /// Was the code approved yet?
    pub fn ask(&self, base: &str) -> Request {
        token_request(base, &self.poll)
    }

    /// Seconds until the next ask (0: now), or None once the code has run out:
    /// then stop asking and say [`EXPIRED`].
    pub fn wait_s(&self, now: i64) -> Option<u64> {
        (now < self.expires_at).then(|| (self.next_at.min(self.expires_at) - now).max(0) as u64)
    }

    /// What an answer to [`Pending::ask`] means, and when to ask next.
    pub fn heard(&mut self, r: &Result<Reply, NetFail>, now: i64) -> Poll {
        match parse_claim(r) {
            Some(Claim::Approved { key, account }) => return Poll::Approved { key, account },
            Some(Claim::Denied) => return Poll::Failed(DENIED),
            Some(Claim::Expired) => return Poll::Failed(EXPIRED),
            Some(Claim::SlowDown { retry_after_s }) => {
                self.every_s = (self.every_s + 5).min(SLOWEST_S);
                self.next_at = now + self.every_s.max(retry_after_s.unwrap_or(0).min(CODE_MAX_S)) as i64;
            }
            Some(Claim::Pending) => self.next_at = now + self.every_s as i64,
            // A request the server won't take (an older server): asking again
            // won't change that.
            None if matches!(r, Ok(a) if (400..500).contains(&a.status)) => return Poll::Failed(OTHER),
            // No answer, or a server error: ask again until the code runs out.
            None => self.next_at = now + self.every_s as i64,
        }
        if now >= self.expires_at {
            Poll::Failed(EXPIRED)
        } else {
            Poll::Wait
        }
    }
}

// ---------------------------------------------------------------- words

/// The card before connecting: the button, and the line beside it.
pub const CONNECT: &str = "Connect";
pub const CONNECT_LINE: &str = "Connect to see your balance and spend here";
/// While the code is on its way.
pub const STARTING: &str = "Getting a code…";
/// The button once a code ran out or was refused, or the key stopped working
/// ([`Refusal::Key`]).
pub const CONNECT_AGAIN: &str = "Connect again";
pub const DISCONNECT: &str = "Disconnect";

/// Why connecting stopped without a key ([`started`], [`Poll::Failed`]): the
/// app's link state keeps one for [`connect_failure_text`].
pub const EXPIRED: &str = "expired";
pub const DENIED: &str = "denied";
pub const OFFLINE: &str = "offline";
pub const BUSY: &str = "busy";
pub const OTHER: &str = "other";
/// The server has no device connect yet (404): it's on its way, not broken.
pub const SOON: &str = "soon";
/// Disconnected on this computer, but the server couldn't be told: the key
/// still works there until it is removed under Connected apps.
pub const UNTOLD: &str = "untold";

/// "Approve KPJX-TMNQ on inference.oyenza.com". `at` is the server or the
/// approve link: only its host shows.
pub fn approve_text(code: &str, at: &str) -> String {
    let host = at.split_once("://").map_or(at, |(_, h)| h);
    format!("Approve {code} on {}", host.split(['/', '?', '#']).next().unwrap_or(host))
}

/// "9:41" left on the code.
pub fn time_left(expires_at: i64, now: i64) -> String {
    poni_core::oyenza::code_time_left((expires_at - now) as f64)
}

/// "Connected as l•••@example.com".
pub fn connected_text(account: &str) -> String {
    match account.trim() {
        "" => "Connected".into(),
        a => format!("Connected as {}", mask_email(a)),
    }
}

/// Title and line when connecting stops without a key; the card offers
/// [`CONNECT_AGAIN`] with each.
pub fn connect_failure_text(code: &str) -> (&'static str, &'static str) {
    match code {
        EXPIRED => ("Code expired", "Connect again for a new one"),
        DENIED => ("Not connected", "The code was refused on inference.oyenza.com"),
        OFFLINE => ("Can't reach Oyenza Inference", "Check the connection and try again"),
        BUSY => ("Too many tries", "Wait a few minutes, then connect again"),
        SOON => ("Coming soon", "Oyenza Inference can't connect apps yet"),
        UNTOLD => ("Disconnected here only", "Remove this computer under Connected apps too"),
        _ => ("Couldn't connect", "Try again in a minute"),
    }
}

/// Title and line for a refused key. [`Refusal::Key`] offers
/// [`CONNECT_AGAIN`]; a switched-off account doesn't (the dashboard's words).
pub fn refused_text(why: Refusal) -> (&'static str, &'static str) {
    match why {
        Refusal::Key => ("No longer connected", "This computer was disconnected from your account"),
        Refusal::Disabled => ("Account switched off", "Write to hello@oyenza.com"),
    }
}

/// The nudge's short form, for a notice's second line.
pub const NUDGE: &str = "Keep going on Oyenza Inference";

/// The line under a plan that ran out: "Out of Claude Code until 14:20? Keep
/// going on Oyenza Inference." `until` is its reset as the card says it
/// ("14:20", "Thu"). None for Oyenza Inference itself.
pub fn nudge(out: ProviderId, until: Option<&str>) -> Option<String> {
    if out == ProviderId::Oyenza {
        return None;
    }
    let until = until.map(str::trim).filter(|u| !u.is_empty()).map(|u| format!(" until {u}")).unwrap_or_default();
    Some(format!("Out of {}{until}? {NUDGE}.", out.name()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_FETCHES;

    const SUMMARY: &str = include_str!("fixtures/oyenza/summary.json");
    const ZEROS: &str = include_str!("fixtures/oyenza/summary-zeros.json");
    const START: &str = include_str!("fixtures/oyenza/start.json");
    const APPROVED: &str = include_str!("fixtures/oyenza/token-approved.json");
    const KEY: &str = "oyis_test-token";
    /// 2026-10-27T05:00:00Z, the fixtures' `as_of`.
    const NOW: i64 = 1_793_077_200;

    fn reply(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    fn key() -> Got {
        Got::Text(format!(" {KEY}\n"))
    }

    /// One refresh as the worker runs it: `got` read, each request answered
    /// by `answer`, until the card is done. Returns what it asked, too.
    fn refresh(dirs: &Dirs, got: Got, reach: Reach, answer: &dyn Fn(&Request) -> Result<Reply, NetFail>) -> (Outcome, Vec<Request>) {
        let got = [got];
        let cx = Ctx { dirs, got: &got, now: NOW, reach };
        let (mut replies, mut asked) = (Vec::new(), Vec::new());
        loop {
            match Oyenza.next(&cx, &replies) {
                Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
                Step::Done(o) => return (o, asked),
                Step::Fetch(req) => {
                    assert!(asked.len() < MAX_FETCHES, "too many requests");
                    replies.push(answer(&req));
                    asked.push(req);
                }
            }
        }
    }

    fn answered(status: u16, body: &str) -> Outcome {
        let (o, asked) = refresh(&Dirs::sample(Os::Windows), key(), Reach::Online, &|_| reply(status, body));
        assert_eq!(asked.len(), 1);
        o
    }

    fn spend(label: &str, usd: f64) -> Spend {
        Spend { label: label.into(), money: Money::usd(usd), cap: None }
    }

    #[test]
    fn asks_for_its_key_on_both_systems() {
        for os in [Os::Windows, Os::Mac] {
            assert_eq!(Oyenza.wants(&Dirs::sample(os)), [Want::Secret("oyenza-inference")]);
        }
        assert_eq!(Oyenza.id(), ProviderId::Oyenza);
        assert_eq!(Oyenza.env(), ["PONI_INFERENCE_URL"]);
        assert_eq!(Oyenza.min_gap_s(), 60);
        // Connect, not a field to paste a key into.
        assert_eq!(Oyenza.takes_key(), None);
    }

    #[test]
    fn without_a_key_it_offers_connect() {
        let never = |_: &Request| -> Result<Reply, NetFail> { panic!("asked without a key") };
        for os in [Os::Windows, Os::Mac] {
            for reach in [Reach::Online, Reach::Local] {
                for got in [Got::Missing, Got::Text("  ".into()), Got::Text("sk-or-v1-abc".into()), Got::Text("oyis_ has spaces".into()), Got::Text("oyis_".into())] {
                    assert_eq!(refresh(&Dirs::sample(os), got, reach, &never).0, Err(Trouble::NeedsKey));
                }
            }
            assert_eq!(refresh(&Dirs::sample(os), Got::Denied, Reach::Online, &never).0, Err(Trouble::Denied));
        }
    }

    #[test]
    fn reads_the_summary() {
        let (o, asked) = refresh(&Dirs::sample(Os::Mac), key(), Reach::Online, &|_| reply(200, SUMMARY));
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].method, crate::Method::Get);
        assert_eq!(asked[0].url, "https://inference.oyenza.com/v1/studio/summary");
        assert_eq!(asked[0].headers, [("Authorization".to_string(), "Bearer oyis_test-token".to_string())]);
        let s = o.unwrap();
        assert_eq!((s.provider, s.source, s.as_of), (ProviderId::Oyenza, Source::Online, NOW));
        assert_eq!(s.plan.as_deref(), Some("Trial"));
        assert_eq!(s.account.as_deref(), Some("a•••@example.com"));
        assert_eq!(s.balance, Some(Money::usd(4.2)));
        assert_eq!(s.balance.as_ref().unwrap().fmt(), "$4.20");
        assert_eq!(s.spend, [spend("Today", 0.12), spend("7 days", 1.03), spend("30 days", 3.4)]);
        assert_eq!(s.days.len(), 14);
        assert_eq!(s.days[0], DayUse { day: "2026-10-14".into(), money: Some(Money::usd(0.1)), requests: Some(20), tokens: None });
        assert_eq!(s.days[13], DayUse { day: "2026-10-27".into(), money: Some(Money::usd(0.12)), requests: Some(41), tokens: None });
        // The last 7 days add up to the week's spend.
        assert_eq!(s.days[7..].iter().map(|d| d.money.as_ref().unwrap().micros).sum::<i64>(), Money::usd(1.03).micros);
        assert!(s.windows.is_empty() && s.tokens_today.is_none());
        // The key never lands in what the tab keeps.
        assert!(!format!("{s:?}").contains("test-token"));
    }

    #[test]
    fn zeros_are_zeros() {
        let s = answered(200, ZEROS).unwrap();
        assert_eq!(s.plan.as_deref(), Some("Pay as you go"));
        assert_eq!(s.balance, Some(Money::usd(0.0)));
        assert_eq!(s.spend, [spend("Today", 0.0), spend("7 days", 0.0), spend("30 days", 0.0)]);
        assert_eq!(s.spend[0].money.fmt(), "$0.00");
        assert_eq!(s.days.len(), 14);
        assert!(s.days.iter().all(|d| d.money == Some(Money::usd(0.0)) && d.requests == Some(0)));
    }

    #[test]
    fn unknown_is_not_zero() {
        // A tier this version doesn't know, a missing week, days newest first
        // and one without a date, no time: what is there, in order, and no more.
        let body = r#"{"tier":"team","balance_usd":"1.5","spend":{"today_usd":0.2,"month_usd":2},
            "days":[{"day":"2026-10-27","usd":0.2},{"day":"2026-10-26","requests":3},{"usd":9}]}"#;
        let s = answered(200, body).unwrap();
        assert_eq!((s.plan, s.account, s.as_of), (None, None, NOW));
        assert_eq!(s.balance, Some(Money::usd(1.5)));
        assert_eq!(s.spend, [spend("Today", 0.2), spend("30 days", 2.0)]);
        let days: Vec<_> = s.days.iter().map(|d| (d.day.as_str(), d.money.clone(), d.requests)).collect();
        assert_eq!(days, [("2026-10-26", None, Some(3)), ("2026-10-27", Some(Money::usd(0.2)), None)]);
        // A balance alone is a card; neither a balance nor a spend is not.
        assert!(answered(200, r#"{"balance_usd":-0.5}"#).is_ok());
        assert_eq!(answered(200, r#"{"account":"a•••@example.com","tier":"trial"}"#), Err(Trouble::Unreadable("summary without a balance or spend".into())));
    }

    #[test]
    fn refused_keys_say_why() {
        let bad = r#"{"error":"invalid_key"}"#;
        let off = r#"{"error":"account_disabled"}"#;
        assert_eq!(answered(401, bad), Err(Trouble::Refused));
        assert_eq!(answered(403, off), Err(Trouble::Disabled));
        let r = |status, body: &str| Reply { status, body: body.into(), retry_after_s: None };
        assert_eq!(refusal(&r(401, bad)), Some(Refusal::Key));
        assert_eq!(refusal(&r(403, off)), Some(Refusal::Disabled));
        assert_eq!(refusal(&r(403, "<html>")), Some(Refusal::Key));
        assert_eq!(refusal(&r(200, off)), None);
        assert_eq!(refused_text(Refusal::Disabled), ("Account switched off", "Write to hello@oyenza.com"));
        assert_eq!(refused_text(Refusal::Key).0, "No longer connected");
    }

    #[test]
    fn busy_offline_and_unreadable() {
        let busy = |_: &Request| Ok(Reply { status: 429, body: r#"{"error":"rate_limited"}"#.into(), retry_after_s: Some(30) });
        assert_eq!(refresh(&Dirs::sample(Os::Windows), key(), Reach::Online, &busy).0, Err(Trouble::Busy { retry_after_s: Some(30) }));
        assert_eq!(answered(503, "<html>"), Err(Trouble::Offline));
        let timeout = |_: &Request| Err(NetFail::Timeout);
        assert_eq!(refresh(&Dirs::sample(Os::Windows), key(), Reach::Online, &timeout).0, Err(Trouble::Offline));
        assert_eq!(answered(200, "<html>"), Err(Trouble::Unreadable("not JSON".into())));
        assert_eq!(answered(404, "{}"), Err(Trouble::Unreadable("HTTP 404".into())));
        for body in ["{}", "[]", "null", r#"{"spend":"none","days":7}"#] {
            assert!(matches!(answered(200, body), Err(Trouble::Unreadable(_))), "{body}");
        }
    }

    #[test]
    fn a_local_refresh_never_asks() {
        let never = |_: &Request| -> Result<Reply, NetFail> { panic!("asked on a local refresh") };
        assert_eq!(refresh(&Dirs::sample(Os::Windows), key(), Reach::Local, &never).0, Err(Trouble::NeedsOnline));
    }

    #[test]
    fn a_test_build_asks_another_server() {
        for os in [Os::Windows, Os::Mac] {
            let mut dirs = Dirs::sample(os);
            dirs.env.push((URL_VAR.into(), " http://127.0.0.1:8090/ ".into()));
            let (_, asked) = refresh(&dirs, key(), Reach::Online, &|_| reply(200, SUMMARY));
            assert_eq!(asked[0].url, "http://127.0.0.1:8090/v1/studio/summary");
        }
        assert_eq!(base(None), BASE);
        assert_eq!(base(Some("https://staging.example.com")), "https://staging.example.com");
        for good in ["https://staging.example.com:8443", "http://localhost:8080", "http://[::1]:9000"] {
            assert_eq!(base(Some(good)), good);
        }
        // Plain http anywhere but this computer would send the key in the clear.
        for bad in ["", "ftp://example.com", "inference.example.com", "https://", "https:///", "http://a b", "http://example.com", "http://192.168.1.5:8090", "http://localhost.example.com", "http://localhost:1@evil.example/", "https://user@inference.oyenza.com"] {
            assert_eq!(base(Some(bad)), BASE, "{bad}");
        }
    }

    #[test]
    fn connect_requests() {
        let start = start_request(BASE, "  DESKTOP-7Q2K\n", Os::Windows, "0.2.11");
        assert_eq!((start.method, start.url.as_str()), (crate::Method::Post, "https://inference.oyenza.com/v1/studio/connect/start"));
        let body: Value = serde_json::from_str(start.json.as_deref().unwrap()).unwrap();
        assert_eq!(body, serde_json::json!({ "device": "DESKTOP-7Q2K", "os": "windows", "app": "studio", "version": "0.2.11" }));
        assert!(!start.headers.iter().any(|(k, _)| k == "Authorization"));
        let mac = start_request("http://127.0.0.1:8090", "Sam’s MacBook Air", Os::Mac, "0.2.11");
        let body: Value = serde_json::from_str(mac.json.as_deref().unwrap()).unwrap();
        assert_eq!((body["device"].as_str(), body["os"].as_str()), (Some("Sam’s MacBook Air"), Some("mac")));
        assert_eq!(mac.url, "http://127.0.0.1:8090/v1/studio/connect/start");

        let p = started(&reply(200, START), BASE, NOW).unwrap();
        let ask = p.ask(BASE);
        assert_eq!(ask.url, "https://inference.oyenza.com/v1/studio/connect/token");
        assert_eq!(ask.json.as_deref(), Some(r#"{"poll":"test-poll-token"}"#));

        let bye = disconnect_request(BASE, KEY);
        assert_eq!((bye.method, bye.url.as_str(), bye.json.as_deref()), (crate::Method::Post, "https://inference.oyenza.com/v1/studio/disconnect", Some("{}")));
        assert!(bye.headers.contains(&("Authorization".into(), "Bearer oyis_test-token".into())));
        assert!(disconnected(&reply(200, r#"{"ok":true}"#)));
        assert!(disconnected(&reply(401, r#"{"error":"invalid_key"}"#)));
        assert!(!disconnected(&reply(502, "")));
        assert!(!disconnected(&Err(NetFail::Network("dns".into()))));
    }

    #[test]
    fn a_code_to_show() {
        let p = started(&reply(200, START), BASE, NOW).unwrap();
        assert_eq!((p.code.as_str(), p.url.as_str()), ("KPJX-TMNQ", "https://inference.oyenza.com/app/connect?code=KPJXTMNQ"));
        assert_eq!((p.expires_at, p.next_at, p.every_s), (NOW + 600, NOW + 5, 5));
        assert_eq!(p.wait_s(NOW), Some(5));
        assert!(format!("{p:?}").contains("KPJX-TMNQ") && !format!("{p:?}").contains("test-poll-token"));
        // However the server writes the code; a link to another site is never
        // opened, the page the contract names is.
        let odd = r#"{"code":"zxwv tsrq","poll":"p","url":"https://elsewhere.example/connect","interval":1,"expires_in":99999}"#;
        let q = started(&reply(200, odd), BASE, NOW).unwrap();
        assert_eq!((q.code.as_str(), q.url.as_str()), ("ZXWV-TSRQ", "https://inference.oyenza.com/app/connect?code=ZXWVTSRQ"));
        assert_eq!((q.every_s, q.expires_at), (2, NOW + 1800));
        let bare = started(&reply(200, r#"{"code":"BCDFGHJK","poll":"p"}"#), "http://127.0.0.1:8090", NOW).unwrap();
        assert_eq!((bare.url.as_str(), bare.every_s, bare.expires_at), ("http://127.0.0.1:8090/app/connect?code=BCDFGHJK", 5, NOW + 600));
        assert_eq!(show_code("kpjx-tmnq").as_deref(), Some("KPJX-TMNQ"));
        for bad in ["KPJ", "KPJX-TMNQ-B", "KPJX_TMNQ", "KPJXTMNÉ", ""] {
            assert_eq!(show_code(bad), None, "{bad}");
        }
        // Why it couldn't start.
        let starts = |r: Result<Reply, NetFail>| started(&r, BASE, NOW).err();
        assert_eq!(starts(Err(NetFail::Timeout)), Some(OFFLINE));
        assert_eq!(starts(Err(NetFail::Other("tls".into()))), Some(OTHER));
        assert_eq!(starts(Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(300) })), Some(BUSY));
        assert_eq!(starts(reply(502, "<html>")), Some(OFFLINE));
        assert_eq!(starts(reply(404, "{}")), Some(SOON));
        assert_eq!(connect_failure_text(SOON), ("Coming soon", "Oyenza Inference can't connect apps yet"));
        assert_eq!(starts(reply(403, "{}")), Some(OTHER));
        assert_eq!(starts(reply(200, "<html>")), Some(OTHER));
        assert_eq!(starts(reply(200, r#"{"code":"KPJ","poll":"p"}"#)), Some(OTHER));
        assert_eq!(starts(reply(200, r#"{"code":"KPJX-TMNQ","poll":" "}"#)), Some(OTHER));
    }

    #[test]
    fn every_connect_state() {
        let claim = |body: &str| parse_claim(&reply(200, body));
        assert_eq!(claim(r#"{"status":"pending"}"#), Some(Claim::Pending));
        let approved = claim(APPROVED);
        assert_eq!(approved, Some(Claim::Approved { key: Key(KEY.into()), account: "a•••@example.com".into() }));
        // Printed, the key shows only its kind.
        assert_eq!(format!("{approved:?}"), r#"Some(Approved { key: oyis_…, account: "a•••@example.com" })"#);
        assert_eq!(claim(r#"{"status":"denied"}"#), Some(Claim::Denied));
        // Ran out, already claimed, or a poll token the server doesn't know.
        assert_eq!(claim(r#"{"status":"expired"}"#), Some(Claim::Expired));
        // An approval without a usable key, or a status this version doesn't know.
        assert_eq!(claim(r#"{"status":"approved","key":"sk-test-token","account":"a•••@example.com"}"#), None);
        assert_eq!(claim(r#"{"status":"approved"}"#), None);
        assert_eq!(claim(r#"{"status":"thinking"}"#), None);
        assert_eq!(parse_claim(&reply(500, "")), None);
        assert_eq!(parse_claim(&Err(NetFail::Timeout)), None);

        let mut p = started(&reply(200, START), BASE, NOW).unwrap();
        assert_eq!(p.heard(&reply(200, r#"{"status":"pending"}"#), NOW + 5), Poll::Wait);
        assert_eq!((p.next_at, p.wait_s(NOW + 5)), (NOW + 10, Some(5)));
        // No answer or a server error: keep asking at the same pace.
        assert_eq!(p.heard(&Err(NetFail::Network("reset".into())), NOW + 10), Poll::Wait);
        assert_eq!(p.heard(&reply(502, "<html>"), NOW + 15), Poll::Wait);
        assert_eq!(p.next_at, NOW + 20);
        let Poll::Approved { key, account } = p.heard(&reply(200, APPROVED), NOW + 20) else { panic!("approved") };
        assert_eq!((key.as_str(), account.as_str()), (KEY, "a•••@example.com"));
        assert_eq!(p.heard(&reply(200, r#"{"status":"denied"}"#), NOW + 25), Poll::Failed(DENIED));
        assert_eq!(p.heard(&reply(200, r#"{"status":"expired"}"#), NOW + 25), Poll::Failed(EXPIRED));
        // A request the server won't take.
        assert_eq!(p.heard(&reply(404, "{}"), NOW + 25), Poll::Failed(OTHER));
    }

    #[test]
    fn slow_down_asks_less_often() {
        let mut p = started(&reply(200, START), BASE, NOW).unwrap();
        let slow = |retry| Ok(Reply { status: 429, body: r#"{"error":"slow_down"}"#.into(), retry_after_s: retry });
        assert_eq!(parse_claim(&slow(Some(12))), Some(Claim::SlowDown { retry_after_s: Some(12) }));
        // Retry-After longer than the new pace wins...
        assert_eq!(p.heard(&slow(Some(12)), NOW + 5), Poll::Wait);
        assert_eq!((p.every_s, p.next_at), (10, NOW + 17));
        // ...else the pace, 5 seconds slower each time, up to a minute.
        assert_eq!(p.heard(&slow(None), NOW + 17), Poll::Wait);
        assert_eq!((p.every_s, p.next_at), (15, NOW + 32));
        for _ in 0..20 {
            p.heard(&slow(Some(1)), NOW + 32);
        }
        assert_eq!(p.every_s, 60);
        // An absurd Retry-After still wakes the worker when the code runs out.
        p.heard(&slow(Some(u64::MAX)), NOW + 40);
        assert_eq!(p.wait_s(NOW + 40), Some(560));
    }

    #[test]
    fn a_code_runs_out() {
        let mut p = started(&reply(200, START), BASE, NOW).unwrap();
        assert_eq!(p.wait_s(NOW + 599), Some(0));
        assert_eq!(p.wait_s(NOW + 600), None);
        assert_eq!(p.heard(&reply(200, r#"{"status":"pending"}"#), NOW + 600), Poll::Failed(EXPIRED));
        assert_eq!(p.heard(&Err(NetFail::Timeout), NOW + 700), Poll::Failed(EXPIRED));
        assert_eq!(connect_failure_text(EXPIRED), ("Code expired", "Connect again for a new one"));
    }

    #[test]
    fn the_cards_words() {
        assert_eq!(CONNECT, "Connect");
        assert_eq!(approve_text("KPJX-TMNQ", BASE), "Approve KPJX-TMNQ on inference.oyenza.com");
        assert_eq!(approve_text("KPJX-TMNQ", "http://127.0.0.1:8090"), "Approve KPJX-TMNQ on 127.0.0.1:8090");
        let p = started(&reply(200, START), BASE, NOW).unwrap();
        assert_eq!(approve_text(&p.code, &p.url), "Approve KPJX-TMNQ on inference.oyenza.com");
        assert_eq!(time_left(NOW + 581, NOW), "9:41");
        assert_eq!(time_left(NOW, NOW + 3), "0:00");
        assert_eq!(connected_text("l•••@example.com"), "Connected as l•••@example.com");
        assert_eq!(connected_text("ana@example.com"), "Connected as a•••@example.com");
        assert_eq!(connected_text(" "), "Connected");
        assert_eq!(CONNECT_AGAIN, "Connect again");
        assert_eq!(connect_failure_text(DENIED).0, "Not connected");
        assert_eq!(connect_failure_text(OFFLINE).0, "Can't reach Oyenza Inference");
        assert_eq!(connect_failure_text(BUSY).0, "Too many tries");
        assert_eq!(connect_failure_text("whatever").0, "Couldn't connect");
        assert_eq!(nudge(ProviderId::Claude, Some("14:20")).as_deref(), Some("Out of Claude Code until 14:20? Keep going on Oyenza Inference."));
        assert_eq!(nudge(ProviderId::Codex, Some(" ")).as_deref(), Some("Out of Codex? Keep going on Oyenza Inference."));
        assert_eq!(nudge(ProviderId::Oyenza, None), None);
        let mut words = vec![CONNECT, CONNECT_LINE, STARTING, CONNECT_AGAIN, DISCONNECT, NUDGE, refused_text(Refusal::Key).0, refused_text(Refusal::Key).1];
        for code in [EXPIRED, DENIED, OFFLINE, BUSY, OTHER, UNTOLD] {
            words.extend([connect_failure_text(code).0, connect_failure_text(code).1]);
        }
        assert!(words.iter().all(|w| !w.contains('—') && !w.contains('–')));
    }
}
