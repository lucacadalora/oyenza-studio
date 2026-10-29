//! Claude Code: Anthropic's Pro, Max 5x and Max 20x plans. The 5-hour and
//! weekly limits come from the endpoint Claude Code's own `/usage` asks.
//!
//! - Sign-in: Claude Code keeps its claude.ai sign-in (`claudeAiOauth`: an
//!   access token, when it expires in milliseconds, the plan) in the Mac
//!   keychain, item "Claude Code-credentials", else in
//!   `~/.claude/.credentials.json`; Windows has only the file. The keychain
//!   comes first, the way Claude Code reads it. `CLAUDE_CONFIG_DIR` moves the
//!   file and, on a Mac, renames the item the way Claude Code 2.x does. We
//!   never refresh it: once it expires, Claude Code renews it when it next runs.
//! - Online: `GET api.anthropic.com/api/oauth/usage` with the token and the
//!   beta flag the endpoint asks for. CodexBar sends a `claude-cli/<version>`
//!   User-Agent there; we ask as Oyenza Studio.
//! - The account, masked, comes from Claude Code's settings (`~/.claude.json`,
//!   `oauthAccount.emailAddress`), which also tell "signed out" from "not on
//!   this computer".
//! - Nothing comes from Claude Code's logs: they carry no running total (each
//!   reply counts its own tokens), so a day's tokens would mean reading whole
//!   files on every refresh. `tokens_today` stays empty, and a [`Reach::Local`]
//!   refresh says [`Trouble::NeedsOnline`] once the sign-in checks out.
//!
//! Ported from CodexBar (MIT): `ClaudeOAuth/`, `ClaudePlan.swift`,
//! `ClaudeScopedWeeklyLimitMapper.swift`, `ClaudeConfigPaths.swift`.

use super::{mask_email, micros, num, reply_json, time, Ctx, Dirs, Got, Money, NetFail, Os, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Spend, Step, Trouble, Want, Window, WindowKind};
use poni_core::sha256::{hex, sha256};
use serde::Deserialize;
use serde_json::Value;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// The usage endpoint answers only with this beta flag.
const BETA: &str = "oauth-2025-04-20";
/// The keychain item Claude Code keeps its sign-in in ([`keychain_service`]).
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const FIVE_HOURS_S: u32 = 5 * 3600;
const WEEK_S: u32 = 7 * 86_400;

// What `wants` asks for, by position. The keychain is last, so on Windows,
// which doesn't ask for it, the list just ends before it (`Ctx::got` answers
// Missing past the end).
const CREDENTIALS: usize = 0;
const OLD_SETTINGS: usize = 1;
const SETTINGS: usize = 2;
const KEYCHAIN: usize = 3;

pub struct Claude;

