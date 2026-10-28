//! Gemini CLI: Google's Code Assist quota, the share of each model family
//! (Pro, Flash, Flash Lite) a Google sign-in has left today.
//!
//! The sign-in is Gemini CLI's own, `~/.gemini/oauth_creds.json` (the folder
//! moves with `GEMINI_CLI_HOME`), and `settings.json` beside it says which
//! sign-in the CLI uses. Google's Code Assist server is asked what CodexBar's
//! `GeminiStatusProbe` (MIT) asks: `loadCodeAssist` for the plan and the
//! project, then `retrieveUserQuota` for the per-model buckets, which are
//! grouped the way Gemini CLI's own `/stats` groups them.
//!
//! - The token goes only to cloudcode-pa.googleapis.com, under the app's own
//!   User-Agent (CodexBar asks under its own; only Antigravity's flavour of
//!   this server wants its app's). The refresh token is never used: Google's
//!   tokens last an hour, and an expired one is [`Trouble::Expired`] until
//!   Gemini CLI runs again and renews it.
//! - An API key, Vertex AI or a gateway pays per call: no plan quota to read,
//!   [`Trouble::NotFound`].
//! - Since 18 June 2026 Google no longer serves Gemini CLI to personal
//!   accounts (individuals, AI Pro, AI Ultra); such an account, like one
//!   without a licence, reads as [`Trouble::Disabled`]. Standard and
//!   Enterprise plans (on a schedule of their own) and, CodexBar finds,
//!   Workspace accounts are still served.

use super::{jwt_claims, mask_email, num, reply_json, time, Ctx, Dirs, Got, NetFail, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Step, Trouble, Want, Window, WindowKind};
use serde_json::{json, Value};
use std::path::PathBuf;

const LOAD_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";

/// Sign-ins (settings.json) that aren't a Google account's plan: an API key,
/// Vertex AI, a gateway, a cloud machine's own account.
const NO_PLAN: [&str; 6] = ["gemini-api-key", "api-key", "vertex-ai", "gateway", "cloud-shell", "compute-default-credentials"];

/// A token this close to its expiry counts as expired: the two asks take a
/// moment.
const EXPIRY_MARGIN_S: i64 = 60;

/// The families Gemini CLI's `/stats` groups the buckets into, in the order
/// the card lists them.
const FAMILIES: [&str; 3] = ["Pro", "Flash", "Flash Lite"];

pub struct Gemini;

impl Provider for Gemini {
    fn id(&self) -> ProviderId {
        ProviderId::Gemini
    }

    /// `GEMINI_CLI_HOME` stands in for the home folder that holds `.gemini`.
    /// The project variables are what Gemini CLI sends when Google keeps no
    /// project for the account (Standard and Enterprise plans name their own).
    fn env(&self) -> &'static [&'static str] {
        &["GEMINI_CLI_HOME", "GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_PROJECT_ID"]
    }

    fn wants(&self, dirs: &Dirs) -> Vec<Want> {
        let base = dirs.var("GEMINI_CLI_HOME").map_or_else(|| dirs.home.clone(), PathBuf::from);
        vec![Want::file(dirs.join(&base, &[".gemini", "oauth_creds.json"])), Want::file(dirs.join(&base, &[".gemini", "settings.json"]))]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        step(cx, replies).unwrap_or_else(|t| Step::Done(Err(t)))
    }

    /// The buckets are daily; five minutes is plenty.
    fn min_gap_s(&self) -> u64 {
        300
    }
}

