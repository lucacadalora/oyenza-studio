//! Notes page, the Meeting notes add-on's (off until switched on; Windows):
//! one click and Oyen listens to a meeting, then the Oyenza account writes
//! its notes (`poni_core::notes`).
//!
//! - Signed out: the Oyenza sign-in card, saying what the notes do.
//! - Signed in without Studio Pro (the server's word, `GET /island/pro`):
//!   what the notes do and that they're part of Pro; nothing records.
//! - Before a meeting: Oyen beside the record button, what he hears (the
//!   microphone and the PC's own sound: a call in Meet, Zoom or Teams, with
//!   no bot joining it), and the notes saved last with Open.
//! - Listening: his head lifts with the voices, the time under him, a bar
//!   for each source, and Stop.
//! - After Stop: the meeting going up, then Oyen writing.
//! - The notes: the title, the summary's first lines, how many action
//!   items, and Open, Copy, the folder and a new meeting.
//! - What went wrong, in plain words, with Try again while the recording
//!   is kept.
//!
//! The page only draws and asks (`Command::Notes`); the app runs the
//! recording and the handing over (`services::notes`). It moves while Oyen
//! listens or writes, and rests otherwise.

use crate::commands::{Command, NotesCmd};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::oyen::{self, Coach as Oyen};
use crate::state::{AppState, OyenzaLink};
use crate::ui::{wid, PillStyle, Ui, PILL_GAP, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::notes::{fmt_length, CaptureIssue, Phase, Problem};

/// Every state is this tall, so the island never jumps between them.
const HEIGHT: f32 = 180.0;
/// Signed out: the Oyenza sign-in card's height.
const SIGNED_OUT_H: f32 = 156.0;
/// Oyen's column, and the gap to the right one.
const LEFT_W: f32 = 150.0;
const COL_GAP: f32 = 18.0;
const HEAD_W: f32 = 84.0;
const HEAD_Y: f32 = 46.0;
const BUTTON_R: f32 = 24.0;
const BUTTON_Y: f32 = 124.0;
/// The level bars' rows.
const BAR_H: f32 = 6.0;
const METER_ROW: f32 = 26.0;
/// Oyen's orange: the page's accent in every theme.
const ACCENT: Color = oyen::ORANGE;
const REC: Color = palette::RED;
const MIC_SETTINGS: &str =
    if cfg!(target_os = "macos") { "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone" } else { "ms-settings:privacy-microphone" };
/// What the computer's own sound is called on this one.
const OWN_SOUND: &str = if cfg!(target_os = "macos") { "the Mac's sound" } else { "the PC's sound" };
const SIGN_IN_TITLE: &str = "Meeting notes by Oyen";
const SIGN_IN_BODY: &str = "One click and Oyen listens to your meeting, Meet, Zoom, Teams or in the room, then writes the notes and the transcript. Sign in to Oyenza to start.";

pub fn height(st: &AppState) -> f32 {
    if signed_in(st) || st.notes.live() {
        HEIGHT
    } else {
        SIGNED_OUT_H
    }
}

/// The time under Oyen moves on each second while he listens.
pub fn next_wake(st: &AppState) -> Option<f64> {
    st.notes.next_second(st.now).map(|b| st.now + b)
}

fn signed_in(st: &AppState) -> bool {
    matches!(st.oyenza.link, OyenzaLink::Connected { .. })
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if !signed_in(st) && !st.notes.live() {
        let card = Rect::new(area.x, area.y, area.w, SIGNED_OUT_H);
        super::oyenza::sign_in_card(ui, st, card, SIGN_IN_TITLE, SIGN_IN_BODY, "notes.sign-in");
        return;
    }
    // Without Pro nothing records (a meeting under way, or kept, still finishes:
    // the server says if it can't write it up).
    if st.studio_pro() == Some(false) && st.notes.phase == Phase::Idle && !st.ui.notes_kept {
        draw_pro(ui, st, Rect::new(area.x, area.y, area.w, HEIGHT));
        return;
    }
    // Fixed layout height: the island's spring may still be opening.
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    let left = Rect::new(area.x, area.y, LEFT_W, area.h);
    let right = Rect::new(area.x + LEFT_W + COL_GAP, area.y, (area.w - LEFT_W - COL_GAP).max(0.0), area.h);
    let phase = st.notes.phase.clone();
    let at = Point::new(left.center().x, left.y + HEAD_Y);
    let what = match &phase {
        Phase::Listening { .. } => Oyen::Listen { level: st.notes.mic.max(st.notes.pc) },
        Phase::Sending { .. } | Phase::Writing { .. } => Oyen::Write,
        Phase::Ready => Oyen::Logged { since: st.ui.notes_saved_at },
        Phase::Failed(_) => Oyen::Sorry { since: f64::NEG_INFINITY },
        Phase::Idle => Oyen::Rest { look: look(ui, at, area) },
    };
    if oyen::coach(ui.c, at, HEAD_W, ui.now, what) {
        ui.request_live();
    }
    draw_button(ui, st, &phase, Point::new(left.center().x, left.y + BUTTON_Y));
    match phase {
        Phase::Idle => draw_idle(ui, st, right),
        Phase::Listening { .. } => draw_listening(ui, st, right),
        Phase::Sending { sent, parts } => draw_sending(ui, right, sent, parts),
        Phase::Writing { since } => draw_writing(ui, st, right, since),
        Phase::Ready => draw_ready(ui, st, right),
        Phase::Failed(p) => draw_failed(ui, st, right, p),
    }
}

/// Signed in without Studio Pro: Oyen, what the notes do, and that they're
/// part of Pro (in early access: there is nothing to buy yet).
fn draw_pro(ui: &mut Ui, st: &AppState, area: Rect) {
    let left = Rect::new(area.x, area.y, LEFT_W, area.h);
    let at = Point::new(left.center().x, left.y + HEAD_Y);
    if oyen::coach(ui.c, at, HEAD_W, ui.now, Oyen::Rest { look: look(ui, at, area) }) {
        ui.request_live();
    }
    let tag = Rect::new(left.center().x - 22.0, left.y + BUTTON_Y - 11.0, 44.0, 22.0);
    ui.c.fill_rrect(tag, 11.0, ACCENT.with_alpha(0.2));
    ui.c.text("PRO", tag, &TextStyle::caption().weight(Weight::Bold).center().color(ACCENT));
    let r = Rect::new(area.x + LEFT_W + COL_GAP, area.y + 4.0, (area.w - LEFT_W - COL_GAP).max(0.0).min(420.0), area.h);
    let y = headline(ui, r, "Meeting notes are part of Studio Pro");
    let body = "One click and Oyen listens to your meeting, Meet, Zoom, Teams or in the room, then writes the notes and the whole transcript. Studio Pro is in early access, so it isn't on this account yet.";
    let y = para(ui, Rect::new(r.x, y + 6.0, r.w, 80.0), body, palette::TEXT_DIM);
    if let OyenzaLink::Connected { account } = &st.oyenza.link {
        para(ui, Rect::new(r.x, y + 10.0, r.w, 20.0), &format!("Signed in as {account}"), palette::TEXT_FAINT);
    }
}

/// Where the pointer is beside Oyen (-1 .. 1) while it's over the page.
fn look(ui: &Ui, at: Point, page: Rect) -> Option<f32> {
    ui.input.mouse.filter(|m| page.contains(*m)).map(|m| ((m.x - at.x) / 220.0).clamp(-1.0, 1.0))
}

/// The round button under Oyen: record (Take notes), Stop while listening,
/// faint while the notes are on their way. Its line under it.
fn draw_button(ui: &mut Ui, st: &mut AppState, phase: &Phase, c: Point) {
    let listening = matches!(phase, Phase::Listening { .. });
    // A kept recording waits for Try again or Throw away before a new one.
    let busy = matches!(phase, Phase::Sending { .. } | Phase::Writing { .. }) || (matches!(phase, Phase::Failed(_)) && st.ui.notes_kept);
    let r = Rect::from_center(c, 2.0 * BUTTON_R, 2.0 * BUTTON_R);
    let resp = if busy { Default::default() } else { ui.button(wid("notes.button"), r) };
    let lift = |col: Color| if resp.pressed { Color::lerp(col, Color::white(1.0), 0.2) } else if resp.hovered { Color::lerp(col, Color::white(1.0), 0.1) } else { col };
    let under = Rect::new(c.x - 70.0, c.y + BUTTON_R + 8.0, 140.0, 18.0);
    if listening {
        let level = st.notes.mic.max(st.notes.pc).clamp(0.0, 1.0);
        ui.c.fill_circle(c, BUTTON_R + 4.0 + 10.0 * level, REC.with_alpha(0.22));
        ui.c.fill_circle(c, BUTTON_R, lift(REC));
        ui.c.icon(Icon::Stop, Rect::from_center(c, 20.0, 20.0), palette::ON_ACCENT);
        let t = fmt_length(st.notes.elapsed(st.now));
        ui.c.text(&t, under, &TextStyle::body().weight(Weight::Semibold).tabular().center().color(REC));
        if resp.clicked {
            st.cmd(Command::Notes(NotesCmd::Stop));
        }
        return;
    }
    let fill = if busy { palette::SURFACE } else { REC };
    ui.c.fill_circle(c, BUTTON_R, lift(fill));
    ui.c.fill_circle(c, 8.0, if busy { palette::TEXT_FAINT } else { palette::ON_ACCENT });
    let line = match phase {
        Phase::Sending { .. } | Phase::Writing { .. } => "Taking notes\u{2026}",
        _ if busy => "Recording kept",
        _ => "Take notes",
    };
    ui.c.text(line, under, &TextStyle::footnote().weight(Weight::Semibold).center().color(if busy { palette::TEXT_FAINT } else { palette::TEXT }));
    if resp.clicked {
        st.cmd(Command::Notes(NotesCmd::Start));
    }
}

fn headline(ui: &mut Ui, r: Rect, text: &str) -> f32 {
    let st = TextStyle::headline().wrap().top();
    let (_, h) = ui.c.measure(text, &st, r.w);
    ui.c.text(text, Rect::new(r.x, r.y, r.w, h + 1.0), &st);
    r.y + h
}

fn para(ui: &mut Ui, r: Rect, text: &str, color: Color) -> f32 {
    let st = TextStyle::footnote().color(color).wrap().top();
    let (_, h) = ui.c.measure(text, &st, r.w);
    ui.c.text(text, Rect::new(r.x, r.y, r.w, h + 1.0), &st);
    r.y + h
}

/// Capsules left to right from (`x`, `y`): each one's click, in order.
fn buttons(ui: &mut Ui, x: f32, y: f32, list: &[(&str, &str, Option<Icon>, PillStyle)]) -> Vec<bool> {
    let mut x = x;
    list.iter()
        .map(|(id, label, icon, style)| {
            let w = ui.pill_width(label, icon.is_some(), true);
            let clicked = ui.pill(wid(id), Rect::new(x, y, w, PILL_SMALL), label, *icon, *style).clicked;
            x += w + PILL_GAP;
            clicked
        })
        .collect()
}

fn draw_idle(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let r = Rect::new(r.x, r.y + 4.0, r.w.min(420.0), r.h);
    let y = headline(ui, r, "Oyen takes your meeting notes");
    let body = if cfg!(target_os = "macos") {
        "He listens to your microphone and your Mac's sound, so a call in Meet, Zoom or Teams is heard with no bot joining. Stop when it's over: the notes and the transcript are saved to Documents \u{203a} Oyenza Studio \u{203a} Notes."
    } else {
        "He listens to your microphone and your PC's sound, so a call in Meet, Zoom or Teams is heard with no bot joining. Stop when it's over: the notes and the transcript are saved to Documents \u{203a} Oyenza Studio \u{203a} Notes."
    };
    let y = para(ui, Rect::new(r.x, y + 6.0, r.w, 80.0), body, palette::TEXT_DIM);
    let foot = Rect::new(r.x, r.bottom() - PILL_SMALL - 6.0, r.w, PILL_SMALL);
    // Only what the line shows: a meeting's transcript can run to thousands of lines.
    let last = st.notes.last.as_ref().map(|l| (l.notes.title.clone(), l.notes.secs));
    match last {
        Some((title, secs)) => {
            let line = format!("Last: {title} \u{b7} {} min", (secs as f32 / 60.0).round().max(1.0));
            let w = ui.pill_width("Open", true, true);
            ui.c.text(&line, Rect::new(foot.x, foot.y, foot.w - w - 10.0, foot.h), &TextStyle::footnote().color(palette::TEXT_DIM));
            if ui.pill(wid("notes.open-last"), Rect::new(foot.right() - w, foot.y, w, PILL_SMALL), "Open", Some(Icon::Open), PillStyle::Utility).clicked {
                st.cmd(Command::Notes(NotesCmd::Open));
            }
        }
        None => {
            para(ui, Rect::new(r.x, (y + 10.0).max(foot.y + 6.0), r.w, 30.0), "Let people know you're taking notes.", palette::TEXT_FAINT);
        }
    }
}

fn draw_listening(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let r = Rect::new(r.x, r.y + 4.0, r.w.min(420.0), r.h);
    let y = headline(ui, r, "Oyen is listening");
    let y = para(ui, Rect::new(r.x, y + 4.0, r.w, 40.0), "Stop when the meeting ends, and he'll write it up.", palette::TEXT_DIM) + 14.0;
    let rows = [("You", "microphone", st.notes.mic_on, st.notes.mic), ("The call", OWN_SOUND, st.notes.pc_on, st.notes.pc)];
    for (i, (name, source, on, level)) in rows.into_iter().enumerate() {
        let ry = y + i as f32 * METER_ROW;
        let label = TextStyle::footnote().weight(Weight::Semibold);
        ui.c.text(name, Rect::new(r.x, ry, 70.0, 16.0), &label);
        let bar = Rect::new(r.x + 74.0, ry + 5.0, (r.w - 74.0).max(0.0), BAR_H);
        if on {
            ui.c.fill_rrect(bar, BAR_H / 2.0, palette::SURFACE);
            // A square-root scale, so quiet speech still moves the bar.
            let w = (bar.w * level.clamp(0.0, 1.0).sqrt()).max(BAR_H);
            ui.c.fill_rrect(Rect::new(bar.x, bar.y, w, bar.h), BAR_H / 2.0, ACCENT);
        } else {
            let off = format!("No {source}");
            ui.c.text(&off, Rect::new(bar.x, ry, bar.w, 16.0), &TextStyle::footnote().color(palette::TEXT_FAINT));
        }
    }
    let foot = y + 2.0 * METER_ROW + 6.0;
    para(ui, Rect::new(r.x, foot, r.w, 30.0), "Let people know you're taking notes.", palette::TEXT_FAINT);
    // The bars follow the voices.
    ui.request_live();
}

fn draw_sending(ui: &mut Ui, r: Rect, sent: u32, parts: u32) {
    let r = Rect::new(r.x, r.y + 4.0, r.w.min(420.0), r.h);
    let y = headline(ui, r, "Sending the meeting to Oyen");
    let line = if parts > 1 { format!("Part {} of {parts}", (sent + 1).min(parts)) } else { "One part".to_string() };
    let y = para(ui, Rect::new(r.x, y + 4.0, r.w, 20.0), &line, palette::TEXT_DIM) + 12.0;
    let bar = Rect::new(r.x, y, r.w, BAR_H);
    ui.c.fill_rrect(bar, BAR_H / 2.0, palette::SURFACE);
    let done = if parts == 0 { 0.0 } else { sent as f32 / parts as f32 };
    ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * done).max(BAR_H), bar.h), BAR_H / 2.0, ACCENT);
    para(ui, Rect::new(r.x, y + 20.0, r.w, 40.0), "You can close the island: the notes come by themselves.", palette::TEXT_FAINT);
}

