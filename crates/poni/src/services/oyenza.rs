//! Oyenza (oyenza.app), optional: signing in with a code, then asking for
//! money that arrived or went out by itself, the last 7 days of transactions
//! while the Oyenza page is open, bills due soon, and the steps the phone
//! sent, and saving the spends typed in the island. Words, URLs and parsing
//! live in `poni_core::oyenza`; this file does the HTTP on one worker thread
//! and keeps the key with `services::secret` at `paths::oyenza_link_path()`:
//! sealed with DPAPI on Windows, a file only the Mac account can read on a
//! Mac (which shares this file as is).
//!
//! Nothing runs until the person signs in (Settings or the tray). Then:
//! - money every minute while the PC is in use on mains power, every 3
//!   minutes on battery or battery saver, and not at all while the session is
//!   locked or the display is off (one call as soon as it's back);
//! - the day list every 15 seconds while the Oyenza page is on screen, and
//!   never otherwise; an answer with nothing new is a few bytes;
//! - bills every half hour, and all three again when the Oyenza tab opens;
//! - the steps, only while the Health add-on is on: every 15 minutes while
//!   the PC is in use and battery saver is off, and when the Health tab opens
//!   (at most once in 30 seconds, battery saver or not); a server from before
//!   steps (404) counts as none to show. They go to the island only, never to
//!   disk or the log;
//! - the day for the Today page (`poni_core::agenda`) when it opens (at most
//!   once in 30 seconds) and every 5 minutes while it stays on screen, and
//!   never otherwise; a server without it (404) or with it paused (503) is
//!   asked again only after half an hour. It goes to the island only;
//! - failures back off to 1, 2, 4 … 15 minutes, honouring `Retry-After`;
//! - a 401 means the key was disconnected in oyenza.app (or ran out): the key
//!   is deleted and the island says so once.
//!
//! The thread sleeps on a condition variable between calls, so the island's
//! idle stays at zero CPU. Calls block this thread only (8 s budget; a note,
//! which the server reads with AI, 30 s).

use crate::commands::OyenzaCmd;
use crate::events::{AppEvent, EventSink, OyenzaEvent};
use crate::services::http::{Client, FetchError, Method, MtaGuard, Response};
use poni_core::agenda;
use poni_core::health::HealthSnapshot;
use poni_core::oyenza::{self, Claim, Link};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const USER_AGENT: &str = concat!("OyenzaStudio/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lucacadalora/oyenza-studio)");
const TIMEOUT: Duration = Duration::from_secs(8);
const NOTE_TIMEOUT: Duration = Duration::from_secs(30);
/// Opening the tab again within this time asks for nothing new.
const REFRESH_GAP: Duration = Duration::from_secs(30);
/// A disconnect that couldn't reach the server is retried this often.
const REVOKE_EVERY: Duration = Duration::from_secs(3600);

/// What oyenza.app lists this computer as: the PC's name, or the Mac's as
/// System Settings › General › About shows it.
#[cfg(windows)]
fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

#[cfg(not(windows))]
fn computer_name() -> String {
    crate::services::phonedrop::computer_name().unwrap_or_default()
}

pub struct OyenzaService {
    shared: Arc<Shared>,
}

impl OyenzaService {
    pub fn start(sink: EventSink, activity: Activity) -> std::io::Result<Self> {
        let shared = Arc::new(Shared { q: Mutex::new(Queue { activity, ..Default::default() }), cv: Condvar::new() });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("poni-oyenza".into())
            .spawn(move || worker_main(worker, sink))
            .map_err(|e| std::io::Error::other(format!("could not start the Oyenza thread: {e}")))?;
        Ok(Self { shared })
    }

    /// Queue a command and return at once (UI thread safe).
    pub fn send(&self, cmd: OyenzaCmd) {
        self.shared.lock().cmds.push_back(cmd);
        self.shared.cv.notify_all();
    }