fn step(cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Result<Step, Trouble> {
    let me = sign_in(cx)?;
    if cx.reach == Reach::Local {
        // The numbers live only on Google's server.
        return Err(Trouble::NeedsOnline);
    }
    let own_project = cx.dirs.var("GOOGLE_CLOUD_PROJECT").or_else(|| cx.dirs.var("GOOGLE_CLOUD_PROJECT_ID"));
    let Some(load) = replies.first() else {
        return Ok(Step::Fetch(Request::post_json(LOAD_URL, load_body(own_project)).bearer(&me.token)));
    };
    let ca = code_assist(load, &me)?;
    let Some(quota) = replies.get(1) else {
        // Google's project for the account, else the one Gemini CLI was told
        // to use; with neither, an empty ask, as CodexBar makes.
        let body = match project(&ca).or(own_project) {
            Some(p) => json!({ "project": p }),
            None => json!({}),
        };
        return Ok(Step::Fetch(Request::post_json(QUOTA_URL, body.to_string()).bearer(&me.token)));
    };
    if let Some(t) = failed(quota) {
        return Err(t);
    }
    let mut s = Snapshot::new(ProviderId::Gemini, cx.now, Source::Online);
    s.windows = windows(&reply_json(quota)?, cx.now)?;
    s.plan = plan(&ca, me.workspace);
    s.account = me.email.as_deref().map(mask_email);
    Ok(Step::Done(Ok(s)))
}

/// Gemini CLI's sign-in, once it is known to be usable.
struct SignIn {
    token: String,
    email: Option<String>,
    /// A Workspace or school account: Google's ID token names its domain.
    workspace: bool,
}

/// The sign-in to ask with, or why there is none.
fn sign_in(cx: &Ctx) -> Result<SignIn, Trouble> {
    let (creds, settings) = (cx.got(0), cx.got(1));
    if auth_type(settings).is_some_and(|t| NO_PLAN.contains(&t.as_str())) {
        return Err(Trouble::NotFound);
    }
    if creds.is_missing() {
        // Settings mean Gemini CLI has run here; nobody is signed in now.
        return Err(if settings.is_missing() { Trouble::NotFound } else { Trouble::SignedOut });
    }
    let c = creds.json().filter(Value::is_object).ok_or_else(|| Trouble::Unreadable("Gemini CLI's sign-in file".into()))?;
    let Some(token) = c["access_token"].as_str().map(str::trim).filter(|t| !t.is_empty()) else {
        // Only whether a refresh token is there matters (it is never used):
        // with one, Gemini CLI renews the sign-in when it next runs.
        let renewable = c["refresh_token"].as_str().is_some_and(|t| !t.trim().is_empty());
        return Err(if renewable { Trouble::Expired } else { Trouble::SignedOut });
    };
    // expiry_date is in milliseconds.
    if time(&c["expiry_date"]).is_some_and(|at| at <= cx.now + EXPIRY_MARGIN_S) {
        return Err(Trouble::Expired);
    }
    let claims = c["id_token"].as_str().and_then(jwt_claims).unwrap_or_default();
    let workspace = claims["hd"].as_str().is_some_and(|d| !d.trim().is_empty());
    Ok(SignIn { token: token.to_string(), email: claims["email"].as_str().map(str::to_string), workspace })
}

/// Which sign-in Gemini CLI uses: `security.auth.selectedType`, or
/// `selectedAuthType` in older settings. Settings it can't read (comments)
/// leave the sign-in file to decide, as in CodexBar.
fn auth_type(settings: &Got) -> Option<String> {
    let s = settings.json()?;
    let t = s.pointer("/security/auth/selectedType").or_else(|| s.get("selectedAuthType"))?;
    Some(t.as_str()?.trim().to_ascii_lowercase())
}

/// How a failed ask reads. Google's 401 or 403 that names the account's
/// standing, no licence (`SUBSCRIPTION_REQUIRED`) or a client it no longer
/// serves (`UNSUPPORTED_CLIENT`), is [`Trouble::Disabled`]: signing in again
/// wouldn't help.
fn failed(r: &Result<Reply, NetFail>) -> Option<Trouble> {
    match r {
        Ok(a) if matches!(a.status, 401 | 403) && ["SUBSCRIPTION_REQUIRED", "UNSUPPORTED_CLIENT"].iter().any(|code| a.body.contains(code)) => Some(Trouble::Disabled),
        _ => Trouble::from_net(r),
    }
}

/// The first answer (loadCodeAssist): the account's tier and project. An
/// answer we can't read costs only the plan's name; the quota is still asked.
fn code_assist(r: &Result<Reply, NetFail>, me: &SignIn) -> Result<Value, Trouble> {
    let ca = match failed(r) {
        Some(t @ (Trouble::Refused | Trouble::Disabled | Trouble::Busy { .. } | Trouble::Offline)) => return Err(t),
        Some(_) => Value::Null,
        None => reply_json(r).unwrap_or_default(),
    };
    if shut_out(&ca, me.workspace) {
        return Err(Trouble::Disabled);
    }
    Ok(ca)
}

/// Google's answer to a personal account since 18 June 2026: no current
/// tier, and its old one listed as no longer served to this client
/// (`UNSUPPORTED_CLIENT`). A named paid plan or a Workspace domain is still
/// served, so neither is shut out.
fn shut_out(ca: &Value, workspace: bool) -> bool {
    let unsupported = ca["ineligibleTiers"].as_array().is_some_and(|tiers| tiers.iter().any(|t| t["reasonCode"] == "UNSUPPORTED_CLIENT"));
    let paid = ca.pointer("/paidTier/name").and_then(Value::as_str).is_some_and(|n| !n.trim().is_empty());
    unsupported && !paid && !workspace && !ca["currentTier"].is_object()
}

/// `cloudaicompanionProject`: the project's id, or an object holding it.
fn project(ca: &Value) -> Option<&str> {
    let p = &ca["cloudaicompanionProject"];
    p.as_str().or_else(|| p["id"].as_str()).or_else(|| p["projectId"].as_str()).map(str::trim).filter(|p| !p.is_empty())
}

/// What Gemini CLI sends when it sets up (no IDE claimed), with the project
/// it was told to use, if any.
fn load_body(project: Option<&str>) -> String {
    let mut body = json!({ "metadata": { "ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI" } });
    if let Some(p) = project {
        body["cloudaicompanionProject"] = json!(p);
        body["metadata"]["duetProject"] = json!(p);
    }
    body.to_string()
}

/// The plan's short name: Google's paid-plan name when it gives one
/// ("Gemini Code Assist in Google One AI Pro" is "AI Pro"), else the tier's.
fn plan(ca: &Value, workspace: bool) -> Option<String> {
    let name = |at: &str| ca.pointer(at).and_then(Value::as_str).and_then(short_plan);
    let tier = |at: &str| match ca.pointer(at).and_then(Value::as_str)? {
        "free-tier" if workspace => Some("Workspace"),
        "free-tier" => Some("Free"),
        "standard-tier" => Some("Standard"),
        "legacy-tier" => Some("Legacy"),
        _ => None,
    };
    name("/paidTier/name").or_else(|| tier("/paidTier/id").or_else(|| tier("/currentTier/id")).map(str::to_string)).or_else(|| name("/currentTier/name"))
}

/// A plan's name without the product's own words: "Gemini Code Assist
/// Standard" is "Standard", "Google One AI Pro" is "AI Pro", and the bare
/// "Gemini Code Assist" is no plan.
fn short_plan(name: &str) -> Option<String> {
    let mut s = name.trim();
    for lead in ["gemini code assist", "in", "for", "google one", "google"] {
        let rest = match s.get(..lead.len()) {
            Some(head) if head.eq_ignore_ascii_case(lead) => &s[lead.len()..],
            _ => continue,
        };
        if !rest.starts_with(char::is_alphanumeric) {
            s = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ':' || c == '-');
        }
    }
    let mut chars = s.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect())
}

