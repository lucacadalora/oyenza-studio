//! Z.ai's GLM Coding Plan: how much of each limit is used and when it
//! resets (the 5-hour token window, the week, the month's tool calls), read
//! with the API key the person typed into Oyenza Studio, or `Z_AI_API_KEY`.
//!
//! One GET to `/api/monitor/usage/quota/limit` on api.z.ai, the endpoint
//! z.ai's own usage page reads, with the key as a bearer token. A key made
//! on bigmodel.cn (Zhipu's China platform, the same plan) only works on
//! open.bigmodel.cn, so a key api.z.ai refuses is asked about there once:
//! the key goes to Zhipu's two API hosts and nowhere else. Ported from
//! CodexBar's `zai.js` (MIT): the limit types and units, and its guard
//! against impossible 5-hour resets.

use super::{
    num, reply_json, sort_small_by, time, Ctx, Dirs, Got, KeyAsk, NetFail, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Step,
    Trouble, Want, Window, WindowKind,
};
use serde_json::Value;

pub struct Zai;

/// Where the typed key is kept, and the variable CodexBar reads it from.
const SECRET: &str = "zai";
const VAR: &str = "Z_AI_API_KEY";
/// z.ai first; bigmodel.cn only for a key z.ai refused.
const HOSTS: [&str; 2] = ["https://api.z.ai", "https://open.bigmodel.cn"];
const QUOTA: &str = "/api/monitor/usage/quota/limit";

impl Provider for Zai {
    fn id(&self) -> ProviderId {
        ProviderId::Zai
    }

    fn env(&self) -> &'static [&'static str] {
        &[VAR]
    }

    fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
        vec![Want::Secret(SECRET)]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let key = match find_key(cx, &[VAR], |_| true) {
            Ok(key) => key,
            Err(t) => return Step::Done(Err(t)),
        };
        if cx.reach == Reach::Local {
            return Step::Done(Err(Trouble::NeedsOnline));
        }
        let ask = |host: &str| Step::Fetch(Request::get(format!("{host}{QUOTA}")).bearer(&key).header("Accept", "application/json"));
        match replies {
            [] => ask(HOSTS[0]),
            [first] => match read(first, cx.now) {
                // Maybe a bigmodel.cn key.
                Err(Trouble::Refused) => ask(HOSTS[1]),
                outcome => Step::Done(outcome),
            },
            // bigmodel.cn's answer for a key z.ai refused; when it gives
            // none, the refusal is all we know.
            [_, second, ..] => Step::Done(match read(second, cx.now) {
                Err(Trouble::Offline) => Err(Trouble::Refused),
                outcome => outcome,
            }),
        }
    }

    /// The plan's windows move over hours: five minutes is as fresh as the
    /// card needs.
    fn min_gap_s(&self) -> u64 {
        300
    }

    fn takes_key(&self) -> Option<KeyAsk> {
        Some(KeyAsk { secret: SECRET, url: "https://z.ai/manage-apikey/apikey-list", hint: "Z.ai API key" })
    }
}

