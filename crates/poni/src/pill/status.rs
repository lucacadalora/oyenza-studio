//! The status bar idle look (`IdleStyle::Status`): a wider resting pill
//! like a status bar (ref4, spec 6.6). The volume on the left, the date and
//! the time in a lighter inner pill, the battery on the right (its
//! percentage and a drawn battery, a bolt inside while it charges).
//!
//! Only state the app already keeps: `st.volume`, `st.power` and
//! `st.local`, each redrawn by the event or the minute tick that changes
//! it. Nothing new is read, polled or animated for it. No Bluetooth glyph:
//! the app keeps no list of connected devices (the service only reports
//! changes, as pop-ups), so there is nothing true to show; and none of the
//! reference's chevrons or bell, which Poni has nothing behind.

use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::island::Geom;
use crate::state::AppState;
use crate::ui::Ui;
use poni_core::datetime::{fmt_clock, fmt_date_short};
use poni_core::geom::{palette, Color, Rect};
use poni_core::settings::{IdleStyle, Theme};
use std::cell::Cell;

/// Every resting pill is this tall, a capsule.
const H: f32 = 32.0;
/// Glyphs (the speaker) in the bar's ink.
const GLYPH: Color = Color::white(0.85);
/// The speaker's box at the left end: its lines fill about 14 x 11 of it, at
/// a 1.5 DIP stroke. The inner pill starts 8 past it.
const VOL_X: f32 = 10.0;
const VOL_BOX: f32 = 18.0;
const VOL_W: f32 = 36.0;
/// The inner pill: the date and the time with a gap, 10 DIPs from its ends.
const CHIP_H: f32 = 22.0;
const CHIP_PAD: f32 = 10.0;
const CHIP_FILL: Color = Color::white(0.14);
const CHIP_RIM: Color = Color::white(0.16);
const DATE_GAP: f32 = 6.0;
/// The battery at the right end: "76%", 6 DIPs, then the battery (body,
/// gap, nub), the nub 14 from the end; 8 between it and the inner pill.
const PCT_GAP: f32 = 8.0;
const BATT_GAP: f32 = 6.0;
const BATT: (f32, f32) = (20.0, 10.0);
const NUB: (f32, f32, f32) = (0.75, 1.75, 4.0);
const BATT_END: f32 = 14.0;
const BATT_W: f32 = BATT.0 + NUB.0 + NUB.1;
/// Ink for the percentage.
const PCT: Color = Color::white(0.55);

thread_local! {
    /// The width the content measured when it was last drawn. The island's
    /// size is asked for without a canvas to measure text with, so it hugs
    /// the last drawing; a drawing that measures a new width (the date at
    /// midnight, a battery coming or going) asks for one more frame and the
    /// island springs to it.
    static WIDTH: Cell<Option<f32>> = const { Cell::new(None) };
}

/// The idle looks a theme offers, in the pickers' order. Status is the Pill
/// theme's own: the other themes draw the clock for it. Diorama rests as
/// its picture, with the time in it (Clock) or without (Minimal), or
/// hidden: it has no eyes.
pub fn looks(theme: Theme) -> &'static [IdleStyle] {
    const ALL: [IdleStyle; 5] = [IdleStyle::Clock, IdleStyle::Eyes, IdleStyle::Minimal, IdleStyle::Hidden, IdleStyle::Status];
    const DIORAMA: [IdleStyle; 3] = [IdleStyle::Clock, IdleStyle::Minimal, IdleStyle::Hidden];
    match theme {
        Theme::Pill => &ALL,
        Theme::Diorama => &DIORAMA,
        _ => &ALL[..4],
    }
}

/// The look `theme` draws for `style`, which a picker marks: outside Pill,
/// Status shows the clock (and comes back when Pill does); in Diorama, Eyes
/// shows the picture alone, as Minimal does (and comes back with another
/// theme).
pub fn shown(theme: Theme, style: IdleStyle) -> IdleStyle {
    match style {
        IdleStyle::Status if theme != Theme::Pill => IdleStyle::Clock,
        IdleStyle::Eyes if theme == Theme::Diorama => IdleStyle::Minimal,
        _ => style,
    }
}

