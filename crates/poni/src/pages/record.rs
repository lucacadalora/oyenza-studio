//! Record page: the island as the face of Oyenza Studio Recorder, the
//! separate app (built from Cap) that makes the video (`poni_core::recorder`).
//!
//! Before a recording: what to record (this screen at once, or a window or
//! an area picked on the recorder's own overlay), whether the camera, the
//! microphone and the system sound go in, and Record. During one: its time,
//! Pause or Resume, and Stop, which saves it and opens the recorder's editor.
//! The closed island shows the recording too (`activities`), and the island
//! stays out of the video (`window::CaptureOwner::Recording`).
//!
//! Nothing here runs by itself: while it records, the time repaints once a
//! second through the app's wake timer, and not at all while paused.

use crate::commands::Command;
use crate::gfx::{ts, TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_GAP, PILL_REGULAR, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::recorder::{fmt_length, Phase, Target};

/// Focus's height, so this page never makes the window grow.
const HEIGHT: f32 = 144.0;
/// The choices' row (targets on the left, inputs on the right).
const ROW_H: f32 = PILL_SMALL;
const ROW_Y: f32 = 6.0;
/// Record (or the recording's buttons) under the choices.
const ACTION_Y: f32 = ROW_Y + ROW_H + 20.0;
const ACTION_MIN_W: f32 = 124.0;
/// Segmented targets and input chips: inset, label padding, icon.
const SEG_INSET: f32 = 2.0;
const CELL_PAD: f32 = 12.0;
const CELL_ICON: f32 = 15.0;
const ICON_GAP: f32 = 6.0;
const CHIP_GAP: f32 = 6.0;
/// The recording's dot and time.
const DOT_R: f32 = 6.0;
const TIME_X: f32 = 22.0;
/// The closing line at the page's foot.
const FOOT_H: f32 = 16.0;

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// The page's own changes: the recording's next second (it rests while
/// paused), "Opening the recorder" giving up, an error going away.
pub fn next_wake(st: &AppState) -> Option<f64> {
    let mut next = st.rec_next_second().map(|b| st.now + b);
    for t in [Some(st.rec.opening_until), st.rec.error.as_ref().map(|e| e.1)].into_iter().flatten() {
        if t > st.now {
            next = Some(next.map_or(t, |n| n.min(t)));
        }
    }
    next
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if st.rec_live() {
        draw_recording(ui, st, area);
    } else {
        draw_choices(ui, st, area);
    }
}

// ---------------------------------------------------------------- before a recording

fn draw_choices(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let y = area.y + ROW_Y;
    let targets_right = draw_targets(ui, st, area.x, y);
    draw_inputs(ui, st, targets_right + 16.0, area.right(), y);

    let ay = area.y + ACTION_Y;
    let opening = st.now < st.rec.opening_until;
    let label = "Record";
    let w = ui.pill_width(label, true, false).max(ACTION_MIN_W);
    let r = Rect::new(area.x, ay, w, PILL_REGULAR);
    if opening {
        disabled_pill(ui, r, label, Icon::Record);
    } else if ui.pill(wid("record.start"), r, label, Some(Icon::Record), PillStyle::Primary(palette::RED)).clicked {
        st.cmd(Command::Record);
    }
    let (status, color) = match &st.rec.error {
        Some((text, until)) if st.now < *until => (text.clone(), palette::ORANGE),
        _ if opening => ("Opening the recorder\u{2026}".to_string(), palette::TEXT_DIM),
        _ => (target_hint(st.settings.record.target).to_string(), palette::TEXT_DIM),
    };
    let sx = r.right() + 14.0;
    ui.c.text(&status, Rect::new(sx, ay, (area.right() - sx).max(0.0), PILL_REGULAR), &TextStyle::body().color(color));
    foot(ui, area, "Oyenza Studio Recorder makes the video. The island stays out of it.");
}

fn target_hint(t: Target) -> &'static str {
    match t {
        Target::Screen => "Records the screen the island is on.",
        Target::Window => "Then click the window to record.",
        Target::Area => "Then drag over the part to record.",
    }
}

fn target_icon(t: Target) -> Icon {
    match t {
        Target::Screen => Icon::Screen,
        Target::Window => Icon::AppWindow,
        Target::Area => Icon::Area,
    }
}

