//! Antigravity: Google's coding agent CLI (`agy`), which took over from Gemini
//! CLI. Its limits are a 5-hour and a weekly one for each group of models
//! (Gemini, Claude and GPT, one model on its own).
//!
//! Google's Code Assist server answers those numbers to Antigravity itself
//! only: asked as Oyenza Studio it says `UNSUPPORTED_CLIENT`, and we don't
//! ask as another app (see the rules in the crate's docs). So the numbers
//! come from `agy`'s own `/usage` command, which asks Google itself, with no
//! model turn:
//!
//! - Sign-in: `agy` keeps it in the Mac keychain, item "gemini" of account
//!   "antigravity" (the Go `keyring` library: `go-keyring-base64:` and a JSON
//!   text with the OAuth token and the account's email). It is read only to
//!   see whether somebody is signed in (a refresh token) and who; the tokens
//!   are never used or sent. The sign-in file `~/.gemini/jetski-...` is a
//!   leftover and is not read. Windows keeps it elsewhere: not read yet.
//! - Numbers: [`Cli::AgyUsage`] (`agy -p /usage --output-format json`) only
//!   after [`Cli::AgyVersion`] says it is a version that answers `/usage`
//!   itself (before 1.1.11 it sends the command to the model, which spends
//!   quota). A reply that shows a model turn is not read. Each run is one
//!   process of about 2 seconds of CPU, so the worker runs it only while the
//!   AI tab is open, at most every 15 minutes ([`GAP_S`]).
//! - Never refreshed here: `agy` renews its own sign-in when it runs.
//!
//! The answer format and the guards around `/usage` follow what Orca
//! (stablyai/orca, MIT) learned reading it; no code taken.

use super::{b64url, mask_email, num, time, Cli, Ctx, Got, NetFail, Provider, ProviderId, Reach, Reply, Snapshot, Source, Step, Trouble, Want, Window, WindowKind};
use serde_json::Value;

/// The keychain item `agy` keeps its sign-in in.
const KEYCHAIN_SERVICE: &str = "gemini";
const KEYCHAIN_ACCOUNT: &str = "antigravity";
/// How the Go `keyring` library marks a base64 value.
const BASE64_PREFIX: &str = "go-keyring-base64:";

/// The first `agy` that answers `/usage` itself in print mode.
const MIN_VERSION: [u32; 3] = [1, 1, 11];

/// The shortest gap between two runs of `agy`: its limits move slowly (a
/// 5-hour and a weekly window), and a run costs about 2 seconds of CPU.
const GAP_S: u64 = 15 * 60;

const FIVE_HOURS_S: u32 = 5 * 3600;
const WEEK_S: u32 = 7 * 86_400;

pub struct Antigravity;

impl Provider for Antigravity {
    fn id(&self) -> ProviderId {
        ProviderId::Antigravity
    }

    /// The keychain item, on a Mac. Windows has none yet: nothing to read.
    fn wants(&self, dirs: &super::Dirs) -> Vec<Want> {
        match dirs.os {
            super::Os::Mac => vec![Want::Keychain { service: KEYCHAIN_SERVICE.into(), account: Some(KEYCHAIN_ACCOUNT.into()) }],
            super::Os::Windows => Vec::new(),
        }
    }

