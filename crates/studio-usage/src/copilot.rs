//! GitHub Copilot: the plan and its monthly allowances (premium requests;
//! on Copilot Free also chat and code completions), from the answer VS
//! Code's own Copilot status reads, `api.github.com/copilot_internal/user`.
//!
//! The sign-in is Copilot's own: the GitHub token the editors built on
//! GitHub's Copilot language server (JetBrains, Vim and Neovim, Xcode,
//! Eclipse) keep in `apps.json`, or the older `hosts.json`, under
//! `%USERPROFILE%\AppData\Local\github-copilot` on Windows and
//! `~/.config/github-copilot` on the Mac (`$XDG_CONFIG_HOME/github-copilot`
//! when that is set). Never the gh CLI's token or VS Code's: only what the
//! person gave Copilot. It goes only to GitHub's API: `api.github.com`, or
//! `api.<name>.ghe.com` for a GitHub Enterprise sign-in on its own domain.
//!
//! Since 1.500 (June 2026) the language server keeps sign-ins in `auth.db`
//! and leaves `apps.json` as it was, so a sign-in made before the editor
//! updated reads here and a newer one doesn't.

use super::{join, mask_email, num, parse_time, reply_json, time, Ctx, Dirs, NetFail, Os, Outcome, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Step, Trouble, Want, Window, WindowKind};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Copilot;

/// The answer's allowances, in the order the card lists them.
const QUOTAS: [(&str, &str); 3] = [("premium_interactions", "Premium requests"), ("chat", "Chat"), ("completions", "Code completions")];

impl Provider for Copilot {
    fn id(&self) -> ProviderId {
        ProviderId::Copilot
    }

    fn env(&self) -> &'static [&'static str] {
        &["XDG_CONFIG_HOME"]
    }

    fn wants(&self, dirs: &Dirs) -> Vec<Want> {
        let dir = config_dir(dirs);
        vec![
            Want::file(dir.join("apps.json")),
            Want::file(dir.join("hosts.json")),
            // Only whether the newer store is there (no byte of it is read):
            // then apps.json no longer follows the person's sign-ins.
            Want::Newest { dir, suffix: "auth.db", depth: 0, count: 1, tail: 0 },
        ]
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let s = match sign_in(cx) {
            Ok(s) => s,
            Err(t) => return Step::Done(Err(t)),
        };
        if cx.reach == Reach::Local {
            return Step::Done(Err(Trouble::NeedsOnline));
        }
        match replies.last() {
            // The token and nothing else: VS Code asks the same way, so no
            // editor's name is needed (the app adds Oyenza Studio's own
            // User-Agent, which GitHub requires).
            None => Step::Fetch(Request::get(format!("https://{}/copilot_internal/user", s.api)).header("Authorization", format!("token {}", s.token)).header("Accept", "application/json")),
            Some(r) => Step::Done(answer(r, cx, s.user.as_deref())),
        }
    }

    /// Monthly allowances that move a request at a time: five minutes is
    /// fresh enough.
    fn min_gap_s(&self) -> u64 {
        300
    }
}

/// Where Copilot keeps its files, by the language server's own rule:
/// `$XDG_CONFIG_HOME` when it is a full path, else the per-OS default.
fn config_dir(dirs: &Dirs) -> PathBuf {
    if let Some(x) = dirs.var("XDG_CONFIG_HOME").filter(|x| absolute(x)) {
        return join(Path::new(x), &["github-copilot"]);
    }
    match dirs.os {
        Os::Windows => join(&dirs.home, &["AppData", "Local", "github-copilot"]),
        Os::Mac => join(&dirs.home, &[".config", "github-copilot"]),
    }
}