pub fn geom(st: &AppState) -> Option<Geom> {
    let w = WIDTH.with(|w| w.get()).unwrap_or_else(|| estimate(st));
    Some(Geom::new(w, H, H / 2.0))
}

/// A first guess before anything was measured, on the wide side (a
/// stretched bar looks better than a squeezed one for its first frame).
fn estimate(st: &AppState) -> f32 {
    let chars = |s: &str| s.chars().count() as f32 * 7.6;
    let text = chars(&fmt_date_short(&st.local)) + DATE_GAP + chars(&fmt_clock(&st.local, st.settings.use_24h));
    let right = if st.power.has_battery { PCT_GAP + chars("100%") + BATT_GAP + BATT_W + BATT_END } else { end_inset(H) };
    even(left_w(st, H) + text + 2.0 * CHIP_PAD + right)
}

/// A whole number of DIP pairs, so the centred body keeps crisp edges at 100 %.
fn even(w: f32) -> f32 {
    (w / 2.0).ceil() * 2.0
}

/// Where the inner pill ends when nothing sits beside it: concentric with
/// the bar's round end.
fn end_inset(h: f32) -> f32 {
    ((h - CHIP_H) / 2.0).max(0.0)
}

fn left_w(st: &AppState, h: f32) -> f32 {
    if st.volume.known {
        VOL_W
    } else {
        end_inset(h)
    }
}

fn text_style() -> TextStyle {
    TextStyle::body().weight(Weight::Medium).tabular()
}

pub fn draw(ui: &mut Ui, st: &mut AppState, body: Rect) -> bool {
    let cy = body.center().y;
    let ts = text_style();
    let date = fmt_date_short(&st.local);
    let time = fmt_clock(&st.local, st.settings.use_24h);
    let (dw, _) = ui.c.measure(&date, &ts, 400.0);
    let (tw, _) = ui.c.measure(&time, &ts, 400.0);
    let chip_w = (dw + DATE_GAP + tw + 2.0 * CHIP_PAD).ceil();

    // Left: the volume, as the volume pop-up picks its glyph.
    let mut x0 = body.x + left_w(st, body.h);
    if st.volume.known {
        let v = st.volume;
        let icon = if v.muted || v.level <= 0.001 {
            Icon::VolumeMute
        } else if v.level < 0.5 {
            Icon::VolumeLow
        } else {
            Icon::Volume
        };
        ui.c.icon(icon, Rect::new(body.x + VOL_X, cy - VOL_BOX / 2.0, VOL_BOX, VOL_BOX), GLYPH);
    }

    // Right: the battery, its nub BATT_END from the end.
    let mut x1 = body.right() - end_inset(body.h);
    let mut right_w = end_inset(H);
    if st.power.has_battery {
        let p = st.power;
        // Plugged in counts as charging, as the Charging pop-up and the
        // resting clock have it: a full battery on its charger keeps the bolt.
        let charging = p.charging || p.on_ac;
        let pct = format!("{}%", p.percent.min(100));
        // Low, the number turns red with the charge (as the clock's badge
        // does): a sliver of red alone is easy to miss.
        let pct_ts = ts.color(if low(p.percent, charging) { palette::RED } else { PCT });
        let (pw, _) = ui.c.measure(&pct, &pct_ts, 100.0);
        let bx = snap(ui, body.right() - BATT_END - BATT_W);
        let by = snap(ui, cy - BATT.1 / 2.0);
        battery(ui, Rect::new(bx, by, BATT.0, BATT.1), p.percent, charging);
        let tx = bx - BATT_GAP - pw;
        ui.c.text(&pct, Rect::new(tx, body.y, pw + 2.0, body.h), &pct_ts);
        x1 = tx - PCT_GAP;
        right_w = PCT_GAP + pw + BATT_GAP + BATT_W + BATT_END;
    }

    // The inner pill, centred between the two (they sit at the ends, so
    // the hover lift's extra width goes either side of it).
    let room = (x1 - x0).max(0.0);
    let cw = chip_w.min(room);
    x0 += (room - cw) / 2.0;
    let chip = Rect::new(x0, cy - CHIP_H / 2.0, cw, CHIP_H);
    ui.c.fill_rrect(chip, CHIP_H / 2.0, CHIP_FILL);
    // stroke_rrect keeps the stroke inside `chip` itself.
    ui.c.stroke_rrect(chip, CHIP_H / 2.0 - 0.5, CHIP_RIM, 1.0);
    let tx = chip.x + (cw - (dw + DATE_GAP + tw)) / 2.0;
    ui.c.text(&date, Rect::new(tx, body.y, dw + 2.0, body.h), &ts);
    ui.c.text(&time, Rect::new(tx + dw + DATE_GAP, body.y, tw + 2.0, body.h), &ts);

    // Hug what was just measured from the next frame on.
    let want = even(left_w(st, H) + chip_w + right_w);
    if WIDTH.with(|w| w.replace(Some(want))) != Some(want) {
        ui.request_repaint();
    }
    true
}

