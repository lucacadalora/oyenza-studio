//! Kimi Code (Moonshot's Kimi coding plan, kimi.com/code and kimi.ai/code):
//! the plan's 5-hour, weekly and monthly limits and the Extra Usage balance
//! from `GET <server>/usages`, the request Kimi Code's own `/usage` makes,
//! then the plan's name from `GET <server>/me`.
//!
//! What it asks with, first found wins:
//! 1. A Kimi Code API key typed into Oyenza Studio (`sk-kimi-…`, made in the
//!    Kimi Code console). It never expires, so it is the steady way. Nothing
//!    in a key says which of Kimi Code's two servers made it (kimi.ai for
//!    most of the world, kimi.com for mainland China), so it goes first to
//!    the one this computer's Kimi Code signs in to (else kimi.ai) and,
//!    refused there (401), to the other: both are Kimi Code's own.
//! 2. Kimi Code's own sign-in, shared by its terminal app, desktop app and
//!    VS Code extension: `.kimi-code/credentials/` in the home folder
//!    (`%USERPROFILE%` on Windows, `~` on the Mac; `KIMI_CODE_HOME` moves
//!    it), `kimi-code.json` for a kimi.com sign-in and
//!    `kimi-code-env-0e4f99c69cc27850.json` for kimi.ai. The token lives 15
//!    minutes and only Kimi Code renews it (the refresh token is never used
//!    here), so an old one is [`Trouble::Expired`]: open Kimi Code once.
//!
//! Asked as Oyenza Studio. Kimi Code's own usage request carries only the
//! token and `Accept`, and so does this one: never Kimi's `X-Msh-*` device
//! headers or its User-Agent (Kimi's rules count imitating its client as
//! abuse). Should Kimi ever answer only its own client, that shows as
//! [`Trouble::Refused`].

use super::{mask_email, micros, num, reply_json, time, Ctx, Dirs, Got, KeyAsk, Money, NetFail, Provider, ProviderId, Reach, Reply, Request, Snapshot, Source, Spend, Step, Trouble, Want, Window, WindowKind};
use serde_json::Value;

/// Kimi Code's two servers: mainland China's, and the rest of the world's.
const KIMI_COM: &str = "https://api.kimi.com/coding/v1";
const KIMI_AI: &str = "https://api.kimi.ai/coding/v1";
/// Kimi Code's sign-in files under `<home>/credentials/` (wants 0 and 1),
/// with the server each one's token belongs to. The kimi.ai name is Kimi
/// Code's own (`resolveKimiCodeOAuthKey`): "kimi-code-env-" and the first 16
/// hex digits of the SHA-256 of
/// `{"oauthHost":"https://auth.kimi.ai","baseUrl":"https://api.kimi.ai/coding/v1"}`.
const SIGNINS: [(&str, &str); 2] = [("kimi-code.json", KIMI_COM), ("kimi-code-env-0e4f99c69cc27850.json", KIMI_AI)];
/// Want 2: the typed key, kept under this name.
const KEY: usize = 2;
const SECRET: &str = "kimi";
/// Want 3: a file Kimi Code writes on its first run, so a computer with
/// Kimi Code but no sign-in is [`Trouble::SignedOut`], not NotFound.
const INSTALLED: usize = 3;
/// A token this close to its end is taken as expired: the ask may land after.
const MARGIN_S: i64 = 60;
const WEEK: u32 = 7 * 86_400;
/// The limits Kimi reports as shares (`usages`), as the tab calls them.
const POOLS: [(&str, WindowKind, &str, Option<u32>); 3] = [
    ("limit_5h", WindowKind::Session, "5-hour", Some(5 * 3600)),
    ("limit_7d", WindowKind::Weekly, "Weekly", Some(WEEK)),
    ("limit_month_total", WindowKind::Monthly, "Monthly", None),
];

pub struct Kimi;

impl Provider for Kimi {
    fn id(&self) -> ProviderId {
        ProviderId::Kimi
    }

