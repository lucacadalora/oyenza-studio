//! OpenCode Go: the subscription's 5-hour, weekly and monthly limits, plus
//! the OpenCode Zen balance (prepaid credit), from opencode.ai with the API
//! key OpenCode keeps on this computer, or one typed into Oyenza Studio.
//!
//! OpenCode follows the XDG layout on every OS, so its data folder is
//! `$XDG_DATA_HOME/opencode`, else `~/.local/share/opencode`, on Windows too
//! (`%USERPROFILE%\.local\share\opencode`). OpenCode 1 keeps API keys in
//! `auth.json` there, Go's as `{"opencode-go": {"type": "api", "key": ..}}`.
//! OpenCode 2 moved them into its SQLite database (`opencode.db`), which
//! this reader doesn't open: 2 imports `auth.json` once and leaves it as it
//! was, so a copy upgraded from 1 still has it, and a fresh 2 takes the key
//! typed in. `OPENCODE_API_KEY`, which OpenCode itself reads for Go and
//! Zen, works too.
//!
//! Online, with the key (CodexBar's `OpenCodeGoUsageFetcher.fetchAPIUsage`,
//! and OpenCode 2's own `usage-limits.ts`):
//! 1. `GET https://opencode.ai/zen/go/v1/usage`: `{"usage": {"rolling",
//!    "weekly", "monthly": {"status": "ok" | "rate-limited", "percent":
//!    0..100, "resetsAt": RFC 3339}}}`. A 403 is a key whose workspace has
//!    no Go subscription.
//! 2. `GET https://opencode.ai/console/api/billing/status`: the prepaid
//!    balance, `balanceMicroCents` (a signed integer string, 100,000,000 to
//!    the dollar). CodexBar reads it with the Console's browser session;
//!    the Console's API also takes API keys (they can be allowed to read
//!    billing), so this asks with the key, and a refusal only leaves the
//!    balance unknown.
//!
//! Nothing without the network: OpenCode's own history (in SQLite) holds
//! costs only, and Go's limits are set per model, so no percentage can be
//! worked out here (CodexBar's local guess divides by fixed amounts that no
//! longer match the plans).

use super::{join, num, reply_json, time, Ctx, Dirs, KeyAsk, Money, NetFail, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Step, Trouble, Want, Window, WindowKind};
use serde_json::Value;
use std::path::PathBuf;

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const BILLING_URL: &str = "https://opencode.ai/console/api/billing/status";
/// The typed key's name ([`Want::Secret`]).
const SECRET: &str = "opencode";

/// Go's windows: the answer's name for each, what the tab calls it, and how
/// long it is (a month's length varies, so it stays unknown).
const WINDOWS: [(&str, WindowKind, &str, Option<u32>); 3] = [
    ("rolling", WindowKind::Session, "5-hour", Some(5 * 3600)),
    ("weekly", WindowKind::Weekly, "Weekly", Some(7 * 86_400)),
    ("monthly", WindowKind::Monthly, "Monthly", None),
];

pub struct OpenCode;

/// OpenCode's data folder (`auth.json`, `opencode.db`).
pub fn data_dir(dirs: &Dirs) -> PathBuf {
    match dirs.var("XDG_DATA_HOME") {
        Some(v) => PathBuf::from(v).join("opencode"),
        None => join(&dirs.home, &[".local", "share", "opencode"]),
    }
}

impl Provider for OpenCode {
    fn id(&self) -> ProviderId {
        ProviderId::OpenCode
    }