impl Provider for Claude {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn env(&self) -> &'static [&'static str] {
        &["CLAUDE_CONFIG_DIR"]
    }

    fn wants(&self, dirs: &Dirs) -> Vec<Want> {
        let root = dirs.var_or_home("CLAUDE_CONFIG_DIR", ".claude");
        // Claude Code's settings: `.config.json` in its folder while an older
        // version's copy is there, else `.claude.json` in the home folder (in
        // CLAUDE_CONFIG_DIR when that is set).
        let settings = match dirs.var("CLAUDE_CONFIG_DIR") {
            Some(_) => dirs.join(&root, &[".claude.json"]),
            None => dirs.home_join(&[".claude.json"]),
        };
        let mut wants = vec![Want::file(dirs.join(&root, &[".credentials.json"])), Want::file(dirs.join(&root, &[".config.json"])), Want::file(settings)];
        if dirs.os == Os::Mac {
            wants.push(Want::Keychain { service: keychain_service(dirs), account: None });
        }
        wants
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let settings = match cx.got(OLD_SETTINGS) {
            Got::Missing => cx.got(SETTINGS),
            old => old,
        };
        let keychain = cx.got(KEYCHAIN);
        let Some(s) = sign_in(keychain).or_else(|| sign_in(cx.got(CREDENTIALS))) else {
            // No sign-in we can read: the keychain said no, or Claude Code is
            // here but signed out (its files are), or it isn't here at all.
            let here = !cx.got(CREDENTIALS).is_missing() || !settings.is_missing() || matches!(keychain, Got::Text(_));
            return Step::Done(Err(match keychain {
                Got::Denied => Trouble::Denied,
                _ if here => Trouble::SignedOut,
                _ => Trouble::NotFound,
            }));
        };
        if s.expires_at.is_some_and(|t| t <= cx.now) {
            return Step::Done(Err(Trouble::Expired));
        }
        match (cx.reach, replies) {
            (Reach::Local, _) => Step::Done(Err(Trouble::NeedsOnline)),
            (Reach::Online, []) => Step::Fetch(Request::get(USAGE_URL).bearer(&s.token).header("anthropic-beta", BETA).header("Accept", "application/json")),
            (Reach::Online, [.., r]) => Step::Done(reply_json(r).and_then(|v| read(&v, s.plan, account(settings), cx.now))),
        }
    }

    /// Anthropic turns away callers that ask this endpoint too often, and the
    /// numbers don't move faster than this anyway.
    fn min_gap_s(&self) -> u64 {
        120
    }
}

/// The keychain item Claude Code 2.x keeps its sign-in in: with
/// `CLAUDE_CONFIG_DIR` set, a dash and the first 8 hex digits of that
/// folder's SHA-256 follow the name, so each folder signs in on its own.
/// (Claude Code hashes the folder after Unicode NFC; paths typed in ASCII
/// are the same either way.)
fn keychain_service(dirs: &Dirs) -> String {
    match dirs.var("CLAUDE_CONFIG_DIR") {
        Some(dir) => format!("{KEYCHAIN_SERVICE}-{}", &hex(&sha256(dir.as_bytes()))[..8]),
        None => KEYCHAIN_SERVICE.to_string(),
    }
}

/// The parts of Claude Code's sign-in this reader uses (never the refresh
/// token).
struct SignIn {
    token: String,
    /// Unix seconds (the sign-in has milliseconds); None when it doesn't say.
    expires_at: Option<i64>,
    plan: Option<String>,
}

/// `{"claudeAiOauth": {"accessToken", "expiresAt", "subscriptionType",
/// "rateLimitTier", ...}}`, the same JSON in the keychain and the file. An
/// item with only `mcpOAuth` in it (MCP servers' sign-ins) is nobody signed in.
fn sign_in(got: &Got) -> Option<SignIn> {
    let v = got.json()?;
    let o = v.get("claudeAiOauth")?;
    let token = o.get("accessToken")?.as_str()?.trim();
    let text = |key: &str| o.get(key).and_then(Value::as_str);
    (!token.is_empty()).then(|| SignIn { token: token.to_string(), expires_at: o.get("expiresAt").and_then(time), plan: plan(text("subscriptionType"), text("rateLimitTier")) })
}

/// "Pro", "Max 5x", "Max 20x", "Team", "Enterprise": the subscription type
/// first, else the rate-limit tier ("default_claude_max_20x"); a Max plan
/// takes its multiplier from the tier. None for anything else.
fn plan(subscription: Option<&str>, tier: Option<&str>) -> Option<String> {
    let words = |s: Option<&str>| s.unwrap_or("").split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect::<Vec<_>>();
    let (subscription, tier) = (words(subscription), words(tier));
    let kind = |words: &[String]| ["max", "pro", "team", "enterprise", "ultra"].into_iter().find(|k| words.iter().any(|w| w == k));
    let name = match kind(&subscription).or_else(|| kind(&tier))? {
        "max" => {
            // The word after "max" in the tier, when it is a multiplier ("5x").
            let times = tier.iter().skip_while(|w| w.as_str() != "max").nth(1).filter(|w| w.strip_suffix('x').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())));
            return Some(times.map_or_else(|| "Max".to_string(), |t| format!("Max {t}")));
        }
        "pro" => "Pro",
        "team" => "Team",
        "enterprise" => "Enterprise",
        _ => "Ultra",
    };
    Some(name.to_string())
}

