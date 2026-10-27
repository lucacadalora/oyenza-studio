//! MiniMax's Token Plan (its Coding Plan before that): how much of the
//! 5-hour window and of the week is used and when each resets, plus the
//! plan's daily video, speech or image quotas when it has them, read with
//! the plan's key typed into Oyenza Studio, or `MINIMAX_CODING_API_KEY` /
//! `MINIMAX_API_KEY`.
//!
//! The Token Plan's endpoint (`/v1/token_plan/remains`) first, then the
//! Coding Plan's (`/v1/api/openplatform/coding_plan/remains`) when the
//! first has nothing for this key, both on api.minimax.io. A key made on
//! minimaxi.com (MiniMax's China platform) is refused there, so a refused
//! key asks the same two on api.minimaxi.com: at most four asks, and the
//! key goes to MiniMax's two API hosts and nowhere else. Ported from
//! CodexBar's `Providers/MiniMax/` (MIT): the endpoints, the order it tries
//! them in, and `model_remains`, whose `*_usage_count` is what is left, not
//! what was used.

use super::zai::find_key;
use super::{
    num, reply_json, sort_small_by, time, Ctx, Dirs, KeyAsk, NetFail, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Step, Trouble,
    Want, Window, WindowKind,
};
use serde_json::Value;

pub struct MiniMax;

/// Where the typed key is kept, and the variables CodexBar reads, in its
/// order (the coding-plan one wins).
const SECRET: &str = "minimax";
const VARS: [&str; 2] = ["MINIMAX_CODING_API_KEY", "MINIMAX_API_KEY"];
/// minimax.io first; minimaxi.com only for a key minimax.io refused.
const HOSTS: [&str; 2] = ["https://api.minimax.io", "https://api.minimaxi.com"];
/// The Token Plan's endpoint, then the older Coding Plan's.
const PATHS: [&str; 2] = ["/v1/token_plan/remains", "/v1/api/openplatform/coding_plan/remains"];

impl Provider for MiniMax {
    fn id(&self) -> ProviderId {
        ProviderId::MiniMax
    }

    fn env(&self) -> &'static [&'static str] {
        &VARS
    }

    fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
        vec![Want::Secret(SECRET)]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        // A pay-as-you-go key in `MINIMAX_API_KEY` is the API's, not a
        // plan's, so it doesn't count as finding one.
        let key = match find_key(cx, &VARS, |k| !standard(k)) {
            Ok(key) => key,
            Err(t) => return Step::Done(Err(t)),
        };
        // A pay-as-you-go key can't read a plan: CodexBar never sends one to
        // these endpoints, and neither do we.
        if standard(&key) {
            return Step::Done(Err(Trouble::Refused));
        }
        if cx.reach == Reach::Local {
            return Step::Done(Err(Trouble::NeedsOnline));
        }
        let ask = |host: &str, path: &str| {
            let req = Request::get(format!("{host}{path}")).bearer(&key).header("Accept", "application/json");
            // CodexBar's request: JSON, and a source tag naming the app asking.
            Step::Fetch(req.header("Content-Type", "application/json").header("MM-API-Source", "Oyenza Studio"))
        };
        match round(replies, cx.now) {
            Round::Ask(path) => ask(HOSTS[0], path),
            // Maybe a minimaxi.com key; when that host gives no answer, the
            // refusal is all we know.
            Round::Done(Err(Trouble::Refused), used) => match round(&replies[used..], cx.now) {
                Round::Ask(path) => ask(HOSTS[1], path),
                Round::Done(Err(Trouble::Offline), _) => Step::Done(Err(Trouble::Refused)),
                Round::Done(outcome, _) => Step::Done(outcome),
            },
            Round::Done(outcome, _) => Step::Done(outcome),
        }
    }

    /// The plan's windows move over hours: five minutes is as fresh as the
    /// card needs.
    fn min_gap_s(&self) -> u64 {
        300
    }

    fn takes_key(&self) -> Option<KeyAsk> {
        Some(KeyAsk { secret: SECRET, url: "https://platform.minimax.io/user-center/payment/coding-plan", hint: "sk-cp-…" })
    }
}