    /// The PC's use and power changed: the worker paces itself by them.
    pub fn set_activity(&self, a: Activity) {
        let mut q = self.shared.lock();
        if q.activity != a {
            q.activity = a;
            drop(q);
            self.shared.cv.notify_all();
        }
    }
}

impl Drop for OyenzaService {
    fn drop(&mut self) {
        // The worker exits at its next wait; a call in flight ends on its own
        // timeout. Never join from the UI thread.
        self.shared.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

/// What paces the calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// Unlocked with the display on.
    pub in_use: bool,
    pub on_ac: bool,
    pub saver: bool,
    /// The Oyenza page is on screen: its day list stays live.
    pub watching: bool,
    /// The Today page is on screen: the day is asked for.
    pub today: bool,
    /// The Health add-on is on: the steps are asked for.
    pub health: bool,
}

#[derive(Default)]
struct Queue {
    cmds: VecDeque<OyenzaCmd>,
    activity: Activity,
    shutdown: bool,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
}

impl Shared {
    /// The queue holds plain data, so a panic elsewhere can't leave it
    /// inconsistent: recover from poisoning instead of panicking.
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.q.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn wait_for<'a>(&self, g: MutexGuard<'a, Queue>, timeout: Option<Duration>) -> MutexGuard<'a, Queue> {
        match timeout {
            Some(d) => match self.cv.wait_timeout(g, d) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            },
            None => self.cv.wait(g).unwrap_or_else(|p| p.into_inner()),
        }
    }
}

// ----------------------------------------------------------------- worker

/// A sign-in code on screen, waiting for the person's approval.
struct Pending {
    device_code: String,
    until: Instant,
    next_ask: Instant,
    every: Duration,
}

struct Worker {
    sink: EventSink,
    client: Option<Client>,
    link: Option<Link>,
    pending: Option<Pending>,
    cursor: Option<String>,
    next_money: Instant,
    next_bills: Instant,
    /// The day list's change number, and the server's today it was read on.
    activity_cursor: Option<String>,
    activity_today: String,
    next_activity: Instant,
    was_watching: bool,
    last_refresh: Option<Instant>,
    next_health: Instant,
    /// When the steps were last asked for: the Health tab opening asks again only after `REFRESH_GAP`.
    health_asked: Option<Instant>,
    /// The Health tab asked: the next look goes ahead even in battery saver, where the timer rests.
    health_page: bool,
    /// What the steps check last answered, so the log says each change once (never the numbers).
    health_said: Option<&'static str>,
    /// The Today page's day: next ask while it's open, when it was last asked
    /// (opening asks again only after `agenda::OPEN_GAP_S`), and whether it
    /// was open at the last look.
    next_day: Instant,
    day_asked: Option<Instant>,
    was_today: bool,
    /// Studio Pro: asked when a Pro add-on's page or Add-ons opens (never
    /// on a timer), and when it was last asked.
    next_pro: Option<Instant>,
    pro_asked: Option<Instant>,
    fails: u32,
    /// A key whose disconnect couldn't reach the server yet.
    revoke: Option<String>,
    next_revoke: Instant,
    was_in_use: bool,
}

fn worker_main(shared: Arc<Shared>, sink: EventSink) {
    let _com = MtaGuard::new();
    let now = Instant::now();
    let mut w = Worker {
        sink,
        client: None,
        link: None,
        pending: None,
        cursor: None,
        next_money: now,
        next_bills: now,
        activity_cursor: None,
        activity_today: String::new(),
        next_activity: now,
        was_watching: false,
        last_refresh: None,
        next_health: now,
        health_asked: None,
        health_page: false,
        health_said: None,
        next_day: now,
        day_asked: None,
        was_today: false,
        next_pro: None,
        pro_asked: None,
        fails: 0,
        revoke: None,
        next_revoke: now,
        was_in_use: true,
    };
    w.restore();
    loop {
        let (cmds, activity) = {
            let mut q = shared.lock();
            loop {
                if q.shutdown {
                    return;
                }
                if !q.cmds.is_empty() {
                    break;
                }
                let now = Instant::now();
                match w.next_due(q.activity) {
                    Some(t) if t <= now => break,
                    Some(t) => q = shared.wait_for(q, Some(t - now)),
                    None => q = shared.wait_for(q, None),
                }
            }
            (q.cmds.drain(..).collect::<Vec<_>>(), q.activity)
        };
        for c in cmds {
            w.run(c);
        }
        w.due(activity);
    }
}