/// The usage answer as a snapshot. An answer with none of the fields read
/// here has changed shape (a field that is there but null still counts: an
/// idle window).
fn read(v: &Value, plan: Option<String>, account: Option<String>, now: i64) -> Outcome {
    const KNOWN: [&str; 6] = ["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet", "limits", "extra_usage"];
    if !KNOWN.iter().any(|k| v.get(k).is_some()) {
        return Err(Trouble::Unreadable("no limits in the answer".into()));
    }
    let mut s = Snapshot::new(ProviderId::Claude, now, Source::Online);
    s.windows = [
        window(v, "five_hour", Some("session"), WindowKind::Session, "5-hour", FIVE_HOURS_S),
        window(v, "seven_day", Some("weekly_all"), WindowKind::Weekly, "Weekly", WEEK_S),
        window(v, "seven_day_opus", None, WindowKind::Weekly, "Weekly · Opus", WEEK_S),
        window(v, "seven_day_sonnet", None, WindowKind::Weekly, "Weekly · Sonnet", WEEK_S),
    ]
    .into_iter()
    .flatten()
    .collect();
    scoped(v, &mut s.windows);
    s.spend = extra_usage(v).into_iter().collect();
    s.balance = cloud_credits(v, now);
    s.plan = plan;
    s.account = account;
    Ok(s)
}

/// `{"utilization": 62.0, "resets_at": "2026-10-27T07:20:00.28+00:00"}` at
/// `key`: percent used and when it empties. When that is absent or null, the
/// `limits` entry of kind `listed` (`{"kind": "session", "percent": 62,
/// "resets_at": ...}`): newer answers have been moving limits there.
fn window(v: &Value, key: &str, listed: Option<&str>, kind: WindowKind, label: &str, length_s: u32) -> Option<Window> {
    let flat = v.get(key).and_then(|w| Some((num(w.get("utilization")?)?, w.get("resets_at"))));
    let in_limits = || {
        let want = listed?;
        let l = v.get("limits")?.as_array()?.iter().find(|l| l.get("kind").and_then(Value::as_str) == Some(want))?;
        Some((num(l.get("percent")?)?, l.get("resets_at")))
    };
    let (used, resets_at) = flat.or_else(in_limits)?;
    Some(Window::new(kind, label, (used / 100.0) as f32).resets(resets_at.and_then(time)).length(Some(length_s)))
}

/// The weekly limits on one model in `limits` ("weekly_scoped", named by
/// `scope.model.display_name`, e.g. "Fable"), after the flat rows; "All
/// models" is the Weekly row itself. `is_active` is no filter: limits that
/// apply have been seen saying false (CodexBar).
fn scoped(v: &Value, windows: &mut Vec<Window>) {
    let all_models = |s: &str| s.to_ascii_lowercase().replace(['_', ' '], "-").ends_with("all-models");
    for l in v.get("limits").and_then(Value::as_array).into_iter().flatten() {
        let text = |path: &str| l.pointer(path).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let (Some("weekly_scoped"), Some("weekly"), Some(name), Some(used)) = (text("/kind"), text("/group"), text("/scope/model/display_name"), l.get("percent").and_then(num)) else {
            continue;
        };
        if all_models(name) || text("/scope/model/id").is_some_and(all_models) {
            continue;
        }
        let label = format!("Weekly · {name}");
        if !windows.iter().any(|w| w.label == label) {
            windows.push(Window::new(WindowKind::Weekly, label, (used / 100.0) as f32).resets(l.get("resets_at").and_then(time)).length(Some(WEEK_S)));
        }
    }
}