    fn env(&self) -> &'static [&'static str] {
        &["KIMI_CODE_HOME"]
    }

    fn wants(&self, dirs: &Dirs) -> Vec<Want> {
        let home = dirs.var_or_home("KIMI_CODE_HOME", ".kimi-code");
        let mut wants: Vec<Want> = SIGNINS.iter().map(|(file, _)| Want::File { path: dirs.join(&home, &["credentials", file]), max: 64 * 1024 }).collect();
        wants.push(Want::Secret(SECRET));
        wants.push(Want::File { path: dirs.join(&home, &["device_id"]), max: 1024 });
        wants
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        let (token, servers) = match ask_with(cx) {
            Ok(a) => a,
            Err(t) => return Step::Done(Err(t)),
        };
        if cx.reach == Reach::Local {
            return Step::Done(Err(Trouble::NeedsOnline));
        }
        // One usage ask per server, moving on only when one refused the key.
        let mut i = 0;
        while let Some(Ok(r)) = replies.get(i) {
            if r.status != 401 || i + 1 == servers.len() {
                break;
            }
            i += 1;
        }
        let Some(usages) = replies.get(i) else {
            return Step::Fetch(ask(servers[i], "usages", &token));
        };
        let mut snap = match read_usages(usages, cx.now) {
            Ok(s) => s,
            Err(t) => return Step::Done(Err(t)),
        };
        // Then the plan's name, best effort: a failed `/me` leaves it unknown.
        match replies.get(i + 1) {
            None => Step::Fetch(ask(servers[i], "me", &token)),
            Some(me) => {
                read_me(me, &mut snap);
                Step::Done(Ok(snap))
            }
        }
    }

    fn min_gap_s(&self) -> u64 {
        300
    }

    fn takes_key(&self) -> Option<KeyAsk> {
        Some(KeyAsk { secret: SECRET, url: "https://www.kimi.ai/code/console", hint: "sk-kimi-…" })
    }
}

/// One of Kimi Code's sign-ins, read.
struct SignIn {
    token: String,
    expires_at: i64,
    server: &'static str,
}

/// A sign-in file's token. Kimi Code marks a sign-in it can no longer
/// renew by emptying the token, and a file it can't read counts as none.
fn sign_in(got: &Got, server: &'static str) -> Option<SignIn> {
    let v = got.json()?;
    let token = v["access_token"].as_str()?.trim();
    if token.is_empty() || !header_safe(token) {
        return None;
    }
    Some(SignIn { token: token.to_string(), expires_at: time(&v["expires_at"]).unwrap_or(0), server })
}

/// Printable ASCII only, so a mangled key never reaches a header (where an
/// HTTP error could quote it back).
fn header_safe(t: &str) -> bool {
    t.bytes().all(|b| b.is_ascii_graphic())
}

/// The key or token to ask with and the servers to ask, in order, or why
/// there is none.
fn ask_with(cx: &Ctx) -> Result<(String, Vec<&'static str>), Trouble> {
    // Signed in to both servers: the newer sign-in.
    let signed = SIGNINS.iter().enumerate().filter_map(|(i, (_, server))| sign_in(cx.got(i), server)).max_by_key(|s| s.expires_at);
    if let Some(key) = cx.got(KEY).text().map(str::trim).filter(|k| !k.is_empty()) {
        if !header_safe(key) {
            return Err(Trouble::Refused);
        }
        // Which server made it is unsaid: this computer's Kimi Code's first.
        let first = signed.as_ref().map_or(KIMI_AI, |s| s.server);
        let other = if first == KIMI_AI { KIMI_COM } else { KIMI_AI };
        return Ok((key.to_string(), vec![first, other]));
    }
    match signed {
        Some(s) if s.expires_at > cx.now + MARGIN_S => Ok((s.token, vec![s.server])),
        Some(_) => Err(Trouble::Expired),
        None if (0..SIGNINS.len()).chain([INSTALLED]).any(|i| !cx.got(i).is_missing()) => Err(Trouble::SignedOut),
        None => Err(Trouble::NotFound),
    }
}