    fn next(&self, cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Step {
        step(cx, replies).unwrap_or_else(|t| Step::Done(Err(t)))
    }

    fn min_gap_s(&self) -> u64 {
        GAP_S
    }

    fn runs_programs(&self) -> bool {
        true
    }
}

fn step(cx: &Ctx, replies: &[Result<Reply, NetFail>]) -> Result<Step, Trouble> {
    let account = sign_in(cx)?;
    if cx.reach == Reach::Local {
        // The numbers come only from running `agy`.
        return Err(Trouble::NeedsOnline);
    }
    let Some(version) = replies.first() else { return Ok(Step::Run(Cli::AgyVersion)) };
    let found = version_of(&ran(version)?.body).ok_or_else(|| Trouble::Unreadable("Antigravity's version".into()))?;
    if found < MIN_VERSION {
        let [a, b, c] = MIN_VERSION;
        return Err(Trouble::Unreadable(format!("Antigravity {a}.{b}.{c} or newer (found {}.{}.{})", found[0], found[1], found[2])));
    }
    let Some(usage) = replies.get(1) else { return Ok(Step::Run(Cli::AgyUsage)) };
    snapshot(&ran(usage)?.body, cx.now, account).map(|s| Step::Done(Ok(s)))
}

/// Who is signed in (masked), once somebody is: the keychain item holds a
/// refresh token. The access token's own expiry doesn't matter: it lasts an
/// hour and `agy` renews it when it runs.
fn sign_in(cx: &Ctx) -> Result<Option<String>, Trouble> {
    let item = match cx.got(0) {
        Got::Missing => return Err(Trouble::NotFound),
        Got::Denied => return Err(Trouble::Denied),
        got => got.text().ok_or_else(|| Trouble::Unreadable("Antigravity's sign-in".into()))?,
    };
    let item = item.trim();
    let json = match item.strip_prefix(BASE64_PREFIX) {
        Some(b64) => b64url(b64.trim()).and_then(|b| String::from_utf8(b).ok()),
        None => Some(item.to_string()),
    };
    let me: Value = json.and_then(|j| serde_json::from_str(&j).ok()).filter(Value::is_object).ok_or_else(|| Trouble::Unreadable("Antigravity's sign-in".into()))?;
    if me.pointer("/token/refresh_token").and_then(Value::as_str).is_none_or(|t| t.trim().is_empty()) {
        return Err(Trouble::SignedOut);
    }
    Ok(me["id_token_email"].as_str().map(str::trim).filter(|e| !e.is_empty()).map(mask_email))
}

/// What running `agy` came to: its reply, or why not.
fn ran(r: &Result<Reply, NetFail>) -> Result<&Reply, Trouble> {
    match r {
        Ok(r) if r.status == 200 => Ok(r),
        // Needs the person to sign in again (the app stopped it, or never
        // ran it).
        Ok(r) if r.status == 401 => Err(Trouble::SignedOut),
        Ok(_) => Err(Trouble::Unreadable("Antigravity did not give its limits".into())),
        Err(NetFail::Timeout) => Err(Trouble::Unreadable("Antigravity did not answer in time".into())),
        // Not installed (the sign-in is from its IDE), or it would not start.
        Err(_) => Err(Trouble::NotFound),
    }
}

/// `1.4.12` out of `agy --version`'s text.
fn version_of(text: &str) -> Option<[u32; 3]> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.')).find_map(|word| {
        let mut parts = word.split('.');
        Some([parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?])
    })
}

