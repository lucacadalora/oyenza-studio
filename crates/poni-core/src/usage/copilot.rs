//! Copilot: placeholder until its lane lands (reads nothing, finds nothing).

use super::{Ctx, Dirs, NetFail, Provider, ProviderId, Reply, Step, Trouble, Want};

pub struct Copilot;

impl Provider for Copilot {
    fn id(&self) -> ProviderId {
        ProviderId::Copilot
    }

    fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
        Vec::new()
    }

    fn next(&self, _cx: &Ctx, _replies: &[Result<Reply, NetFail>]) -> Step {
        Step::Done(Err(Trouble::NotFound))
    }
}
