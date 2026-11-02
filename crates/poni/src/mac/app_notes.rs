//! Meeting notes on the Mac, as in the Windows app (`../app.rs`): the page
//! and its moments are `poni_core::notes` in `AppState`; the recording and
//! the handing over are the shared `services::notes`, recording the
//! microphone and the Mac's own sound (`mac/services/capture.rs`). A child
//! of `app`, so it reaches the app's state.

use super::App;
use crate::commands::NotesCmd;
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::{paths, services};
use poni_core::geom::palette;
use poni_core::notes::{CaptureIssue, Problem, Reply};

impl App {
    /// Each frame while a meeting is recorded: how loud each source is, for
    /// Oyen's head and the page's bars.
    pub(super) fn notes_tick(&mut self) {
        if let Some(r) = &self.notes_rec {
            (self.st.notes.mic, self.st.notes.pc) = r.levels();
        }
    }

    /// A press on the Notes page.
    pub(super) fn notes_cmd(&mut self, c: NotesCmd) {
        let now = self.st.now;
        match c {
            NotesCmd::Start => self.notes_start(),
            NotesCmd::Stop => self.notes_stop(),
            NotesCmd::Retry => self.notes_send(),
            NotesCmd::Discard => {
                self.notes_take = None;
                self.st.ui.notes_kept = false;
                services::notes::clear_audio(None);
                self.st.notes.reset();
                pinfo!("notes: the kept recording was thrown away");
            }
            NotesCmd::Done => self.st.notes.reset(),
            NotesCmd::Open => {
                if let Some(last) = &self.st.notes.last {
                    services::open(&last.file);
                }
            }
            NotesCmd::Folder => {
                let dir = paths::notes_dir();
                let _ = std::fs::create_dir_all(&dir);
                services::open(&dir.to_string_lossy());
            }
            NotesCmd::Copy => {
                if let Some(last) = &self.st.notes.last {
                    let md = poni_core::notes::markdown(&last.notes, &last.started, false);
                    services::clipboard::set_text(&md);
                    self.st.ui.notes_note = Some(("Copied".into(), now + 2.0));
                }
            }
        }
    }

    /// Take notes: the microphone and the Mac's sound start recording.
    fn notes_start(&mut self) {
        // Studio Pro only: the page shows its Pro card instead of the button.
        if self.notes_rec.is_some() || self.st.notes.live() || self.notes_take.is_some() || self.st.studio_pro() == Some(false) {
            return;
        }
        // Whatever an earlier meeting left (the app closed on it) goes first.
        services::notes::clear_audio(None);
        let now = self.st.now;
        let started = self.st.local;
        match services::notes::Recording::start(&paths::notes_audio_dir(), &started, &self.sink) {
            Ok((rec, mic, pc)) => {
                pinfo!("notes: listening (microphone {}, the Mac's sound {})", if mic { "on" } else { "off" }, if pc { "on" } else { "off" });
                self.notes_started = started;
                self.st.notes.started(now, mic, pc);
                self.notes_rec = Some(rec);
            }
            Err(issue) => {
                pwarn!("notes: no recording ({issue:?})");
                self.st.ui.notes_kept = false;
                self.st.notes.failed(Problem::Capture(issue));
            }
        }
    }

    /// Stop: the meeting is over, its notes get written.
    pub(super) fn notes_stop(&mut self) {
        let Some(rec) = self.notes_rec.take() else { return };
        match rec.stop() {
            Ok(take) if take.secs >= 1.0 && !take.parts.is_empty() => {
                self.notes_take = Some(take);
                self.st.ui.notes_kept = true;
                self.notes_send();
            }
            Ok(take) => {
                let _ = std::fs::remove_dir_all(&take.dir);
                self.st.ui.notes_kept = false;
                self.st.notes.failed(Problem::NothingHeard);
            }
            Err(issue) => {
                self.st.ui.notes_kept = false;
                self.st.notes.failed(Problem::Capture(if issue == CaptureIssue::MicDenied { issue } else { CaptureIssue::Failed }));
            }
        }
    }

    /// Hand the kept meeting over (again, for Try again).
    fn notes_send(&mut self) {
        let Some(take) = self.notes_take.clone() else { return };
        self.st.notes.stopped(take.parts.len() as u32, take.secs);
        let job = services::notes::Job { take, started: self.notes_started, lang: poni_core::notes::Lang::Auto, api: None, out: paths::notes_dir() };
        services::notes::send(job, &self.sink);
    }

    /// The meeting on its way: how far, the notes saved, or what went wrong.
    /// With the island closed, the end of it shows as a notice.
    pub(super) fn notes_reply(&mut self, r: Reply) {
        if !self.st.notes_on() {
            return;
        }
        let now = self.st.now;
        let end = match &r {
            Reply::Saved(s) => Some(("Your notes are ready".to_string(), s.notes.title.clone(), true)),
            Reply::Failed(p) => Some((p.title().to_string(), "Open Notes to try again".to_string(), false)),
            _ => None,
        };
        let saved = matches!(r, Reply::Saved(_));
        self.st.notes.reply(r, now);
        if saved {
            self.notes_take = None;
            self.st.ui.notes_kept = false;
            self.st.ui.notes_saved_at = now;
        }
        if let Some((title, subtitle, good)) = end {
            if !(self.expanded && self.st.page == PageKind::Notes) {
                let color = if good { crate::oyen::ORANGE } else { palette::TEXT_DIM };
                self.st.notify(Icon::Notes, color, title, subtitle, 8.0, Some(crate::state::NoticeAction::OpenPage(PageKind::Notes)));
            }
        }
    }

    /// The recording stopped by itself (the longest meeting taken, or the
    /// disk): what was heard is handed over.
    pub(super) fn notes_stopped(&mut self, issue: CaptureIssue) {
        pwarn!("notes: the recording stopped by itself ({issue:?})");
        self.notes_stop();
    }

    /// A meeting the app closed on before its notes were saved: its
    /// recording waits on the page for Write it up or Throw away.
    pub(super) fn notes_restore(&mut self) {
        if let Some((take, started)) = services::notes::kept() {
            pinfo!("notes: a meeting of {:.0} s from before wasn't written up; it's kept", take.secs);
            self.notes_started = started;
            self.st.notes.secs = take.secs;
            self.notes_take = Some(take);
            self.st.ui.notes_kept = true;
            self.st.notes.failed(Problem::Unfinished);
        }
    }

    /// The add-on switched off: a recording stops, its audio goes, and the
    /// page forgets the meeting.
    pub(super) fn notes_off(&mut self) {
        drop(self.notes_rec.take());
        self.notes_take = None;
        services::notes::clear_audio(None);
        self.st.notes = Default::default();
        self.st.ui.notes_kept = false;
    }
}
