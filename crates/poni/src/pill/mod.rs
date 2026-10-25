//! The Pill theme: the island floats a little below the top edge as a black
//! capsule instead of hanging from it, and the theme is its layouts as much
//! as its look. At rest it shows the time and the battery; while music plays,
//! the song (and the line being sung); open, every page sits on a rounded
//! card. Everything else (pages' behaviour, services, settings) is shared
//! with the other themes.
//!
//! How it plugs in: `poni_core::look::PILL` has `Shape::Pill`, the island
//! floats [`TOP_GAP`] below the edge (`Island::set_floating`), [`shape`]
//! draws the outline, and the hooks below let each surface lay itself out
//! and draw the Pill way. A hook that returns `None` / `false` leaves that
//! surface to the Classic code, so every surface can move over on its own.
//!
//! One module per surface: [`idle`] (the resting pill), [`status`] (the
//! resting pill under the pointer), [`media`] (music, collapsed), [`lyric`]
//! (the line being sung, for the music pill), [`live`] (timer, prayer,
//! game), [`hud`] (volume, charging, Bluetooth), [`notice`], [`expanded`]
//! (the open card's frame and header), [`music`] (the open Music page) and
//! [`motion`] (how the floating shape moves).

pub mod expanded;
pub mod hud;
pub mod idle;
pub mod live;
pub mod lyric;
pub mod media;
pub mod motion;
pub mod music;
pub mod notice;
pub mod shape;
pub mod status;

use crate::activities::{Activity, Mode};
use crate::island::Geom;
use crate::state::AppState;
use crate::ui::Ui;
use poni_core::geom::Rect;
use poni_core::settings::Theme;

/// The island floats this far below the screen's top edge (DIPs).
pub const TOP_GAP: f32 = 8.0;

pub fn on(theme: Theme) -> bool {
    theme == Theme::Pill
}

/// The island's size for `m` in the Pill theme, or `None` for Classic's.
pub fn geom(m: &Mode, st: &AppState) -> Option<Geom> {
    if !on(st.settings.theme) {
        return None;
    }
    match m {
        Mode::Idle => idle::geom(st),
        Mode::Activity(Activity::Media) => media::geom(st),
        Mode::Activity(a) => live::geom(*a, st),
        Mode::Hud(t) => hud::geom(*t, st),
        Mode::Notice => notice::geom(st),
        Mode::Expanded(p) => expanded::geom(*p, st),
        // Classic's invisible strip, as for the Hidden idle look.
        Mode::Aside => None,
    }
}

/// Draw the collapsed `m` the Pill way. `false` = draw it as Classic does.
pub fn draw(ui: &mut Ui, st: &mut AppState, m: &Mode, body: Rect) -> bool {
    if !on(st.settings.theme) {
        return false;
    }
    match m {
        Mode::Idle => idle::draw(ui, st, body),
        Mode::Activity(Activity::Media) => media::draw(ui, st, body),
        Mode::Activity(a) => live::draw(ui, st, *a, body),
        Mode::Hud(t) => hud::draw(ui, st, *t, body),
        Mode::Notice => notice::draw(ui, st, body),
        Mode::Expanded(_) | Mode::Aside => false,
    }
}

/// Whether the collapsed `m` needs ambient frames in the Pill theme
/// (`None` = as in Classic).
pub fn is_live(m: &Mode, st: &AppState) -> Option<bool> {
    if !on(st.settings.theme) {
        return None;
    }
    match m {
        Mode::Activity(Activity::Media) => media::is_live(st),
        Mode::Activity(a) => live::is_live(*a, st),
        _ => None,
    }
}

/// The next time the Pill content of `m` changes on its own (a one-shot
/// wake, never a poll), on top of Classic's.
pub fn next_wake(m: &Mode, st: &AppState) -> Option<f64> {
    if !on(st.settings.theme) {
        return None;
    }
    match m {
        Mode::Idle => idle::next_wake(st),
        Mode::Activity(Activity::Media) => media::next_wake(st),
        _ => None,
    }
}
