//! OpenRouter: what the person's API key spent today, this week and this
//! month (against the key's limit when it has one) and the credits left on
//! the account, from OpenRouter's own API.
//!
//! The key is the one typed into Oyenza Studio, else `OPENROUTER_API_KEY`
//! (the variable tools that use OpenRouter read). A refresh asks
//! openrouter.ai twice with it, and it goes nowhere else:
//! - `GET /api/v1/key`: the key's spend, its limit, and whether the account
//!   is on the free tier. Every key may ask this, so it goes first, and a
//!   refused key or an ask to wait ends the refresh there.
//! - `GET /api/v1/credits`: credits bought and used; the balance is the
//!   difference. OpenRouter documents it for management keys only, so a
//!   regular key may be refused here: the card then shows the key's spend
//!   without a balance. A management key spends nothing itself, so its card
//!   is the balance alone.

use super::{micros, num, reply_json, Ctx, Dirs, KeyAsk, Money, NetFail, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Spend, Step, Trouble, Want, Window, WindowKind};
use serde_json::Value;

const KEY_URL: &str = "https://openrouter.ai/api/v1/key";
const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
/// The name a typed key is kept under.
const SECRET: &str = "openrouter";
const ENV: &str = "OPENROUTER_API_KEY";

pub struct OpenRouter;

impl Provider for OpenRouter {
    fn id(&self) -> ProviderId {
        ProviderId::OpenRouter
    }

    fn env(&self) -> &'static [&'static str] {
        &[ENV]
    }

    fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
        vec![Want::Secret(SECRET)]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let key = match key_in(cx) {
            None => return Step::Done(Err(Trouble::NeedsKey)),
            // Keys are printable ASCII ("sk-or-v1-" and hex). A space or a
            // line break pasted into one would only be refused, or break
            // the header it rides in, so such a key never leaves.
            Some(k) if !k.bytes().all(|b| b.is_ascii_graphic()) => return Step::Done(Err(Trouble::Refused)),
            Some(k) => k,
        };
        if cx.reach == Reach::Local {
            return Step::Done(Err(Trouble::NeedsOnline));
        }
        match replies {
            [] => Step::Fetch(Request::get(KEY_URL).bearer(key)),
            [first] => match ends(first) {
                Some(t) => Step::Done(Err(t)),
                None => Step::Fetch(Request::get(CREDITS_URL).bearer(key)),
            },
            [first, second, ..] => Step::Done(card(cx.now, first, second)),
        }
    }

    /// OpenRouter's own numbers can lag up to a minute; two is plenty.
    fn min_gap_s(&self) -> u64 {
        120
    }

    fn takes_key(&self) -> Option<KeyAsk> {
        Some(KeyAsk { secret: SECRET, url: "https://openrouter.ai/settings/keys", hint: "sk-or-v1-…" })
    }
}

/// The typed key, else the variable.
fn key_in<'c>(cx: &'c Ctx) -> Option<&'c str> {
    cx.got(0).text().or_else(|| cx.dirs.var(ENV)).map(str::trim)
}

/// Whether the key's answer ends the refresh: no answer at all (the credits
/// would get none either), a refused key, or an ask to wait. Any other
/// trouble still asks for the credits, which may answer.
fn ends(r: &Result<Reply, NetFail>) -> Option<Trouble> {
    match (r, Trouble::from_net(r)) {
        (Err(_), t) => t,
        (Ok(_), Some(t @ (Trouble::Refused | Trouble::Busy { .. }))) => Some(t),
        _ => None,
    }
}

/// Both answers into one card. Either alone still makes one: the key's
/// spend without a balance, or a balance without spend.
fn card(now: i64, key: &Result<Reply, NetFail>, credits: &Result<Reply, NetFail>) -> Outcome {
    let balance = reply_json(credits).and_then(|v| read_credits(&v));
    match (reply_json(key).and_then(|v| read_key(&v, now)), balance) {
        (Ok(mut s), Ok(b)) => {
            s.balance = Some(b);
            Ok(s)
        }
        // A management key's card is the balance: without it, say why.
        (Ok(s), Err(t)) if s.spend.is_empty() && s.windows.is_empty() => Err(t),
        (Ok(s), Err(_)) => Ok(s),
        (Err(_), Ok(b)) => {
            let mut s = Snapshot::new(ProviderId::OpenRouter, now, Source::Online);
            s.balance = Some(b);
            Ok(s)
        }
        // The key's trouble says more: many keys are refused the credits.
        (Err(t), Err(_)) => Err(t),
    }
}