/// One answer from the quota endpoint.
fn read(r: &Result<Reply, NetFail>, now: i64) -> Outcome {
    let root = reply_json(r)?;
    // The answer carries its own code: 200 and success, or why not (a bad
    // key can come back as HTTP 200 with code 401).
    let code = root.get("code").and_then(num).map(|c| c as i64);
    if code != Some(200) || root.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(match code {
            // Zhipu's 1000 to 1004: the key is missing, wrong or expired.
            Some(401 | 403 | 1000..=1004) => Trouble::Refused,
            // 1302, 1303 and 1305: too many asks.
            Some(429 | 1302 | 1303 | 1305) => Trouble::Busy { retry_after_s: None },
            Some(c) => Trouble::Unreadable(format!("code {c}")),
            None => Trouble::Unreadable("no code".into()),
        });
    }
    let data = &root["data"];
    let limits = data.get("limits").and_then(Value::as_array).ok_or_else(|| Trouble::Unreadable("no limits".into()))?;
    let (mut plan, mut tools) = (Vec::new(), None);
    for e in limits {
        let ty = e.get("type").and_then(Value::as_str).ok_or_else(|| Trouble::Unreadable("a limit without a type".into()))?;
        if !matches!(ty, "TOKENS_LIMIT" | "CREDIT_LIMIT" | "TIME_LIMIT") {
            continue; // a kind of limit z.ai added since: left out, not guessed
        }
        let (Some(unit), Some(number), Some(pct)) = (int(&e["unit"]), int(&e["number"]), num(&e["percentage"])) else {
            return Err(Trouble::Unreadable("a limit's shape".into()));
        };
        // `usage` is the limit, `currentValue` what's used and `remaining`
        // what's left; counts beat the rounded percentage when they're there.
        let count = int(&e["usage"]).filter(|&of| of > 0).and_then(|of| {
            let used = match (int(&e["remaining"]), int(&e["currentValue"])) {
                (Some(left), cur) => (of - left).max(cur.unwrap_or(of - left)),
                (None, Some(cur)) => cur,
                (None, None) => return None,
            };
            Some((used.clamp(0, of) as u64, of as u64))
        });
        let used = count.map_or(pct / 100.0, |(u, of)| u as f64 / of as f64).clamp(0.0, 1.0) as f32;
        // Units: 1 day, 3 hour, 5 minute, 6 week.
        let minutes: Option<i64> = match unit {
            1 => Some(1440),
            3 => Some(60),
            5 => Some(1),
            6 => Some(10_080),
            _ => None,
        }
        .filter(|_| number > 0)
        .map(|m| m * number);
        let mut reset = e.get("nextResetTime").and_then(time).filter(|&t| t > 0);
        let window = |kind: WindowKind, label: String, len: Option<i64>, at: Option<i64>| {
            let mut w = Window::new(kind, label, used).resets(at).length(len.and_then(|s| u32::try_from(s).ok()));
            w.count = count;
            w
        };
        if ty == "TIME_LIMIT" {
            // Unit 5 times 1 is z.ai's mark for the monthly tool quota (web
            // search, web reader, zread), not a 1-minute window.
            let (kind, len) = if unit == 5 && number == 1 { (WindowKind::Monthly, None) } else { (span(minutes, false).0, minutes.map(|m| m * 60)) };
            tools = Some(window(kind, "Tool calls".into(), len, reset));
        } else {
            // A 5-hour window can't reset further off than 5 hours, but z.ai
            // has sent such times (CodexBar #2871): keep the percentage,
            // drop the time.
            if minutes == Some(300) {
                reset = reset.filter(|&t| t <= now + 5 * 3600 + 60);
            }
            let (kind, label) = span(minutes, ty == "CREDIT_LIMIT");
            plan.push((minutes.unwrap_or(i64::MAX), window(kind, label, minutes.map(|m| m * 60), reset)));
        }
    }
    // Shortest window first, the tools last.
    sort_small_by(&mut plan, |a, b| a.0 > b.0);
    let mut s = Snapshot::new(ProviderId::Zai, now, Source::Online);
    s.plan = plan_name(data);
    // No limit we know leaves no windows, never a made-up 0%.
    s.windows = plan.into_iter().map(|(_, w)| w).chain(tools).collect();
    Ok(s)
}

/// The kind and name of a window `minutes` long ("5-hour", "Weekly").
fn span(minutes: Option<i64>, credits: bool) -> (WindowKind, String) {
    match minutes {
        Some(1440) => (WindowKind::Daily, "Daily".into()),
        Some(10_080) => (WindowKind::Weekly, "Weekly".into()),
        Some(m) if m < 1440 && m % 60 == 0 => (WindowKind::Session, format!("{}-hour", m / 60)),
        Some(43_200) => (WindowKind::Monthly, "30-day".into()),
        Some(m) if m % 1440 == 0 => (WindowKind::Other, format!("{}-day", m / 1440)),
        Some(m) => (WindowKind::Other, format!("{m}-minute")),
        None => (WindowKind::Other, if credits { "Credits" } else { "Tokens" }.into()),
    }
}