/// `/x`, `\x` or `C:\x`, checked by hand so the Windows rule holds in tests
/// on any OS.
fn absolute(p: &str) -> bool {
    let b = p.as_bytes();
    matches!(b.first(), Some(b'/' | b'\\')) || (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'))
}

/// A sign-in read from Copilot's files. No `Debug`, so the token can't
/// slip into a log line.
struct SignIn {
    /// GitHub's API host for the sign-in's server.
    api: String,
    token: String,
    user: Option<String>,
}

/// The sign-in to use: `apps.json` (keys "github.com:Iv1.…", one per
/// server and Copilot app) before the older `hosts.json` (keys
/// "github.com"), github.com before an enterprise domain, then by key, so
/// the pick never depends on the file's order.
fn sign_in(cx: &Ctx) -> Result<SignIn, Trouble> {
    let (mut read, mut elsewhere) = (false, false);
    for got in [cx.got(0), cx.got(1)] {
        let Some(bytes) = got.bytes() else { continue };
        read = true;
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let Ok(Value::Object(entries)) = serde_json::from_slice::<Value>(bytes) else { continue };
        let mut best: Option<((bool, &str), SignIn)> = None;
        for (key, e) in &entries {
            let token = e["oauth_token"].as_str().or_else(|| e["access_token"].as_str()).unwrap_or_default().trim();
            if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
                continue;
            }
            let host = key.rsplit_once(':').map_or(key.as_str(), |(host, _app)| host);
            let Some(api) = api_host(host) else {
                elsewhere = true;
                continue;
            };
            let rank = (api != "api.github.com", key.as_str());
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                let user = e["user"].as_str().or_else(|| e["login"].as_str()).map(str::to_string);
                best = Some((rank, SignIn { api, token: token.to_string(), user }));
            }
        }
        if let Some((_, s)) = best {
            return Ok(s);
        }
    }
    // Copilot deletes apps.json with the last sign-in; one that is still
    // there but holds none means the same. A sign-in only to some other
    // server (a company's own GitHub Enterprise Server) isn't asked.
    Err(if read && !elsewhere { Trouble::SignedOut } else { Trouble::NotFound })
}

/// GitHub's API host for a sign-in's server: github.com's, or that of a
/// GitHub Enterprise domain of its own (`<name>.ghe.com`). None for any
/// other server: its token goes nowhere.
fn api_host(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host == "github.com" {
        return Some("api.github.com".into());
    }
    let name = host.strip_suffix(".ghe.com")?;
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')).then(|| format!("api.{host}"))
}

fn answer(r: &Result<Reply, NetFail>, cx: &Ctx, user: Option<&str>) -> Outcome {
    if let Ok(reply) = r {
        // A dead token in an apps.json the newer store left behind: signing
        // in again (what Refused asks for) would never reach this file.
        if reply.status == 401 && !cx.got(2).files().is_empty() {
            return Err(Trouble::NotFound);
        }
        // GitHub answers a rate limit with a 403 too.
        if reply.status == 403 && (reply.retry_after_s.is_some() || reply.body.contains("rate limit")) {
            return Err(Trouble::Busy { retry_after_s: reply.retry_after_s });
        }
    }
    read_user(&reply_json(r)?, cx.now, user)
}