fn draw_writing(ui: &mut Ui, st: &AppState, r: Rect, since: f64) {
    let r = Rect::new(r.x, r.y + 4.0, r.w.min(420.0), r.h);
    let y = headline(ui, r, "Oyen is writing your notes\u{2026}");
    // About a minute for each 20 minutes of meeting, as the server takes it.
    let mins = (st.notes.secs / 1200.0).ceil().max(1.0) as u32;
    let line = format!("{} so far \u{b7} usually about {mins} min for a meeting this long", fmt_length(st.now - since));
    let y = para(ui, Rect::new(r.x, y + 4.0, r.w, 36.0), &line, palette::TEXT_DIM);
    para(ui, Rect::new(r.x, y + 12.0, r.w, 40.0), "You can close the island: the notes come by themselves.", palette::TEXT_FAINT);
}

fn draw_ready(ui: &mut Ui, st: &mut AppState, r: Rect) {
    // Only what the page shows: a meeting's transcript can run to thousands of lines.
    let Some((title, secs, s, actions, summary)) = st.notes.last.as_ref().map(|l| (l.notes.title.clone(), l.notes.secs, l.started, l.notes.actions.len(), l.notes.summary.iter().take(6).cloned().collect::<Vec<_>>())) else {
        return;
    };
    let r = Rect::new(r.x, r.y + 2.0, r.w.min(440.0), r.h);
    let title_st = TextStyle::headline();
    ui.c.text(&title, Rect::new(r.x, r.y, r.w, 20.0), &title_st);
    let when = format!("{:02}:{:02} \u{b7} {} min \u{b7} {actions} action item{}", s.hour, s.minute, (secs as f32 / 60.0).round().max(1.0), if actions == 1 { "" } else { "s" });
    ui.c.text(&when, Rect::new(r.x, r.y + 22.0, r.w, 15.0), &TextStyle::caption().color(palette::TEXT_DIM));
    // The summary's first lines, as many as fit above the buttons.
    let buttons_y = r.bottom() - PILL_SMALL - 6.0;
    let mut y = r.y + 44.0;
    let body = TextStyle::footnote().wrap().top();
    ui.c.clip(Rect::new(r.x, y, r.w, buttons_y - 8.0 - y));
    for line in &summary {
        let text = format!("\u{2022}  {line}");
        let (_, h) = ui.c.measure(&text, &body, r.w);
        if y + h > buttons_y - 8.0 {
            break;
        }
        ui.c.text(&text, Rect::new(r.x, y, r.w, h + 1.0), &body);
        y += h + 3.0;
    }
    ui.c.unclip();
    let clicks = buttons(
        ui,
        r.x,
        buttons_y,
        &[
            ("notes.open", "Open notes", Some(Icon::Open), PillStyle::Primary(ACCENT)),
            ("notes.copy", "Copy", Some(Icon::Copy), PillStyle::Neutral),
            ("notes.folder", "", Some(Icon::Folder), PillStyle::Utility),
            ("notes.done", "Done", None, PillStyle::Utility),
        ],
    );
    for (clicked, cmd) in clicks.into_iter().zip([NotesCmd::Open, NotesCmd::Copy, NotesCmd::Folder, NotesCmd::Done]) {
        if clicked {
            st.cmd(Command::Notes(cmd));
        }
    }
    if let Some((text, until)) = st.ui.notes_note.clone() {
        if ui.now < until {
            let w = ui.c.measure(&text, &TextStyle::caption(), 200.0).0 + 2.0;
            ui.c.text(&text, Rect::new(r.right() - w, r.y + 22.0, w, 15.0), &TextStyle::caption().color(ACCENT));
        }
    }
}