impl Worker {
    fn send(&self, ev: OyenzaEvent) {
        self.sink.send(AppEvent::Oyenza(ev));
    }

    fn client(&mut self) -> Result<&Client, FetchError> {
        if self.client.is_none() {
            self.client = Some(Client::new(USER_AGENT)?);
        }
        Ok(self.client.as_ref().expect("client was just made"))
    }

    fn call(&mut self, method: Method, url: &str, key: Option<&str>, json: Option<&str>, timeout: Duration) -> Result<Response, FetchError> {
        let auth = key.map(|k| format!("Bearer {k}"));
        let headers: Vec<(&str, &str)> = auth.as_deref().map(|a| vec![("Authorization", a)]).unwrap_or_default();
        self.client()?.send(method, url, &headers, json, timeout)
    }

    /// The sign-in saved on this PC, and a disconnect still to tell the server.
    fn restore(&mut self) {
        self.link = crate::services::secret::load(&crate::paths::oyenza_link_path()).and_then(|s| oyenza::parse_link(&s));
        if let Some(l) = &self.link {
            crate::pinfo!("oyenza: signed in (this PC is {})", l.device_name);
            self.send(OyenzaEvent::Restored { account: l.account.clone() });
        }
        self.revoke = crate::services::secret::load(&crate::paths::oyenza_revoke_path()).filter(|k| k.starts_with("oyzd_"));
    }

    /// When the worker next has something to do on its own.
    fn next_due(&mut self, a: Activity) -> Option<Instant> {
        let now = Instant::now();
        if a.in_use && !self.was_in_use {
            // Back from a lock or a dark display: one look at once.
            self.next_money = now;
        }
        self.was_in_use = a.in_use;
        if a.watching && !self.was_watching {
            // The page just opened: its list now.
            self.next_activity = now;
        }
        self.was_watching = a.watching;
        if a.today && !self.was_today {
            // The page just opened: the day now, unless it was just asked for.
            let gap = Duration::from_secs(agenda::OPEN_GAP_S);
            self.next_day = self.next_day.min(self.day_asked.map_or(now, |t| (t + gap).max(now)));
        }
        self.was_today = a.today;
        let mut next: Option<Instant> = None;
        let mut consider = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        if let Some(p) = &self.pending {
            consider(p.next_ask.min(p.until));
        }
        if self.link.is_some() && a.in_use {
            consider(self.next_money);
            consider(self.next_bills);
            // The steps change only when the phone sends them: every 15 minutes is
            // enough, and battery saver stops the timer (the Health tab still asks).
            // With the Health add-on off, never.
            if a.health && (!a.saver || self.health_page) {
                consider(self.next_health);
            }
            if a.watching {
                consider(self.next_activity);
            }
            if a.today {
                consider(self.next_day);
            }
            if let Some(t) = self.next_pro {
                consider(t);
            }
        }
        if self.revoke.is_some() && a.in_use {
            consider(self.next_revoke);
        }
        next
    }

