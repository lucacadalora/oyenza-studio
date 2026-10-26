//! Heads-up notices from two readings of the same service: a limit getting
//! close, running out, or coming back after a reset. Placeholder until its
//! lane lands (says nothing).

use super::{ProviderId, Snapshot};

/// What the person chose on the AI tab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlertCfg {
    /// Say so once a window passes this much used (0.0..=1.0).
    pub near: f32,
}

impl Default for AlertCfg {
    fn default() -> Self {
        AlertCfg { near: 0.8 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    /// Passed `near` (percent used).
    Near(u8),
    /// All used.
    Out,
    /// Was out or near, and the window reset.
    Back,
}

/// One notice for the island.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub provider: ProviderId,
    pub kind: AlertKind,
    /// "Claude Code: 90% of 5-hour".
    pub title: String,
    /// "Resets 14:20".
    pub subtitle: String,
}

/// The notices `after` deserves, compared with `before` (None: the first
/// reading this run, which never alerts on its own).
pub fn changes(_before: Option<&Snapshot>, _after: &Snapshot, _now: i64, _cfg: &AlertCfg) -> Vec<Alert> {
    Vec::new()
}
