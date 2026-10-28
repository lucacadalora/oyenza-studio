//! Codex (OpenAI's coding agent on ChatGPT plans): how much of its 5-hour
//! and weekly limits is used, when they reset, the plan and credits left.
//!
//! Two sources, both read only:
//! - This computer, no network: Codex writes the limits ChatGPT last gave it
//!   into its session logs, `sessions/YYYY/MM/DD/rollout-*.jsonl` under
//!   `$CODEX_HOME` or `~/.codex`. Their `token_count` events carry
//!   `rate_limits` (codex-rs `protocol.rs`, `RateLimitSnapshot`): `primary`
//!   and `secondary` windows with `used_percent`, `window_minutes` and
//!   `resets_at` in unix seconds (`resets_in_seconds` before Codex 0.50).
//! - ChatGPT, on online refreshes: the access token in Codex's `auth.json`
//!   goes to chatgpt.com's usage endpoint (the one Codex itself and CodexBar
//!   ask) and nowhere else, so a `chatgpt_base_url` in Codex's config is
//!   never followed.
//!
//! The token is never refreshed here: Codex does that the next time it runs.
//! While it is expired, and whenever the online ask fails, the logs answer if
//! they can. A sign-in with an API key has no plan windows: not found.

use super::{jwt_claims, mask_email, micros, num, parse_time, reply_json, time, Ctx, Dirs, FileRead, Got, Money, NetFail, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Step, Trouble, Want, Window, WindowKind};
use serde_json::Value;
use std::path::PathBuf;

/// ChatGPT's Codex usage endpoint (codex-rs `backend-client`, CodexBar's
/// OAuth fetcher): the plan, both windows and the credits in one answer.
const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// The newest session logs to look through, and how much of each one's end.
/// A turn ends with its token count and a few short lines, so the latest
/// limits sit in the last few KB unless a long tool output came after them.
const LOGS: usize = 4;
const LOG_TAIL: u64 = 128 * 1024;

/// A token this close to running out counts as expired: it could run out on
/// the way.
const EXPIRY_MARGIN_S: i64 = 60;

/// What Codex credits are counted in ([`Money::currency`]): credits, not
/// dollars.
pub const CREDITS: &str = "credits";

/// Keys `auth.json` holds instead of a ChatGPT sign-in (codex-rs
/// `AuthDotJson`): an API key, a personal access token, Bedrock keys.
const OTHER_KEYS: [&str; 6] = ["OPENAI_API_KEY", "personal_access_token", "personalAccessToken", "agent_identity", "bedrock_api_key", "bedrock_access_keys"];

pub struct Codex;

impl Provider for Codex {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn env(&self) -> &'static [&'static str] {
        &["CODEX_HOME"]
    }

    fn wants(&self, dirs: &Dirs) -> Vec<Want> {
        let home = home(dirs);
        vec![Want::file(dirs.join(&home, &["auth.json"])), Want::Newest { dir: dirs.join(&home, &["sessions"]), suffix: ".jsonl", depth: 3, count: LOGS, tail: LOG_TAIL }]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let login = match sign_in(cx.got(0)) {
            SignIn::Chatgpt(login) => login,
            // No auth.json: never signed in here, signed out, or the sign-in
            // lives in the keyring. The logs may still know.
            SignIn::None => return Step::Done(logs(cx, None).ok_or(Trouble::NotFound)),
            SignIn::Broken => return Step::Done(logs(cx, None).ok_or(Trouble::Unreadable("Codex's sign-in file can't be read".into()))),
            SignIn::Key => return Step::Done(Err(Trouble::NotFound)),
            SignIn::Out => return Step::Done(Err(Trouble::SignedOut)),
        };
        let local = || logs(cx, Some(&login));
        if let Some(r) = replies.last() {
            return Step::Done(answer(r, &login, cx.now).or_else(|t| local().ok_or(t)));
        }
        if login.exp.is_some_and(|exp| exp <= cx.now + EXPIRY_MARGIN_S) {
            return Step::Done(local().ok_or(Trouble::Expired));
        }
        if cx.reach == Reach::Local {
            return Step::Done(local().ok_or(Trouble::NeedsOnline));
        }
        let ask = Request::get(USAGE_URL).bearer(&login.token).header("Accept", "application/json");
        Step::Fetch(match &login.account {
            Some(id) => ask.header("ChatGPT-Account-Id", id.as_str()),
            None => ask,
        })
    }

    /// Codex's limits move with every turn: two minutes keeps the bars
    /// current without asking ChatGPT often.
    fn min_gap_s(&self) -> u64 {
        120
    }
}