/// Extra usage (paid use past the plan's limits), in cents: spent this month
/// and the monthly cap, once the person has switched it on.
fn extra_usage(v: &Value) -> Option<Spend> {
    let e = v.get("extra_usage").filter(|e| e.get("is_enabled").and_then(Value::as_bool) == Some(true))?;
    let currency = e.get("currency").and_then(Value::as_str).map(str::trim).filter(|c| c.len() == 3).unwrap_or("USD").to_ascii_uppercase();
    let cents = |key: &str| e.get(key).and_then(num).map(|c| Money { micros: micros(c / 100.0), currency: currency.clone() });
    Some(Spend { label: "Extra usage this month".into(), money: cents("used_credits")?, cap: cents("monthly_limit").filter(|m| m.micros > 0) })
}

/// Promotional credit for Claude Code on the web (`iguana_necktie`, in
/// dollars, not cents): what's left, unless it has expired or is locked.
fn cloud_credits(v: &Value, now: i64) -> Option<Money> {
    let c = v.get("iguana_necktie")?;
    let limit = c.get("limit_dollars").and_then(num).filter(|l| *l > 0.0)?;
    let left = c.get("remaining_dollars").and_then(num).or_else(|| Some(limit - c.get("used_dollars").and_then(num)?))?;
    let locked = c.get("locked_reason").and_then(Value::as_str).is_some_and(|r| !r.trim().is_empty());
    let expired = c.get("resets_at").and_then(time).is_some_and(|t| t <= now);
    ((0.0..=limit).contains(&left) && !locked && !expired).then(|| Money::usd(left))
}

/// Claude Code's settings file, read for the signed-in account only (it also
/// holds every project's settings, skipped and never kept).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    oauth_account: Option<OAuthAccount>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OAuthAccount {
    email_address: Option<String>,
}