    fn run(&mut self, cmd: OyenzaCmd) {
        match cmd {
            OyenzaCmd::SignIn => self.sign_in(),
            OyenzaCmd::Cancel => {
                if self.pending.take().is_some() {
                    crate::pinfo!("oyenza: sign-in cancelled");
                    self.send(OyenzaEvent::ConnectCancelled);
                }
            }
            OyenzaCmd::Disconnect => self.disconnect(),
            OyenzaCmd::Note(text) => self.note(text),
            OyenzaCmd::Undo(id) => self.undo(id),
            OyenzaCmd::Refresh => {
                let now = Instant::now();
                if self.link.is_some() && self.last_refresh.is_none_or(|t| now - t >= REFRESH_GAP) {
                    self.last_refresh = Some(now);
                    self.next_money = now;
                    self.next_bills = now;
                }
                self.next_activity = now;
            }
            OyenzaCmd::RefreshDay => {
                let now = Instant::now();
                if self.link.is_some() && self.day_asked.is_none_or(|t| now - t >= REFRESH_GAP) {
                    self.next_day = now;
                }
            }
            OyenzaCmd::RefreshHealth => {
                let now = Instant::now();
                if self.link.is_some() && self.health_asked.is_none_or(|t| now - t >= REFRESH_GAP) {
                    self.next_health = now;
                    self.health_page = true;
                }
            }
            OyenzaCmd::CheckPro => {
                let now = Instant::now();
                if self.link.is_some() && self.pro_asked.is_none_or(|t| now - t >= REFRESH_GAP) {
                    self.next_pro = Some(now);
                }
            }
        }
    }

    fn due(&mut self, a: Activity) {
        let now = Instant::now();
        if self.pending.as_ref().is_some_and(|p| now >= p.until) {
            self.pending = None;
            self.send(OyenzaEvent::ConnectFailed("expired".into()));
        } else if self.pending.as_ref().is_some_and(|p| now >= p.next_ask) {
            self.ask_for_key();
        }
        if self.link.is_some() && a.in_use {
            if Instant::now() >= self.next_money {
                self.poll_money(a);
            }
            if self.link.is_some() && Instant::now() >= self.next_bills {
                self.poll_bills();
            }
            if self.link.is_some() && a.watching && Instant::now() >= self.next_activity {
                self.poll_activity();
            }
            if self.link.is_some() && a.health && (!a.saver || self.health_page) && Instant::now() >= self.next_health {
                self.poll_health();
            }
            if self.link.is_some() && a.today && Instant::now() >= self.next_day {
                self.poll_day();
            }
            if self.link.is_some() && self.next_pro.is_some_and(|t| Instant::now() >= t) {
                self.poll_pro();
            }
        }
        if self.revoke.is_some() && a.in_use && Instant::now() >= self.next_revoke {
            self.retry_revoke();
        }
    }

    // ------------------------------------------------------------ signing in

    fn sign_in(&mut self) {
        if let Some(l) = &self.link {
            // Already signed in: say so again.
            self.send(OyenzaEvent::Restored { account: l.account.clone() });
            return;
        }
        let name = oyenza::pc_name(&computer_name());
        let body = oyenza::start_body(&name, env!("CARGO_PKG_VERSION"), oyenza::PLATFORM);
        match self.call(Method::Post, &oyenza::start_url(), None, Some(&body), TIMEOUT) {
            Ok(r) if r.status == 200 => match oyenza::parse_started(&r.body) {
                Some(s) => {
                    let now = Instant::now();
                    let every = Duration::from_secs(s.interval.clamp(2, 30));
                    self.pending =
                        Some(Pending { device_code: s.device_code, until: now + Duration::from_secs(s.expires_in.clamp(30, 1800)), next_ask: now + every, every });
                    crate::pinfo!("oyenza: showing a sign-in code");
                    self.send(OyenzaEvent::Connecting { code: s.user_code, url: s.verify_url_complete, expires_in_s: s.expires_in });
                }
                None => self.sign_in_failed("other", "unreadable answer"),
            },
            Ok(r) => self.sign_in_failed("other", &format!("HTTP {}", r.status)),
            Err(e) => self.sign_in_failed("offline", &e.to_string()),
        }
    }

