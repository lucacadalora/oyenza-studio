//! The AI tab's worker: reads how much of each AI coding plan is left
//! (`poni_core::usage`: the service's own files on this computer, then its
//! own server) on one thread, which the Mac app runs too (it shares this
//! file as is).
//!
//! The thread starts the first time the AI tab opens or the person has the
//! heads-up notices on (`Watch`), and reads only while one of them wants it:
//! nothing at rest. A placeholder until its lane lands: the thread sleeps on
//! a condition variable and answers nothing.

use crate::commands::UsageCmd;
use crate::events::EventSink;
use crate::state::InferenceLink;
use poni_core::usage::alerts::Alert;
use poni_core::usage::{Outcome, ProviderId};
use std::collections::BTreeSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// What the island wants from the worker (`App::set_usage_watch`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    /// The AI tab is on screen and someone is at the computer.
    pub open: bool,
    /// The heads-up notices are on, someone is at the computer and battery
    /// saver is off.
    pub notices: bool,
}

/// What the worker tells the island.
// Until the AI tab's worker lands, nothing sends these.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum UsageEvent {
    /// One service's refresh, finished at unix `at`: what it read, or why
    /// there is nothing.
    Reading { id: ProviderId, outcome: Outcome, at: i64 },
    /// The secret names that hold a typed key.
    Keys(BTreeSet<&'static str>),
    /// inference.oyenza.com's link changed.
    Link(InferenceLink),
    /// Heads-up notices for the island (`poni_core::usage::alerts`).
    Alerts(Vec<Alert>),
    /// A refresh started (true) or ended.
    Refreshing(bool),
}

pub struct UsageService {
    shared: Arc<Shared>,
}

impl UsageService {
    pub fn start(sink: EventSink) -> Result<Self, String> {
        let shared = Arc::new(Shared { q: Mutex::new(Queue::default()), cv: Condvar::new() });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("poni-usage".into())
            .spawn(move || worker_main(worker, sink))
            .map_err(|e| format!("could not start the AI usage thread: {e}"))?;
        Ok(Self { shared })
    }

    /// The tab opened or closed, someone came or went, the notices or
    /// battery saver switched (UI thread safe).
    pub fn set_watch(&self, w: Watch) {
        let mut q = self.shared.lock();
        if q.watch != w {
            q.watch = w;
            drop(q);
            self.shared.cv.notify_all();
        }
    }

    /// Something the AI tab asked for (UI thread safe).
    pub fn command(&self, c: UsageCmd) {
        self.shared.lock().cmds.push(c);
        self.shared.cv.notify_all();
    }
}

impl Drop for UsageService {
    fn drop(&mut self) {
        // The worker exits at its next wait. Never join from the UI thread.
        self.shared.lock().shutdown = true;
        self.shared.cv.notify_all();
    }
}

#[derive(Default)]
struct Queue {
    watch: Watch,
    cmds: Vec<UsageCmd>,
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
}

/// The placeholder: asleep at zero CPU until there's something new, which it
/// drops, or until the app quits.
fn worker_main(shared: Arc<Shared>, _sink: EventSink) {
    let mut q = shared.lock();
    while !q.shutdown {
        q.cmds.clear();
        q = shared.cv.wait(q).unwrap_or_else(|p| p.into_inner());
    }
}