/// Who is signed in, masked.
fn account(settings: &Got) -> Option<String> {
    let f: SettingsFile = serde_json::from_slice(settings.bytes()?).ok()?;
    Some(mask_email(&f.oauth_account?.email_address?)).filter(|a| !a.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileRead, Method, MAX_FETCHES};
    use std::path::PathBuf;

    /// 2026-10-27 05:00 UTC; the fixtures' sign-in expires an hour later.
    const NOW: i64 = 1_793_077_200;
    const CREDENTIALS_JSON: &str = include_str!("fixtures/claude/credentials.json");
    const MCP_ONLY: &str = include_str!("fixtures/claude/keychain-mcp-only.json");
    const SETTINGS_JSON: &str = include_str!("fixtures/claude/claude.json");
    const USAGE: &str = include_str!("fixtures/claude/usage.json");

    fn file(text: &str) -> Got {
        Got::One(FileRead { path: PathBuf::from("fixture"), bytes: text.as_bytes().to_vec(), modified: Some(NOW) })
    }

    fn reply(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    /// What Windows reads: the sign-in file, no older settings, the settings.
    fn windows(credentials: Got) -> Vec<Got> {
        vec![credentials, Got::Missing, file(SETTINGS_JSON)]
    }

    /// What a Mac reads: the same, then the keychain.
    fn mac(credentials: Got, keychain: Got) -> Vec<Got> {
        vec![credentials, Got::Missing, file(SETTINGS_JSON), keychain]
    }

    /// One refresh to the end, answering each request in turn.
    fn run(os: Os, got: &[Got], reach: Reach, answers: &[Result<Reply, NetFail>]) -> (Vec<Request>, Outcome) {
        let dirs = Dirs::sample(os);
        let cx = Ctx { dirs: &dirs, got, now: NOW, reach };
        let (mut asked, mut replies) = (Vec::new(), Vec::new());
        loop {
            match Claude.next(&cx, &replies) {
                Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
                Step::Done(out) => return (asked, out),
                Step::Fetch(r) => {
                    assert!(asked.len() < MAX_FETCHES, "too many requests");
                    asked.push(r);
                    replies.push(answers.get(replies.len()).cloned().expect("an answer for every request"));
                }
            }
        }
    }

    fn online(got: &[Got], answer: Result<Reply, NetFail>) -> Outcome {
        run(Os::Windows, got, Reach::Online, &[answer]).1
    }

    #[test]
    fn windows_reads_the_file_in_its_home() {
        let d = Dirs::sample(Os::Windows);
        let want = vec![Want::file(PathBuf::from(r"C:\Users\u\.claude\.credentials.json")), Want::file(PathBuf::from(r"C:\Users\u\.claude\.config.json")), Want::file(PathBuf::from(r"C:\Users\u\.claude.json"))];
        assert_eq!(Claude.wants(&d), want);
        let (asked, out) = run(Os::Windows, &windows(file(CREDENTIALS_JSON)), Reach::Online, &[reply(200, USAGE)]);
        assert_eq!(asked.len(), 1);
        assert_eq!(out.unwrap().windows.len(), 5);
    }

    #[test]
    fn mac_reads_the_keychain_first_then_the_file() {
        let d = Dirs::sample(Os::Mac);
        let w = Claude.wants(&d);
        assert_eq!(w.len(), 4);
        assert_eq!(w[CREDENTIALS], Want::file(d.home.join(".claude").join(".credentials.json")));
        assert_eq!(w[SETTINGS], Want::file(d.home.join(".claude.json")));
        assert_eq!(w[KEYCHAIN], Want::Keychain { service: "Claude Code-credentials".into(), account: None });
        let max20 = CREDENTIALS_JSON.replace("default_claude_max_5x", "default_claude_max_20x");
        let plan_of = |got: Vec<Got>| run(Os::Mac, &got, Reach::Online, &[reply(200, USAGE)]).1.unwrap().plan;
        // Both there: the keychain's.
        assert_eq!(plan_of(mac(file(CREDENTIALS_JSON), Got::Text(max20.clone()))).as_deref(), Some("Max 20x"));
        assert_eq!(plan_of(mac(Got::Missing, Got::Text(max20))).as_deref(), Some("Max 20x"));
        // No item, only MCP servers' sign-ins in it, or the keychain said no: the file's.
        assert_eq!(plan_of(mac(file(CREDENTIALS_JSON), Got::Missing)).as_deref(), Some("Max 5x"));
        assert_eq!(plan_of(mac(file(CREDENTIALS_JSON), Got::Text(MCP_ONLY.into()))).as_deref(), Some("Max 5x"));
        assert_eq!(plan_of(mac(file(CREDENTIALS_JSON), Got::Denied)).as_deref(), Some("Max 5x"));
    }

    #[test]
    fn claude_config_dir_moves_the_files_and_renames_the_keychain_item() {
        assert_eq!(Claude.env(), &["CLAUDE_CONFIG_DIR"][..]);
        assert_eq!(Claude.min_gap_s(), 120);
        let mut d = Dirs::sample(Os::Mac);
        d.env.push(("CLAUDE_CONFIG_DIR".into(), "/Users/u/claude-work".into()));
        let root = PathBuf::from("/Users/u/claude-work");
        let want = vec![
            Want::file(root.join(".credentials.json")),
            Want::file(root.join(".config.json")),
            Want::file(root.join(".claude.json")),
            // printf '%s' /Users/u/claude-work | sha256sum: 54cfd2c8f800...
            Want::Keychain { service: "Claude Code-credentials-54cfd2c8".into(), account: None },
        ];
        assert_eq!(Claude.wants(&d), want);
        let mut w = Dirs::sample(Os::Windows);
        w.env.push(("CLAUDE_CONFIG_DIR".into(), r"D:\claude".into()));
        assert_eq!(Claude.wants(&w), vec![Want::file(PathBuf::from(r"D:\claude\.credentials.json")), Want::file(PathBuf::from(r"D:\claude\.config.json")), Want::file(PathBuf::from(r"D:\claude\.claude.json"))]);
        // A blank variable is no override.
        let mut blank = Dirs::sample(Os::Mac);
        blank.env.push(("CLAUDE_CONFIG_DIR".into(), " ".into()));
        assert_eq!(Claude.wants(&blank), Claude.wants(&Dirs::sample(Os::Mac)));
    }

    #[test]
    fn not_found_signed_out_and_denied() {
        let none = [Got::Missing, Got::Missing, Got::Missing, Got::Missing];
        assert_eq!(run(Os::Mac, &none, Reach::Online, &[]).1, Err(Trouble::NotFound));
        assert_eq!(run(Os::Windows, &none[..3], Reach::Local, &[]).1, Err(Trouble::NotFound));
        // Claude Code's settings but no sign-in: signed out (or on an API key).
        assert_eq!(run(Os::Windows, &windows(Got::Missing), Reach::Online, &[]).1, Err(Trouble::SignedOut));
        // A sign-in file without a claude.ai sign-in in it.
        let blank = [file(r#"{"claudeAiOauth": {"accessToken": " "}}"#), Got::Missing, Got::Missing];
        assert_eq!(run(Os::Windows, &blank, Reach::Online, &[]).1, Err(Trouble::SignedOut));
        // A keychain item holding only MCP servers' sign-ins.
        let mcp = [Got::Missing, Got::Missing, Got::Missing, Got::Text(MCP_ONLY.into())];
        assert_eq!(run(Os::Mac, &mcp, Reach::Online, &[]).1, Err(Trouble::SignedOut));
        // The keychain said no and there is no file.
        assert_eq!(run(Os::Mac, &mac(Got::Missing, Got::Denied), Reach::Online, &[]).1, Err(Trouble::Denied));
    }

    #[test]
    fn an_expired_sign_in_asks_nobody() {
        let expired = CREDENTIALS_JSON.replace("1793080800000", "1793077199000");
        for reach in [Reach::Online, Reach::Local] {
            let (asked, out) = run(Os::Windows, &windows(file(&expired)), reach, &[]);
            assert!(asked.is_empty());
            assert_eq!(out, Err(Trouble::Expired));
        }
        let (asked, out) = run(Os::Mac, &mac(Got::Missing, Got::Text(expired)), Reach::Online, &[]);
        assert!(asked.is_empty());
        assert_eq!(out, Err(Trouble::Expired));
    }

    #[test]
    fn local_reach_never_asks() {
        for got in [windows(file(CREDENTIALS_JSON)), mac(Got::Missing, Got::Text(CREDENTIALS_JSON.into()))] {
            let (asked, out) = run(Os::Mac, &got, Reach::Local, &[]);
            assert!(asked.is_empty());
            assert_eq!(out, Err(Trouble::NeedsOnline));
        }
    }

    #[test]
    fn a_full_answer_reads_every_window() {
        let (asked, out) = run(Os::Windows, &windows(file(CREDENTIALS_JSON)), Reach::Online, &[reply(200, USAGE)]);
        let r = &asked[0];
        assert_eq!((r.method, r.url.as_str(), r.json.as_deref()), (Method::Get, "https://api.anthropic.com/api/oauth/usage", None));
        let header = |name: &str| r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        assert_eq!(header("Authorization"), Some("Bearer test-token"));
        assert_eq!(header("anthropic-beta"), Some("oauth-2025-04-20"));
        assert_eq!(header("User-Agent"), None, "the app sends its own");
        let s = out.unwrap();
        assert_eq!((s.provider, s.source, s.as_of), (ProviderId::Claude, Source::Online, NOW));
        assert_eq!(s.plan.as_deref(), Some("Max 5x"));
        assert_eq!(s.account.as_deref(), Some("a•••@example.com"));
        // 2026-10-27 07:20 and 2026-10-30 09:00 UTC.
        let (five, week) = (Some(1_793_085_600), Some(1_793_350_800));
        let rows: Vec<_> = s.windows.iter().map(|w| (w.kind, w.label.as_str(), w.used, w.resets_at, w.length_s, w.count)).collect();
        let want = vec![
            (WindowKind::Session, "5-hour", 0.62, five, Some(18_000), None),
            (WindowKind::Weekly, "Weekly", 0.31, week, Some(604_800), None),
            (WindowKind::Weekly, "Weekly · Opus", 0.12, week, Some(604_800), None),
            (WindowKind::Weekly, "Weekly · Sonnet", 0.045, week, Some(604_800), None),
            (WindowKind::Weekly, "Weekly · Fable", 0.05, week, Some(604_800), None),
        ];
        assert_eq!(rows, want);
        assert_eq!(s.tightest().map(|w| w.label.as_str()), Some("5-hour"));
        // Extra usage arrives in cents; cloud credit in dollars.
        let spent = Spend { label: "Extra usage this month".into(), money: Money::usd(3.25), cap: Some(Money::usd(20.0)) };
        assert_eq!(s.spend, vec![spent]);
        assert_eq!(s.balance, Some(Money::usd(75.0)));
        assert_eq!((s.tokens_today, s.days.is_empty()), (None, true));
        // The sign-in goes into the request only, never into the snapshot.
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("test-token") && !json.contains("test-refresh-token"));
        assert!(!format!("{asked:?}").contains("test-refresh-token"));
    }

    #[test]
    fn plans_for_each_tier() {
        let cases = [
            (Some("pro"), Some("default_claude_ai"), Some("Pro")),
            (Some("max"), Some("default_claude_max_5x"), Some("Max 5x")),
            (Some("max"), Some("default_claude_max_20x"), Some("Max 20x")),
            (Some("max"), None, Some("Max")),
            (Some("Max"), Some("default_claude_max"), Some("Max")),
            (None, Some("default_claude_max_20x"), Some("Max 20x")),
            (Some("team"), Some("default_claude_max_5x"), Some("Team")),
            (Some("enterprise"), None, Some("Enterprise")),
            (None, Some("default_claude_ai"), None),
            (Some("free"), None, None),
            (None, None, None),
        ];
        for (subscription, tier, want) in cases {
            assert_eq!(plan(subscription, tier).as_deref(), want, "{subscription:?} {tier:?}");
        }
        // Each plan, read through from the sign-in to the snapshot.
        for (subscription, tier, want) in [("pro", "default_claude_ai", "Pro"), ("max", "default_claude_max_5x", "Max 5x"), ("max", "default_claude_max_20x", "Max 20x")] {
            let creds = CREDENTIALS_JSON.replace("\"max\"", &format!("\"{subscription}\"")).replace("default_claude_max_5x", tier);
            assert_eq!(online(&windows(file(&creds)), reply(200, USAGE)).unwrap().plan.as_deref(), Some(want));
        }
    }

    #[test]
    fn refused_busy_and_offline() {
        let got = windows(file(CREDENTIALS_JSON));
        assert_eq!(online(&got, reply(401, r#"{"type":"error","error":{"type":"authentication_error"}}"#)), Err(Trouble::Refused));
        assert_eq!(online(&got, reply(403, "")), Err(Trouble::Refused));
        let busy = Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(90) });
        assert_eq!(online(&got, busy), Err(Trouble::Busy { retry_after_s: Some(90) }));
        assert_eq!(online(&got, Err(NetFail::Timeout)), Err(Trouble::Offline));
        assert_eq!(online(&got, reply(503, "")), Err(Trouble::Offline));
    }

    #[test]
    fn answers_it_cannot_read() {
        let got = windows(file(CREDENTIALS_JSON));
        for body in ["<html>Bad gateway</html>", "[]", r#"{"type":"error","error":{"type":"not_found_error"}}"#] {
            assert!(matches!(online(&got, reply(200, body)), Err(Trouble::Unreadable(_))), "{body}");
        }
    }

    #[test]
    fn an_idle_account_and_extras_that_are_off() {
        let body = r#"{"five_hour": null, "seven_day": {"utilization": 0.0, "resets_at": null}, "limits": [],
            "extra_usage": {"is_enabled": false, "monthly_limit": null, "used_credits": null},
            "iguana_necktie": {"limit_dollars": 100, "remaining_dollars": 40, "resets_at": "2026-10-01T00:00:00Z"}}"#;
        let s = online(&windows(file(CREDENTIALS_JSON)), reply(200, body)).unwrap();
        let rows: Vec<_> = s.windows.iter().map(|w| (w.label.as_str(), w.used, w.resets_at)).collect();
        assert_eq!(rows, vec![("Weekly", 0.0, None)]);
        assert!(s.spend.is_empty());
        assert_eq!(s.balance, None, "expired credit is no balance");
        // Extra usage on with no cap, and credit known only by what's used.
        let body = r#"{"seven_day": null, "extra_usage": {"is_enabled": true, "monthly_limit": null, "used_credits": 150, "currency": "eur"},
            "iguana_necktie": {"limit_dollars": 50, "used_dollars": 10, "locked_reason": null}}"#;
        let s = online(&windows(file(CREDENTIALS_JSON)), reply(200, body)).unwrap();
        assert_eq!(s.spend[0].money, Money { micros: 1_500_000, currency: "EUR".into() });
        assert_eq!(s.spend[0].cap, None);
        assert_eq!(s.balance, Some(Money::usd(40.0)));
    }

    #[test]
    fn limits_moved_into_the_list_still_read() {
        // The 5-hour and weekly rows only in `limits`, and a model's limit in
        // both places: the flat field wins, without a second row.
        let body = r#"{"five_hour": null, "seven_day": null, "seven_day_sonnet": null,
            "seven_day_opus": {"utilization": 12.0, "resets_at": "2026-10-30T09:00:00Z"},
            "limits": [
              {"kind": "session", "group": "session", "percent": 40, "resets_at": "2026-10-27T07:20:00Z", "scope": null, "is_active": true},
              {"kind": "weekly_all", "group": "weekly", "percent": 20, "resets_at": "2026-10-30T09:00:00Z", "scope": null, "is_active": false},
              {"kind": "weekly_scoped", "group": "weekly", "percent": 7, "resets_at": "2026-10-30T09:00:00Z",
               "scope": {"model": {"id": null, "display_name": "Opus"}, "surface": null}, "is_active": false}]}"#;
        let s = online(&windows(file(CREDENTIALS_JSON)), reply(200, body)).unwrap();
        let rows: Vec<_> = s.windows.iter().map(|w| (w.kind, w.label.as_str(), w.used, w.resets_at)).collect();
        let (five, week) = (Some(1_793_085_600), Some(1_793_350_800));
        assert_eq!(rows, vec![(WindowKind::Session, "5-hour", 0.4, five), (WindowKind::Weekly, "Weekly", 0.2, week), (WindowKind::Weekly, "Weekly · Opus", 0.12, week)]);
    }

    #[test]
    fn the_account_is_masked_and_an_older_settings_file_wins() {
        let plain = SETTINGS_JSON.replace("a•••@example.com", "alex@example.com");
        let who = |got: [Got; 3]| online(&got, reply(200, USAGE)).unwrap().account;
        assert_eq!(who([file(CREDENTIALS_JSON), Got::Missing, file(&plain)]).as_deref(), Some("a•••@example.com"));
        // Claude Code reads an older version's `.config.json` while it's there.
        let old = SETTINGS_JSON.replace("a•••@example.com", "old@example.com");
        assert_eq!(who([file(CREDENTIALS_JSON), file(&old), file(&plain)]).as_deref(), Some("o•••@example.com"));
        // Settings without an account, or not JSON: no account, still a snapshot.
        assert_eq!(who([file(CREDENTIALS_JSON), Got::Missing, file("{}")]), None);
        assert_eq!(who([file(CREDENTIALS_JSON), Got::Missing, file("not json")]), None);
    }
}