/// `copilot_internal/user`: the plan, then one bar per allowance that has
/// a limit. Unlimited ones (chat and completions on paid plans, everything
/// on token-billed seats) get none, so such a card shows its plan alone.
fn read_user(v: &Value, now: i64, user: Option<&str>) -> Outcome {
    let reset = ["quota_reset_date_utc", "quota_reset_date", "limited_user_reset_date"].into_iter().find_map(|k| day_or_time(&v[k]));
    let (snaps, monthly, left) = (&v["quota_snapshots"], &v["monthly_quotas"], &v["limited_user_quotas"]);
    let mut unlimited = v["token_based_billing"] == true;
    let mut windows = Vec::new();
    for (key, label) in QUOTAS {
        let q = &snaps[key];
        unlimited |= q["unlimited"] == true;
        let w = if q.is_object() && q["unlimited"] != true {
            // A quota's own reset (unix seconds) wins over the account's.
            let resets = time(&q["quota_reset_at"]).filter(|t| *t > 0).or(reset);
            allowance(label, num(&q["entitlement"]), num(&q["quota_remaining"]).or_else(|| num(&q["remaining"])), num(&q["percent_remaining"]), num(&q["overage_count"]).unwrap_or(0.0), resets)
        } else {
            None
        };
        // Copilot Free's older answer: the month's allowance and what's left.
        windows.extend(w.or_else(|| allowance(label, num(&monthly[key]), num(&left[key]), None, 0.0, reset)));
    }
    let plan = plan(v);
    if windows.is_empty() && plan.is_none() && !unlimited {
        // Signed in without a plan (never signed up, or it ended): nothing
        // to show. Without the plan fields it's an answer we don't know.
        let copilot = v.get("copilot_plan").is_some() || v.get("access_type_sku").is_some();
        return Err(if copilot { Trouble::NotFound } else { Trouble::Unreadable("no plan or limits in the answer".into()) });
    }
    let mut s = Snapshot::new(ProviderId::Copilot, now, Source::Online);
    s.plan = plan.map(String::from);
    s.account = user.map(mask_email).filter(|a| !a.is_empty());
    s.windows = windows;
    Ok(s)
}

/// One allowance as a bar: `of` granted this month and `left` of it, or the
/// percent left. Paid requests past the allowance (`extra`) take it past
/// 100%.
fn allowance(label: &str, of: Option<f64>, left: Option<f64>, pct_left: Option<f64>, extra: f64, resets: Option<i64>) -> Option<Window> {
    if of.is_some_and(|of| of <= 0.0) {
        return None; // none granted, like Copilot Free's premium requests
    }
    let mut used = match (pct_left, of, left) {
        (Some(p), _, _) => 1.0 - p / 100.0,
        (None, Some(of), Some(l)) => (of - l) / of,
        _ => return None,
    };
    let mut count = None;
    if let Some(of) = of {
        let mut n = left.map_or(of * used, |l| of - l).max(0.0);
        if extra > 0.0 && n >= of {
            n = n.max(of + extra);
            used = used.max(n / of);
        }
        count = Some((n.round() as u64, of.round() as u64));
    }
    let mut w = Window::new(WindowKind::Monthly, label, used as f32).resets(resets);
    w.count = count;
    Some(w)
}

/// The plan as GitHub sells it, read the way VS Code reads it: the SKU
/// first (Free, Student), then `copilot_plan`.
fn plan(v: &Value) -> Option<&'static str> {
    let sku = v["access_type_sku"].as_str().unwrap_or_default();
    match sku {
        "free_limited_copilot" => return Some("Free"),
        "free_educational_quota" => return Some("Student"),
        "subscription_ended" => return None,
        _ if v["can_signup_for_limited"] == true => return None,
        _ => {}
    }
    Some(match v["copilot_plan"].as_str()?.trim().to_ascii_lowercase().as_str() {
        "free" => "Free",
        "individual" | "pro" => "Pro",
        "individual_pro" | "pro+" | "pro_plus" => "Pro+",
        "individual_max" | "max" => "Max",
        "individual_edu" | "edu" | "student" => "Student",
        "business" => "Business",
        "enterprise" => "Enterprise",
        _ => return None,
    })
}