/// The limits out of `agy -p /usage --output-format json`: one JSON line
/// (log lines may come before it).
fn snapshot(stdout: &str, now: i64, account: Option<String>) -> Result<Snapshot, Trouble> {
    let answer = stdout
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["status"] == "SUCCESS" && v.pointer("/command/name").and_then(Value::as_str) == Some("usage"))
        .ok_or_else(|| Trouble::Unreadable("Antigravity's limits".into()))?;
    // A model turn means `agy` took `/usage` as a prompt: quota was spent
    // and these are not the limits.
    if answer["num_turns"].as_u64().is_some_and(|n| n > 0) || answer["conversation_id"].as_str().is_some_and(|c| !c.is_empty()) {
        return Err(Trouble::Unreadable("Antigravity answered /usage as a prompt".into()));
    }
    let groups = answer.pointer("/command/data/groups").and_then(Value::as_array).ok_or_else(|| Trouble::Unreadable("Antigravity's limits".into()))?;
    let mut s = Snapshot::new(ProviderId::Antigravity, now, Source::Online);
    s.account = account;
    for group in groups {
        let name = group["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Antigravity");
        for bucket in group["buckets"].as_array().into_iter().flatten() {
            // Unmetered: drawn as unused it would invent room next to a pool that is empty.
            if bucket["disabled"].as_bool() == Some(true) {
                continue;
            }
            let Some(left) = num(&bucket["remaining_fraction"]) else { continue };
            let (kind, length, what) = match bucket["window"].as_str() {
                Some("5h") => (WindowKind::Session, Some(FIVE_HOURS_S), "5-hour"),
                Some("weekly") => (WindowKind::Weekly, Some(WEEK_S), "Weekly"),
                _ => (WindowKind::Other, None, bucket["name"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Limit")),
            };
            let used = (1.0 - left).clamp(0.0, 1.0) as f32;
            s.windows.push(Window::new(kind, format!("{name} · {what}"), used).resets(time(&bucket["reset_time"])).length(length));
        }
    }
    if s.windows.is_empty() {
        return Err(Trouble::Unreadable("Antigravity's limits".into()));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{drive_runs, parse_time, Dirs, Os};

    const ITEM: &str = include_str!("fixtures/antigravity/keychain_item.txt");
    const ITEM_NO_REFRESH: &str = include_str!("fixtures/antigravity/keychain_item_no_refresh.txt");
    const USAGE: &str = include_str!("fixtures/antigravity/usage.json");
    const PROMPT: &str = include_str!("fixtures/antigravity/usage_prompt.json");
    const EMPTY: &str = include_str!("fixtures/antigravity/usage_empty.json");
    const VERSION: &str = include_str!("fixtures/antigravity/version.txt");
    const NOW: i64 = 1_793_000_000;

    fn ok(body: &str) -> Result<Reply, NetFail> {
        Ok(Reply { status: 200, body: body.into(), retry_after_s: None })
    }

    /// A refresh with the keychain answering `got` and `agy` answering
    /// `version` and then `usage`; returns the programs run and the outcome.
    fn go(os: Os, got: Got, reach: Reach, version: Result<Reply, NetFail>, usage: Result<Reply, NetFail>) -> (Vec<Cli>, crate::Outcome) {
        let dirs = Dirs::sample(os);
        let gots = [got];
        let cx = Ctx { dirs: &dirs, got: &gots, now: NOW, reach };
        let mut answers = vec![usage, version];
        let (asked, ran, out) = drive_runs(&Antigravity, &cx, |_| answers.pop().expect("one answer per program"));
        assert!(asked.is_empty());
        (ran, out)
    }

    fn signed_in() -> Got {
        Got::Text(ITEM.into())
    }

    #[test]
    fn reads_the_limits_of_every_group() {
        let (ran, out) = go(Os::Mac, signed_in(), Reach::Online, ok(VERSION), ok(USAGE));
        assert_eq!(ran, [Cli::AgyVersion, Cli::AgyUsage]);
        let s = out.unwrap();
        assert_eq!(s.provider, ProviderId::Antigravity);
        assert_eq!(s.account.as_deref(), Some("s•••@example.com"));
        let w = &s.windows;
        // The unmetered bucket of the last group is left out.
        assert_eq!(w.len(), 5);
        assert_eq!(w[0].label, "Gemini Models · 5-hour");
        assert!((w[0].used - 0.2).abs() < 1e-6 && w[0].kind == WindowKind::Session && w[0].resets_at == parse_time("2026-10-31T12:08:50Z") && w[0].length_s == Some(FIVE_HOURS_S));
        assert_eq!(w[1].label, "Gemini Models · Weekly");
        assert!((w[1].used - 0.45).abs() < 1e-6 && w[1].kind == WindowKind::Weekly && w[1].resets_at == parse_time("2026-11-07T06:19:11Z") && w[1].length_s == Some(WEEK_S));
        assert_eq!(w[2].label, "Claude and GPT models · 5-hour");
        assert!(w[2].used.abs() < 1e-6);
        assert_eq!(w[3].label, "Claude and GPT models · Weekly");
        assert!((w[3].used - 0.75).abs() < 1e-6);
        assert_eq!(w[4].label, "Gemini 4 Argon · Weekly");
        assert!((w[4].used - 0.3).abs() < 1e-6);
        assert_eq!(s.as_of, NOW);
    }

    #[test]
    fn nobody_signed_in() {
        assert_eq!(go(Os::Mac, Got::Missing, Reach::Online, ok(VERSION), ok(USAGE)).1, Err(Trouble::NotFound));
        assert_eq!(go(Os::Mac, Got::Denied, Reach::Online, ok(VERSION), ok(USAGE)).1, Err(Trouble::Denied));
        // A sign-in with no refresh token: signed out, and nothing is run.
        let (ran, out) = go(Os::Mac, Got::Text(ITEM_NO_REFRESH.into()), Reach::Online, ok(VERSION), ok(USAGE));
        assert!(ran.is_empty());
        assert_eq!(out, Err(Trouble::SignedOut));
        // Something else in the item.
        for junk in ["go-keyring-base64:@@@", "not json", "[]", ""] {
            let out = go(Os::Mac, Got::Text(junk.into()), Reach::Online, ok(VERSION), ok(USAGE)).1;
            assert!(matches!(out, Err(Trouble::Unreadable(_))), "{junk:?}: {out:?}");
        }
    }

    #[test]
    fn a_look_at_this_computer_runs_nothing() {
        let dirs = Dirs::sample(Os::Mac);
        let gots = [signed_in()];
        let cx = Ctx { dirs: &dirs, got: &gots, now: NOW, reach: Reach::Local };
        assert!(matches!(Antigravity.next(&cx, &[]), Step::Done(Err(Trouble::NeedsOnline))));
        // Signed out still shows on a look at this computer.
        let gots = [Got::Text(ITEM_NO_REFRESH.into())];
        let cx = Ctx { dirs: &dirs, got: &gots, now: NOW, reach: Reach::Local };
        assert!(matches!(Antigravity.next(&cx, &[]), Step::Done(Err(Trouble::SignedOut))));
    }

    #[test]
    fn windows_has_no_sign_in_to_read_yet() {
        assert!(Antigravity.wants(&Dirs::sample(Os::Windows)).is_empty());
        let (ran, out) = go(Os::Windows, Got::Missing, Reach::Online, ok(VERSION), ok(USAGE));
        assert!(ran.is_empty());
        assert_eq!(out, Err(Trouble::NotFound));
    }

    #[test]
    fn asks_the_keychain_item_agy_keeps() {
        let w = Antigravity.wants(&Dirs::sample(Os::Mac));
        assert_eq!(w, [Want::Keychain { service: "gemini".into(), account: Some("antigravity".into()) }]);
        assert_eq!(Antigravity.min_gap_s(), 900);
        // It runs a program, so the notices alone never ask it.
        assert!(Antigravity.runs_programs());
    }

    #[test]
    fn an_old_agy_is_not_asked_for_its_limits() {
        // Before 1.1.11 `/usage` goes to the model and spends quota.
        let (ran, out) = go(Os::Mac, signed_in(), Reach::Online, ok("1.1.10\n"), ok(USAGE));
        assert_eq!(ran, [Cli::AgyVersion]);
        assert!(matches!(out, Err(Trouble::Unreadable(ref t)) if t.contains("1.1.11") && t.contains("1.1.10")), "{out:?}");
        let (ran, out) = go(Os::Mac, signed_in(), Reach::Online, ok("agy version 1.1.11"), ok(USAGE));
        assert_eq!(ran, [Cli::AgyVersion, Cli::AgyUsage]);
        assert!(out.is_ok());
        assert!(matches!(go(Os::Mac, signed_in(), Reach::Online, ok("no digits"), ok(USAGE)).1, Err(Trouble::Unreadable(_))));
    }

    #[test]
    fn agy_not_there_or_slow() {
        // Not installed: only its IDE signed in.
        let (ran, out) = go(Os::Mac, signed_in(), Reach::Online, Err(NetFail::Other("no program".into())), ok(USAGE));
        assert_eq!((ran.len(), out), (1, Err(Trouble::NotFound)));
        let out = go(Os::Mac, signed_in(), Reach::Online, ok(VERSION), Err(NetFail::Timeout)).1;
        assert!(matches!(out, Err(Trouble::Unreadable(ref t)) if t.contains("in time")), "{out:?}");
        // Asks for a new sign-in: the app stopped it; no limits, signed out.
        let needs = Ok(Reply { status: 401, body: String::new(), retry_after_s: None });
        assert_eq!(go(Os::Mac, signed_in(), Reach::Online, ok(VERSION), needs).1, Err(Trouble::SignedOut));
        let failed = Ok(Reply { status: 500, body: String::new(), retry_after_s: None });
        assert!(matches!(go(Os::Mac, signed_in(), Reach::Online, ok(VERSION), failed).1, Err(Trouble::Unreadable(_))));
    }

    #[test]
    fn a_reply_that_is_not_the_limits_is_not_read() {
        // A model turn: `/usage` went to the model.
        let out = go(Os::Mac, signed_in(), Reach::Online, ok(VERSION), ok(PROMPT)).1;
        assert!(matches!(out, Err(Trouble::Unreadable(ref t)) if t.contains("prompt")), "{out:?}");
        for body in [EMPTY, "", "plain text", "{\"status\":\"ERROR\",\"error\":\"x\"}"] {
            let out = go(Os::Mac, signed_in(), Reach::Online, ok(VERSION), ok(body)).1;
            assert!(matches!(out, Err(Trouble::Unreadable(_))), "{body:?}: {out:?}");
        }
    }
}
