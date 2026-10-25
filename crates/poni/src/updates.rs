//! New versions, the app's side (docs/updates.md): when to read the update
//! file, what to remember between starts (`update.json` in the data folder),
//! and what the island, the menu and Settings show. The rules are in
//! `poni_core::update`, the reading in `services::update`. Both apps keep one
//! [`Updater`] and drive it from their own timers.
//!
//! Costs at rest: one read of a ~1 KB file a couple of minutes after start
//! and then every 12 hours, while the checks are on. A read that comes due
//! while nobody is at the PC (locked, display off) or on battery saver waits
//! until it is back; the app asks [`Updater::due_in`] again then.

use crate::icons::Icon;
use crate::state::{AppState, InlineAction, Notice, NoticeAction};
use crate::version::VERSION;
use poni_core::geom::{palette, Color};
use poni_core::store::{load_json, save_json_atomic};
use poni_core::update::{self, Memory, Release};
use std::path::PathBuf;

/// Which download is this system's.
const MAC: bool = cfg!(target_os = "macos");
/// How long the island shows "Update available" (Hide puts it away sooner)
/// and "Updated to".
const ANNOUNCE_S: f64 = 15.0;
const UPDATED_S: f64 = 10.0;

pub struct Updater {
    path: PathBuf,
    memory: Memory,
    /// When to read the file next (Unix seconds); None while a read runs.
    due: Option<u64>,
    /// "Updated to" waits for the island to be free.
    updated: bool,
}

impl Updater {
    /// At start: what was remembered, the offer it still holds (shown at
    /// once, before any read), and whether to say "Updated to".
    pub fn start(path: PathBuf, st: &mut AppState, now: u64) -> Self {
        let (mut memory, _) = load_json::<Memory>(&path);
        let updated = memory.updated(VERSION);
        if memory.ran != VERSION {
            memory.ran = VERSION.to_string();
            save(&path, &memory);
        }
        let due = Some(memory.next_check(now));
        let u = Updater { path, memory, due, updated };
        u.show(st);
        let latest = u.memory.latest.as_ref().map_or("none", |r| r.version.as_str());
        crate::pinfo!("updates: latest known {latest}, offering {:?}, next look in {} s", st.update.offer.as_ref().map(|r| &r.version), due.unwrap_or(now).saturating_sub(now));
        u
    }

    /// Seconds until the next read, if the checks are on and one is due
    /// (0 = now). None while a read runs.
    pub fn due_in(&self, st: &AppState, now: u64) -> Option<u64> {
        let due = self.due.filter(|_| st.settings.update_check)?;
        Some(due.saturating_sub(now))
    }

    /// The read starts now (on the add-on thread).
    pub fn begin(&mut self) {
        self.due = None;
    }

    /// The file's answer, at `now` (Unix seconds).
    pub fn checked(&mut self, st: &mut AppState, r: Result<Release, String>, now: u64) {
        match r {
            Ok(rel) => {
                self.memory.read(rel, now);
                self.due = Some(now + update::CHECK_EVERY_S);
            }
            // The file is kept as last read; the next look is in an hour.
            Err(_) => self.due = Some(now + update::RETRY_S),
        }
        save(&self.path, &self.memory);
        self.show(st);
    }

    /// The checks were switched on (read in a moment) or off (no more reads,
    /// and the menu and Settings stop offering).
    pub fn switched(&mut self, st: &mut AppState, now: u64) {
        if st.settings.update_check {
            self.due = Some(now + update::SWITCHED_ON_S);
        }
        self.show(st);
    }

    /// The page to open for Update until the app can install it itself: the
    /// DMG on a Mac, the release page on Windows.
    pub fn download_page(&self, st: &AppState) -> Option<String> {
        let rel = st.update.offer.as_ref()?;
        Some(if MAC { rel.download(true)?.url.clone() } else { rel.notes.clone() })
    }

    /// Put a waiting notice on the island if it is `free` (shown, nobody
    /// away, no other notice, closed): "Updated to" first, then a new
    /// version, once per version. True when one went up.
    pub fn announce(&mut self, st: &mut AppState, free: bool) -> bool {
        if !free {
            return false;
        }
        if self.updated {
            self.updated = false;
            let headline = self.memory.latest.as_ref().filter(|r| r.version == VERSION).map(|r| r.headline.clone());
            st.notice = Some(updated_notice(headline, st.now + UPDATED_S));
            return true;
        }
        if !st.settings.update_check || !self.memory.should_announce(VERSION, MAC) {
            return false;
        }
        let Some(rel) = st.update.offer.clone() else { return false };
        crate::pinfo!("updates: telling about {}", rel.version);
        st.notice = Some(new_version_notice(&rel, st.accent, st.now + ANNOUNCE_S));
        self.memory.announced = rel.version;
        save(&self.path, &self.memory);
        true
    }

    /// The offer Settings and the menu show, from what is remembered (none
    /// while the checks are off, and then no sheet either).
    fn show(&self, st: &mut AppState) {
        st.update.offer = self.memory.offer(VERSION, MAC).filter(|_| st.settings.update_check).cloned();
        st.update.sheet &= st.update.offer.is_some();
    }
}

/// "Update available 0.2.5" (the version dimmer) over the release's
/// headline, and Hide. Clicked, it opens the Update sheet.
pub fn new_version_notice(rel: &Release, accent: Color, until: f64) -> Notice {
    Notice {
        icon: Icon::Download,
        color: accent,
        title: "Update available".into(),
        title_dim: rel.version.clone(),
        subtitle: rel.headline.clone(),
        until,
        action: Some(NoticeAction::OpenUpdate),
        inline: Some(InlineAction::Hide),
        reveal: None,
    }
}

/// "Updated to 0.2.5" once, with What's new.
pub fn updated_notice(headline: Option<String>, until: f64) -> Notice {
    Notice {
        icon: Icon::CheckCircle,
        color: palette::GREEN,
        title: format!("Updated to {VERSION}"),
        title_dim: String::new(),
        subtitle: headline.unwrap_or_else(|| "See what's new in this version".into()),
        until,
        action: None,
        inline: Some(InlineAction::WhatsNew),
        reveal: None,
    }
}

fn save(path: &PathBuf, memory: &Memory) {
    if let Err(e) = save_json_atomic(path, memory) {
        crate::pwarn!("updates: couldn't save {} ({e})", path.display());
    }
}

/// Unix seconds now.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