fn ask(server: &str, path: &str, token: &str) -> Request {
    Request::get(format!("{server}/{path}")).bearer(token).header("Accept", "application/json")
}

/// The `/usages` answer as a snapshot.
fn read_usages(r: &Result<Reply, NetFail>, now: i64) -> Result<Snapshot, Trouble> {
    let mut snap = Snapshot::new(ProviderId::Kimi, now, Source::Online);
    // A plan with nothing left answers 429 "Credits used up." with no
    // limits: that is the reading, not a busy server.
    if let Ok(rep) = r {
        if rep.status == 429 && used_up(&rep.body) {
            snap.windows.push(Window::new(WindowKind::Other, "Plan", 1.0));
            return Ok(snap);
        }
    }
    let v = reply_json(r)?;
    snap.windows = windows(&v);
    (snap.balance, snap.spend) = wallet(&v["boosterWallet"]);
    if snap.windows.is_empty() && snap.balance.is_none() {
        return Err(Trouble::Unreadable("no limits".into()));
    }
    Ok(snap)
}

fn used_up(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    ["resource_exhausted", "quota_exceeded", "exceeded_current_quota", "used up"].iter().any(|w| b.contains(w))
}

/// How much of one limit is used, and when it resets.
#[derive(Clone, Copy)]
struct Part {
    used: f32,
    resets: Option<i64>,
}

/// The limits, from the shares Kimi Code itself reads (`usages`) and the
/// older counted fields (`limits[]`, each with its length, and `usage`,
/// the plan's, its length unsaid). A count of the same limit, known by its
/// reset (within 2 s: the two clocks differ by about 1.5 s), that says
/// more is used wins: the share has read 0 while the count said 100 of 100.
/// A count stands alone only where no share covers it (older answers).
fn windows(v: &Value) -> Vec<Window> {
    let mut counts: Vec<(Option<u32>, Part)> = Vec::new();
    for l in v["limits"].as_array().into_iter().flatten() {
        if let (Some(len), Some(p)) = (length(&l["window"]), count(&l["detail"])) {
            counts.push((Some(len), p));
        }
    }
    if let Some(p) = count(&v["usage"]) {
        counts.push((None, p));
    }
    let mut out = Vec::new();
    for (key, kind, label, len) in POOLS {
        let Some(mut p) = share(&v["usages"][key]) else { continue };
        for (clen, c) in &counts {
            let same_limit = *clen == len || (clen.is_none() && kind != WindowKind::Session);
            if same_limit && matches!((c.resets, p.resets), (Some(a), Some(b)) if (a - b).abs() <= 2) {
                p.used = p.used.max(c.used);
            }
        }
        out.push(Window::new(kind, label, p.used).resets(p.resets).length(len));
    }
    for (len, c) in counts {
        let covered = out.iter().any(|w| match len {
            Some(l) => w.length_s == Some(l),
            None => w.kind != WindowKind::Session,
        });
        if !covered {
            // `usage` is the plan's limit, which Kimi's own apps called weekly.
            let (kind, label) = span(len.unwrap_or(WEEK));
            out.push(Window::new(kind, label, c.used).resets(c.resets).length(len.or(Some(WEEK))));
        }
    }
    out
}

/// `{"used_ratio": 0.38, "reset_time": "…"}`. A negative share means nothing.
fn share(p: &Value) -> Option<Part> {
    let used = num(&p["used_ratio"]).filter(|r| *r >= 0.0)?;
    Some(Part { used: used as f32, resets: time(&p["reset_time"]) })
}

