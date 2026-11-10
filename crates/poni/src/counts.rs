//! Opt-in usage counts, the app's side (docs/usage-counts.md, the rules in
//! `poni_core::counts`). Nothing is counted, kept or sent until the person
//! says yes (`Settings::share_counts`), and saying no deletes what's here.
//!
//! Costs at rest: nothing. There is no thread or timer of its own. A tab is
//! marked as it draws (a comparison, then at most one small write per tab
//! per day), and a finished week's report goes with the next hourly look at
//! the update file, on the add-on thread (`services::update::check_job`).

use crate::commands::Command;
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::services::http::Client;
use crate::state::{AppState, InlineAction, Notice, NoticeAction};
use crate::updates::unix_now;
use crate::version::VERSION;
use poni_core::counts::{month_of, Counts, ASK_AFTER_DAYS, SEND_URL};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OS: &str = if cfg!(target_os = "macos") { "mac" } else { "windows" };
const SEND_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the question stays on the island. It comes once.
const ASK_S: f64 = 30.0;
/// A page already marked isn't looked at again for this long: it draws every
/// frame while something on it moves.
const PAGE_AGAIN_S: f64 = 60.0;

/// The counts in memory while sharing is on.
#[derive(Debug, Default)]
pub struct Tally {
    counts: Option<Counts>,
    page: Option<PageKind>,
    page_again: f64,
}

fn counts_file() -> PathBuf {
    crate::paths::data_dir().join("counts.txt")
}

/// A finished week's report, until it has gone.
fn report_file() -> PathBuf {
    crate::paths::data_dir().join("counts-report.json")
}

fn preview_file() -> PathBuf {
    crate::paths::cache_dir().join("counts-preview.txt")
}