    fn env(&self) -> &'static [&'static str] {
        &["OPENCODE_API_KEY", "XDG_DATA_HOME"]
    }

    fn wants(&self, dirs: &Dirs) -> Vec<Want> {
        vec![Want::Secret(SECRET), Want::file(data_dir(dirs).join("auth.json"))]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let (key, origin) = match key(cx) {
            Ok(k) => k,
            Err(t) => return Step::Done(Err(t)),
        };
        if cx.reach == Reach::Local {
            return Step::Done(Err(Trouble::NeedsOnline));
        }
        let ask = |url: &str| Step::Fetch(Request::get(url).bearer(&key).header("Accept", "application/json"));
        let go = match replies.first() {
            None => return ask(USAGE_URL),
            Some(r) => match read_usage(r, cx.now) {
                Ok(go) => go,
                Err(t) => return Step::Done(Err(t)),
            },
        };
        let balance = match replies.get(1) {
            None => return ask(BILLING_URL),
            Some(r) => read_balance(r),
        };
        Step::Done(match (go, balance) {
            (Some(mut s), balance) => {
                s.balance = balance;
                Ok(s)
            }
            // No Go in the key's workspace, but a Zen balance to show.
            (None, Some(money)) => {
                let mut s = Snapshot::new(ProviderId::OpenCode, cx.now, Source::Online);
                s.balance = Some(money);
                Ok(s)
            }
            // `OPENCODE_API_KEY` also serves Zen, so Go turning it down only
            // means there's no Go here; a key meant for Go was refused.
            (None, None) if origin == Origin::Env => Err(Trouble::NotFound),
            (None, None) => Err(Trouble::Refused),
        })
    }

    fn min_gap_s(&self) -> u64 {
        // Go's meters move with every request; two minutes is plenty.
        120
    }

    fn takes_key(&self) -> Option<KeyAsk> {
        // For OpenCode 2, whose keys live in its database.
        Some(KeyAsk { secret: SECRET, url: "https://opencode.ai/console", hint: "oc_sk_…" })
    }
}

/// Where the key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// Typed into Oyenza Studio for this service.
    Typed,
    /// The Go entry of OpenCode 1's `auth.json`.
    File,
    /// `OPENCODE_API_KEY`, which Zen shares.
    Env,
}

/// The key to ask with: a typed one, then OpenCode's own order (its file
/// before the variable); else why there is none.
fn key(cx: &Ctx) -> Result<(String, Origin), Trouble> {
    let file = cx.got(1).json().and_then(|j| go_entry(&j));
    let found = [(clean(cx.got(0).text()), Origin::Typed), (file.clone().flatten(), Origin::File), (clean(cx.dirs.var("OPENCODE_API_KEY")), Origin::Env)];
    match found.into_iter().find_map(|(k, origin)| Some((k?, origin))) {
        Some(k) => Ok(k),
        // A Go entry that holds no key: signed out of Go.
        None if file.is_some() => Err(Trouble::SignedOut),
        None => Err(Trouble::NotFound),
    }
}

/// OpenCode 1's Go entry: None without one, Some(None) when it holds no
/// usable API key.
fn go_entry(auth: &Value) -> Option<Option<String>> {
    let e = auth.get("opencode-go")?;
    // Only "api" entries hold a key ("wellknown" ones name a variable).
    let api = matches!(e.get("type").and_then(Value::as_str), Some("api") | None);
    Some(if api { clean(e.get("key").and_then(Value::as_str)) } else { None })
}

/// A key as stored or pasted: trimmed, quotes dropped, and nothing that
/// can't go in a header.
fn clean(s: Option<&str>) -> Option<String> {
    let s = s?.trim().trim_matches(|c: char| c == '"' || c == '\'');
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic())).then(|| s.to_string())
}

/// Go's answer: its windows, None when the key's workspace has no Go (403,
/// or `"usage": null`), or why it can't be read.
fn read_usage(r: &Result<Reply, NetFail>, now: i64) -> Result<Option<Snapshot>, Trouble> {
    if matches!(r, Ok(r) if r.status == 403) {
        return Ok(None);
    }
    let j = reply_json(r)?;
    if j.is_null() || j.get("usage").is_some_and(Value::is_null) {
        return Ok(None);
    }
    let usage = j.get("usage").ok_or_else(|| Trouble::Unreadable("no usage in the answer".into()))?;
    let mut s = Snapshot::new(ProviderId::OpenCode, now, Source::Online);
    s.windows = WINDOWS.iter().filter_map(|&(name, kind, label, len)| meter(usage.get(name)?, kind, label, len, now)).collect();
    if s.windows.is_empty() {
        return Err(Trouble::Unreadable("no limits in the answer".into()));
    }
    // The answer doesn't say Go or Go Plus (a Go tier): Go covers both.
    s.plan = Some("Go".into());
    Ok(Some(s))
}

/// One window: `percent` used (0..100) and `resetsAt`; `status`
/// "rate-limited" means Go refuses requests until the reset, so it's full
/// whatever the percent says.
fn meter(m: &Value, kind: WindowKind, label: &str, len: Option<u32>, now: i64) -> Option<Window> {
    let mut used = (num(m.get("percent")?)? / 100.0) as f32;
    if m.get("status").and_then(Value::as_str) == Some("rate-limited") {
        used = used.max(1.0);
    }
    let resets = m.get("resetsAt").and_then(time).or_else(|| m.get("resetInSec").and_then(num).map(|s| now + s as i64));
    Some(Window::new(kind, label, used).resets(resets).length(len))
}

