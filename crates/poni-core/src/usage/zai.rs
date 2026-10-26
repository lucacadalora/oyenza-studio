//! Zai: placeholder until its lane lands (reads nothing, finds nothing).

use super::{Ctx, Dirs, NetFail, Provider, ProviderId, Reply, Step, Trouble, Want};

pub struct Zai;

impl Provider for Zai {
    fn id(&self) -> ProviderId {
        ProviderId::Zai
    }

    fn wants(&self, _dirs: &Dirs) -> Vec<Want> {
        Vec::new()
    }

    fn next(&self, _cx: &Ctx, _replies: &[Result<Reply, NetFail>]) -> Step {
        Step::Done(Err(Trouble::NotFound))
    }
}