    fn sign_in_failed(&mut self, code: &str, detail: &str) {
        crate::pwarn!("oyenza: sign-in didn't start ({detail})");
        self.send(OyenzaEvent::ConnectFailed(code.into()));
    }

    fn ask_for_key(&mut self) {
        let Some(device_code) = self.pending.as_ref().map(|p| p.device_code.clone()) else { return };
        let body = oyenza::token_body(&device_code);
        let answer = self.call(Method::Post, &oyenza::token_url(), None, Some(&body), TIMEOUT);
        let now = Instant::now();
        let claim = match &answer {
            Ok(r) => oyenza::parse_claim(r.status, &r.body),
            Err(_) => None,
        };
        match claim {
            Some(Claim::Connected(link)) => {
                self.pending = None;
                if let Err(e) = crate::services::secret::save(&crate::paths::oyenza_link_path(), &oyenza::link_json(&link)) {
                    // Still signed in for this run; it just won't survive a restart.
                    crate::pwarn!("oyenza: cannot save the sign-in ({e})");
                }
                crate::pinfo!("oyenza: signed in (this PC is {})", link.device_name);
                self.send(OyenzaEvent::Connected { account: link.account.clone() });
                self.link = Some(link);
                self.cursor = None;
                self.activity_cursor = None;
                self.fails = 0;
                self.next_money = now;
                self.next_bills = now;
                self.next_activity = now;
                self.next_health = now;
                self.next_day = now;
                self.day_asked = None;
            }
            Some(Claim::Waiting) => self.ask_again(now, false),
            Some(Claim::SlowDown) => self.ask_again(now, true),
            Some(Claim::Expired | Claim::Invalid) => {
                self.pending = None;
                self.send(OyenzaEvent::ConnectFailed("expired".into()));
            }
            Some(Claim::Denied) => {
                self.pending = None;
                crate::pinfo!("oyenza: the sign-in code was refused in oyenza.app");
                self.send(OyenzaEvent::ConnectFailed("denied".into()));
            }
            // A network hiccup or a server error: keep asking until the code runs out.
            None => self.ask_again(now, false),
        }
    }

    fn ask_again(&mut self, now: Instant, slower: bool) {
        if let Some(p) = self.pending.as_mut() {
            if slower {
                p.every += Duration::from_secs(5);
            }
            p.next_ask = now + p.every;
        }
    }

    fn disconnect(&mut self) {
        self.pending = None;
        let Some(link) = self.link.take() else {
            self.send(OyenzaEvent::Disconnected { by_server: false });
            return;
        };
        let told = matches!(self.call(Method::Post, &oyenza::disconnect_url(), Some(&link.key), Some("{}"), TIMEOUT), Ok(r) if r.status == 200 || r.status == 401);
        if !told {
            // Keep the key only to tell the server later; it's no longer used for anything else.
            crate::pwarn!("oyenza: couldn't reach Oyenza to disconnect; will retry");
            let _ = crate::services::secret::save(&crate::paths::oyenza_revoke_path(), &link.key);
            self.revoke = Some(link.key.clone());
            self.next_revoke = Instant::now() + REVOKE_EVERY;
        }
        self.forget();
        crate::pinfo!("oyenza: disconnected from this PC");
        self.send(OyenzaEvent::Disconnected { by_server: false });
    }

    fn retry_revoke(&mut self) {
        let Some(key) = self.revoke.clone() else { return };
        let done = matches!(self.call(Method::Post, &oyenza::disconnect_url(), Some(&key), Some("{}"), TIMEOUT), Ok(r) if r.status == 200 || r.status == 401);
        if done {
            let _ = crate::services::secret::delete(&crate::paths::oyenza_revoke_path());
            self.revoke = None;
            crate::pinfo!("oyenza: the earlier disconnect reached Oyenza");
        } else {
            self.next_revoke = Instant::now() + REVOKE_EVERY;
        }
    }