/// One bar per family: its tightest bucket, as Gemini CLI's `/stats` shows
/// it. A bucket without a share left is skipped, as Gemini CLI skips it.
fn windows(q: &Value, now: i64) -> Result<Vec<Window>, Trouble> {
    let buckets: &[Value] = match q.get("buckets") {
        Some(Value::Array(b)) => b,
        // Google's JSON leaves an empty list out: no buckets, no bars.
        None if q.is_object() => &[],
        _ => return Err(Trouble::Unreadable("no quota buckets".into())),
    };
    let mut tightest: [Option<(f64, Option<i64>)>; 3] = [None; 3];
    for b in buckets {
        let (Some(model), Some(left)) = (b["modelId"].as_str(), num(&b["remainingFraction"])) else { continue };
        let Some(f) = family(model) else { continue };
        if tightest[f].is_none_or(|(least, _)| left < least) {
            tightest[f] = Some((left, time(&b["resetTime"])));
        }
    }
    Ok(FAMILIES.iter().zip(tightest).filter_map(|(label, t)| t.map(|(left, at)| window(label, left, at, now))).collect())
}

/// Which family a model is in: "gemini-2.5-flash-lite" is Flash Lite,
/// "gemini-3-pro-preview" Pro. Other models get no bar.
fn family(model: &str) -> Option<usize> {
    let m = model.to_ascii_lowercase();
    let has = |word: &str| m.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == word);
    match (has("flash"), has("lite"), has("pro")) {
        (true, true, _) => Some(2),
        (true, false, _) => Some(1),
        (false, _, true) => Some(0),
        _ => None,
    }
}