/// Screen | Window | Area, as Focus picks its phase. Returns its right edge.
fn draw_targets(ui: &mut Ui, st: &mut AppState, x0: f32, y: f32) -> f32 {
    let label_st = TextStyle::body().weight(Weight::Semibold);
    let inner_h = ROW_H - 2.0 * SEG_INSET;
    let mut x = x0 + SEG_INSET;
    let cells: Vec<(Target, Rect)> = Target::ALL
        .into_iter()
        .map(|t| {
            let w = (CELL_PAD + CELL_ICON + ICON_GAP + ui.c.measure(t.label(), &label_st, 200.0).0 + CELL_PAD).ceil();
            let r = Rect::new(x, y + SEG_INSET, w, inner_h);
            x += w;
            (t, r)
        })
        .collect();
    let track = Rect::new(x0, y, x + SEG_INSET - x0, ROW_H);
    ui.c.fill_rrect(track, ROW_H / 2.0, palette::SURFACE);
    let picked = st.settings.record.target;
    let mut choice = None;
    for (i, (t, r)) in cells.iter().enumerate() {
        let color = if *t == picked {
            ui.c.fill_rrect(*r, inner_h / 2.0, Color::white(0.16));
            palette::TEXT
        } else {
            let resp = ui.button(wid_n("record.target", i as u64), Rect::new(r.x, y, r.w, ROW_H));
            if resp.clicked {
                choice = Some(*t);
            }
            if resp.pressed || resp.hovered {
                ui.c.fill_rrect(*r, inner_h / 2.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
                palette::TEXT
            } else {
                palette::TEXT_DIM
            }
        };
        let ix = r.x + CELL_PAD;
        ui.c.icon(target_icon(*t), Rect::new(ix, r.center().y - CELL_ICON / 2.0, CELL_ICON, CELL_ICON), color);
        let tx = ix + CELL_ICON + ICON_GAP;
        ui.c.text(t.label(), Rect::new(tx, y, (r.right() - tx).max(0.0), ROW_H), &label_st.color(color));
    }
    if let Some(t) = choice {
        st.settings.record.target = t;
        st.cmd(Command::SaveSettings);
    }
    track.right()
}

/// Camera, Mic and Sound, each on or off, right-aligned in `x0..x1`; icons
/// only when the labels don't fit.
fn draw_inputs(ui: &mut Ui, st: &mut AppState, x0: f32, x1: f32, y: f32) {
    let rec = st.settings.record;
    let chips = [(Icon::Camera, "Camera", rec.camera), (Icon::Mic, "Mic", rec.microphone), (Icon::Volume, "Sound", rec.system_audio)];
    let label_st = TextStyle::body().weight(Weight::Semibold);
    let full: Vec<f32> = chips.iter().map(|(_, l, _)| (CELL_PAD + CELL_ICON + ICON_GAP + ui.c.measure(l, &label_st, 200.0).0 + CELL_PAD).ceil()).collect();
    let labels = full.iter().sum::<f32>() + CHIP_GAP * 2.0 <= x1 - x0;
    let widths: Vec<f32> = if labels { full } else { vec![ROW_H + 8.0; 3] };
    let mut x = x1 - widths.iter().sum::<f32>() - CHIP_GAP * 2.0;
    let mut flip = None;
    for (i, ((icon, label, on), w)) in chips.iter().zip(widths).enumerate() {
        let r = Rect::new(x, y, w, ROW_H);
        x += w + CHIP_GAP;
        let resp = ui.button(wid_n("record.input", i as u64), r);
        if resp.clicked {
            flip = Some(i);
        }
        let fill = match (*on, resp.pressed, resp.hovered) {
            (_, true, _) => Color::white(0.22),
            (true, _, true) => Color::white(0.20),
            (true, _, false) => Color::white(0.16),
            (false, _, true) => Color::white(0.12),
            (false, _, false) => palette::SURFACE,
        };
        ui.c.fill_rrect(r, ROW_H / 2.0, fill);
        let color = if *on { palette::TEXT } else { palette::TEXT_DIM };
        let ix = if labels { r.x + CELL_PAD } else { r.center().x - CELL_ICON / 2.0 };
        let ir = Rect::new(ix, r.center().y - CELL_ICON / 2.0, CELL_ICON, CELL_ICON);
        ui.c.icon(*icon, ir, color);
        if !*on {
            // Off: struck through, as Windows marks a muted microphone.
            ui.c.line(Point::new(ir.x, ir.bottom()), Point::new(ir.right(), ir.y), color, 1.6);
        }
        if labels {
            let tx = ix + CELL_ICON + ICON_GAP;
            ui.c.text(label, Rect::new(tx, y, (r.right() - tx).max(0.0), ROW_H), &label_st.color(color));
        }
    }
    if let Some(i) = flip {
        let rec = &mut st.settings.record;
        match i {
            0 => rec.camera = !rec.camera,
            1 => rec.microphone = !rec.microphone,
            _ => rec.system_audio = !rec.system_audio,
        }
        st.cmd(Command::SaveSettings);
    }
}

// ---------------------------------------------------------------- during one

fn draw_recording(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let phase = st.rec.phase.unwrap_or(Phase::Idle);
    let paused = phase == Phase::Paused;
    let y = area.y + ROW_Y;
    let time_h = 40.0;
    let cy = y + time_h / 2.0;
    let dot = Point::new(area.x + DOT_R + 1.0, cy);
    match phase {
        Phase::Recording => ui.c.fill_circle(dot, DOT_R, palette::RED),
        Phase::Paused => ui.c.icon(Icon::Pause, Rect::from_center(dot, 14.0, 14.0), palette::TEXT_DIM),
        _ => ui.c.stroke_circle(dot, DOT_R - 0.8, palette::TEXT_DIM, 1.6),
    }
    let time = fmt_length(st.rec_elapsed());
    let time_color = if phase == Phase::Recording { palette::TEXT } else { palette::TEXT_DIM };
    let time_st = TextStyle::new(ts::DISPLAY).weight(Weight::Semibold).tabular().color(time_color);
    ui.c.text(&time, Rect::new(area.x + TIME_X, y, 160.0, time_h), &time_st);
    let status = match phase {
        Phase::Starting => match st.settings.record.target {
            Target::Screen => "Starting\u{2026}",
            Target::Window => "Click the window to record",
            Target::Area => "Drag over the part to record",
        },
        Phase::Recording => "Recording",
        Phase::Paused => "Paused",
        Phase::Stopping => "Saving\u{2026}",
        Phase::Idle => "",
    };
    ui.c.text(status, Rect::new(area.x + TIME_X, y + time_h + 2.0, 240.0, 18.0), &TextStyle::footnote().color(palette::TEXT_DIM));

    // Stop on the right, Pause or Resume before it.
    let stop = "Stop";
    let sw = ui.pill_width(stop, true, false).max(96.0);
    let stop_r = Rect::new(area.right() - sw, cy - PILL_REGULAR / 2.0, sw, PILL_REGULAR);
    let busy = matches!(phase, Phase::Starting | Phase::Stopping);
    if phase == Phase::Stopping {
        disabled_pill(ui, stop_r, stop, Icon::Stop);
    } else if ui.pill(wid("record.stop"), stop_r, stop, Some(Icon::Stop), PillStyle::Primary(palette::RED)).clicked {
        st.cmd(Command::RecordStop);
    }
    let (label, icon, cmd) = if paused { ("Resume", Icon::Play, Command::RecordResume) } else { ("Pause", Icon::Pause, Command::RecordPause) };
    let pw = ["Pause", "Resume"].iter().map(|l| ui.pill_width(l, true, false)).fold(0.0, f32::max);
    let pause_r = Rect::new(stop_r.x - PILL_GAP - pw, stop_r.y, pw, PILL_REGULAR);
    if busy {
        disabled_pill(ui, pause_r, label, icon);
    } else if ui.pill(wid("record.pause"), pause_r, label, Some(icon), PillStyle::Neutral).clicked {
        st.cmd(cmd);
    }
    foot(ui, area, "Stop saves it and opens the recorder's editor. The island stays out of the video.");
}

fn foot(ui: &mut Ui, area: Rect, text: &str) {
    let r = Rect::new(area.x, area.bottom() - FOOT_H, area.w, FOOT_H);
    ui.c.text(text, r, &TextStyle::footnote().color(palette::TEXT_FAINT));
}

/// A pill that can't be pressed right now (Focus's look for Reset while idle).
fn disabled_pill(ui: &mut Ui, r: Rect, label: &str, icon: Icon) {
    ui.c.fill_rrect(r, r.h / 2.0, palette::SURFACE.mul_alpha(0.6));
    let st = TextStyle::body().weight(Weight::Semibold).color(palette::DISABLED);
    let tw = ui.c.measure(label, &st, r.w).0;
    let (isz, gap) = (14.0, 6.0);
    let x = r.x + (r.w - isz - gap - tw) / 2.0;
    ui.c.icon(icon, Rect::new(x, r.center().y - isz / 2.0, isz, isz), palette::DISABLED);
    ui.c.text(label, Rect::new(x + isz + gap, r.y, tw + 2.0, r.h), &st);
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::recorder::Phase;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    fn state() -> AppState {
        let mut st = AppState::new(Settings::default(), Store { recorder_island: true, ..Store::default() });
        st.recorder = true;
        st.now = 100.0;
        st
    }

    #[test]
    fn the_time_wakes_the_page_once_a_second_only_while_recording() {
        let mut st = state();
        assert_eq!(next_wake(&st), None, "nothing to wait for");
        st.rec.phase = Some(Phase::Recording);
        st.rec.elapsed_ms = 41_250;
        st.rec.at = 99.5;
        let wake = next_wake(&st).unwrap();
        assert!((wake - (st.now + 0.25)).abs() < 1e-6, "41.75 s in: the next second is a quarter away ({wake})");
        st.rec.phase = Some(Phase::Paused);
        assert_eq!(next_wake(&st), None, "paused: the time stands still");
        st.rec.opening_until = 104.0;
        assert_eq!(next_wake(&st), Some(104.0), "Opening the recorder gives up then");
    }

    #[test]
    fn the_page_never_makes_the_window_taller() {
        let st = state();
        assert!(height(&st) <= super::super::content_height(crate::pages::PageKind::System, &st));
    }
}