    /// Drop the sign-in and everything kept from Oyenza.
    fn forget(&mut self) {
        self.link = None;
        self.cursor = None;
        self.activity_cursor = None;
        self.activity_today.clear();
        self.health_asked = None;
        self.health_page = false;
        self.health_said = None;
        self.day_asked = None;
        self.fails = 0;
        if let Err(e) = crate::services::secret::delete(&crate::paths::oyenza_link_path()) {
            crate::pwarn!("oyenza: cannot remove the saved sign-in ({e})");
        }
    }

    /// The server no longer takes the key (disconnected in oyenza.app, a
    /// password reset, 90 days unused).
    fn signed_out_by_server(&mut self) {
        if self.link.is_none() {
            return;
        }
        crate::pinfo!("oyenza: the key was disconnected in Oyenza");
        self.forget();
        self.send(OyenzaEvent::Disconnected { by_server: true });
    }

    // ------------------------------------------------------------ reading

    fn failed(&mut self, what: &str, detail: String, retry_after_s: Option<u64>) -> Instant {
        self.fails = self.fails.saturating_add(1);
        let wait = oyenza::backoff_s(self.fails, retry_after_s);
        crate::pwarn!("oyenza: {what} failed ({detail}); next try in {wait} s");
        Instant::now() + Duration::from_secs(wait)
    }

    fn poll_money(&mut self, a: Activity) {
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else { return };
        let url = oyenza::money_url(self.cursor.as_deref(), &tz_now());
        match self.call(Method::Get, &url, Some(&key), None, TIMEOUT) {
            Ok(r) if r.status == 200 => match oyenza::parse_money(&r.body) {
                Some(feed) => {
                    self.fails = 0;
                    self.cursor = Some(feed.cursor);
                    self.next_money = Instant::now() + Duration::from_secs(oyenza::money_every_s(a.on_ac, a.saver));
                    self.send(OyenzaEvent::Money { arrived: feed.arrived });
                }
                None => self.next_money = self.failed("money check", "unreadable answer".into(), None),
            },
            Ok(r) if r.status == 401 => self.signed_out_by_server(),
            Ok(r) => self.next_money = self.failed("money check", format!("HTTP {}", r.status), r.retry_after_s),
            Err(e) => self.next_money = self.failed("money check", e.to_string(), None),
        }
    }

    /// The page's day list: in full the first time and whenever the space
    /// changed, else only "nothing new". A new day (the PC's midnight) asks
    /// in full again.
    fn poll_activity(&mut self) {
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else { return };
        let today = local_day_now();
        let since = if self.activity_today == today { self.activity_cursor.clone() } else { None };
        let every = Duration::from_secs(oyenza::ACTIVITY_EVERY_S);
        match self.call(Method::Get, &oyenza::activity_url(&tz_now(), since.as_deref()), Some(&key), None, TIMEOUT) {
            Ok(r) if r.status == 200 => match oyenza::parse_activity(&r.body) {
                Some(oyenza::ActivityAnswer::Unchanged { cursor, today }) => {
                    self.fails = 0;
                    self.activity_cursor = Some(cursor);
                    if today != self.activity_today {
                        // The server's day moved on: the list needs its new day.
                        self.activity_cursor = None;
                        self.next_activity = Instant::now();
                    } else {
                        self.next_activity = Instant::now() + every;
                    }
                    self.send(OyenzaEvent::ActivityChecked);
                }
                Some(oyenza::ActivityAnswer::Full(a)) => {
                    self.fails = 0;
                    self.activity_cursor = Some(a.cursor.clone());
                    self.activity_today = a.today.clone();
                    self.next_activity = Instant::now() + every;
                    self.send(OyenzaEvent::Activity(a));
                }
                None => self.next_activity = self.failed("day list", "unreadable answer".into(), None),
            },
            Ok(r) if r.status == 401 => self.signed_out_by_server(),
            Ok(r) => self.next_activity = self.failed("day list", format!("HTTP {}", r.status), r.retry_after_s),
            Err(e) => self.next_activity = self.failed("day list", e.to_string(), None),
        }
    }