fn draw_failed(ui: &mut Ui, st: &mut AppState, r: Rect, p: Problem) {
    let r = Rect::new(r.x, r.y + 18.0, r.w.min(420.0), r.h);
    let y = headline(ui, r, p.title());
    let y = para(ui, Rect::new(r.x, y + 4.0, r.w, 60.0), &p.line(), palette::TEXT_DIM) + 14.0;
    let mut list: Vec<(&str, &str, Option<Icon>, PillStyle)> = Vec::new();
    let mut cmds = Vec::new();
    if p == Problem::Capture(CaptureIssue::MicDenied) {
        list.push(("notes.mic-settings", "Open microphone settings", Some(Icon::Open), PillStyle::Primary(ACCENT)));
        cmds.push(Command::OpenUrl(MIC_SETTINGS.into()));
    }
    let kept = st.ui.notes_kept;
    if kept && p.retryable() {
        let label = if p == Problem::Unfinished { "Write it up" } else { "Try again" };
        list.push(("notes.retry", label, None, PillStyle::Primary(ACCENT)));
        cmds.push(Command::Notes(NotesCmd::Retry));
    }
    if kept {
        list.push(("notes.discard", "Throw away", Some(Icon::Trash), PillStyle::Utility));
        cmds.push(Command::Notes(NotesCmd::Discard));
    } else {
        list.push(("notes.done", "OK", None, PillStyle::Utility));
        cmds.push(Command::Notes(NotesCmd::Done));
    }
    for (clicked, cmd) in buttons(ui, r.x, y, &list).into_iter().zip(cmds) {
        if clicked {
            st.cmd(cmd);
        }
    }
}