/// `/api/v1/key`: the plan, the free models' daily requests, and the key's
/// spend, its limit as the cap of the period the limit resets in.
fn read_key(v: &Value, now: i64) -> Result<Snapshot, Trouble> {
    let d = &v["data"];
    if !d.is_object() {
        return Err(Trouble::Unreadable("no key data".into()));
    }
    let mut s = Snapshot::new(ProviderId::OpenRouter, now, Source::Online);
    let free_tier = d["is_free_tier"].as_bool() == Some(true);
    if free_tier {
        s.plan = Some("Free tier".into());
    }
    // The account's allowance of free (":free") model requests, 50 a UTC day
    // until it buys 10 credits, then 1000: the limit a free tier account runs
    // into, so it shows there, or once any were used today.
    let free = &d["free_model_daily_requests"];
    if let (Some(used), Some(of)) = (count(&free["used"]), count(&free["limit"])) {
        if of > 0 && (free_tier || used > 0) {
            let midnight = (now.div_euclid(86_400) + 1) * 86_400;
            s.windows.push(Window::new(WindowKind::Daily, "Free model requests", used as f32 / of as f32).resets(Some(midnight)).length(Some(86_400)).counted(used, of));
        }
    }
    // A management key only manages other keys: its spend is always zero,
    // which would read as an idle account.
    if d["is_management_key"].as_bool() == Some(true) || d["is_provisioning_key"].as_bool() == Some(true) {
        return Ok(s);
    }
    // OpenRouter's days, weeks (from Monday) and months are UTC. A limit
    // that never resets caps all the key ever spent; one that resets on a
    // period we don't know caps nothing we show.
    let cap = num(&d["limit"]).filter(|l| *l > 0.0).map(Money::usd);
    let resets = |period: &str| d["limit_reset"].as_str().is_some_and(|r| r.trim().eq_ignore_ascii_case(period));
    for (label, field, period) in [("Today", "usage_daily", "daily"), ("This week", "usage_weekly", "weekly"), ("This month", "usage_monthly", "monthly")] {
        if let Some(x) = num(&d[field]) {
            s.spend.push(Spend { label: label.into(), money: Money::usd(x), cap: cap.clone().filter(|_| resets(period)) });
        }
    }
    if d["limit_reset"].is_null() {
        if let (Some(cap), Some(x)) = (cap, num(&d["usage"])) {
            s.spend.push(Spend { label: "All time".into(), money: Money::usd(x), cap: Some(cap) });
        }
    }
    if s.spend.is_empty() {
        return Err(Trouble::Unreadable("no spend in the key".into()));
    }
    Ok(s)
}

/// `/api/v1/credits`: credits bought less credits used. OpenRouter bills a
/// request after it ran, so this can dip below zero, and then says so.
fn read_credits(v: &Value) -> Result<Money, Trouble> {
    let d = &v["data"];
    match (num(&d["total_credits"]), num(&d["total_usage"])) {
        (Some(bought), Some(used)) => Ok(Money { micros: micros(bought).saturating_sub(micros(used)), currency: "USD".into() }),
        _ => Err(Trouble::Unreadable("no credits".into())),
    }
}