/// Codex's folder: `$CODEX_HOME`, else `.codex` in the home folder, on both
/// apps.
fn home(dirs: &Dirs) -> PathBuf {
    dirs.var_or_home("CODEX_HOME", ".codex")
}

/// What Codex's `auth.json` holds.
enum SignIn {
    None,
    /// Not JSON (or caught while Codex rewrote it).
    Broken,
    /// An API key or another key: Codex without a plan.
    Key,
    /// There, but nobody is signed in.
    Out,
    Chatgpt(Login),
}

/// A ChatGPT sign-in, from the file and its tokens' claims.
struct Login {
    token: String,
    /// The workspace, sent as `ChatGPT-Account-Id`.
    account: Option<String>,
    /// When the access token runs out, unix seconds.
    exp: Option<i64>,
    /// The plan id the sign-in carries ("plus").
    plan: Option<String>,
    email: Option<String>,
}

fn sign_in(got: &Got) -> SignIn {
    if got.is_missing() {
        return SignIn::None;
    }
    let Some(v) = got.json().filter(Value::is_object) else { return SignIn::Broken };
    let tokens = &v["tokens"];
    let mode = v["auth_mode"].as_str().unwrap_or_default().to_ascii_lowercase();
    match text(tokens, "access_token", "accessToken") {
        Some(token) if mode != "apikey" && mode != "personalaccesstoken" => {
            // The id token names the person; the access token carries the same
            // ChatGPT claims, and its expiry.
            let id = text(tokens, "id_token", "idToken").and_then(jwt_claims).unwrap_or_default();
            let access = jwt_claims(token).unwrap_or_default();
            let claim = |k: &str| [&id, &access].into_iter().find_map(|c| nonempty(&c["https://api.openai.com/auth"][k]).or_else(|| nonempty(&c[k])));
            let email = [&id, &access].into_iter().find_map(|c| nonempty(&c["email"]).or_else(|| nonempty(&c["https://api.openai.com/profile"]["email"])));
            SignIn::Chatgpt(Login {
                token: token.to_string(),
                account: text(tokens, "account_id", "accountId").or_else(|| claim("chatgpt_account_id")).map(String::from),
                exp: num(&access["exp"]).map(|e| e as i64),
                plan: claim("chatgpt_plan_type").map(String::from),
                email: email.map(String::from),
            })
        }
        _ if OTHER_KEYS.iter().any(|k| held(&v[*k])) => SignIn::Key,
        _ => SignIn::Out,
    }
}

/// Holds something: not null, not an empty string.
fn held(v: &Value) -> bool {
    !v.is_null() && v.as_str().is_none_or(|s| !s.trim().is_empty())
}

/// A non-empty string, trimmed.
fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn text<'a>(v: &'a Value, snake: &str, camel: &str) -> Option<&'a str> {
    nonempty(&v[snake]).or_else(|| nonempty(&v[camel]))
}

/// The latest limits Codex wrote into its logs, as of that line. The
/// sign-in, when there is one, names the account and fills in a plan the
/// line leaves out.
fn logs(cx: &Ctx, login: Option<&Login>) -> Option<Snapshot> {
    let (at, rl) = latest(cx.got(1).files())?;
    let mut s = Snapshot::new(ProviderId::Codex, at, Source::Local);
    for (slot, key) in ["primary", "secondary"].into_iter().enumerate() {
        let w = &rl[key];
        let Some(used) = num(&w["used_percent"]) else { continue };
        let resets = time(&w["resets_at"]).or_else(|| num(&w["resets_in_seconds"]).map(|s| at + s as i64));
        s.windows.push(window(slot, used, num(&w["window_minutes"]).map(|m| m * 60.0), resets));
    }
    s.balance = credits(&rl["credits"]);
    Some(finish(s, nonempty(&rl["plan_type"]), login, cx.now))
}