/// The plan's name ("pro" becomes "Pro"), from whichever field carries it.
fn plan_name(data: &Value) -> Option<String> {
    let fields = ["planName", "plan", "plan_type", "packageName", "level"];
    let raw = fields.iter().find_map(|k| data.get(*k)?.as_str().map(str::trim).filter(|s| !s.is_empty()))?;
    Some(if raw.bytes().all(|b| b.is_ascii_lowercase()) { raw[..1].to_ascii_uppercase() + &raw[1..] } else { raw.to_string() })
}

/// A whole number, from a JSON number or a numeric string.
fn int(v: &Value) -> Option<i64> {
    num(v).map(|f| f as i64)
}

/// The key typed into Studio (want 0), else the first of `vars` holding one
/// that `fits`. The keychain refusing the typed key is `Denied`, not a
/// missing key. (MiniMax uses this too.)
pub(super) fn find_key(cx: &Ctx, vars: &[&str], fits: fn(&str) -> bool) -> Result<String, Trouble> {
    match cx.got(0) {
        Got::Denied => Err(Trouble::Denied),
        got => {
            let set = || vars.iter().filter_map(|v| cx.dirs.var(v).and_then(clean_key)).find(|k| fits(k));
            got.text().and_then(clean_key).or_else(set).ok_or(Trouble::NeedsKey)
        }
    }
}