/// A count that may arrive as `12` or `12.0`.
fn count(v: &Value) -> Option<u64> {
    num(v).filter(|x| *x >= 0.0).map(|x| x as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Got, Os, MAX_FETCHES};

    const KEY: &str = include_str!("fixtures/openrouter/key.json");
    const KEY_FREE: &str = include_str!("fixtures/openrouter/key_free.json");
    const KEY_MANAGEMENT: &str = include_str!("fixtures/openrouter/key_management.json");
    const CREDITS: &str = include_str!("fixtures/openrouter/credits.json");
    const CREDITS_403: &str = include_str!("fixtures/openrouter/credits_403.json");
    /// 2026-10-27 05:00 UTC.
    const NOW: i64 = 1_793_077_200;

    fn ok(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    /// The fixtures' answers, by URL.
    fn fixtures(r: &Request) -> Result<Reply, NetFail> {
        ok(200, if r.url == KEY_URL { KEY } else { CREDITS })
    }

    /// One refresh against canned answers: what it asked, and how it ended.
    fn refresh(dirs: &Dirs, got: &[Got], reach: Reach, answer: impl Fn(&Request) -> Result<Reply, NetFail>) -> (Vec<Request>, Outcome) {
        let cx = Ctx { dirs, got, now: NOW, reach };
        let (mut asked, mut replies) = (Vec::new(), Vec::new());
        loop {
            match OpenRouter.next(&cx, &replies) {
                Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
                Step::Fetch(r) => {
                    assert!(asked.len() < MAX_FETCHES, "asked too often");
                    replies.push(answer(&r));
                    asked.push(r);
                }
                Step::Done(out) => return (asked, out),
            }
        }
    }

    /// A refresh with the typed key `test-token` and these two answers.
    fn typed(key: Result<Reply, NetFail>, credits: Result<Reply, NetFail>) -> (Vec<Request>, Outcome) {
        let got = [Got::Text("test-token".into())];
        refresh(&Dirs::sample(Os::Windows), &got, Reach::Online, |r| if r.url == KEY_URL { key.clone() } else { credits.clone() })
    }

    #[test]
    fn no_key_needs_one() {
        for os in [Os::Windows, Os::Mac] {
            let (asked, out) = refresh(&Dirs::sample(os), &[Got::Missing], Reach::Online, |_| unreachable!());
            assert!(asked.is_empty());
            assert_eq!(out, Err(Trouble::NeedsKey));
        }
        // A blank typed key or variable is no key either.
        let mut dirs = Dirs::sample(Os::Mac);
        dirs.env.push((ENV.into(), "  ".into()));
        assert_eq!(refresh(&dirs, &[Got::Text(" \n".into())], Reach::Online, |_| unreachable!()).1, Err(Trouble::NeedsKey));
    }

    #[test]
    fn a_typed_key_asks_for_the_key_then_the_credits() {
        let got = [Got::Text(" test-token\n".into())];
        let (asked, out) = refresh(&Dirs::sample(Os::Mac), &got, Reach::Online, fixtures);
        assert_eq!(asked, [Request::get(KEY_URL).bearer("test-token"), Request::get(CREDITS_URL).bearer("test-token")]);
        assert!(out.is_ok());
        let ask = OpenRouter.takes_key().unwrap();
        assert_eq!(OpenRouter.wants(&Dirs::sample(Os::Windows)), [Want::Secret(ask.secret)]);
        assert_eq!((ask.url, ask.hint), ("https://openrouter.ai/settings/keys", "sk-or-v1-…"));
        assert_eq!(OpenRouter.min_gap_s(), 120);
    }

    #[test]
    fn the_variable_stands_in_for_a_typed_key() {
        assert_eq!(OpenRouter.env(), &["OPENROUTER_API_KEY"][..]);
        for os in [Os::Windows, Os::Mac] {
            let mut dirs = Dirs::sample(os);
            dirs.env.push(("OPENROUTER_API_KEY".into(), "env-token".into()));
            let (asked, out) = refresh(&dirs, &[Got::Missing], Reach::Online, fixtures);
            assert_eq!(asked, [Request::get(KEY_URL).bearer("env-token"), Request::get(CREDITS_URL).bearer("env-token")]);
            assert!(out.is_ok());
            // A typed key wins.
            let (asked, _) = refresh(&dirs, &[Got::Text("test-token".into())], Reach::Online, fixtures);
            assert!(asked.iter().all(|r| r.headers == [("Authorization".to_string(), "Bearer test-token".to_string())]));
        }
    }

    #[test]
    fn both_answers_make_the_card() {
        let (_, out) = typed(ok(200, KEY), ok(200, CREDITS));
        let spend = |label: &str, x: f64, cap: Option<f64>| Spend { label: label.into(), money: Money::usd(x), cap: cap.map(Money::usd) };
        let mut want = Snapshot::new(ProviderId::OpenRouter, NOW, Source::Online);
        want.balance = Some(Money::usd(74.75));
        // The $100 limit resets monthly, so it caps this month.
        want.spend = vec![spend("Today", 1.25, None), spend("This week", 7.5, None), spend("This month", 25.5, Some(100.0))];
        assert_eq!(out, Ok(want));
        // The key itself stays out of the card.
        assert!(!serde_json::to_string(&out.unwrap()).unwrap().contains("test-"));
    }

    #[test]
    fn a_free_tier_key_shows_its_plan_and_free_requests() {
        let s = typed(ok(200, KEY_FREE), ok(200, r#"{"data":{"total_credits":0,"total_usage":0}}"#)).1.unwrap();
        assert_eq!(s.plan.as_deref(), Some("Free tier"));
        assert_eq!(s.balance, Some(Money::usd(0.0)));
        // Spend it reported as zero is zero, not unknown.
        assert_eq!(s.spend.iter().map(|x| (x.label.as_str(), x.money.micros, x.cap.is_some())).collect::<Vec<_>>(), [("Today", 0, false), ("This week", 0, false), ("This month", 0, false)]);
        let w = &s.windows[0];
        assert_eq!((w.kind, w.label.as_str(), w.count, w.length_s), (WindowKind::Daily, "Free model requests", Some((12, 50)), Some(86_400)));
        assert!((w.used - 0.24).abs() < 1e-6);
        // UTC midnight, 2026-10-28.
        assert_eq!(w.resets_at, Some(1_793_145_600));
        // A paid account that used no free models today doesn't show them.
        assert!(typed(ok(200, KEY), ok(200, CREDITS)).1.unwrap().windows.is_empty());
    }

    #[test]
    fn a_limit_caps_the_period_it_resets_in() {
        let caps = |key: &str| -> Vec<(String, i64)> {
            let s = typed(ok(200, key), ok(403, CREDITS_403)).1.unwrap();
            s.spend.into_iter().filter_map(|x| x.cap.map(|c| (x.label, c.micros))).collect()
        };
        let key = |limit: &str, reset: &str| format!(r#"{{"data":{{"limit":{limit},"limit_reset":{reset},"usage":9,"usage_daily":0,"usage_weekly":2,"usage_monthly":4}}}}"#);
        assert_eq!(caps(&key("20", r#""weekly""#)), [("This week".to_string(), 20_000_000)]);
        assert_eq!(caps(&key("5", r#""Daily""#)), [("Today".to_string(), 5_000_000)]);
        // A limit that never resets caps all the key spent.
        assert_eq!(caps(&key("10", "null")), [("All time".to_string(), 10_000_000)]);
        // No limit, a zero one, or a period we don't know: no cap.
        assert!(caps(&key("null", "null")).is_empty());
        assert!(caps(&key("0", r#""monthly""#)).is_empty());
        assert!(caps(&key("20", r#""yearly""#)).is_empty());
    }

    #[test]
    fn one_answer_alone_still_makes_a_card() {
        // Regular keys are often refused the credits: spend, no balance.
        let s = typed(ok(200, KEY), ok(403, CREDITS_403)).1.unwrap();
        assert_eq!((s.balance, s.spend.len()), (None, 3));
        // OpenRouter stumbling on the key's answer still leaves the balance.
        let (asked, out) = typed(ok(502, "<html>Bad gateway</html>"), ok(200, CREDITS));
        let s = out.unwrap();
        assert_eq!((asked.len(), s.balance, s.spend.len()), (2, Some(Money::usd(74.75)), 0));
        // Both failing: the key's trouble, which says more.
        assert_eq!(typed(ok(502, ""), ok(403, CREDITS_403)).1, Err(Trouble::Offline));
    }

    #[test]
    fn a_management_key_shows_the_balance() {
        let s = typed(ok(200, KEY_MANAGEMENT), ok(200, CREDITS)).1.unwrap();
        assert_eq!((s.spend.len(), s.balance), (0, Some(Money::usd(74.75))));
        // Without the credits it has nothing to show, and says why.
        assert_eq!(typed(ok(200, KEY_MANAGEMENT), Err(NetFail::Timeout)).1, Err(Trouble::Offline));
        let busy = Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(30) });
        assert_eq!(typed(ok(200, KEY_MANAGEMENT), busy).1, Err(Trouble::Busy { retry_after_s: Some(30) }));
    }

    #[test]
    fn a_local_refresh_never_asks() {
        let (asked, out) = refresh(&Dirs::sample(Os::Windows), &[Got::Text("test-token".into())], Reach::Local, |_| unreachable!());
        assert_eq!((asked.len(), out), (0, Err(Trouble::NeedsOnline)));
    }

    #[test]
    fn a_refused_key_stops_at_the_first_ask() {
        let (asked, out) = typed(ok(401, r#"{"error":{"code":401,"message":"User not found."}}"#), ok(200, CREDITS));
        assert_eq!((asked.len(), out), (1, Err(Trouble::Refused)));
        // So does a key that can't be sent as it is, without asking.
        let (asked, out) = refresh(&Dirs::sample(Os::Mac), &[Got::Text("sk-or-v1-12 34".into())], Reach::Online, |_| unreachable!());
        assert_eq!((asked.len(), out), (0, Err(Trouble::Refused)));
    }

    #[test]
    fn busy_or_no_answer_stops_at_the_first_ask() {
        let busy = Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(42) });
        let (asked, out) = typed(busy, ok(200, CREDITS));
        assert_eq!((asked.len(), out), (1, Err(Trouble::Busy { retry_after_s: Some(42) })));
        let (asked, out) = typed(Err(NetFail::Network("no route".into())), ok(200, CREDITS));
        assert_eq!((asked.len(), out), (1, Err(Trouble::Offline)));
    }

    #[test]
    fn an_unreadable_answer_says_so() {
        assert!(matches!(typed(ok(200, "<html>"), ok(200, "<html>")).1, Err(Trouble::Unreadable(_))));
        // JSON, in shapes we don't read.
        assert!(matches!(typed(ok(200, r#"{"data":[]}"#), ok(200, r#"{"data":{"total_credits":"many","total_usage":1}}"#)).1, Err(Trouble::Unreadable(_))));
        assert!(matches!(typed(ok(200, r#"{"data":{"label":"test-key"}}"#), ok(403, CREDITS_403)).1, Err(Trouble::Unreadable(_))));
        // An answer read in part keeps what it has and invents no zeros.
        let s = typed(ok(200, r#"{"data":{"usage_daily":0.5,"usage_weekly":null}}"#), ok(200, "{")).1.unwrap();
        assert_eq!((s.spend.len(), s.spend[0].label.as_str(), s.balance), (1, "Today", None));
    }
}