/// The newest `token_count` line in the logs' tails that carries Codex's own
/// limits, as (unix seconds, its `rate_limits`). A line the tail cut in half
/// isn't JSON, so it drops out.
fn latest(files: &[FileRead]) -> Option<(i64, Value)> {
    let mut best: Option<(i64, Value)> = None;
    for f in files {
        for line in f.bytes.split(|b| *b == b'\n').rev() {
            if !has(line, b"token_count") || !has(line, b"rate_limits") {
                continue;
            }
            let Ok(mut v) = serde_json::from_slice::<Value>(line) else { continue };
            if v["type"] != "event_msg" || v["payload"]["type"] != "token_count" || !main_limits(&v["payload"]["rate_limits"]) {
                continue;
            }
            let at = v["timestamp"].as_str().and_then(parse_time).or(f.modified).unwrap_or(0);
            if best.as_ref().is_none_or(|(t, _)| at > *t) {
                best = Some((at, v["payload"]["rate_limits"].take()));
            }
            break;
        }
    }
    best
}

/// Codex's own limits: at least one window, and not a model's separate limit
/// (its `limit_id` isn't "codex"). Codex logs whichever it heard last.
fn main_limits(rl: &Value) -> bool {
    matches!(rl["limit_id"].as_str(), None | Some("codex")) && ["primary", "secondary"].iter().any(|k| num(&rl[*k]["used_percent"]).is_some())
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// ChatGPT's answer (codex-rs `RateLimitStatusPayload`): `plan_type`,
/// `rate_limit.primary_window`/`secondary_window` (`used_percent`,
/// `limit_window_seconds`, `reset_at`) and `credits`.
fn answer(r: &Result<Reply, NetFail>, login: &Login, now: i64) -> Outcome {
    let v = reply_json(r)?;
    let rl = &v["rate_limit"];
    if !rl.is_object() && !v["credits"].is_object() && !v["plan_type"].is_string() {
        return Err(Trouble::Unreadable("no usage in ChatGPT's answer".into()));
    }
    let mut s = Snapshot::new(ProviderId::Codex, now, Source::Online);
    for (slot, key) in ["primary_window", "secondary_window"].into_iter().enumerate() {
        let w = &rl[key];
        let Some(used) = num(&w["used_percent"]) else { continue };
        let resets = time(&w["reset_at"]).or_else(|| num(&w["reset_after_seconds"]).map(|s| now + s as i64));
        s.windows.push(window(slot, used, num(&w["limit_window_seconds"]), resets));
    }
    s.balance = credits(&v["credits"]);
    let mut s = finish(s, nonempty(&v["plan_type"]), Some(login), now);
    if s.account.is_none() {
        s.account = nonempty(&v["email"]).map(mask_email);
    }
    Ok(s)
}

/// One window from Codex's numbers: `used` in percent, its length in seconds.
/// Codex's first window is the 5-hour one and its second the week, unless
/// their lengths say otherwise.
fn window(slot: usize, used: f64, len_s: Option<f64>, resets: Option<i64>) -> Window {
    let len = len_s.filter(|s| (60.0..4.0e9).contains(s)).map(|s| s as u32);
    let (kind, label) = match len {
        Some(s) => named(s / 60),
        None if slot == 0 => (WindowKind::Session, "5-hour".to_string()),
        None => (WindowKind::Weekly, "Weekly".to_string()),
    };
    Window::new(kind, label, (used / 100.0) as f32).resets(resets).length(len)
}

/// What a window of `m` minutes is called.
fn named(m: u32) -> (WindowKind, String) {
    match m {
        10_080 => (WindowKind::Weekly, "Weekly".into()),
        1_440 => (WindowKind::Daily, "Daily".into()),
        m if m >= 28 * 1_440 => (WindowKind::Monthly, "Monthly".into()),
        m if m % 1_440 == 0 => (WindowKind::Other, format!("{}-day", m / 1_440)),
        m if m % 60 == 0 => (WindowKind::Session, format!("{}-hour", m / 60)),
        m => (WindowKind::Session, format!("{m}-minute")),
    }
}

/// Credits left when the plan has them; an unlimited or missing amount stays
/// unknown.
fn credits(c: &Value) -> Option<Money> {
    if c["has_credits"] != true || c["unlimited"] == true {
        return None;
    }
    num(&c["balance"]).map(|n| Money { micros: micros(n), currency: CREDITS.into() })
}

/// Names the plan and the account, puts the shorter window first and shows
/// each window as it stands `now`: a reset that passed since the reading
/// empties it ([`Window::at`]).
fn finish(mut s: Snapshot, plan: Option<&str>, login: Option<&Login>, now: i64) -> Snapshot {
    s.plan = plan.and_then(plan_name).or_else(|| login?.plan.as_deref().and_then(plan_name));
    s.account = login.and_then(|l| l.email.as_deref()).map(mask_email);
    if let [a, b] = &s.windows[..] {
        if matches!((a.length_s, b.length_s), (Some(x), Some(y)) if x > y) {
            s.windows.swap(0, 1);
        }
    }
    for w in &mut s.windows {
        *w = w.at(now);
        // Read more than a whole window ago: when it resets next isn't known.
        if w.resets_at.is_some_and(|t| t <= now) {
            w.resets_at = None;
        }
    }
    s
}

/// The plan as the tab names it, from Codex's plan id (codex-rs `account.rs`,
/// `PlanType`; "prolite" and "promax" are the smaller and the bigger Pro).
fn plan_name(id: &str) -> Option<String> {
    let id = id.trim().to_ascii_lowercase();
    let name = match id.as_str() {
        "" | "unknown" | "guest" => return None,
        "free" | "free_workspace" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "pro" => "Pro",
        "prolite" | "pro_lite" => "Pro Lite",
        "promax" | "pro_max" => "Pro Max",
        "team" => "Team",
        "business" | "self_serve_business_prolite" | "self_serve_business_usage_based" => "Business",
        "enterprise" | "ent26" | "hc" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" => "Enterprise",
        "edu" | "education" => "Edu",
        "edu_plus" => "Edu Plus",
        "edu_pro" => "Edu Pro",
        // A plan newer than this list: its id in words ("sci_plus" is "Sci Plus").
        _ => {
            let words: Vec<String> = id.split(['_', '-', ' ']).filter(|w| !w.is_empty()).map(capital).collect();
            return (!words.is_empty()).then(|| words.join(" "));
        }
    };
    Some(name.into())
}

fn capital(word: &str) -> String {
    let mut c = word.chars();
    c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Method, Os, FILE_MAX};

    /// 2026-10-27T05:00:00Z.
    const NOW: i64 = 1_793_077_200;
    // Hand-made, every value fake. The tokens are JWTs signed "test-token"
    // whose claims hold a@example.com, acct_test and the plan "plus"; the
    // access token runs out in 2100 (auth-expired.json: 2026-10-26T07:33:20Z).
    const AUTH: &str = include_str!("fixtures/codex/auth.json");
    const AUTH_EXPIRED: &str = include_str!("fixtures/codex/auth-expired.json");
    const AUTH_KEY: &str = include_str!("fixtures/codex/auth-apikey.json");
    const AUTH_OUT: &str = include_str!("fixtures/codex/auth-signed-out.json");
    /// ChatGPT's answer for a Pro plan with credits (and a model's own limit,
    /// which the card leaves out).
    const USAGE: &str = include_str!("fixtures/codex/usage.json");
    /// Today's session: the last token count with Codex's own limits is at
    /// 02:58:20Z (42% / 21%); a model's separate limit comes after it.
    const LOG: &str = include_str!("fixtures/codex/rollout.jsonl");
    const LOG_AT: i64 = 1_793_069_900;
    /// A session from Codex 0.46 (resets in seconds), 2026-10-25T10:00:07Z.
    const LOG_OLD: &str = include_str!("fixtures/codex/rollout-old.jsonl");
    const LOG_OLD_AT: i64 = 1_792_922_407;

    fn read(path: &str, text: &str) -> FileRead {
        FileRead { path: PathBuf::from(path), bytes: text.as_bytes().to_vec(), modified: Some(NOW - 60) }
    }

    /// What the app read: Codex's auth.json, if any, and its newest logs.
    fn got(auth: Option<&str>, logs: &[&str]) -> Vec<Got> {
        let logs = logs.iter().enumerate().map(|(i, l)| read(&format!("/Users/u/.codex/sessions/2026/10/27/rollout-{i}.jsonl"), l)).collect();
        vec![auth.map_or(Got::Missing, |a| Got::One(read("/Users/u/.codex/auth.json", a))), Got::Many(logs)]
    }

    fn step(got: &[Got], reach: Reach, now: i64, replies: &[Result<Reply, NetFail>]) -> Step {
        let dirs = Dirs::sample(Os::Mac);
        Codex.next(&Ctx { dirs: &dirs, got, now, reach }, replies)
    }

    /// One refresh the way the app runs it: read, ask at most once, answer.
    /// `reply: None` means it must not ask.
    fn refresh(auth: Option<&str>, logs: &[&str], reach: Reach, now: i64, reply: Option<Result<Reply, NetFail>>) -> Outcome {
        let got = got(auth, logs);
        match step(&got, reach, now, &[]) {
            Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
            Step::Done(o) => o,
            Step::Fetch(ask) => match step(&got, reach, now, &[reply.unwrap_or_else(|| panic!("asked {} without a reply ready", ask.url))]) {
                Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
                Step::Done(o) => o,
                Step::Fetch(ask) => panic!("a second ask: {}", ask.url),
            },
        }
    }

    fn ok(body: &str) -> Option<Result<Reply, NetFail>> {
        Some(Ok(Reply { status: 200, body: body.into(), retry_after_s: None }))
    }

    /// Each window as (kind, label, percent used, reset, length).
    fn bars(s: &Snapshot) -> Vec<(WindowKind, &str, i32, Option<i64>, Option<u32>)> {
        s.windows.iter().map(|w| (w.kind, w.label.as_str(), (w.used * 100.0).round() as i32, w.resets_at, w.length_s)).collect()
    }

    #[test]
    fn reads_codex_home_on_both_apps() {
        let mac = Dirs::sample(Os::Mac);
        assert_eq!(
            Codex.wants(&mac),
            vec![
                Want::File { path: PathBuf::from("/Users/u/.codex/auth.json"), max: FILE_MAX },
                Want::Newest { dir: PathBuf::from("/Users/u/.codex/sessions"), suffix: ".jsonl", depth: 3, count: 4, tail: 128 * 1024 },
            ]
        );
        let win = Dirs::sample(Os::Windows);
        assert_eq!(Codex.wants(&win), vec![Want::file(PathBuf::from(r"C:\Users\u\.codex\auth.json")), Want::Newest { dir: PathBuf::from(r"C:\Users\u\.codex\sessions"), suffix: ".jsonl", depth: 3, count: 4, tail: 128 * 1024 }]);
        // CODEX_HOME moves all of it.
        assert_eq!(Codex.env(), &["CODEX_HOME"]);
        for (mut dirs, home) in [(Dirs::sample(Os::Windows), r"D:\tools\codex"), (Dirs::sample(Os::Mac), "/Users/u/work/.codex")] {
            dirs.env.push(("CODEX_HOME".into(), home.into()));
            let home = PathBuf::from(home);
            assert_eq!(Codex.wants(&dirs)[0], Want::file(dirs.join(&home, &["auth.json"])));
            assert!(matches!(&Codex.wants(&dirs)[1], Want::Newest { dir, .. } if *dir == dirs.join(&home, &["sessions"])));
        }
    }

    #[test]
    fn online_answer_reads_plan_windows_and_credits() {
        let got = got(Some(AUTH), &[LOG]);
        let Step::Fetch(ask) = step(&got, Reach::Online, NOW, &[]) else { panic!("didn't ask ChatGPT") };
        let token = serde_json::from_str::<Value>(AUTH).unwrap()["tokens"]["access_token"].as_str().unwrap().to_string();
        assert_eq!((ask.method, ask.url.as_str(), ask.json.as_deref()), (Method::Get, "https://chatgpt.com/backend-api/wham/usage", None));
        assert!(ask.headers.contains(&("Authorization".into(), format!("Bearer {token}"))));
        assert!(ask.headers.contains(&("ChatGPT-Account-Id".into(), "acct_test".into())));
        assert!(!ask.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent")), "the app asks as itself");

        let s = refresh(Some(AUTH), &[LOG], Reach::Online, NOW, ok(USAGE)).unwrap();
        assert_eq!((s.source, s.as_of), (Source::Online, NOW));
        assert_eq!(s.plan.as_deref(), Some("Pro"), "the answer's plan wins over the sign-in's Plus");
        assert_eq!(s.account.as_deref(), Some("a•••@example.com"));
        assert_eq!(bars(&s), vec![(WindowKind::Session, "5-hour", 62, Some(1_793_085_600), Some(18_000)), (WindowKind::Weekly, "Weekly", 31, Some(1_793_258_400), Some(604_800))]);
        assert_eq!(s.balance, Some(Money { micros: 1_250_000_000, currency: "credits".into() }));
        assert_eq!(s.tokens_today, None);
        // The token went to chatgpt.com only, never into the snapshot.
        let saved = serde_json::to_string(&s).unwrap();
        assert!(!saved.contains(&token) && !saved.contains("test-token") && !saved.contains("acct_test"));
    }

    #[test]
    fn logs_answer_without_a_sign_in_file() {
        // The sign-in kept in the keyring: the newest line with Codex's own
        // limits wins, over the model's separate one after it and the older
        // session listed first.
        let s = refresh(None, &[LOG_OLD, LOG], Reach::Online, NOW, None).unwrap();
        assert_eq!((s.source, s.as_of), (Source::Local, LOG_AT));
        assert_eq!(s.plan.as_deref(), Some("Plus"));
        assert_eq!(s.account, None);
        assert_eq!(bars(&s), vec![(WindowKind::Session, "5-hour", 42, Some(1_793_082_600), Some(18_000)), (WindowKind::Weekly, "Weekly", 21, Some(1_793_258_400), Some(604_800))]);
        assert_eq!(s.balance, Some(Money { micros: 40_000_000, currency: CREDITS.into() }));
        // A tail that starts mid-line reads the same.
        let cut = &LOG[LOG.find("\"function_call\"").unwrap()..];
        assert_eq!(refresh(None, &[cut], Reach::Local, NOW, None).unwrap().as_of, LOG_AT);
    }

    #[test]
    fn older_codex_logs_count_resets_from_the_line() {
        let s = refresh(None, &[LOG_OLD], Reach::Local, LOG_OLD_AT + 60, None).unwrap();
        assert_eq!(bars(&s), vec![(WindowKind::Session, "5-hour", 80, Some(LOG_OLD_AT + 3_600), Some(18_000)), (WindowKind::Weekly, "Weekly", 55, Some(LOG_OLD_AT + 400_000), Some(604_800))]);
        assert_eq!((s.plan, s.balance), (None, None));
    }

    #[test]
    fn a_reset_that_passed_reads_as_empty() {
        // Read after the 5-hour window reset at 06:30Z: empty, the next reset
        // a window later, while the week carries on.
        let reset = 1_793_082_600;
        let s = refresh(None, &[LOG], Reach::Local, reset + 60, None).unwrap();
        assert_eq!(bars(&s)[0], (WindowKind::Session, "5-hour", 0, Some(reset + 18_000), Some(18_000)));
        assert_eq!(bars(&s)[1].2, 21);
        // More than a whole window later, when it resets next isn't known.
        let s = refresh(None, &[LOG], Reach::Local, reset + 18_001, None).unwrap();
        assert_eq!(bars(&s)[0], (WindowKind::Session, "5-hour", 0, None, Some(18_000)));
        assert_eq!(s.windows[0].at(reset + 18_001), s.windows[0]);
    }

    #[test]
    fn local_refreshes_never_ask() {
        let s = refresh(Some(AUTH), &[LOG], Reach::Local, NOW, None).unwrap();
        assert_eq!((s.source, s.as_of), (Source::Local, LOG_AT));
        assert_eq!(s.account.as_deref(), Some("a•••@example.com"));
        assert_eq!(refresh(Some(AUTH), &[], Reach::Local, NOW, None), Err(Trouble::NeedsOnline));
    }

    #[test]
    fn an_expired_sign_in_is_never_sent() {
        // Codex renews it the next time it runs; until then the logs answer.
        let s = refresh(Some(AUTH_EXPIRED), &[LOG], Reach::Online, NOW, None).unwrap();
        assert_eq!((s.source, s.as_of), (Source::Local, LOG_AT));
        assert_eq!((s.plan.as_deref(), s.account.as_deref()), (Some("Plus"), Some("a•••@example.com")));
        assert_eq!(refresh(Some(AUTH_EXPIRED), &[], Reach::Online, NOW, None), Err(Trouble::Expired));
    }

    #[test]
    fn failed_asks_say_why_unless_the_logs_can_answer() {
        let reply = |status, retry_after_s| Some(Ok(Reply { status, body: "{}".into(), retry_after_s }));
        assert_eq!(refresh(Some(AUTH), &[], Reach::Online, NOW, reply(401, None)), Err(Trouble::Refused));
        assert_eq!(refresh(Some(AUTH), &[], Reach::Online, NOW, reply(429, Some(300))), Err(Trouble::Busy { retry_after_s: Some(300) }));
        assert_eq!(refresh(Some(AUTH), &[], Reach::Online, NOW, Some(Err(NetFail::Timeout))), Err(Trouble::Offline));
        let s = refresh(Some(AUTH), &[LOG], Reach::Online, NOW, reply(401, None)).unwrap();
        assert_eq!((s.source, s.as_of), (Source::Local, LOG_AT));
    }

    #[test]
    fn an_unreadable_answer_says_so() {
        // A challenge page instead of JSON, and JSON without any usage.
        for body in ["<!DOCTYPE html><html><head><title>Just a moment...</title></head></html>", r#"{"detail":"Not here"}"#] {
            assert!(matches!(refresh(Some(AUTH), &[], Reach::Online, NOW, ok(body)), Err(Trouble::Unreadable(_))), "{body}");
        }
    }

    #[test]
    fn windows_follow_their_lengths() {
        // The week first, then a plan with only a week.
        let swapped = r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_at":1793258400},"secondary_window":{"used_percent":50,"limit_window_seconds":18000,"reset_at":1793085600}}}"#;
        let s = refresh(Some(AUTH), &[], Reach::Online, NOW, ok(swapped)).unwrap();
        assert_eq!(bars(&s), vec![(WindowKind::Session, "5-hour", 50, Some(1_793_085_600), Some(18_000)), (WindowKind::Weekly, "Weekly", 10, Some(1_793_258_400), Some(604_800))]);
        let week = r#"{"plan_type":"free","rate_limit":{"primary_window":{"used_percent":7,"limit_window_seconds":604800,"reset_after_seconds":3600},"secondary_window":null},"credits":{"has_credits":false,"unlimited":false,"balance":"0"}}"#;
        let s = refresh(Some(AUTH), &[], Reach::Online, NOW, ok(week)).unwrap();
        assert_eq!(bars(&s), vec![(WindowKind::Weekly, "Weekly", 7, Some(NOW + 3_600), Some(604_800))]);
        assert_eq!((s.plan.as_deref(), s.balance), (Some("Free"), None));
    }

    #[test]
    fn nothing_here_or_nobody_signed_in() {
        assert_eq!(refresh(None, &[], Reach::Online, NOW, None), Err(Trouble::NotFound));
        // Codex with another provider or a key: its token counts carry no limits.
        let no_limits = r#"{"timestamp":"2026-10-27T04:12:09.004Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":null}}"#;
        assert_eq!(refresh(None, &[no_limits], Reach::Online, NOW, None), Err(Trouble::NotFound));
        // An API key has no plan, whatever older logs say.
        assert_eq!(refresh(Some(AUTH_KEY), &[LOG], Reach::Online, NOW, None), Err(Trouble::NotFound));
        assert_eq!(refresh(Some(AUTH_OUT), &[LOG], Reach::Online, NOW, None), Err(Trouble::SignedOut));
        assert!(matches!(refresh(Some(r#"{"tokens": {"access"#), &[], Reach::Online, NOW, None), Err(Trouble::Unreadable(_))));
    }

    #[test]
    fn plans_and_windows_get_plain_names() {
        for (id, name) in [("plus", Some("Plus")), ("pro", Some("Pro")), ("prolite", Some("Pro Lite")), ("team", Some("Team")), ("self_serve_business_usage_based", Some("Business")), ("ent26", Some("Enterprise")), ("education", Some("Edu")), ("free", Some("Free")), ("sci_plus", Some("Sci Plus")), ("unknown", None), ("", None), ("__", None)] {
            assert_eq!(plan_name(id).as_deref(), name, "{id}");
        }
        assert_eq!(named(300), (WindowKind::Session, "5-hour".into()));
        assert_eq!(named(10_080), (WindowKind::Weekly, "Weekly".into()));
        assert_eq!(named(43_200), (WindowKind::Monthly, "Monthly".into()));
    }
}
