//! The island around a MacBook's camera notch.
//!
//! The notch has no pixels: whatever Poni draws in its middle is never seen.
//! So on a screen with a notch each island state picks one of three fits:
//!
//! * **Inside**: the resting island hides inside the notch (nothing shows;
//!   pointing at the notch still lifts and opens it).
//! * **Ears**: a live activity stays in the menu bar's line, its content
//!   left and right of the camera (the timer's ring on the left, its digits
//!   on the right; album art and the equalizer the same way).
//! * **Below**: notices, pop-ups and the open island hang under the notch:
//!   a strip the notch's height on top, the usual content under it.
//!
//! Screens without a notch (older Macs, external displays) show the island
//! exactly as Windows does.

use crate::activities::{Activity, Mode};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::island::{Geom, SHOULDER};
use crate::oyen;
use crate::state::AppState;
use crate::ui::Ui;
use poni_core::datetime::{fmt_clock, fmt_countdown};
use poni_core::geom::{clamp01, palette, Color, Point, Rect};
use poni_core::pomodoro::Phase;
use poni_core::settings::IdleStyle;

/// The notch as macOS reports it, in DIPs (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Notch {
    pub w: f32,
    pub h: f32,
}

impl Notch {
    /// A 14-inch MacBook Pro's notch at the default scaling (for `--render`).
    pub const MACBOOK: Notch = Notch { w: 185.0, h: 32.0 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Inside,
    Ears,
    Below,
}

/// The narrowest ear a hanging island keeps beside the notch.
const BELOW_EAR: f32 = 24.0;
/// The ears' bottom corner radius.
const EARS_R: f32 = 14.0;
/// The one neutral track under every progress ring (as in activities.rs).
const RING_TRACK: Color = Color::white(0.14);

pub fn fit(m: &Mode, st: &AppState) -> Fit {
    match m {
        Mode::Idle if st.settings.idle_style == IdleStyle::Clock => Fit::Ears,
        Mode::Idle => Fit::Inside,
        Mode::Activity(_) => Fit::Ears,
        Mode::Hud(_) | Mode::Notice | Mode::Expanded(_) => Fit::Below,
    }
}

/// Width of each ear in an ears fit.
fn ear(m: &Mode, st: &AppState) -> f32 {
    match m {
        Mode::Idle => {
            let clock = if st.settings.use_24h { 64.0 } else { 84.0 };
            if oyen::on(st.settings.theme) {
                clock + 6.0
            } else {
                clock
            }
        }
        Mode::Activity(Activity::Timer) => {
            let long = fmt_countdown(st.pomodoro.remaining(st.now)).len() > 5;
            if long {
                80.0
            } else {
                66.0
            }
        }
        Mode::Activity(Activity::MediaTimer) => 96.0,
        _ => 56.0,
    }
}

/// The island's size for `m` around notch `n`; `base` is its size without one.
pub fn geom(m: &Mode, st: &AppState, n: Notch, base: Geom) -> Geom {
    match fit(m, st) {
        // A hair smaller than the notch on every side, so not a pixel of it
        // shows; a larger corner than the notch's own stays hidden too.
        Fit::Inside => Geom::new(n.w - 2.0, n.h - 1.0, n.h / 2.0),
        Fit::Ears => Geom::new(n.w + 2.0 * ear(m, st), n.h, EARS_R),
        Fit::Below => Geom::new(base.w.max(n.w + 2.0 * BELOW_EAR), base.h + n.h, base.r),
    }
}

/// Where the content of `m` draws inside the island's `body`.
pub fn content_body(m: &Mode, st: &AppState, n: Notch, body: Rect) -> Rect {
    match fit(m, st) {
        Fit::Below => Rect::new(body.x, body.y + n.h, body.w, (body.h - n.h).max(0.0)),
        _ => body,
    }
}

/// The shoulders grow in as the island comes out of the notch (inside it,
/// flares would poke out beside the camera).
pub fn shoulder(n: Notch, w: f32) -> f32 {
    SHOULDER * clamp01((w - n.w) / 24.0)
}

/// Draw `m`'s ears content. Returns false when the shared activity drawing
/// already fits (album art and equalizer sit at the island's two ends).
pub fn draw_ears(ui: &mut Ui, st: &mut AppState, m: &Mode, body: Rect, n: Notch) -> bool {
    let ear_w = ((body.w - n.w) / 2.0).max(0.0);
    let left = Rect::new(body.x, body.y, ear_w, body.h);
    let right = Rect::new(body.right() - ear_w, body.y, ear_w, body.h);
    match m {
        Mode::Idle => {
            if oyen::on(st.settings.theme) {
                // Oyen rests in the left ear and watches the cursor.
                let look = ui.input.mouse.map(|p| (p.x - body.center().x) / (body.w / 2.0));
                let (frame, moving) = oyen::rest(st.now, st.local.hour < 5, look);
                if moving {
                    ui.request_anim();
                }
                oyen::peek(ui.c, &frame, left.center().x + 4.0, body.bottom() - 5.0, 22.0, body.y + 1.0);
            }
            let time = fmt_clock(&st.local, st.settings.use_24h);
            let ts = TextStyle::body().weight(Weight::Semibold).tabular().center();
            ui.c.text(&time, right.inset(4.0, 0.0), &ts);
            true
        }
        Mode::Activity(Activity::Timer) => {
            let p = &st.pomodoro;
            let col = match p.phase {
                Phase::Focus => palette::ORANGE,
                Phase::ShortBreak | Phase::LongBreak => palette::GREEN,
            };
            let paused = !p.is_running();
            let c = Point::new(left.center().x + 6.0, body.center().y);
            ui.c.ring(c, 9.0, 2.4, p.progress(st.now), RING_TRACK, col);
            let glyph = Rect::from_center(c, 9.0, 9.0);
            if paused {
                ui.c.icon(Icon::Pause, glyph, palette::TEXT_DIM);
            } else if p.phase != Phase::Focus {
                ui.c.icon(Icon::Coffee, glyph, col);
            }
            let text = fmt_countdown(p.remaining(st.now));
            let ts = TextStyle::body().weight(Weight::Semibold).tabular().color(if paused { palette::TEXT_DIM } else { col }).center();
            ui.c.text(&text, Rect::new(right.x, body.y, right.w - 6.0, body.h), &ts);
            true
        }
        _ => false,
    }
}