    fn poll_bills(&mut self) {
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else { return };
        match self.call(Method::Get, &oyenza::bills_url(&tz_now()), Some(&key), None, TIMEOUT) {
            Ok(r) if r.status == 200 => match oyenza::parse_bills(&r.body) {
                Some(feed) => {
                    self.next_bills = Instant::now() + Duration::from_secs(oyenza::BILLS_EVERY_S);
                    self.send(OyenzaEvent::Bills { today: feed.today, bills: feed.bills });
                }
                None => self.next_bills = self.failed("bills check", "unreadable answer".into(), None),
            },
            Ok(r) if r.status == 401 => self.signed_out_by_server(),
            Ok(r) => self.next_bills = self.failed("bills check", format!("HTTP {}", r.status), r.retry_after_s),
            Err(e) => self.next_bills = self.failed("bills check", e.to_string(), None),
        }
    }

    /// The steps the phone sent, for the Health page (None: none to show).
    fn poll_health(&mut self) {
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else { return };
        self.health_asked = Some(Instant::now());
        self.health_page = false;
        match self.call(Method::Get, &oyenza::health_url(), Some(&key), None, TIMEOUT) {
            Ok(r) if r.status == 200 => match oyenza::parse_health(&r.body) {
                Some(Some(h)) => self.got_health(Some(h), "steps from the phone came in"),
                Some(None) => self.got_health(None, "no steps to show (off in Oyenza, or none sent yet)"),
                None => self.next_health = self.failed("steps check", "unreadable answer".into(), None),
            },
            // A server from before steps came to the PC: none to show, and nothing wrong.
            Ok(r) if r.status == 404 => self.got_health(None, "this server doesn't send steps yet"),
            Ok(r) if r.status == 401 => self.signed_out_by_server(),
            Ok(r) => self.next_health = self.failed("steps check", format!("HTTP {}", r.status), r.retry_after_s),
            Err(e) => self.next_health = self.failed("steps check", e.to_string(), None),
        }
    }

    /// An answer (`said` goes to the log when it changes): the island gets it, and the next look is in 15 minutes.
    fn got_health(&mut self, health: Option<HealthSnapshot>, said: &'static str) {
        self.fails = 0;
        self.next_health = Instant::now() + Duration::from_secs(oyenza::HEALTH_EVERY_S);
        if self.health_said != Some(said) {
            self.health_said = Some(said);
            crate::pinfo!("oyenza: {said}");
        }
        self.send(OyenzaEvent::Health(health));
    }

    /// Whether the account has Studio Pro: asked once a page needs it.
    fn poll_pro(&mut self) {
        self.next_pro = None;
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else { return };
        self.pro_asked = Some(Instant::now());
        match self.call(Method::Get, &oyenza::pro_url(), Some(&key), None, TIMEOUT) {
            Ok(r) if r.status == 200 => match oyenza::parse_pro(&r.body) {
                Some(pro) => self.send(OyenzaEvent::Pro(pro)),
                None => crate::pwarn!("oyenza: Pro check: unreadable answer"),
            },
            // A server from before Studio Pro: nobody has it there.
            Ok(r) if r.status == 404 => self.send(OyenzaEvent::Pro(false)),
            Ok(r) if r.status == 401 => self.signed_out_by_server(),
            Ok(r) => crate::pwarn!("oyenza: Pro check: HTTP {}", r.status),
            Err(e) => crate::pwarn!("oyenza: Pro check: {e}"),
        }
    }