/// A typed or set key without the spaces or quotes around it (a `.env` line
/// keeps its quotes), or `None` when nothing usable is left: a key with a
/// control character in it never goes into a header.
fn clean_key(s: &str) -> Option<String> {
    let s = s.trim();
    let quoted = |q: char| s.strip_prefix(q).and_then(|t| t.strip_suffix(q));
    let s = quoted('"').or_else(|| quoted('\'')).unwrap_or(s).trim();
    (!s.is_empty() && !s.chars().any(char::is_control)).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Method, Os, MAX_FETCHES};

    const QUOTA_JSON: &str = include_str!("fixtures/zai/quota.json");
    const CREDITS_JSON: &str = include_str!("fixtures/zai/quota-credits.json");
    /// 2026-10-27 05:00 UTC, the time the fixtures were written for.
    const NOW: i64 = 1_793_077_200;

    fn reply(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    fn typed() -> Vec<Got> {
        vec![Got::Text("test-token".into())]
    }

    fn header<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
        r.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// One refresh the way the app runs it, each ask answered by `answer`:
    /// the outcome and the asks made.
    fn refresh(dirs: &Dirs, got: &[Got], reach: Reach, mut answer: impl FnMut(&Request) -> Result<Reply, NetFail>) -> (Outcome, Vec<Request>) {
        let cx = Ctx { dirs, got, now: NOW, reach };
        let (mut replies, mut asked) = (Vec::new(), Vec::new());
        loop {
            match Zai.next(&cx, &replies) {
                Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
                Step::Done(outcome) => return (outcome, asked),
                Step::Fetch(req) => {
                    assert!(asked.len() < MAX_FETCHES, "more asks than MAX_FETCHES");
                    replies.push(answer(&req));
                    asked.push(req);
                }
            }
        }
    }

    #[test]
    fn no_key_needs_one() {
        let dirs = Dirs::sample(Os::Windows);
        assert_eq!(Zai.wants(&dirs), vec![Want::Secret("zai")]);
        for got in [Got::Missing, Got::Text("  ".into())] {
            let (outcome, asked) = refresh(&dirs, &[got], Reach::Online, |_| unreachable!());
            assert_eq!(outcome, Err(Trouble::NeedsKey));
            assert!(asked.is_empty());
        }
        // The keychain refusing the typed key says so, and asks nothing.
        let (outcome, asked) = refresh(&dirs, &[Got::Denied], Reach::Online, |_| unreachable!());
        assert_eq!(outcome, Err(Trouble::Denied));
        assert!(asked.is_empty());
        let ask = Zai.takes_key().unwrap();
        assert_eq!((ask.secret, ask.url), ("zai", "https://z.ai/manage-apikey/apikey-list"));
        assert_eq!(Zai.min_gap_s(), 300);
    }

    #[test]
    fn a_typed_key_goes_to_z_ai_as_a_bearer_and_nowhere_else() {
        let (outcome, asked) = refresh(&Dirs::sample(Os::Mac), &typed(), Reach::Online, |_| reply(200, QUOTA_JSON));
        assert!(outcome.is_ok());
        assert_eq!(asked.len(), 1);
        assert_eq!((asked[0].method, asked[0].url.as_str()), (Method::Get, "https://api.z.ai/api/monitor/usage/quota/limit"));
        assert_eq!(header(&asked[0], "Authorization"), Some("Bearer test-token"));
        assert_eq!(header(&asked[0], "Accept"), Some("application/json"));
        assert_eq!(asked[0].headers.iter().filter(|(_, v)| v.contains("test-token")).count(), 1);
        assert!(!format!("{outcome:?}").contains("test-token"));
    }

    #[test]
    fn the_key_from_its_variable_on_both_systems() {
        assert_eq!(Zai.env(), &["Z_AI_API_KEY"]);
        for os in [Os::Windows, Os::Mac] {
            let mut dirs = Dirs::sample(os);
            dirs.env.push((VAR.into(), " \"env-token\" ".into()));
            let (outcome, asked) = refresh(&dirs, &[Got::Missing], Reach::Online, |_| reply(200, QUOTA_JSON));
            assert!(outcome.is_ok());
            assert_eq!(header(&asked[0], "Authorization"), Some("Bearer env-token"));
            // A key typed into Studio wins over the variable.
            let (_, asked) = refresh(&dirs, &typed(), Reach::Online, |_| reply(200, QUOTA_JSON));
            assert_eq!(header(&asked[0], "Authorization"), Some("Bearer test-token"));
        }
    }

    #[test]
    fn reads_the_plan_and_each_window() {
        let (outcome, _) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, QUOTA_JSON));
        let s = outcome.unwrap();
        assert_eq!((s.provider, s.source, s.as_of, s.account.as_deref()), (ProviderId::Zai, Source::Online, NOW, None));
        assert_eq!(s.plan.as_deref(), Some("Pro"));
        let labels: Vec<&str> = s.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5-hour", "Weekly", "Tool calls"]);
        let [five, week, tools] = &s.windows[..] else { unreachable!() };
        let counts = Some((496_000_000, 800_000_000));
        assert_eq!((five.kind, five.resets_at, five.length_s, five.count), (WindowKind::Session, Some(NOW + 7200), Some(18_000), counts));
        assert!((five.used - 0.62).abs() < 1e-6);
        assert_eq!((week.kind, week.resets_at, week.length_s, week.count), (WindowKind::Weekly, Some(NOW + 3 * 86_400), Some(604_800), None));
        assert!((week.used - 0.31).abs() < 1e-6);
        // The month's tool calls: counted, and a month has no fixed length.
        assert_eq!((tools.kind, tools.resets_at, tools.length_s, tools.count), (WindowKind::Monthly, Some(NOW + 20 * 86_400), None, Some((224, 1000))));
        assert!((tools.used - 0.224).abs() < 1e-6);
    }

    #[test]
    fn a_local_refresh_never_asks() {
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Local, |_| unreachable!());
        assert_eq!(outcome, Err(Trouble::NeedsOnline));
        assert!(asked.is_empty());
    }

    #[test]
    fn a_refused_key_is_asked_about_on_bigmodel_once() {
        // HTTP 401, or HTTP 200 carrying code 401: either way, no.
        for no in [reply(401, "{}"), reply(200, r#"{"code":401,"msg":"token expired or incorrect","success":false}"#)] {
            let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| no.clone());
            assert_eq!(outcome, Err(Trouble::Refused));
            let urls: Vec<&str> = asked.iter().map(|r| r.url.as_str()).collect();
            assert_eq!(urls, ["https://api.z.ai/api/monitor/usage/quota/limit", "https://open.bigmodel.cn/api/monitor/usage/quota/limit"]);
        }
    }

    #[test]
    fn a_bigmodel_key_reads_from_bigmodel() {
        let z_ai_says_no = |r: &Request, there| if r.url.starts_with("https://api.z.ai/") { reply(401, "{}") } else { there };
        let (outcome, asked) = refresh(&Dirs::sample(Os::Mac), &typed(), Reach::Online, |r| z_ai_says_no(r, reply(200, CREDITS_JSON)));
        assert_eq!(asked.len(), 2);
        assert_eq!(header(&asked[1], "Authorization"), Some("Bearer test-token"));
        let s = outcome.unwrap();
        assert_eq!(s.plan.as_deref(), Some("Lite"));
        // Credit windows read like token ones; the points pool is left out.
        let got: Vec<(&str, Option<(u64, u64)>)> = s.windows.iter().map(|w| (w.label.as_str(), w.count)).collect();
        assert_eq!(got, [("5-hour", Some((600, 2400))), ("Weekly", Some((1080, 12_000))), ("Tool calls", Some((0, 100)))]);
        // No answer from bigmodel.cn leaves z.ai's refusal standing.
        let (outcome, _) = refresh(&Dirs::sample(Os::Mac), &typed(), Reach::Online, |r| z_ai_says_no(r, Err(NetFail::Timeout)));
        assert_eq!(outcome, Err(Trouble::Refused));
    }

    #[test]
    fn too_many_asks_wait_as_told() {
        let busy: Result<Reply, NetFail> = Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(60) });
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| busy.clone());
        assert_eq!(outcome, Err(Trouble::Busy { retry_after_s: Some(60) }));
        assert_eq!(asked.len(), 1);
    }

    #[test]
    fn answers_we_cannot_read_say_so() {
        for body in [
            "<html>Bad gateway</html>",
            r#"{"code":200,"success":true,"data":{}}"#,
            r#"{"code":200,"success":true,"data":{"limits":[{"unit":3,"number":5,"percentage":25}]}}"#,
            r#"{"code":200,"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","number":5}]}}"#,
            r#"{"code":500,"msg":"system busy","success":false}"#,
        ] {
            let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, body));
            assert!(matches!(outcome, Err(Trouble::Unreadable(_))), "{body}: {outcome:?}");
            assert_eq!(asked.len(), 1);
        }
        let (outcome, _) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| Err(NetFail::Timeout));
        assert_eq!(outcome, Err(Trouble::Offline));
    }

    #[test]
    fn unknown_is_not_zero() {
        // No limit we know: no windows at all, not an empty bar.
        let body = r#"{"code":200,"success":true,"data":{"planName":"Pro","limits":[{"type":"FUTURE_LIMIT","unit":3,"number":5,"percentage":40}]}}"#;
        let s = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, body)).0.unwrap();
        assert_eq!(s.plan.as_deref(), Some("Pro"));
        assert!(s.windows.is_empty());
    }

    #[test]
    fn an_impossible_five_hour_reset_is_dropped() {
        // Ten hours off for a 5-hour window: the time goes, the percentage stays.
        let limits = format!(r#"[{{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":25,"nextResetTime":{}}}]"#, (NOW + 36_000) * 1000);
        let body = format!(r#"{{"code":200,"success":true,"data":{{"limits":{limits}}}}}"#);
        let s = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, &body)).0.unwrap();
        assert_eq!((s.windows[0].label.as_str(), s.windows[0].resets_at), ("5-hour", None));
        assert!((s.windows[0].used - 0.25).abs() < 1e-6);
    }

    #[test]
    fn keys_are_cleaned_before_use() {
        assert_eq!(clean_key("  'k-1'  ").as_deref(), Some("k-1"));
        assert_eq!(clean_key("\"\""), None);
        assert_eq!(clean_key("k-1\r\nX-Evil: 1"), None);
    }
}
