//! Oyen's IELTS coach on the Mac, as in the Windows app (`../app.rs`): the
//! session is `poni_core::ielts::Coach` in `AppState`, and this carries out
//! what it asks for with the shared `services::ielts::Kit` (the calls, and
//! the Mac's microphone and voice: `mac/services/mic.rs`, `voice.rs`). A
//! child of `app`, so it reaches the app's state.

use super::App;
use crate::commands::{IeltsCmd, MediaCmd};
use crate::pages::PageKind;
use crate::services;
use crate::state::OyenzaLink;
use poni_core::ielts::{Call, Effect, Reply};

impl App {
    /// The IELTS page's coach, each frame: told when the page comes and goes
    /// (a tab switch, the island closing, the Mac locked or its displays
    /// asleep, a full-screen app), ticked for its clock, fed the microphone's
    /// level. The worker goes once the page is closed and nothing is in flight.
    pub(super) fn ielts_tick(&mut self) {
        if !self.ielts_live() {
            return;
        }
        let now = self.st.now;
        let signed_in = matches!(self.st.oyenza.link, OyenzaLink::Connected { .. });
        let away = self.away.0 || self.away.1;
        let shown = self.expanded && self.st.page == PageKind::Ielts && self.st.ielts_on() && signed_in && !away && !self.fullscreen_hidden;
        if shown != self.ielts_shown {
            self.ielts_shown = shown;
            let fx = self.st.ielts.show(shown, now);
            self.ielts_do(fx);
        }
        if shown {
            let fx = self.st.ielts.tick(now);
            self.ielts_do(fx);
        }
        if self.ielts.recording() {
            self.st.ielts.meter = self.ielts.level();
        }
        self.ielts.retry_deletes();
        if !shown {
            self.ielts.rest();
        }
        let held = self.st.ielts_holds_island();
        if self.ielts_held && !held {
            self.maybe_schedule_collapse();
        }
        self.ielts_held = held;
    }

    /// The smoke run pictures the island from made-up states: the coach runs
    /// only in the app.
    fn ielts_live(&self) -> bool {
        !self.smoke_run
    }

    /// A press on the IELTS page.
    pub(super) fn ielts_cmd(&mut self, c: IeltsCmd) {
        let now = self.st.now;
        let fx = match c {
            IeltsCmd::Start => self.st.ielts.start(self.st.settings.ielts_spice),
            IeltsCmd::Mic => self.st.ielts.tap(now),
            IeltsCmd::Skip => self.st.ielts.skip(),
            IeltsCmd::Quit => self.st.ielts.quit(now),
            IeltsCmd::Retry => self.st.ielts.retry(now),
        };
        if c == IeltsCmd::Start && self.st.ielts.phase == poni_core::ielts::Phase::Starting {
            // A new session: whatever an earlier one left behind goes first.
            self.ielts.voice.stop();
            services::ielts::clear_files();
        }
        self.ielts_do(fx);
    }

    /// The worker's answer.
    pub(super) fn ielts_reply(&mut self, reply: Reply) {
        let fx = self.st.ielts.reply(reply, self.st.now);
        self.ielts_do(fx);
    }

    /// Oyen's line ended, or couldn't play on. A stopped line's late report
    /// doesn't end the one playing now.
    pub(super) fn ielts_voice_ended(&mut self) {
        if !self.ielts_live() || self.ielts.voice.playing() {
            return;
        }
        let now = self.st.now;
        let failed = self.ielts.voice.failed();
        self.ielts.voice.stop();
        // A line that couldn't play shows its words for their reading time.
        let fx = if failed { self.st.ielts.voice_failed(now) } else { self.st.ielts.voice_ended(now) };
        self.ielts_do(fx);
    }

    /// The microphone stopped mid-answer, or macOS's question was answered no.
    pub(super) fn ielts_mic_failed(&mut self, why: String) {
        pwarn!("ielts: the microphone failed ({why})");
        let issue = self.ielts.mic_failed();
        let fx = self.st.ielts.mic_failed(issue);
        self.ielts_do(fx);
    }

    /// Carry out what the coach asked for, in order; what an effect brings
    /// back from the coach (a line that won't play, a recording that
    /// stopped) runs before the rest.
    fn ielts_do(&mut self, fx: Vec<Effect>) {
        if !self.ielts_live() {
            return;
        }
        let mut todo: std::collections::VecDeque<Effect> = fx.into();
        while let Some(e) = todo.pop_front() {
            let now = self.st.now;
            let more = match e {
                Effect::Call(c) => {
                    self.ielts.call(c, &self.sink);
                    Vec::new()
                }
                Effect::Play(path) => match self.ielts.voice.play(&path, &self.sink) {
                    Ok(()) => Vec::new(),
                    Err(err) => {
                        pwarn!("ielts: a line didn't play ({err})");
                        self.st.ielts.voice_failed(now)
                    }
                },
                Effect::StopVoice => {
                    self.ielts.voice.stop();
                    Vec::new()
                }
                Effect::Record => match self.ielts.record(&self.sink) {
                    Ok(()) => Vec::new(),
                    Err(issue) => self.st.ielts.mic_failed(issue),
                },
                Effect::StopRecord => match self.ielts.stop_record() {
                    Ok((wav, secs)) => self.st.ielts.recorded(wav, secs, now),
                    Err(issue) => self.st.ielts.mic_failed(issue),
                },
                Effect::DropRecord => {
                    self.ielts.drop_record();
                    Vec::new()
                }
                Effect::Delete(path) => {
                    self.ielts.delete(path);
                    Vec::new()
                }
                Effect::PauseMusic => {
                    // Once a session, like the adzan: music played back by hand stays playing.
                    if !self.ielts_music && self.st.media.as_ref().is_some_and(|m| m.is_playing()) {
                        self.st.media_cmd(MediaCmd::PlayPause);
                        self.ielts_music = true;
                    }
                    Vec::new()
                }
                Effect::ResumeMusic => {
                    if std::mem::take(&mut self.ielts_music) && self.st.media.as_ref().is_some_and(|m| !m.is_playing()) {
                        self.st.media_cmd(MediaCmd::PlayPause);
                    }
                    Vec::new()
                }
            };
            for m in more.into_iter().rev() {
                todo.push_front(m);
            }
        }
    }

    /// The add-on switched off, or signed out: the session ends (the server
    /// is told), nothing of it keeps running, and its files go.
    pub(super) fn ielts_off(&mut self) {
        let now = self.st.now;
        // Everything Udahan does but read the ladder again.
        let fx = self.st.ielts.quit(now).into_iter().filter(|e| *e != Effect::Call(Call::Me)).collect();
        self.ielts_do(fx);
        self.ielts.close();
        self.st.ielts = Default::default();
        self.ielts_shown = false;
    }
}