    /// The Today page's day, from the apps switched on in oyenza.app.
    fn poll_day(&mut self) {
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else { return };
        self.day_asked = Some(Instant::now());
        match self.call(Method::Get, &agenda::url(&tz_now(), &local_day_now()), Some(&key), None, TIMEOUT) {
            Ok(r) if r.status == 200 => match agenda::parse(&r.body) {
                Some(day) => {
                    self.fails = 0;
                    self.next_day = Instant::now() + Duration::from_secs(agenda::EVERY_S);
                    self.send(OyenzaEvent::Day(day));
                }
                None => {
                    self.next_day = self.failed("day for Today", "unreadable answer".into(), None);
                    self.send(OyenzaEvent::DayFailed);
                }
            },
            // A server from before the Today page, or with it switched off: the page
            // stays as it was, and asks again later rather than every 5 minutes.
            Ok(r) if r.status == 404 || r.status == 503 => {
                self.next_day = Instant::now() + Duration::from_secs(agenda::UNAVAILABLE_S);
                self.send(OyenzaEvent::DayUnavailable);
            }
            Ok(r) if r.status == 401 => self.signed_out_by_server(),
            Ok(r) => {
                self.next_day = self.failed("day for Today", format!("HTTP {}", r.status), r.retry_after_s);
                self.send(OyenzaEvent::DayFailed);
            }
            Err(e) => {
                self.next_day = self.failed("day for Today", e.to_string(), None);
                self.send(OyenzaEvent::DayFailed);
            }
        }
    }

    // ------------------------------------------------------------ writing

    fn note(&mut self, text: String) {
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else {
            self.send(OyenzaEvent::NoteFailed { text, code: "offline".into() });
            return;
        };
        let body = oyenza::note_body(&text, &tz_now());
        match self.call(Method::Post, &oyenza::note_url(), Some(&key), Some(&body), NOTE_TIMEOUT) {
            Ok(r) if r.status == 200 => match oyenza::parse_saved(&r.body) {
                Some(saved) => {
                    crate::pinfo!("oyenza: {} spend(s) saved", saved.len());
                    self.send(OyenzaEvent::NoteSaved(saved));
                    // The page's list shows it at once.
                    self.next_activity = Instant::now();
                }
                None => self.send(OyenzaEvent::NoteFailed { text, code: "other".into() }),
            },
            Ok(r) if r.status == 401 => {
                self.send(OyenzaEvent::NoteFailed { text, code: "offline".into() });
                self.signed_out_by_server();
            }
            Ok(r) => {
                let code = oyenza::error_code(&r.body).unwrap_or_else(|| "other".into());
                crate::pwarn!("oyenza: note not saved ({code})");
                self.send(OyenzaEvent::NoteFailed { text, code });
            }
            Err(e) => {
                crate::pwarn!("oyenza: note not sent ({e})");
                self.send(OyenzaEvent::NoteFailed { text, code: "offline".into() });
            }
        }
    }

    fn undo(&mut self, id: String) {
        let Some(key) = self.link.as_ref().map(|l| l.key.clone()) else { return };
        match self.call(Method::Delete, &oyenza::undo_url(&id), Some(&key), None, TIMEOUT) {
            Ok(r) if r.status == 200 => {
                self.send(OyenzaEvent::Undone);
                self.next_activity = Instant::now();
            }
            Ok(r) if r.status == 401 => self.signed_out_by_server(),
            Ok(r) => self.send(OyenzaEvent::UndoFailed(oyenza::error_code(&r.body).unwrap_or_else(|| "other".into()))),
            Err(_) => self.send(OyenzaEvent::UndoFailed("offline".into())),
        }
    }
}

/// The local zone as the server reads days in it ("+07:00").
fn tz_now() -> String {
    oyenza::tz_param(oyenza::offset_minutes(&crate::clock::local_now(), crate::clock::unix_now()))
}

/// The PC's date (YYYY-MM-DD), to notice midnight between two looks.
fn local_day_now() -> String {
    let l = crate::clock::local_now();
    format!("{:04}-{:02}-{:02}", l.year, l.month, l.day)
}