/// `{"limit": "100", "used": "38", "remaining": "62", "resetTime": "…"}`:
/// `limit` has been 100 on every account read, a share, never a count of
/// requests, so the tab shows only the percent. `used` is left out when 0.
fn count(d: &Value) -> Option<Part> {
    let limit = num(&d["limit"]).filter(|l| *l > 0.0)?;
    let used = match num(&d["used"]).filter(|u| *u >= 0.0) {
        Some(u) => u,
        None => limit - num(&d["remaining"]).filter(|r| (0.0..=limit).contains(r))?,
    };
    let resets = ["resetTime", "resetAt", "reset_time", "reset_at"].iter().find_map(|k| time(&d[*k]));
    Some(Part { used: (used / limit) as f32, resets })
}

/// `{"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"}` in seconds.
fn length(w: &Value) -> Option<u32> {
    let unit = match w["timeUnit"].as_str()? {
        "TIME_UNIT_MINUTE" => 60,
        "TIME_UNIT_HOUR" => 3600,
        "TIME_UNIT_DAY" => 86_400,
        _ => return None,
    };
    let n = num(&w["duration"]).filter(|n| (1.0..=1e6).contains(n))?;
    u32::try_from(n as u64 * unit).ok()
}

/// A counted limit's name, from its length: "5-hour", "Weekly", "2-day".
fn span(s: u32) -> (WindowKind, String) {
    match s {
        WEEK => (WindowKind::Weekly, "Weekly".into()),
        86_400 => (WindowKind::Daily, "Daily".into()),
        _ if s.is_multiple_of(86_400) => (WindowKind::Other, format!("{}-day", s / 86_400)),
        _ if s.is_multiple_of(3600) => (WindowKind::Session, format!("{}-hour", s / 3600)),
        _ => (WindowKind::Session, format!("{}-minute", s / 60)),
    }
}

/// Extra Usage, the top-up Kimi spends once the plan runs out: what's left,
/// and this month's spend with its cap. The balance comes in millionths of
/// a cent, the monthly sums in cents, and a sum left out is 0 (Kimi Code's
/// own reading).
fn wallet(w: &Value) -> (Option<Money>, Vec<Spend>) {
    let b = &w["balance"];
    if b["type"].as_str() != Some("BOOSTER") || !num(&b["amount"]).is_some_and(|a| a > 0.0) {
        return (None, Vec::new());
    }
    let currency = ["monthlyChargeLimit", "monthlyUsed"].iter().filter_map(|k| w[*k]["currency"].as_str()).find(|c| !c.is_empty()).unwrap_or("USD");
    let money = |units: f64| Money { micros: micros(units), currency: currency.to_string() };
    let cents = |k: &str| num(&w[k]["priceInCents"]);
    let cap = cents("monthlyChargeLimit").filter(|c| *c > 0.0 && w["monthlyChargeLimitEnabled"] == true).map(|c| money(c / 100.0));
    let month = Spend { label: "This month".into(), money: money(cents("monthlyUsed").unwrap_or(0.0) / 100.0), cap };
    (Some(money(num(&b["amountLeft"]).unwrap_or(0.0) / 1e8)), vec![month])
}