/// The battery runs low (as Classic's clock badge counts it).
pub(crate) fn low(percent: u8, charging: bool) -> bool {
    percent <= 20 && !charging
}

/// Snap a DIP coordinate to the device-pixel grid (a crisp outline at 100 % and 150 %).
fn snap(ui: &Ui, v: f32) -> f32 {
    let s = ui.c.scale().max(0.1);
    (v * s).round() / s
}

/// A small battery (spec 6.3) in the 20 x 10 body `r`, its nub to the right:
/// the resting pill's own glyph (`pill::idle::battery`, which takes the box
/// with the nub), so every Pill battery is one drawing.
pub(crate) fn battery(ui: &mut Ui, r: Rect, percent: u8, charging: bool) {
    super::idle::battery(ui, Rect::new(r.x, r.y, r.w + NUB.0 + NUB.1, r.h), percent, charging);
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    #[test]
    fn status_is_the_pill_themes_own() {
        assert_eq!(looks(Theme::Pill).last(), Some(&IdleStyle::Status));
        assert_eq!(shown(Theme::Pill, IdleStyle::Status), IdleStyle::Status);
        for theme in [Theme::Classic, Theme::Oyen, Theme::Glass] {
            assert_eq!(looks(theme), &looks(Theme::Pill)[..4], "{theme:?}");
            // They draw the clock for it, so a picker marks Clock.
            assert_eq!(shown(theme, IdleStyle::Status), IdleStyle::Clock, "{theme:?}");
            assert_eq!(shown(theme, IdleStyle::Eyes), IdleStyle::Eyes, "{theme:?}");
        }
    }

    #[test]
    fn diorama_rests_as_its_picture_with_the_time_or_without() {
        assert_eq!(looks(Theme::Diorama), &[IdleStyle::Clock, IdleStyle::Minimal, IdleStyle::Hidden]);
        // Every look a picker offers is one it then marks.
        for style in looks(Theme::Diorama) {
            assert_eq!(shown(Theme::Diorama, *style), *style);
        }
        // Looks left over from another theme show as what the picture does.
        assert_eq!(shown(Theme::Diorama, IdleStyle::Status), IdleStyle::Clock);
        assert_eq!(shown(Theme::Diorama, IdleStyle::Eyes), IdleStyle::Minimal);
    }

    #[test]
    fn the_bar_is_a_capsule_that_makes_room_for_the_battery() {
        let mut st = AppState::new(Settings { theme: Theme::Pill, idle_style: IdleStyle::Status, ..Settings::default() }, Store::default());
        st.volume.known = true;
        st.power.has_battery = false;
        let bare = geom(&st).unwrap();
        assert_eq!((bare.h, bare.r), (32.0, 16.0));
        st.power.has_battery = true;
        let with = geom(&st).unwrap();
        assert!(with.w >= bare.w + PCT_GAP + BATT_GAP + BATT_W, "{} vs {}", with.w, bare.w);
        // Even widths keep the centred body on whole pixels at 100 %.
        assert_eq!((with.w % 2.0, bare.w % 2.0), (0.0, 0.0));
    }
}