/// When this copy first ran: when its data folder was made.
fn first_ran() -> Option<u64> {
    let made = fs::metadata(crate::paths::data_dir()).and_then(|m| m.created()).ok()?;
    made.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn report(st: &AppState, c: &Counts) -> String {
    let addons: Vec<&str> = poni_core::addons::ALL.iter().map(|a| a.id).filter(|id| st.settings.addon_on(id)).collect();
    c.report(VERSION, OS, first_ran().map(month_of).as_deref(), &addons)
}

fn save(path: &Path, c: &Counts) {
    if let Err(e) = fs::write(path, c.to_text()) {
        crate::pwarn!("counts: couldn't save ({e})");
    }
}

/// Bring the week up to date, then count what `mark` says.
fn update(st: &mut AppState, mark: impl FnOnce(&mut Counts, u64) -> bool) {
    let now = unix_now();
    let mut c = st.tally.counts.take().unwrap_or_else(|| Counts::from_text(&fs::read_to_string(counts_file()).unwrap_or_default()));
    let (done, rolled) = c.roll(now);
    if let Some(done) = done {
        if let Err(e) = fs::write(report_file(), report(st, &done)) {
            crate::pwarn!("counts: couldn't keep the week's report ({e})");
        }
    }
    if mark(&mut c, now) || rolled {
        save(&counts_file(), &c);
    }
    st.tally.counts = Some(c);
}

/// The island shows `page`.
pub fn page(st: &mut AppState, page: PageKind) {
    if !st.settings.share_counts || (st.tally.page == Some(page) && st.now < st.tally.page_again) {
        return;
    }
    st.tally.page = Some(page);
    st.tally.page_again = st.now + PAGE_AGAIN_S;
    update(st, |c, now| c.mark(now, Some(page.key()), None));
}

/// `feature` (`poni_core::counts::FEATURES`) was used.
pub fn feature(st: &mut AppState, feature: &'static str) {
    update(st, |c, now| c.mark(now, None, Some(feature)));
}

/// The report waiting to go with this look at the update file, if any.
pub fn outbox(st: &mut AppState) -> Option<PathBuf> {
    if !st.settings.share_counts {
        return None;
    }
    update(st, |_, _| false);
    Some(report_file()).filter(|p| p.exists())
}

/// On the add-on thread: send the report at `path`. It goes once it has been
/// answered; the next look tries again after a failure.
pub fn send(client: &Client, path: &Path) {
    let Ok(body) = fs::read(path) else { return };
    match client.post_bytes(SEND_URL, &[], &body, "application/json", SEND_TIMEOUT) {
        Ok(r) if r.status < 500 && r.status != 429 => {
            crate::pinfo!("counts: this week's report sent ({})", r.status);
            let _ = fs::remove_file(path);
        }
        Ok(r) => crate::pinfo!("counts: couldn't send the report (HTTP {})", r.status),
        Err(e) => crate::pinfo!("counts: couldn't send the report ({e})"),
    }
}

/// Yes or no, from the question or Settings. No deletes everything kept.
pub fn set_sharing(st: &mut AppState, on: bool) {
    st.settings.share_counts = on;
    st.settings.counts_asked = true;
    st.update.counts_sheet = false;
    if !on {
        st.tally = Tally::default();
        for file in [counts_file(), report_file(), preview_file()] {
            let _ = fs::remove_file(file);
        }
    }
    st.cmd(Command::SaveSettings);
}

/// See what's sent: this week's report so far, as it would go, in a file the
/// system opens.
pub fn preview(st: &mut AppState) {
    let c = if st.settings.share_counts {
        update(st, |_, _| false);
        st.tally.counts.clone().unwrap_or_default()
    } else {
        Counts::default()
    };
    let path = preview_file();
    let text = format!(
        "If you share usage counts, Oyenza Studio sends a report like this once a week.\nIt has no name, account or ID. The numbers are days of that week.\n\n{}\n",
        report(st, &c)
    );
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    match fs::write(&path, text) {
        Ok(()) => st.cmd(Command::OpenPath(path.to_string_lossy().into_owned())),
        Err(e) => crate::pwarn!("counts: couldn't write the preview ({e})"),
    }
}

/// After a look at the update file: put the question on the closed island,
/// once, some days after this copy first ran, when nothing else is showing.
pub fn maybe_ask(st: &mut AppState, expanded: bool) {
    let s = &st.settings;
    if s.counts_asked || s.share_counts || expanded || !st.update.looks || st.notice.is_some() || st.update.offer.is_some() {
        return;
    }
    let age = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(first_ran().unwrap_or(u64::MAX));
    if age < ASK_AFTER_DAYS * 86_400 {
        return;
    }
    st.settings.counts_asked = true;
    st.cmd(Command::SaveSettings);
    st.notice = Some(ask_notice(st.accent, st.now + ASK_S));
}

pub fn ask_notice(color: poni_core::geom::Color, until: f64) -> Notice {
    Notice {
        icon: Icon::Eye,
        color,
        title: "Share usage counts?".into(),
        title_dim: String::new(),
        subtitle: "A weekly count of the tabs you open".into(),
        until,
        action: Some(NoticeAction::OpenCounts),
        inline: Some(InlineAction::Hide),
        reveal: None,
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    use crate::commands::{DownloadCmd, IeltsCmd, MediaCmd, NotesCmd};
    use poni_core::counts::FEATURES;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    fn state(share: bool) -> AppState {
        AppState::new(Settings { share_counts: share, ..Default::default() }, Store::default())
    }

    #[test]
    fn nothing_is_counted_until_the_person_says_yes() {
        let mut st = state(false);
        page(&mut st, PageKind::Media);
        st.cmd(Command::SetPinned(true));
        assert!(st.tally.counts.is_none() && st.tally.page.is_none());
    }

    #[test]
    fn every_counted_command_is_a_listed_feature() {
        let mode = poni_core::ytdlp::Mode::default();
        let counted = [
            Command::Media(MediaCmd::PlayPause),
            Command::Ielts(IeltsCmd::Start),
            Command::Notes(NotesCmd::Start),
            Command::Download(DownloadCmd::Start { url: "https://a.example/b".into(), mode }),
            Command::Record,
            Command::RecordScreen,
            Command::SetPinned(true),
            Command::OpenSettings,
        ];
        for c in &counted {
            let f = c.counted_feature().unwrap_or_else(|| panic!("{c:?} isn't counted"));
            assert!(FEATURES.contains(&f), "{f}");
        }
        for c in [Command::Media(MediaCmd::Prepare), Command::SetPinned(false), Command::Notes(NotesCmd::Stop), Command::SaveSettings] {
            assert_eq!(c.counted_feature(), None, "{c:?}");
        }
    }

    #[test]
    fn the_question_says_no_thanks_and_opens_the_sheet() {
        let n = ask_notice(poni_core::geom::palette::ORANGE, 1.0);
        assert_eq!(n.action, Some(NoticeAction::OpenCounts));
        assert_eq!(n.inline, Some(InlineAction::Hide));
    }

    #[test]
    fn the_server_knows_every_tab() {
        let server = include_str!("../../../server/counts.mjs");
        for p in PageKind::ALL {
            assert!(server.contains(&format!("'{}'", p.key())), "server/counts.mjs doesn't list {}", p.key());
        }
    }
}