/// The plan's name ("Moderato") and the signed-in email from `/me`;
/// nothing else there (phone, nickname) is kept.
fn read_me(r: &Result<Reply, NetFail>, snap: &mut Snapshot) {
    let Ok(v) = reply_json(r) else { return };
    snap.plan = v["user_level_name"].as_str().map(str::trim).filter(|p| !p.is_empty()).map(String::from);
    snap.account = v["email"].as_str().filter(|e| e.contains('@')).map(mask_email);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileRead, Method, Os, Outcome, MAX_FETCHES};
    use std::path::PathBuf;

    /// 2026-10-27T05:00:00Z.
    const NOW: i64 = 1_793_077_200;
    /// Answers shaped like Kimi's, everything personal replaced.
    const WEEKLY: &str = include_str!("fixtures/kimi/usages-weekly.json");
    const MONTHLY: &str = include_str!("fixtures/kimi/usages-monthly.json");
    const ME: &str = include_str!("fixtures/kimi/me.json");
    const USED_UP: &str = include_str!("fixtures/kimi/used-up.json");
    /// Kimi Code's sign-in file, its token good until NOW + 15 minutes.
    const SIGNED_IN: &str = include_str!("fixtures/kimi/credentials.json");
    /// The same file after Kimi Code could no longer renew it.
    const SIGNED_OUT: &str = include_str!("fixtures/kimi/credentials-signed-out.json");

    fn file(text: &str) -> Got {
        Got::One(FileRead { path: PathBuf::from("file"), bytes: text.as_bytes().to_vec(), modified: None })
    }

    /// What this computer holds: the kimi.com and kimi.ai sign-ins, the typed
    /// key, and whether Kimi Code ever ran here.
    fn got(com: Option<&str>, ai: Option<&str>, key: Option<&str>, installed: bool) -> Vec<Got> {
        let f = |t: Option<&str>| t.map_or(Got::Missing, file);
        vec![f(com), f(ai), key.map_or(Got::Missing, |k| Got::Text(k.into())), if installed { file("0c6e6a8e-test") } else { Got::Missing }]
    }

    fn answer(status: u16, body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status, body: body.into(), retry_after_s: None })
    }

    /// One refresh against these answers, in order: what it asked, and the end.
    fn run(got: &[Got], reach: Reach, answers: &[Result<Reply, NetFail>]) -> (Vec<Request>, Outcome) {
        let dirs = Dirs::sample(Os::Windows);
        let cx = Ctx { dirs: &dirs, got, now: NOW, reach };
        let (mut asked, mut replies) = (Vec::new(), Vec::new());
        loop {
            match Kimi.next(&cx, &replies) {
                Step::Read(w) => panic!("this service never reads in steps: {w:?}"),
                Step::Fetch(req) => {
                    assert!(asked.len() < MAX_FETCHES, "too many asks");
                    let a = answers.get(replies.len()).cloned().unwrap_or_else(|| panic!("no answer for {}", req.url));
                    asked.push(req);
                    replies.push(a);
                }
                Step::Done(out) => return (asked, out),
            }
        }
    }

    fn urls(asked: &[Request]) -> Vec<&str> {
        asked.iter().map(|r| r.url.as_str()).collect()
    }

    #[test]
    fn reads_kimi_codes_files_on_windows_and_the_mac() {
        for (os, kimi, sep) in [(Os::Windows, r"C:\Users\u\.kimi-code", r"\"), (Os::Mac, "/Users/u/.kimi-code", "/")] {
            let at = |rel: &[&str]| PathBuf::from(format!("{kimi}{sep}{}", rel.join(sep)));
            assert_eq!(Kimi.wants(&Dirs::sample(os)), vec![
                Want::File { path: at(&["credentials", "kimi-code.json"]), max: 64 * 1024 },
                Want::File { path: at(&["credentials", "kimi-code-env-0e4f99c69cc27850.json"]), max: 64 * 1024 },
                Want::Secret("kimi"),
                Want::File { path: at(&["device_id"]), max: 1024 },
            ]);
        }
        // KIMI_CODE_HOME moves all of it, as it does for Kimi Code.
        assert_eq!(Kimi.env(), &["KIMI_CODE_HOME"]);
        let mut dirs = Dirs::sample(Os::Windows);
        dirs.env.push(("KIMI_CODE_HOME".into(), r"D:\kimi".into()));
        let wants = Kimi.wants(&dirs);
        assert_eq!(wants[1], Want::File { path: PathBuf::from(r"D:\kimi\credentials\kimi-code-env-0e4f99c69cc27850.json"), max: 64 * 1024 });
        assert_eq!(wants[3], Want::File { path: PathBuf::from(r"D:\kimi\device_id"), max: 1024 });
        // The kimi.ai file's name, the way Kimi Code makes it.
        let slot = poni_core::sha256::hex(&poni_core::sha256::sha256(br#"{"oauthHost":"https://auth.kimi.ai","baseUrl":"https://api.kimi.ai/coding/v1"}"#));
        assert_eq!(SIGNINS[1].0, format!("kimi-code-env-{}.json", &slot[..16]));
    }

    #[test]
    fn nothing_of_kimi_here_is_not_found() {
        for reach in [Reach::Online, Reach::Local] {
            let (asked, out) = run(&got(None, None, None, false), reach, &[]);
            assert!(asked.is_empty());
            assert_eq!(out, Err(Trouble::NotFound));
        }
    }

    #[test]
    fn kimi_code_without_a_sign_in_or_key_is_signed_out() {
        // Ran once, never signed in; signed out by Kimi Code itself; a file it can't read either.
        for g in [got(None, None, None, true), got(Some(SIGNED_OUT), None, None, true), got(None, Some("{not json"), None, false)] {
            let (asked, out) = run(&g, Reach::Online, &[]);
            assert!(asked.is_empty());
            assert_eq!(out, Err(Trouble::SignedOut));
        }
        // A typed key that's only spaces is no key, and a token with a break in it no sign-in.
        assert_eq!(run(&got(None, None, Some("  "), true), Reach::Online, &[]).1, Err(Trouble::SignedOut));
        let mangled = SIGNED_IN.replace("test-token", "test-\\ntoken");
        assert_eq!(run(&got(Some(&mangled), None, None, true), Reach::Online, &[]).1, Err(Trouble::SignedOut));
    }

    #[test]
    fn an_old_token_is_expired_and_never_renewed_here() {
        for expires_at in [NOW - 3600, NOW + 30] {
            let creds = SIGNED_IN.replace("1793078100", &expires_at.to_string());
            let (asked, out) = run(&got(Some(&creds), None, None, true), Reach::Online, &[]);
            assert!(asked.is_empty(), "no ask, and never the token endpoint");
            assert_eq!(out, Err(Trouble::Expired));
            // Signed in to both servers: the newer sign-in is the one asked with.
            let (asked, _) = run(&got(Some(&creds), Some(SIGNED_IN), None, true), Reach::Online, &[answer(200, WEEKLY), answer(200, ME)]);
            assert_eq!(urls(&asked)[0], "https://api.kimi.ai/coding/v1/usages");
        }
    }

    #[test]
    fn a_kimi_ai_sign_in_reads_the_plan_limits_and_name() {
        let (asked, out) = run(&got(None, Some(SIGNED_IN), None, true), Reach::Online, &[answer(200, WEEKLY), answer(200, ME)]);
        assert_eq!(urls(&asked), ["https://api.kimi.ai/coding/v1/usages", "https://api.kimi.ai/coding/v1/me"]);
        for r in &asked {
            // Only the token and Accept, like Kimi Code's own ask: no User-Agent or X-Msh-* of Kimi's.
            assert_eq!(r.method, Method::Get);
            assert_eq!(r.headers, [("Authorization".to_string(), "Bearer test-token".to_string()), ("Accept".to_string(), "application/json".to_string())]);
        }
        let s = out.unwrap();
        assert_eq!((s.provider, s.as_of, s.source), (ProviderId::Kimi, NOW, Source::Online));
        assert_eq!(s.plan.as_deref(), Some("Moderato"));
        assert_eq!(s.account.as_deref(), Some("a•••@example.com"));
        assert_eq!(s.windows, vec![
            // The share (38.12%) says more than the count (38 of 100).
            Window::new(WindowKind::Session, "5-hour", 0.3812).resets(Some(1_793_088_750)).length(Some(18_000)),
            // The share reads 0, the count of the same week (its reset 1.4 s apart) 19.
            Window::new(WindowKind::Weekly, "Weekly", 0.19).resets(Some(1_793_378_758)).length(Some(604_800)),
        ]);
        assert!(s.windows.iter().all(|w| w.count.is_none()), "Kimi's counts are shares, not requests");
        assert_eq!((s.balance, s.spend.len(), s.tokens_today), (None, 0, None));
    }

    #[test]
    fn a_monthly_plan_with_extra_usage() {
        let (asked, out) = run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[answer(200, MONTHLY), answer(200, ME)]);
        assert_eq!(urls(&asked), ["https://api.kimi.com/coding/v1/usages", "https://api.kimi.com/coding/v1/me"]);
        let s = out.unwrap();
        assert_eq!(s.windows, vec![
            // 100 of 100 counted beats the share's 0 for the same 5 hours.
            Window::new(WindowKind::Session, "5-hour", 1.0).resets(Some(1_793_093_026)).length(Some(18_000)),
            Window::new(WindowKind::Monthly, "Monthly", 0.5531).resets(Some(1_795_357_589)),
        ]);
        assert_eq!(s.balance, Some(Money { micros: 12_500_000, currency: "USD".into() }));
        assert_eq!(s.spend, vec![Spend { label: "This month".into(), money: Money::usd(37.5), cap: Some(Money::usd(50.0)) }]);
        assert_eq!(s.balance.unwrap().fmt(), "$12.50");
    }

    #[test]
    fn counts_stand_alone_only_in_older_answers() {
        // An early answer, before the shares: counts out of real limits.
        let old = r#"{"usage": {"limit": "2048", "used": "214", "remaining": "1834", "resetTime": "2026-10-30T16:45:59Z"},
            "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"}, "detail": {"limit": "200", "remaining": "50", "resetTime": "2026-10-27T08:12:31Z"}}]}"#;
        let s = read_usages(&answer(200, old), NOW).unwrap();
        assert_eq!(s.windows, vec![
            Window::new(WindowKind::Session, "5-hour", 0.75).resets(Some(1_793_088_751)).length(Some(18_000)),
            Window::new(WindowKind::Weekly, "Weekly", 214.0 / 2048.0).resets(Some(1_793_378_759)).length(Some(604_800)),
        ]);
        // A count of another week (its reset 3 s off) leaves the share alone, and doesn't add a second weekly.
        let off = r#"{"usage": {"limit": "100", "used": "19", "resetTime": "2026-10-30T16:46:01Z"},
            "usages": {"limit_7d": {"used_ratio": 0, "reset_time": "2026-10-30T16:45:58Z"}}}"#;
        let s = read_usages(&answer(200, off), NOW).unwrap();
        assert_eq!(s.windows, vec![Window::new(WindowKind::Weekly, "Weekly", 0.0).resets(Some(1_793_378_758)).length(Some(604_800))]);
    }

    #[test]
    fn local_refreshes_read_files_only() {
        let fresh = got(Some(SIGNED_IN), None, None, true);
        let key = got(None, None, Some("sk-kimi-test"), false);
        for (g, want) in [(fresh, Trouble::NeedsOnline), (key, Trouble::NeedsOnline), (got(None, None, None, true), Trouble::SignedOut)] {
            let (asked, out) = run(&g, Reach::Local, &[]);
            assert!(asked.is_empty());
            assert_eq!(out, Err(want));
        }
    }

    #[test]
    fn a_refused_sign_in_is_refused() {
        // A sign-in's server is known: no second server for it.
        let (asked, out) = run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[answer(401, r#"{"error":{"type":"invalid_authentication_error"}}"#)]);
        assert_eq!(urls(&asked), ["https://api.kimi.com/coding/v1/usages"]);
        assert_eq!(out, Err(Trouble::Refused));
        assert_eq!(run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[answer(403, "")]).1, Err(Trouble::Refused));
    }

    #[test]
    fn a_typed_key_goes_to_the_other_server_when_the_first_refuses_it() {
        let key = Some("sk-kimi-test");
        let (asked, out) = run(&got(None, None, key, false), Reach::Online, &[answer(401, ""), answer(200, WEEKLY), answer(200, ME)]);
        assert_eq!(urls(&asked), ["https://api.kimi.ai/coding/v1/usages", "https://api.kimi.com/coding/v1/usages", "https://api.kimi.com/coding/v1/me"]);
        assert!(asked.iter().all(|r| r.headers[0] == ("Authorization".to_string(), "Bearer sk-kimi-test".to_string())));
        assert_eq!(out.unwrap().windows.len(), 2);
        // A key pasted with a space in it never goes out.
        let (asked, out) = run(&got(None, None, Some("sk-kimi te st"), false), Reach::Online, &[]);
        assert!(asked.is_empty());
        assert_eq!(out, Err(Trouble::Refused));
        // Refused by both: refused.
        let (asked, out) = run(&got(None, None, key, false), Reach::Online, &[answer(401, ""), answer(401, "")]);
        assert_eq!(asked.len(), 2);
        assert_eq!(out, Err(Trouble::Refused));
        // The key wins over Kimi Code's sign-in, and tries that sign-in's server first.
        let (asked, _) = run(&got(Some(SIGNED_IN), None, key, true), Reach::Online, &[answer(200, WEEKLY), answer(200, ME)]);
        assert_eq!(urls(&asked)[0], "https://api.kimi.com/coding/v1/usages");
        assert_eq!(asked[0].headers[0].1, "Bearer sk-kimi-test");
    }

    #[test]
    fn too_many_asks_is_busy_with_its_wait() {
        let busy = Ok(Reply { status: 429, body: r#"{"error":{"message":"We're receiving too many requests at the moment","type":"rate_limit_reached_error"}}"#.into(), retry_after_s: Some(120) });
        let (asked, out) = run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[busy]);
        assert_eq!(asked.len(), 1);
        assert_eq!(out, Err(Trouble::Busy { retry_after_s: Some(120) }));
    }

    #[test]
    fn a_used_up_plan_reads_as_full_not_busy() {
        let (_, out) = run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[answer(429, USED_UP), answer(200, ME)]);
        let s = out.unwrap();
        assert_eq!(s.windows, vec![Window::new(WindowKind::Other, "Plan", 1.0)]);
        assert_eq!(s.plan.as_deref(), Some("Moderato"));
    }

    #[test]
    fn answers_we_cant_read_say_so() {
        // A share without its number, or a negative one, is no reading.
        let empty = r#"{"usages": {"limit_5h": {"reset_time": "2026-10-27T08:12:30Z"}, "limit_7d": {"used_ratio": -0.5}}}"#;
        for (body, why) in [("<html>Bad gateway</html>", "not JSON"), ("{}", "no limits"), (empty, "no limits")] {
            let (asked, out) = run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[answer(200, body)]);
            assert_eq!(asked.len(), 1, "no /me after an answer we can't read");
            assert_eq!(out, Err(Trouble::Unreadable(why.into())));
        }
        // No answer at all: offline.
        assert_eq!(run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[Err(NetFail::Timeout)]).1, Err(Trouble::Offline));
    }

    #[test]
    fn a_failed_me_keeps_the_limits() {
        for me in [answer(500, ""), answer(404, ""), Err(NetFail::Timeout), answer(200, "[]")] {
            let s = run(&got(Some(SIGNED_IN), None, None, true), Reach::Online, &[answer(200, WEEKLY), me]).1.unwrap();
            assert_eq!(s.windows.len(), 2);
            assert_eq!((s.plan, s.account), (None, None));
        }
    }

    #[test]
    fn asks_for_a_key_and_rests_five_minutes() {
        let ask = Kimi.takes_key().unwrap();
        assert_eq!(Want::Secret(ask.secret), Kimi.wants(&Dirs::sample(Os::Mac))[KEY]);
        assert!(ask.url.starts_with("https://www.kimi."));
        assert_eq!(ask.hint, "sk-kimi-…");
        assert_eq!(Kimi.min_gap_s(), 300);
    }
}