/// A family's bar. Code Assist quotas are daily; a reset more than a day
/// (and a bit) off reads as weekly, more than a week off as monthly.
fn window(label: &str, left: f64, resets: Option<i64>, now: i64) -> Window {
    let (kind, length) = match resets.map(|at| at - now) {
        Some(d) if d > 8 * 86_400 => (WindowKind::Monthly, None),
        Some(d) if d > 26 * 3600 => (WindowKind::Weekly, Some(7 * 86_400)),
        _ => (WindowKind::Daily, Some(86_400)),
    };
    Window::new(kind, label, (1.0 - left) as f32).resets(resets).length(length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{drive, FileRead, Method, Os, Outcome};

    const CREDS: &str = include_str!("fixtures/gemini/oauth_creds.json");
    const CREDS_WORKSPACE: &str = include_str!("fixtures/gemini/oauth_creds_workspace.json");
    const SETTINGS: &str = include_str!("fixtures/gemini/settings.json");
    const LOAD: &str = include_str!("fixtures/gemini/load_code_assist.json");
    const SHUT_OUT: &str = include_str!("fixtures/gemini/load_code_assist_shut_out.json");
    const QUOTA: &str = include_str!("fixtures/gemini/retrieve_user_quota.json");
    const NO_LICENCE: &str = include_str!("fixtures/gemini/retrieve_user_quota_403.json");

    /// 2026-10-27T05:00:00Z. The fixture token expires an hour later, and
    /// the buckets reset at 07:00Z (midnight in California).
    const NOW: i64 = 1_793_077_200;

    fn file(body: &str) -> Got {
        Got::One(FileRead { path: PathBuf::from("f.json"), bytes: body.as_bytes().to_vec(), modified: None })
    }

    fn ok(body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status: 200, body: body.into(), retry_after_s: None })
    }

    fn status(code: u16, retry_after_s: Option<u64>) -> Result<Reply, NetFail> {
        Ok(Reply { status: code, body: r#"{"error":{"code":0,"status":"TEST"}}"#.into(), retry_after_s })
    }

    /// One refresh against canned answers, in order: what was asked, and how
    /// it ended.
    fn refresh(dirs: &Dirs, got: &[Got], now: i64, reach: Reach, answers: &[Result<Reply, NetFail>]) -> (Vec<Request>, Outcome) {
        let mut answers = answers.iter();
        drive(&Gemini, &Ctx { dirs, got, now, reach }, |_| answers.next().cloned().expect("an answer for every ask"))
    }

    /// Signed in with the fixture files, online.
    fn online(answers: &[Result<Reply, NetFail>]) -> (Vec<Request>, Outcome) {
        refresh(&Dirs::sample(Os::Windows), &[file(CREDS), file(SETTINGS)], NOW, Reach::Online, answers)
    }

    fn body(r: &Request) -> Value {
        serde_json::from_str(r.json.as_deref().unwrap_or("null")).unwrap()
    }

    fn paths(dirs: &Dirs) -> Vec<PathBuf> {
        Gemini.wants(dirs).into_iter().map(|w| match w {
            Want::File { path, .. } => path,
            w => panic!("only files: {w:?}"),
        }).collect()
    }

    #[test]
    fn reads_the_gemini_folder_on_windows_and_mac() {
        let mac = Dirs::sample(Os::Mac);
        assert_eq!(paths(&mac), [PathBuf::from("/Users/u/.gemini/oauth_creds.json"), PathBuf::from("/Users/u/.gemini/settings.json")]);
        let win = Dirs::sample(Os::Windows);
        assert_eq!(paths(&win), [PathBuf::from(r"C:\Users\u\.gemini\oauth_creds.json"), PathBuf::from(r"C:\Users\u\.gemini\settings.json")]);
        // GEMINI_CLI_HOME stands in for the home folder; .gemini stays.
        let mut moved = Dirs::sample(Os::Windows);
        moved.env.push(("GEMINI_CLI_HOME".into(), r"D:\gem\".into()));
        assert_eq!(paths(&moved), [PathBuf::from(r"D:\gem\.gemini\oauth_creds.json"), PathBuf::from(r"D:\gem\.gemini\settings.json")]);
        let mut moved = Dirs::sample(Os::Mac);
        moved.env.push(("GEMINI_CLI_HOME".into(), "/Volumes/work".into()));
        assert_eq!(paths(&moved)[0], PathBuf::from("/Volumes/work/.gemini/oauth_creds.json"));
        assert!(Gemini.env().contains(&"GEMINI_CLI_HOME"));
        assert_eq!(Gemini.min_gap_s(), 300);
    }

    #[test]
    fn nothing_here_is_not_found_and_settings_alone_signed_out() {
        let d = Dirs::sample(Os::Mac);
        assert_eq!(refresh(&d, &[Got::Missing, Got::Missing], NOW, Reach::Online, &[]), (vec![], Err(Trouble::NotFound)));
        assert_eq!(refresh(&d, &[], NOW, Reach::Online, &[]), (vec![], Err(Trouble::NotFound)));
        assert_eq!(refresh(&d, &[Got::Missing, file(SETTINGS)], NOW, Reach::Online, &[]), (vec![], Err(Trouble::SignedOut)));
        assert!(matches!(refresh(&d, &[file("not json"), file(SETTINGS)], NOW, Reach::Online, &[]).1, Err(Trouble::Unreadable(_))));
    }

    #[test]
    fn api_key_and_vertex_sign_ins_have_no_plan_quota() {
        let d = Dirs::sample(Os::Windows);
        let settings = [
            r#"{"security":{"auth":{"selectedType":"gemini-api-key"}}}"#,
            r#"{"security":{"auth":{"selectedType":"vertex-ai"}},"ui":{"theme":"GitHub"}}"#,
            r#"{"selectedAuthType":"gemini-api-key","theme":"GitHub"}"#,
        ];
        for s in settings {
            // Even with an old Google sign-in still on disk.
            assert_eq!(refresh(&d, &[file(CREDS), file(s)], NOW, Reach::Online, &[]), (vec![], Err(Trouble::NotFound)), "{s}");
            assert_eq!(refresh(&d, &[Got::Missing, file(s)], NOW, Reach::Online, &[]).1, Err(Trouble::NotFound), "{s}");
        }
        // Settings it can't read leave the sign-in file to decide.
        let (asked, out) = refresh(&d, &[file(CREDS), file("{ // a comment\n}")], NOW, Reach::Online, &[ok(LOAD), ok(QUOTA)]);
        assert_eq!(asked.len(), 2);
        assert!(out.is_ok());
    }

    #[test]
    fn an_expired_sign_in_waits_for_gemini_cli() {
        let d = Dirs::sample(Os::Mac);
        let got = [file(CREDS), file(SETTINGS)];
        // Past the token's hour, Local or Online, nothing is asked.
        for reach in [Reach::Local, Reach::Online] {
            assert_eq!(refresh(&d, &got, NOW + 3700, reach, &[]), (vec![], Err(Trouble::Expired)));
        }
        // Half a minute before it ends already counts.
        assert_eq!(refresh(&d, &got, NOW + 3570, Reach::Online, &[]), (vec![], Err(Trouble::Expired)));
        // No access token: a refresh token there means Gemini CLI renews it
        // when it next runs; none means nobody is signed in.
        let renewable = file(r#"{"refresh_token":"test-refresh-token","expiry_date":1793080800000}"#);
        assert_eq!(refresh(&d, &[renewable, file(SETTINGS)], NOW, Reach::Online, &[]), (vec![], Err(Trouble::Expired)));
        let blank = file(r#"{"access_token":"","expiry_date":1793080800000}"#);
        assert_eq!(refresh(&d, &[blank, file(SETTINGS)], NOW, Reach::Online, &[]), (vec![], Err(Trouble::SignedOut)));
    }

    #[test]
    fn asks_code_assist_then_the_quota() {
        let (asked, out) = online(&[ok(LOAD), ok(QUOTA)]);
        assert_eq!(asked.len(), 2);
        let (load, quota) = (&asked[0], &asked[1]);
        assert_eq!((load.method, load.url.as_str()), (Method::Post, "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist"));
        assert_eq!(body(load), json!({ "metadata": { "ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI" } }));
        assert_eq!((quota.method, quota.url.as_str()), (Method::Post, "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota"));
        assert_eq!(body(quota), json!({ "project": "test-project-123456" }));
        for r in &asked {
            assert!(r.headers.contains(&("Authorization".into(), "Bearer test-token".into())));
            // The app adds its own User-Agent, and the refresh token never leaves.
            assert!(!r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("user-agent")));
            let sent = format!("{} {:?} {}", r.url, r.headers, r.json.as_deref().unwrap_or(""));
            assert!(!sent.contains("test-refresh-token") && !sent.contains("a@example.com"), "{sent}");
        }
        let s = out.unwrap();
        assert_eq!((s.provider, s.source, s.as_of), (ProviderId::Gemini, Source::Online, NOW));
        assert_eq!(s.plan.as_deref(), Some("Standard"));
        assert_eq!(s.account.as_deref(), Some("a•••@example.com"));
        let reset = Some(NOW + 2 * 3600);
        let bars: Vec<_> = s.windows.iter().map(|w| (w.label.as_str(), w.kind, (w.used * 100.0).round() as i32, w.resets_at, w.length_s)).collect();
        // Pro is its tightest model (2.5 Pro at 38% left), not 3 Pro's 60%.
        assert_eq!(bars, [
            ("Pro", WindowKind::Daily, 62, reset, Some(86_400)),
            ("Flash", WindowKind::Daily, 10, reset, Some(86_400)),
            ("Flash Lite", WindowKind::Daily, 0, reset, Some(86_400)),
        ]);
        assert!(s.windows.iter().all(|w| w.count.is_none()));
        assert_eq!(s.tightest().map(|w| w.label.as_str()), Some("Pro"));
        // Nothing of the sign-in ends up in what the tab keeps.
        let kept = serde_json::to_string(&s).unwrap();
        assert!(!kept.contains("test-token") && !kept.contains("a@example.com") && !kept.contains("test-project"));
    }

    #[test]
    fn a_project_less_account_asks_without_one() {
        let bare = r#"{"currentTier":{"id":"standard-tier","name":"Gemini Code Assist"}}"#;
        let (asked, out) = online(&[ok(bare), ok(QUOTA)]);
        assert_eq!(body(&asked[1]), json!({}));
        let s = out.unwrap();
        assert_eq!((s.plan.as_deref(), s.windows.len()), (Some("Standard"), 3));
        // The project Gemini CLI was told to use goes in both asks, as
        // Gemini CLI sends it.
        let mut d = Dirs::sample(Os::Windows);
        d.env.push(("GOOGLE_CLOUD_PROJECT".into(), "test-own-project".into()));
        let got = [file(CREDS), file(SETTINGS)];
        let (asked, _) = refresh(&d, &got, NOW, Reach::Online, &[ok(bare), ok(QUOTA)]);
        assert_eq!(body(&asked[0])["cloudaicompanionProject"], "test-own-project");
        assert_eq!(body(&asked[0])["metadata"]["duetProject"], "test-own-project");
        assert_eq!(body(&asked[1]), json!({ "project": "test-own-project" }));
        // Google's own project for the account comes first, and an object
        // holding the id reads the same.
        let (asked, _) = refresh(&d, &got, NOW, Reach::Online, &[ok(LOAD), ok(QUOTA)]);
        assert_eq!(body(&asked[1]), json!({ "project": "test-project-123456" }));
        let (asked, _) = refresh(&d, &got, NOW, Reach::Online, &[ok(r#"{"cloudaicompanionProject":{"id":"test-project-9"}}"#), ok(QUOTA)]);
        assert_eq!(body(&asked[1]), json!({ "project": "test-project-9" }));
    }

    #[test]
    fn local_refreshes_never_ask() {
        let d = Dirs::sample(Os::Mac);
        assert_eq!(refresh(&d, &[file(CREDS), file(SETTINGS)], NOW, Reach::Local, &[]), (vec![], Err(Trouble::NeedsOnline)));
        // What the files alone tell still shows.
        assert_eq!(refresh(&d, &[Got::Missing, file(SETTINGS)], NOW, Reach::Local, &[]), (vec![], Err(Trouble::SignedOut)));
    }

    #[test]
    fn a_refused_sign_in_reads_as_refused() {
        let (asked, out) = online(&[status(401, None)]);
        assert_eq!((asked.len(), out), (1, Err(Trouble::Refused)));
        let (asked, out) = online(&[ok(LOAD), status(401, None)]);
        assert_eq!((asked.len(), out), (2, Err(Trouble::Refused)));
        assert_eq!(online(&[ok(LOAD), status(403, None)]).1, Err(Trouble::Refused));
        // Google's 403 for an account without a licence: signing in again
        // wouldn't help.
        let no_licence = Ok(Reply { status: 403, body: NO_LICENCE.into(), retry_after_s: None });
        assert_eq!(online(&[ok(LOAD), no_licence]), (online(&[ok(LOAD), ok(QUOTA)]).0, Err(Trouble::Disabled)));
    }

    #[test]
    fn a_personal_account_google_no_longer_serves_is_disabled_at_once() {
        let (asked, out) = online(&[ok(SHUT_OUT)]);
        assert_eq!((asked.len(), out), (1, Err(Trouble::Disabled)));
        // The same word in an error answer reads the same.
        let error = r#"{"error":{"code":403,"message":"UNSUPPORTED_CLIENT: This client is no longer supported for Gemini Code Assist for individuals.","status":"PERMISSION_DENIED"}}"#;
        let (asked, out) = online(&[Ok(Reply { status: 403, body: error.into(), retry_after_s: None })]);
        assert_eq!((asked.len(), out), (1, Err(Trouble::Disabled)));
        // A Workspace account is still served: asked on, plan from its tier.
        let d = Dirs::sample(Os::Mac);
        let (asked, out) = refresh(&d, &[file(CREDS_WORKSPACE), file(SETTINGS)], NOW, Reach::Online, &[ok(SHUT_OUT), ok(QUOTA)]);
        assert_eq!(asked.len(), 2);
        assert_eq!(body(&asked[1]), json!({}));
        assert_eq!(out.unwrap().windows.len(), 3);
        let free = r#"{"currentTier":{"id":"free-tier"},"cloudaicompanionProject":"test-project-1"}"#;
        let s = refresh(&d, &[file(CREDS_WORKSPACE), file(SETTINGS)], NOW, Reach::Online, &[ok(free), ok(QUOTA)]).1.unwrap();
        assert_eq!(s.plan.as_deref(), Some("Workspace"));
    }

    #[test]
    fn too_many_asks_wait_as_google_says() {
        let (asked, out) = online(&[status(429, Some(30))]);
        assert_eq!((asked.len(), out), (1, Err(Trouble::Busy { retry_after_s: Some(30) })));
        assert_eq!(online(&[ok(LOAD), status(429, Some(120))]).1, Err(Trouble::Busy { retry_after_s: Some(120) }));
        assert_eq!(online(&[ok(LOAD), status(429, None)]).1, Err(Trouble::Busy { retry_after_s: None }));
    }

    #[test]
    fn an_unreadable_answer_says_so() {
        assert!(matches!(online(&[ok(LOAD), ok("<html>Sign in</html>")]).1, Err(Trouble::Unreadable(_))));
        assert!(matches!(online(&[ok(LOAD), ok(r#"{"buckets":{"gemini-2.5-pro":0.5}}"#)]).1, Err(Trouble::Unreadable(_))));
        assert!(matches!(online(&[ok(LOAD), ok("[]")]).1, Err(Trouble::Unreadable(_))));
        // An empty answer is no buckets: the plan, and no bars.
        let s = online(&[ok(LOAD), ok("{}")]).1.unwrap();
        assert_eq!((s.plan.as_deref(), s.windows.len()), (Some("Standard"), 0));
        // A first answer it can't read costs only the plan.
        let (asked, out) = online(&[ok("<html>"), ok(QUOTA)]);
        assert_eq!((asked.len(), out.as_ref().map(|s| s.plan.clone())), (2, Ok(None)));
        assert_eq!(body(&asked[1]), json!({}));
        let (asked, out) = online(&[status(404, None), ok(QUOTA)]);
        assert_eq!((asked.len(), out.map(|s| s.windows.len())), (2, Ok(3)));
        // No answer at all stops at once.
        assert_eq!(online(&[Err(NetFail::Timeout)]).1, Err(Trouble::Offline));
        assert_eq!(online(&[status(503, None)]).0.len(), 1);
        assert_eq!(online(&[ok(LOAD), Err(NetFail::Network("dns".into()))]).1, Err(Trouble::Offline));
    }

    #[test]
    fn buckets_group_by_family() {
        let q = json!({ "buckets": [
            { "modelId": "gemini-2.5-flash", "remainingFraction": 0.9, "resetTime": "2026-10-27T07:00:00Z" },
            // The tightest model wins, with its own reset.
            { "modelId": "gemini-2.0-flash", "remainingFraction": 0.4, "resetTime": "2026-10-27T06:00:00Z" },
            // No share left in the answer, no model, no family: skipped.
            { "modelId": "gemini-2.5-flash", "resetTime": "2026-10-27T07:00:00Z" },
            { "remainingFraction": 0.0 },
            { "modelId": "gemini-embedding-001", "remainingFraction": 0.0 },
            // Three days off reads as weekly; numbers may come as strings.
            { "modelId": "gemini-2.5-pro", "remainingFraction": "0.25", "resetTime": "2026-10-30T07:00:00Z" },
        ] });
        let w = windows(&q, NOW).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].label.as_str(), w[0].kind, w[0].used, w[0].length_s), ("Pro", WindowKind::Weekly, 0.75, Some(604_800)));
        assert_eq!((w[1].label.as_str(), w[1].kind, w[1].resets_at), ("Flash", WindowKind::Daily, Some(NOW + 3600)));
        assert!((w[1].used - 0.6).abs() < 1e-6);
        let far = window("Pro", 0.5, Some(NOW + 20 * 86_400), NOW);
        assert_eq!((far.kind, far.length_s), (WindowKind::Monthly, None));
        let unknown = window("Pro", 0.5, None, NOW);
        assert_eq!((unknown.kind, unknown.resets_at), (WindowKind::Daily, None));
        assert_eq!(family("gemini-2.5-flash-lite-preview-06-17"), Some(2));
        assert_eq!(family("models/Gemini-3-Pro-Preview"), Some(0));
        assert_eq!(family("gemini-exp-1206"), None);
        assert_eq!(family("gemini-2.5-flash-image"), Some(1));
    }

    #[test]
    fn plans_read_plainly() {
        let p = |ca: Value, workspace: bool| plan(&ca, workspace);
        assert_eq!(p(json!({ "currentTier": { "id": "free-tier" } }), false).as_deref(), Some("Free"));
        assert_eq!(p(json!({ "currentTier": { "id": "free-tier" } }), true).as_deref(), Some("Workspace"));
        assert_eq!(p(json!({ "currentTier": { "id": "legacy-tier" } }), false).as_deref(), Some("Legacy"));
        let g1 = json!({ "currentTier": { "id": "standard-tier" }, "paidTier": { "id": "g1-pro-tier", "name": "Gemini Code Assist in Google One AI Pro" } });
        assert_eq!(p(g1, true).as_deref(), Some("AI Pro"));
        assert_eq!(p(json!({ "paidTier": { "name": "Plus" } }), false).as_deref(), Some("Plus"));
        assert_eq!(p(json!({ "paidTier": { "id": "g1-pro-tier" }, "currentTier": { "id": "standard-tier" } }), false).as_deref(), Some("Standard"));
        assert_eq!(p(json!({ "currentTier": { "id": "new-tier", "name": "Gemini Code Assist Enterprise" } }), false).as_deref(), Some("Enterprise"));
        assert_eq!(p(json!({ "currentTier": { "id": "new-tier", "name": "Gemini Code Assist" } }), false), None);
        assert_eq!(p(Value::Null, false), None);
        assert_eq!(short_plan("Gemini Code Assist for individuals").as_deref(), Some("Individuals"));
        assert_eq!(short_plan("Gemini Code Assistant").as_deref(), Some("Gemini Code Assistant"));
        assert_eq!(short_plan("Google AI Ultra").as_deref(), Some("AI Ultra"));
    }
}