/// A standard pay-as-you-go key (`sk-api-…`); a plan's key is `sk-cp-…`.
fn standard(key: &str) -> bool {
    key.starts_with("sk-api-")
}

/// Where one host's round of asks stands.
enum Round {
    /// Ask this path next.
    Ask(&'static str),
    /// Decided, after this many replies.
    Done(Outcome, usize),
}

/// One host's round, as far as `replies` go: the Token Plan's endpoint, then
/// the Coding Plan's when the first had nothing for this key.
fn round(replies: &[Result<Reply, NetFail>], now: i64) -> Round {
    let Some(first) = replies.first() else {
        return Round::Ask(PATHS[0]);
    };
    let refused = match read(first, now) {
        Ok(s) => return Round::Done(Ok(s), 1),
        Err((t, false)) => return Round::Done(Err(t), 1),
        Err((t, true)) => t == Trouble::Refused,
    };
    let Some(second) = replies.get(1) else {
        return Round::Ask(PATHS[1]);
    };
    let outcome = match read(second, now) {
        Ok(s) => Ok(s),
        // The Token Plan's refusal outranks whatever the older endpoint said.
        Err(_) if refused => Err(Trouble::Refused),
        Err((t, _)) => Err(t),
    };
    Round::Done(outcome, 2)
}

/// One remains answer: a snapshot, or the trouble and whether the older
/// endpoint is worth asking after it (after no answer, a refusal, a missing
/// endpoint or a body we can't read: CodexBar's rule).
fn read(r: &Result<Reply, NetFail>, now: i64) -> Result<Snapshot, (Trouble, bool)> {
    let retry = match r {
        Err(_) => true,
        Ok(x) => matches!(x.status, 200..=299 | 401 | 403 | 404 | 405),
    };
    let root = reply_json(r).map_err(|t| (t, retry))?;
    let data = root.get("data").filter(|d| d.is_object()).unwrap_or(&root);
    // `base_resp`, inside `data` or beside it, says why when it isn't 0.
    let base = data.get("base_resp").or_else(|| root.get("base_resp"));
    let status = base.and_then(|b| b.get("status_code")).and_then(num).map_or(0, |c| c as i64);
    if status != 0 {
        let msg = base.and_then(|b| b.get("status_msg")).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        // 1004 and 2049: no key, a wrong one or a lapsed sign-in.
        return Err(if matches!(status, 1004 | 2049) || ["invalid api key", "login", "log in", "cookie"].iter().any(|w| msg.contains(w)) {
            (Trouble::Refused, true)
        } else if matches!(status, 1002 | 1039 | 2045) {
            // Rate limits.
            (Trouble::Busy { retry_after_s: None }, false)
        } else {
            (Trouble::Unreadable(format!("status {status}")), false)
        });
    }
    let lanes = data.get("model_remains").and_then(Value::as_array).filter(|a| !a.is_empty());
    let lanes = lanes.ok_or_else(|| (Trouble::Unreadable("no model_remains".into()), true))?;
    // The text lanes first, each week after its window, as CodexBar orders
    // them: the lanes coding tools draw on lead the card.
    let mut found = Vec::new();
    for m in lanes {
        let Some(model) = m.get("model_name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let text = is_text(model);
        found.extend(lane(m, model, false, now).map(|w| (if text { 0 } else { 2 }, model, w)));
        if text {
            found.extend(lane(m, model, true, now).map(|w| (1, model, w)));
        }
    }
    sort_small_by(&mut found, |a, b| a.0 > b.0);
    let mut s = Snapshot::new(ProviderId::MiniMax, now, Source::Online);
    for (_, model, mut w) in found {
        // Two lanes of one name (two text models): the second says which.
        if s.windows.iter().any(|x| x.label == w.label) {
            w.label = format!("{} · {model}", w.label);
        }
        s.windows.push(w);
    }
    let titles = ["/current_subscribe_title", "/plan_name", "/combo_title", "/current_plan_title", "/current_combo_card/title"];
    s.plan = titles.iter().filter_map(|t| data.pointer(t)?.as_str()).map(str::trim).find(|t| !t.is_empty()).map(plan_name);
    Ok(s)
}

/// One lane of a `model_remains` entry as a window: its interval (5 hours, a
/// day) or, `weekly`, its week. `None` when there is no limit to show.
fn lane(m: &Value, model: &str, weekly: bool, now: i64) -> Option<Window> {
    let (c, t) = if weekly { ("current_weekly", "weekly_") } else { ("current_interval", "") };
    let n = |k: String| m.get(k.as_str()).and_then(num);
    // Despite its name, `_usage_count` is what's left.
    let (total, left, left_pct) = (n(format!("{c}_total_count")), n(format!("{c}_usage_count")), n(format!("{c}_remaining_percent")));
    // Status 3, all of it left and no counts: a lane this plan doesn't have
    // (a Plus plan's video) or an unlimited week. No limit either way.
    let empty = total.unwrap_or(0.0) == 0.0 && left.unwrap_or(0.0) == 0.0;
    if n(format!("{c}_status")) == Some(3.0) && left_pct.is_some_and(|p| p >= 100.0) && empty {
        return None;
    }
    // The Token Plan gives the percentage left (its counts are 0); the older
    // plan gives counts.
    let (used, count) = match left_pct {
        Some(p) => ((100.0 - p) / 100.0, None),
        None => {
            let (total, left) = (total.filter(|&t| t > 0.0)?, left?);
            let used = (total - left).clamp(0.0, total);
            (used / total, Some((used as u64, total as u64)))
        }
    };
    let at = |k: &str| m.get(format!("{t}{k}").as_str()).and_then(time).filter(|&s| s > 1_000_000_000);
    let (start, end) = (at("start_time"), at("end_time"));
    // The window's end, or what's left of it (milliseconds) once the end has
    // passed.
    let reset = end.filter(|&e| e > now).or_else(|| n(format!("{t}remains_time")).filter(|&ms| ms > 0.0).map(|ms| now + (ms / 1000.0) as i64));
    let (kind, when, len) = if weekly {
        (WindowKind::Weekly, Some("Weekly".to_string()), Some(604_800))
    } else {
        interval(start.zip(end).map(|(s, e)| e - s))
    };
    let label = match (is_text(model), when) {
        (true, Some(w)) => w,
        (true, None) => "Usage".into(),
        (false, Some(w)) => format!("{w} · {}", service(model)),
        (false, None) => service(model),
    };
    let mut w = Window::new(kind, label, used.clamp(0.0, 1.0) as f32).resets(reset).length(len);
    w.count = count;
    Some(w)
}

/// The kind, name and length of an interval lane `span` seconds long. The
/// 5-hour window can read 4 hours from its start and end (CodexBar takes 4
/// to 6 hours as five), so it gets the window's length, not the span.
fn interval(span: Option<i64>) -> (WindowKind, Option<String>, Option<u32>) {
    let Some(s) = span.filter(|&s| s > 0) else {
        return (WindowKind::Other, None, None);
    };
    match s {
        14_400..=21_600 => (WindowKind::Session, Some("5-hour".into()), Some(18_000)),
        82_800..=90_000 => (WindowKind::Daily, Some("Daily".into()), Some(86_400)),
        3600..=14_399 | 21_601..=82_799 => (WindowKind::Session, Some(format!("{}-hour", s / 3600)), u32::try_from(s).ok()),
        _ => (WindowKind::Other, None, u32::try_from(s).ok()),
    }
}

/// The text lane ("general" in the Token Plan, an M2 model in the older
/// plan): the one coding tools draw on.
fn is_text(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m == "general" || m.contains("minimax-m") || m.starts_with("m2.")
}

/// A plain name for a lane that isn't text.
fn service(model: &str) -> String {
    let m = model.to_ascii_lowercase();
    let name = if m == "video" || m.contains("hailuo") {
        "Video"
    } else if m.contains("speech") {
        "Speech"
    } else if m.starts_with("image") {
        "Images"
    } else if m.contains("music") {
        "Music"
    } else {
        model
    };
    name.to_string()
}

/// A plan's name in plain words: "Token Plan · TokenPlanPlus-年度会员" (a
/// yearly Plus) is "Token Plan Plus", and a trailing note in Chinese goes.
fn plan_name(raw: &str) -> String {
    if let Some(i) = raw.to_ascii_lowercase().rfind("tokenplan") {
        let tier: String = raw[i + 9..].chars().take_while(char::is_ascii_alphabetic).collect();
        if !tier.is_empty() {
            return format!("Token Plan {tier}");
        }
    }
    match raw.rsplit_once('-') {
        Some((name, note)) if !note.is_ascii() && !name.trim().is_empty() => name.trim().to_string(),
        _ => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Got, Os, MAX_FETCHES};

    const TOKEN_PLAN: &str = include_str!("fixtures/minimax/token-plan.json");
    const CODING_PLAN: &str = include_str!("fixtures/minimax/coding-plan.json");
    /// 2026-10-27 05:00 UTC, the time the fixtures were written for.
    const NOW: i64 = 1_793_077_200;
    const G1: &str = "https://api.minimax.io/v1/token_plan/remains";
    const G2: &str = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains";
    const C1: &str = "https://api.minimaxi.com/v1/token_plan/remains";
    const C2: &str = "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains";

    fn reply(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    fn typed() -> Vec<Got> {
        vec![Got::Text("sk-cp-test-token".into())]
    }

    fn header<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
        r.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn urls(asked: &[Request]) -> Vec<&str> {
        asked.iter().map(|r| r.url.as_str()).collect()
    }

    /// One refresh the way the app runs it, each ask answered by `answer`:
    /// the outcome and the asks made.
    fn refresh(dirs: &Dirs, got: &[Got], reach: Reach, mut answer: impl FnMut(&Request) -> Result<Reply, NetFail>) -> (Outcome, Vec<Request>) {
        let cx = Ctx { dirs, got, now: NOW, reach };
        let (mut replies, mut asked) = (Vec::new(), Vec::new());
        loop {
            match MiniMax.next(&cx, &replies) {
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
        assert_eq!(MiniMax.wants(&dirs), vec![Want::Secret("minimax")]);
        for (got, trouble) in [(Got::Missing, Trouble::NeedsKey), (Got::Text(" ".into()), Trouble::NeedsKey), (Got::Denied, Trouble::Denied)] {
            let (outcome, asked) = refresh(&dirs, &[got], Reach::Online, |_| unreachable!());
            assert_eq!(outcome, Err(trouble));
            assert!(asked.is_empty());
        }
        let ask = MiniMax.takes_key().unwrap();
        assert_eq!((ask.secret, ask.hint), ("minimax", "sk-cp-…"));
        assert!(ask.url.starts_with("https://platform.minimax.io/"));
        assert_eq!(MiniMax.min_gap_s(), 300);
    }

    #[test]
    fn a_typed_key_goes_to_minimax_as_a_bearer_and_nowhere_else() {
        let (outcome, asked) = refresh(&Dirs::sample(Os::Mac), &typed(), Reach::Online, |_| reply(200, TOKEN_PLAN));
        assert!(outcome.is_ok());
        assert_eq!(urls(&asked), [G1]);
        assert_eq!(header(&asked[0], "Authorization"), Some("Bearer sk-cp-test-token"));
        assert_eq!(header(&asked[0], "MM-API-Source"), Some("Oyenza Studio"));
        assert_eq!(asked[0].headers.iter().filter(|(_, v)| v.contains("sk-cp-test-token")).count(), 1);
        assert!(!format!("{outcome:?}").contains("sk-cp-test-token"));
    }

    #[test]
    fn keys_from_the_variables_on_both_systems() {
        assert_eq!(MiniMax.env(), &["MINIMAX_CODING_API_KEY", "MINIMAX_API_KEY"]);
        let bearer = |dirs: &Dirs, got: &[Got]| {
            let (_, asked) = refresh(dirs, got, Reach::Online, |_| reply(200, TOKEN_PLAN));
            header(&asked[0], "Authorization").map(str::to_string)
        };
        for os in [Os::Windows, Os::Mac] {
            let mut dirs = Dirs::sample(os);
            dirs.env.push(("MINIMAX_API_KEY".into(), "sk-cp-general".into()));
            assert_eq!(bearer(&dirs, &[Got::Missing]).as_deref(), Some("Bearer sk-cp-general"));
            // The coding-plan variable wins, and a typed key wins over both.
            dirs.env.push(("MINIMAX_CODING_API_KEY".into(), "sk-cp-coding".into()));
            assert_eq!(bearer(&dirs, &[Got::Missing]).as_deref(), Some("Bearer sk-cp-coding"));
            assert_eq!(bearer(&dirs, &typed()).as_deref(), Some("Bearer sk-cp-test-token"));
            // A pay-as-you-go key in MINIMAX_API_KEY isn't a plan's: nothing
            // found, nothing sent.
            dirs.env = vec![("MINIMAX_API_KEY".into(), "sk-api-standard".into())];
            let (outcome, asked) = refresh(&dirs, &[Got::Missing], Reach::Online, |_| unreachable!());
            assert_eq!(outcome, Err(Trouble::NeedsKey));
            assert!(asked.is_empty());
        }
    }

    #[test]
    fn reads_the_token_plan() {
        let s = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, TOKEN_PLAN)).0.unwrap();
        assert_eq!((s.provider, s.source, s.as_of, s.plan.as_deref()), (ProviderId::MiniMax, Source::Online, NOW, None));
        // The video lane a Plus plan doesn't have is left out.
        let [five, week] = &s.windows[..] else { panic!("{:?}", s.windows) };
        let five_at = Some(NOW + 7200);
        assert_eq!((five.label.as_str(), five.kind, five.resets_at, five.length_s, five.count), ("5-hour", WindowKind::Session, five_at, Some(18_000), None));
        assert!((five.used - 0.62).abs() < 1e-6);
        let week_at = Some(NOW + 3 * 86_400);
        assert_eq!((week.label.as_str(), week.kind, week.resets_at, week.length_s, week.count), ("Weekly", WindowKind::Weekly, week_at, Some(604_800), None));
        assert!((week.used - 0.31).abs() < 1e-6);
    }

    #[test]
    fn reads_the_older_coding_plan_from_its_endpoint() {
        let older = |r: &Request| if r.url == G1 { reply(404, "") } else { reply(200, CODING_PLAN) };
        let (outcome, asked) = refresh(&Dirs::sample(Os::Mac), &typed(), Reach::Online, older);
        assert_eq!(urls(&asked), [G1, G2]);
        let s = outcome.unwrap();
        assert_eq!(s.plan.as_deref(), Some("Max"));
        let got: Vec<(&str, Option<(u64, u64)>)> = s.windows.iter().map(|w| (w.label.as_str(), w.count)).collect();
        assert_eq!(got, [("5-hour", Some((2790, 4500))), ("Weekly", Some((13_950, 45_000))), ("Daily · Speech", Some((0, 19_000)))]);
        let speech = &s.windows[2];
        assert_eq!((speech.kind, speech.resets_at, speech.length_s, speech.used), (WindowKind::Daily, Some(NOW + 68_400), Some(86_400), 0.0));
        assert!((s.windows[0].used - 0.62).abs() < 1e-6 && (s.windows[1].used - 0.31).abs() < 1e-6);
    }

    #[test]
    fn a_local_refresh_never_asks() {
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Local, |_| unreachable!());
        assert_eq!(outcome, Err(Trouble::NeedsOnline));
        assert!(asked.is_empty());
    }

    #[test]
    fn a_refused_key_tries_both_endpoints_on_both_hosts() {
        // HTTP 401, or HTTP 200 whose base_resp says 1004.
        let login = r#"{"base_resp":{"status_code":1004,
            "status_msg":"login fail: Please carry the API secret key in the 'Authorization' field of the request header"}}"#;
        for no in [reply(401, "{}"), reply(200, login)] {
            let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| no.clone());
            assert_eq!(outcome, Err(Trouble::Refused));
            assert_eq!(urls(&asked), [G1, G2, C1, C2]);
        }
    }

    #[test]
    fn a_china_key_reads_from_minimaxi() {
        let china = |r: &Request, there| if r.url.starts_with("https://api.minimax.io/") { reply(401, "{}") } else { there };
        let (outcome, asked) = refresh(&Dirs::sample(Os::Mac), &typed(), Reach::Online, |r| china(r, reply(200, TOKEN_PLAN)));
        assert_eq!(urls(&asked), [G1, G2, C1]);
        assert_eq!(outcome.unwrap().windows.len(), 2);
        // The Token Plan's refusal outranks the older endpoint's 404, and
        // minimaxi.com not answering leaves the refusal standing.
        let (outcome, asked) = refresh(&Dirs::sample(Os::Mac), &typed(), Reach::Online, |r| match r.url.as_str() {
            G1 => reply(401, "{}"),
            G2 => reply(404, ""),
            _ => Err(NetFail::Timeout),
        });
        assert_eq!(outcome, Err(Trouble::Refused));
        assert_eq!(urls(&asked), [G1, G2, C1, C2]);
    }

    #[test]
    fn too_many_asks_wait_as_told() {
        let busy: Result<Reply, NetFail> = Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(30) });
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| busy.clone());
        assert_eq!(outcome, Err(Trouble::Busy { retry_after_s: Some(30) }));
        assert_eq!(urls(&asked), [G1]);
        let limited = r#"{"base_resp":{"status_code":1002,"status_msg":"rate limit exceeded"}}"#;
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, limited));
        assert_eq!(outcome, Err(Trouble::Busy { retry_after_s: None }));
        assert_eq!(urls(&asked), [G1]);
    }

    #[test]
    fn answers_we_cannot_read_say_so() {
        // Not a refusal, so minimaxi.com isn't asked.
        for body in ["<html>Bad gateway</html>", r#"{"base_resp":{"status_code":0},"model_remains":[]}"#] {
            let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, body));
            assert!(matches!(outcome, Err(Trouble::Unreadable(_))), "{body}: {outcome:?}");
            assert_eq!(urls(&asked), [G1, G2]);
        }
        let odd = r#"{"base_resp":{"status_code":1008,"status_msg":"insufficient balance"}}"#;
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, odd));
        assert_eq!(outcome, Err(Trouble::Unreadable("status 1008".into())));
        assert_eq!(urls(&asked), [G1]);
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| Err(NetFail::Timeout));
        assert_eq!(outcome, Err(Trouble::Offline));
        assert_eq!(urls(&asked), [G1, G2]);
    }

    #[test]
    fn a_standard_key_typed_in_is_refused_unsent() {
        let (outcome, asked) = refresh(&Dirs::sample(Os::Windows), &[Got::Text("sk-api-standard".into())], Reach::Online, |_| unreachable!());
        assert_eq!(outcome, Err(Trouble::Refused));
        assert!(asked.is_empty());
    }

    #[test]
    fn lanes_follow_codexbar() {
        // Video first in the answer, inside `data`, numbers as strings, and an
        // unlimited week (status 3, all left): text leads, the week is left out.
        let body = r#"{"base_resp":{"status_code":"0"},"data":{"current_subscribe_title":"Token Plan · TokenPlanPlus-年度会员","model_remains":[
            {"model_name":"video","current_interval_total_count":100,"current_interval_usage_count":70,"current_interval_remaining_percent":30,
             "start_time":1793059200000,"end_time":1793145600000},
            {"model_name":"general","current_interval_remaining_percent":"96","start_time":1793066400000,"end_time":1793084400000,
             "current_weekly_status":3,"current_weekly_remaining_percent":100,"current_weekly_total_count":0,"current_weekly_usage_count":0,
             "weekly_start_time":1792731600000,"weekly_end_time":1793336400000}
        ]}}"#;
        let s = refresh(&Dirs::sample(Os::Windows), &typed(), Reach::Online, |_| reply(200, body)).0.unwrap();
        assert_eq!(s.plan.as_deref(), Some("Token Plan Plus"));
        let got: Vec<(&str, f32)> = s.windows.iter().map(|w| (w.label.as_str(), (w.used * 100.0).round())).collect();
        assert_eq!(got, [("5-hour", 4.0), ("Daily · Video", 70.0)]);
        assert_eq!(plan_name("Plus-月度会员"), "Plus");
        assert_eq!(plan_name("Coding Plan Pro"), "Coding Plan Pro");
    }
}