/// "2026-11-01" (that day, from midnight UTC, when the month's allowance
/// starts over) or a full time.
fn day_or_time(v: &Value) -> Option<i64> {
    match v.as_str().map(str::trim) {
        Some(s) if s.len() == 10 => parse_time(&format!("{s}T00:00:00Z")),
        _ => time(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileRead, Got, Method};

    const APPS: &str = include_str!("fixtures/copilot/apps.json");
    const HOSTS: &str = include_str!("fixtures/copilot/hosts.json");
    const PRO: &str = include_str!("fixtures/copilot/user-pro.json");
    const FREE: &str = include_str!("fixtures/copilot/user-free.json");
    const FREE_OLD: &str = include_str!("fixtures/copilot/user-free-legacy.json");
    const UNLIMITED: &str = include_str!("fixtures/copilot/user-unlimited.json");
    /// 2026-10-27T05:00:00Z.
    const NOW: i64 = 1_793_077_200;

    fn file(text: &str) -> Got {
        Got::One(FileRead { path: "apps.json".into(), bytes: text.as_bytes().to_vec(), modified: Some(NOW) })
    }

    /// One step: what was read (apps.json, hosts.json, auth.db) and the
    /// answers so far.
    fn step(got: &[Got], reach: Reach, replies: &[Result<Reply, NetFail>]) -> Step {
        let dirs = Dirs::sample(Os::Windows);
        Copilot.next(&Ctx { dirs: &dirs, got, now: NOW, reach }, replies)
    }

    fn done(s: Step) -> Outcome {
        match s {
            Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
            Step::Done(o) => o,
            Step::Fetch(r) => panic!("asked {}", r.url),
        }
    }

    fn reply(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    /// The reading for one answer to the usual sign-in.
    fn read(body: &str) -> Outcome {
        done(step(&[file(APPS), Got::Missing, Got::Many(Vec::new())], Reach::Online, &[reply(200, body)]))
    }

    fn slash(p: &Path) -> String {
        p.to_string_lossy().replace('\\', "/")
    }

    fn paths(dirs: &Dirs) -> Vec<String> {
        Copilot.wants(dirs).iter().map(|w| match w {
            Want::File { path, .. } => slash(path),
            Want::Newest { dir, suffix, depth: 0, count: 1, tail: 0 } => format!("{}/{suffix}", slash(dir)),
            w => panic!("{w:?}"),
        }).collect()
    }

    #[test]
    fn reads_copilots_own_files_on_windows_and_mac() {
        let at = |dir: &str| vec![format!("{dir}/apps.json"), format!("{dir}/hosts.json"), format!("{dir}/auth.db")];
        assert_eq!(paths(&Dirs::sample(Os::Windows)), at("C:/Users/u/AppData/Local/github-copilot"));
        assert_eq!(paths(&Dirs::sample(Os::Mac)), at("/Users/u/.config/github-copilot"));
        let with = |os, xdg: &str| Dirs { env: vec![("XDG_CONFIG_HOME".into(), xdg.into())], ..Dirs::sample(os) };
        assert_eq!(paths(&with(Os::Mac, "/Users/u/xdg")), at("/Users/u/xdg/github-copilot"));
        assert_eq!(paths(&with(Os::Windows, r"D:\cfg")), at("D:/cfg/github-copilot"));
        // A relative one is ignored, as Copilot ignores it.
        assert_eq!(paths(&with(Os::Windows, "cfg")), at("C:/Users/u/AppData/Local/github-copilot"));
        assert_eq!(Copilot.env().to_vec(), vec!["XDG_CONFIG_HOME"]);
        assert_eq!((Copilot.id(), Copilot.min_gap_s()), (ProviderId::Copilot, 300));
    }

    #[test]
    fn nothing_there_or_nobody_signed_in() {
        let none = [Got::Missing, Got::Missing, Got::Missing];
        assert_eq!(done(step(&none, Reach::Online, &[])), Err(Trouble::NotFound));
        for text in ["{}", r#"{"github.com:Iv1.b507a08c87ecfe98":{"user":"test-user","oauth_token":""}}"#, "not json"] {
            assert_eq!(done(step(&[file(text), Got::Missing, Got::Missing], Reach::Online, &[])), Err(Trouble::SignedOut), "{text}");
        }
        // Signed in only to a company's own GitHub server: nothing to ask.
        let elsewhere = r#"{"github.example.com:Iv1.b507a08c87ecfe98":{"user":"test-user","oauth_token":"test-token"}}"#;
        assert_eq!(done(step(&[file(elsewhere), Got::Missing, Got::Missing], Reach::Online, &[])), Err(Trouble::NotFound));
    }

    #[test]
    fn a_local_refresh_never_asks_github() {
        assert_eq!(done(step(&[file(APPS), Got::Missing, Got::Missing], Reach::Local, &[])), Err(Trouble::NeedsOnline));
    }

    #[test]
    fn asks_github_with_the_token_and_nothing_else() {
        let Step::Fetch(r) = step(&[file(APPS), file(HOSTS), Got::Missing], Reach::Online, &[]) else { panic!("no ask") };
        assert_eq!((r.method, r.url.as_str(), r.json), (Method::Get, "https://api.github.com/copilot_internal/user", None));
        // No User-Agent (the app adds its own) and no editor's name.
        assert_eq!(r.headers, vec![("Authorization".to_string(), "token test-token".to_string()), ("Accept".into(), "application/json".into())]);
    }

    #[test]
    fn picks_one_sign_in_the_same_way_every_time() {
        let ask = |got: &[Got]| match step(got, Reach::Online, &[]) {
            Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
            Step::Fetch(r) => (r.url, r.headers[0].1.clone()),
            Step::Done(o) => panic!("{o:?}"),
        };
        let github = "https://api.github.com/copilot_internal/user".to_string();
        assert_eq!(ask(&[Got::Missing, file(HOSTS), Got::Missing]), (github.clone(), "token test-token".into()), "the older hosts.json alone");
        // apps.json first, and github.com before an enterprise domain
        // whatever the file's order.
        let apps = r#"{"octocorp.ghe.com:Iv1.b507a08c87ecfe98":{"user":"b","oauth_token":"ghe-token"},"github.com:Iv1.b507a08c87ecfe98":{"user":"a","oauth_token":"apps-token"}}"#;
        let hosts = r#"{"github.com":{"user":"c","oauth_token":"hosts-token"}}"#;
        assert_eq!(ask(&[file(apps), file(hosts), Got::Missing]), (github, "token apps-token".into()));
        // An enterprise sign-in goes to its own domain's API only.
        let ghe = r#"{"octocorp.ghe.com:Iv1.b507a08c87ecfe98":{"user":"b","oauth_token":"ghe-token"}}"#;
        assert_eq!(ask(&[file(ghe), Got::Missing, Got::Missing]), ("https://api.octocorp.ghe.com/copilot_internal/user".into(), "token ghe-token".into()));
        assert_eq!(api_host("evil.example/x.ghe.com"), None);
        assert_eq!(api_host("a.b.ghe.com"), None);
    }

    #[test]
    fn a_pro_plan_shows_its_premium_requests() {
        let s = read(PRO).unwrap();
        assert_eq!((s.plan.as_deref(), s.account.as_deref(), s.source, s.as_of), (Some("Pro"), Some("t•••"), Source::Online, NOW));
        assert_eq!(s.windows.len(), 1, "chat and completions are unlimited");
        let w = &s.windows[0];
        assert_eq!((w.kind, w.label.as_str(), w.count, w.resets_at), (WindowKind::Monthly, "Premium requests", Some((82, 300)), parse_time("2026-11-01T00:00:00Z")));
        assert!((w.used - 0.274).abs() < 1e-4, "{}", w.used);
    }

    #[test]
    fn copilot_free_shows_chat_and_code_completions() {
        // Today's answer and the older one with monthly_quotas say the same.
        for body in [FREE, FREE_OLD] {
            let s = read(body).unwrap();
            assert_eq!(s.plan.as_deref(), Some("Free"));
            let reset = parse_time("2026-11-14T00:00:00Z");
            let rows: Vec<_> = s.windows.iter().map(|w| (w.label.as_str(), w.count, w.resets_at, (w.used * 100.0).round() as u32)).collect();
            assert_eq!(rows, [("Chat", Some((19, 50)), reset, 38), ("Code completions", Some((480, 2000)), reset, 24)]);
        }
    }

    #[test]
    fn an_unlimited_plan_gets_a_plan_line_and_no_bar() {
        let s = read(UNLIMITED).unwrap();
        assert_eq!((s.plan.as_deref(), s.windows.len()), (Some("Enterprise"), 0));
    }

    #[test]
    fn paid_requests_past_the_allowance_go_past_100_percent() {
        let s = read(r#"{"copilot_plan":"individual_pro","quota_reset_date":"2026-11-01","quota_snapshots":{"premium_interactions":{"entitlement":1500,"overage_count":12,"overage_permitted":true,"percent_remaining":0.0,"quota_remaining":0.0,"remaining":0,"unlimited":false}}}"#).unwrap();
        assert_eq!((s.plan.as_deref(), s.windows[0].count), (Some("Pro+"), Some((1512, 1500))));
        assert!((s.windows[0].used - 1.008).abs() < 1e-4);
    }

    #[test]
    fn plan_names() {
        let name = |sku: &str, plan: &str| super::plan(&serde_json::json!({ "access_type_sku": sku, "copilot_plan": plan }));
        assert_eq!(name("free_limited_copilot", "individual"), Some("Free"));
        assert_eq!(name("free_educational_quota", "individual"), Some("Student"));
        assert_eq!(name("monthly_subscriber_quota", "individual"), Some("Pro"));
        assert_eq!(name("", "individual_max"), Some("Max"));
        assert_eq!(name("copilot_enterprise_seat", "enterprise"), Some("Enterprise"));
        assert_eq!(name("", "something_new"), None);
        // Signed in without a plan: no card at all.
        assert_eq!(read(r#"{"access_type_sku":"","can_signup_for_limited":true,"chat_enabled":false,"copilot_plan":"individual"}"#), Err(Trouble::NotFound));
    }

    #[test]
    fn failed_answers_say_why() {
        let got = [file(APPS), Got::Missing, Got::Missing];
        let after = |got: &[Got], r: Result<Reply, NetFail>| done(step(got, Reach::Online, &[r]));
        assert_eq!(after(&got, reply(401, r#"{"message":"Bad credentials"}"#)), Err(Trouble::Refused));
        assert_eq!(after(&got, reply(403, r#"{"message":"Forbidden"}"#)), Err(Trouble::Refused));
        let busy = Ok(Reply { status: 429, body: String::new(), retry_after_s: Some(60) });
        assert_eq!(after(&got, busy), Err(Trouble::Busy { retry_after_s: Some(60) }));
        assert_eq!(after(&got, reply(403, r#"{"message":"API rate limit exceeded for user ID 1."}"#)), Err(Trouble::Busy { retry_after_s: None }));
        assert_eq!(after(&got, Err(NetFail::Timeout)), Err(Trouble::Offline));
        assert_eq!(after(&got, reply(200, "<html>Unicorn!</html>")), Err(Trouble::Unreadable("not JSON".into())));
        assert!(matches!(after(&got, reply(200, "{}")), Err(Trouble::Unreadable(_))));
        // With the newer store there, a dead token in the old apps.json hides
        // the card instead of asking for a sign-in that would never land here.
        let newer = [file(APPS), Got::Missing, Got::Many(vec![FileRead { path: "auth.db".into(), bytes: Vec::new(), modified: Some(NOW) }])];
        assert_eq!(after(&newer, reply(401, "{}")), Err(Trouble::NotFound));
        assert!(after(&newer, reply(200, PRO)).is_ok(), "an old copy that still works is read");
    }

    #[test]
    fn the_token_stays_out_of_every_outcome() {
        for body in [PRO, FREE, UNLIMITED, "{}", "nope"] {
            let o = format!("{:?}", read(body));
            assert!(!o.contains("test-token"), "{o}");
        }
    }
}
