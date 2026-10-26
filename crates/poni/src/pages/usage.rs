//! The AI tab: how much of each AI coding plan is left, one card per
//! service found on this computer (`poni_core::usage`, read by
//! `services::usage` into `st.usage`). A placeholder until its page lands.

use crate::pages::{empty_state, PageKind};
use crate::state::AppState;
use crate::ui::Ui;
use poni_core::geom::Rect;

/// The placeholder's height: Focus's, well under the tallest page.
const HEIGHT: f32 = 144.0;

/// The AI tab's own state while it shows (scroll, the field being typed in, ...).
#[derive(Clone, Debug, Default)]
pub struct UsageUi {}

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

pub fn draw(ui: &mut Ui, _st: &mut AppState, area: Rect) {
    empty_state(ui, area, PageKind::Ai.icon(), "AI usage is coming", "How much of each AI plan you have left, in one place");
}