/// The Zen balance from the Console's billing status, for prepaid
/// pay-as-you-go workspaces (others have no balance to show). Any other
/// answer, a refusal included, leaves it unknown.
fn read_balance(r: &Result<Reply, NetFail>) -> Option<Money> {
    let j = reply_json(r).ok()?;
    if j.get("billingMode")?.as_str()? != "prepaid" || j.get("mode")?.as_str()? != "pay-as-you-go" {
        return None;
    }
    let micro_cents = match j.get("balanceMicroCents")? {
        Value::String(s) => s.trim().parse::<i64>().ok()?,
        v => v.as_i64()?,
    };
    // 100,000,000 micro-cents to the dollar, 1,000,000 micros.
    Some(Money { micros: micro_cents / 100, currency: "USD".into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_time, FileRead, Got, Method, Os, Outcome, MAX_FETCHES};

    const AUTH: &str = include_str!("fixtures/opencode/auth.json");
    const AUTH_NO_GO: &str = include_str!("fixtures/opencode/auth-no-go.json");
    const AUTH_SIGNED_OUT: &str = include_str!("fixtures/opencode/auth-signed-out.json");
    const USAGE: &str = include_str!("fixtures/opencode/usage.json");
    const BILLING: &str = include_str!("fixtures/opencode/billing.json");
    /// 2026-10-27T05:00:00Z.
    const NOW: i64 = 1_793_077_200;

    fn auth(text: &str) -> Got {
        Got::One(FileRead { path: PathBuf::from("auth.json"), bytes: text.as_bytes().to_vec(), modified: Some(NOW - 3600) })
    }

    fn reply(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    fn with_env_key(mut d: Dirs) -> Dirs {
        d.env.push(("OPENCODE_API_KEY".into(), "test-env-token".into()));
        d
    }

    /// Runs the reader to its outcome, answering its requests in order.
    fn run(dirs: &Dirs, got: &[Got], reach: Reach, answers: &[Result<Reply, NetFail>]) -> (Vec<Request>, Outcome) {
        let cx = Ctx { dirs, got, now: NOW, reach };
        let (mut asked, mut replies) = (Vec::new(), Vec::new());
        loop {
            match OpenCode.next(&cx, &replies) {
                Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
                Step::Done(outcome) => return (asked, outcome),
                Step::Fetch(req) => {
                    assert!(asked.len() < MAX_FETCHES, "too many requests");
                    asked.push(req);
                    replies.push(answers.get(replies.len()).cloned().expect("an answer for every request"));
                }
            }
        }
    }

    /// The usual case: Go's key in auth.json, online.
    fn online(answers: &[Result<Reply, NetFail>]) -> (Vec<Request>, Outcome) {
        run(&Dirs::sample(Os::Windows), &[Got::Missing, auth(AUTH)], Reach::Online, answers)
    }

    #[test]
    fn reads_the_same_folder_on_windows_and_the_mac() {
        for os in [Os::Windows, Os::Mac] {
            let d = Dirs::sample(os);
            let file = Want::file(join(&d.home, &[".local", "share", "opencode", "auth.json"]));
            assert_eq!(OpenCode.wants(&d), vec![Want::Secret("opencode"), file]);
        }
        // Not %APPDATA% or %LOCALAPPDATA%: OpenCode keeps its XDG folders on Windows.
        let w = Dirs::sample(Os::Windows);
        let dir = data_dir(&w);
        assert!(dir.starts_with(&w.home) && !dir.starts_with(&w.config) && !dir.starts_with(&w.local));
    }

    #[test]
    fn xdg_data_home_moves_it() {
        let mut d = Dirs::sample(Os::Mac);
        d.env.push(("XDG_DATA_HOME".into(), "/Users/u/data".into()));
        assert_eq!(OpenCode.wants(&d)[1], Want::file("/Users/u/data/opencode/auth.json"));
        let mut w = Dirs::sample(Os::Windows);
        w.env.push(("XDG_DATA_HOME".into(), r"D:\xdg".into()));
        assert_eq!(OpenCode.wants(&w)[1], Want::file(PathBuf::from(r"D:\xdg").join("opencode").join("auth.json")));
        // Set but blank is not set.
        d.env[0].1 = " ".into();
        assert_eq!(OpenCode.wants(&d)[1], Want::file("/Users/u/.local/share/opencode/auth.json"));
        assert_eq!(OpenCode.env(), &["OPENCODE_API_KEY", "XDG_DATA_HOME"]);
    }

    #[test]
    fn the_key_comes_typed_then_from_auth_json_then_the_variable() {
        let d = with_env_key(Dirs::sample(Os::Windows));
        let first = |got: &[Got]| match OpenCode.next(&Ctx { dirs: &d, got, now: NOW, reach: Reach::Online }, &[]) {
            Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
            Step::Fetch(r) => r.headers.into_iter().find(|(k, _)| k == "Authorization").map(|(_, v)| v),
            Step::Done(o) => panic!("{o:?}"),
        };
        assert_eq!(first(&[Got::Text(" test-typed-token\n".into()), auth(AUTH)]).as_deref(), Some("Bearer test-typed-token"));
        assert_eq!(first(&[Got::Missing, auth(AUTH)]).as_deref(), Some("Bearer test-token"));
        assert_eq!(first(&[Got::Missing, auth(AUTH_NO_GO)]).as_deref(), Some("Bearer test-env-token"));
        assert_eq!(first(&[Got::Missing, auth(AUTH_SIGNED_OUT)]).as_deref(), Some("Bearer test-env-token"));
        assert_eq!(first(&[Got::Missing, Got::Missing]).as_deref(), Some("Bearer test-env-token"));
    }

    #[test]
    fn nothing_of_go_here_is_not_found() {
        let d = Dirs::sample(Os::Mac);
        // No file, an OpenCode without Go (its Zen key isn't Go's), a broken file.
        for file in [Got::Missing, auth(AUTH_NO_GO), auth("{\"opencode-go\": {")] {
            assert_eq!(run(&d, &[Got::Missing, file], Reach::Online, &[]), (vec![], Err(Trouble::NotFound)));
        }
    }

    #[test]
    fn a_go_entry_without_a_key_is_signed_out() {
        let d = Dirs::sample(Os::Mac);
        assert_eq!(run(&d, &[Got::Missing, auth(AUTH_SIGNED_OUT)], Reach::Online, &[]), (vec![], Err(Trouble::SignedOut)));
        let wellknown = r#"{"opencode-go": {"type": "wellknown", "key": "SOME_VARIABLE", "token": "test-token"}}"#;
        assert_eq!(run(&d, &[Got::Missing, auth(wellknown)], Reach::Online, &[]).1, Err(Trouble::SignedOut));
    }

    #[test]
    fn go_windows_and_the_zen_balance() {
        let (asked, out) = online(&[reply(200, USAGE), reply(200, BILLING)]);
        assert_eq!(asked.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(), [USAGE_URL, BILLING_URL]);
        for r in &asked {
            assert_eq!((r.method, r.json.as_deref()), (Method::Get, None));
            assert_eq!(r.headers, [("Authorization".to_string(), "Bearer test-token".to_string()), ("Accept".to_string(), "application/json".to_string())]);
        }
        let s = out.unwrap();
        assert_eq!((s.provider, s.plan.as_deref(), s.account.as_deref(), s.source, s.as_of), (ProviderId::OpenCode, Some("Go"), None, Source::Online, NOW));
        let w: Vec<_> = s.windows.iter().map(|w| (w.kind, w.label.as_str(), w.used, w.resets_at, w.length_s, w.count)).collect();
        assert_eq!(w, [
            (WindowKind::Session, "5-hour", 0.625, parse_time("2026-10-27T07:20:00Z"), Some(18_000), None),
            (WindowKind::Weekly, "Weekly", 0.3125, parse_time("2026-11-02T00:00:00Z"), Some(604_800), None),
            (WindowKind::Monthly, "Monthly", 0.125, parse_time("2026-11-15T09:30:00Z"), None, None),
        ]);
        assert_eq!(s.balance.as_ref().map(Money::fmt).as_deref(), Some("$12.34"));
        assert_eq!((s.spend.len(), s.tokens_today, s.days.len()), (0, None, 0));
        // The key never lands in what the tab keeps.
        assert!(!format!("{s:?}").contains("test-token"));
    }

    #[test]
    fn local_reads_never_ask() {
        let d = Dirs::sample(Os::Windows);
        assert_eq!(run(&d, &[Got::Missing, auth(AUTH)], Reach::Local, &[]), (vec![], Err(Trouble::NeedsOnline)));
        assert_eq!(run(&d, &[Got::Text("test-typed-token".into()), Got::Missing], Reach::Local, &[]), (vec![], Err(Trouble::NeedsOnline)));
        assert_eq!(run(&with_env_key(d.clone()), &[Got::Missing, Got::Missing], Reach::Local, &[]), (vec![], Err(Trouble::NeedsOnline)));
        assert_eq!(run(&d, &[Got::Missing, auth(AUTH_SIGNED_OUT)], Reach::Local, &[]), (vec![], Err(Trouble::SignedOut)));
        assert_eq!(run(&d, &[Got::Missing, Got::Missing], Reach::Local, &[]), (vec![], Err(Trouble::NotFound)));
    }

    #[test]
    fn failed_asks_read_as_trouble() {
        let busy = Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(90) });
        let (asked, out) = online(&[reply(401, r#"{"error":"unauthorized"}"#)]);
        assert_eq!((asked.len(), out), (1, Err(Trouble::Refused)));
        assert_eq!(online(&[busy]).1, Err(Trouble::Busy { retry_after_s: Some(90) }));
        assert_eq!(online(&[Err(NetFail::Timeout)]).1, Err(Trouble::Offline));
        assert_eq!(online(&[reply(502, "Bad gateway")]).1, Err(Trouble::Offline));
        for body in ["<html>Just a moment...</html>", "[]", r#"{"plan": "go"}"#, r#"{"usage": {"rolling": {"status": "ok"}}}"#] {
            assert!(matches!(online(&[reply(200, body)]).1, Err(Trouble::Unreadable(_))), "{body}");
        }
    }

    #[test]
    fn a_rate_limited_window_reads_full_and_a_missing_one_stays_out() {
        let usage = r#"{"usage": {"rolling": {"status": "ok", "percent": 25, "resetInSec": 600},
            "weekly": {"status": "rate-limited", "percent": 99.5, "resetsAt": 1793577600000}}}"#;
        let s = online(&[reply(200, usage), reply(403, "")]).1.unwrap();
        let w: Vec<_> = s.windows.iter().map(|w| (w.label.as_str(), w.used, w.resets_at)).collect();
        assert_eq!(w, [("5-hour", 0.25, Some(NOW + 600)), ("Weekly", 1.0, Some(1_793_577_600))]);
        assert_eq!(s.balance, None);
    }

    #[test]
    fn no_go_here_shows_the_zen_balance_alone() {
        let (asked, out) = online(&[reply(403, ""), reply(200, BILLING)]);
        let s = out.unwrap();
        assert_eq!((asked.len(), s.windows.len(), s.plan, s.balance), (2, 0, None, Some(Money::usd(12.34))));
        let null = online(&[reply(200, r#"{"usage": null}"#), reply(200, BILLING)]).1.unwrap();
        assert_eq!((null.windows.len(), null.balance), (0, Some(Money::usd(12.34))));
        // With no balance either, a key meant for Go was refused...
        assert_eq!(online(&[reply(403, ""), reply(403, "")]).1, Err(Trouble::Refused));
        // ...while OPENCODE_API_KEY, which Zen shares, only says there's no Go here.
        let d = with_env_key(Dirs::sample(Os::Mac));
        assert_eq!(run(&d, &[Got::Missing, Got::Missing], Reach::Online, &[reply(403, ""), reply(401, "")]).1, Err(Trouble::NotFound));
    }

    #[test]
    fn the_balance_is_optional() {
        let invoiced = r#"{"billingMode": "prepaid", "mode": "invoiceable", "balanceMicroCents": "500000000"}"#;
        let owing = r#"{"billingMode": "prepaid", "mode": "pay-as-you-go", "balanceMicroCents": "-150000000"}"#;
        let cases = [(reply(401, ""), None), (reply(500, ""), None), (Err(NetFail::Timeout), None), (reply(200, invoiced), None), (reply(200, "[]"), None), (reply(200, owing), Some(Money::usd(-1.5)))];
        for (billing, balance) in cases {
            let s = online(&[reply(200, USAGE), billing]).1.unwrap();
            assert_eq!((s.windows.len(), s.balance), (3, balance));
        }
    }
}
