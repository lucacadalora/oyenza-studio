//! IELTS page, the IELTS add-on's (off until switched on): Oyen as a
//! GoPrep-style IELTS Speaking examiner (`poni_core::ielts`).
//!
//! - Signed out: the Oyenza sign-in card, saying what the coach does.
//! - Home: the five levels (open ones can be picked, each with its target
//!   band and the best so far), today's sessions, how Oyen talks (Sopan,
//!   Gaul, Ngegas) and Mulai.
//! - A session: Oyen big on the left with the mic under him (tap to answer,
//!   tap to stop; it stops by itself at the turn's limit), its level and
//!   time; on the right the Part, the line he says, or Part 2's card with
//!   its minute to prepare; "Oyen mikir…" while the answer is on its way;
//!   Lewati and Udahan.
//! - The result: the band, the four criteria with Oyen's notes, his roast
//!   (said out loud), three tips, and the level that opened.
//! - What went wrong, in plain words, with a way on; a blocked microphone
//!   gets a button to Windows' microphone settings.
//! - The IELTS® line under it all.
//!
//! The page only draws and asks (`Command::Ielts`); the app runs the session
//! (`AppState::ielts`), the microphone and the voice. It moves only while
//! Oyen talks, listens, thinks or counts down.

use crate::commands::{Command, IeltsCmd};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::oyen::{self, Coach as Oyen};
use crate::state::{AppState, OyenzaLink};
use crate::ui::{wid, wid_n, PillStyle, Resp, Ui, PILL_GAP, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::ielts::{self, MicIssue, Phase, Problem, Spice};

/// Every state is this tall (under System's 256, the island's tallest), so
/// the island never grows, or jumps between them.
const HEIGHT: f32 = 248.0;
/// Signed out: the Oyenza sign-in card's height.
const SIGNED_OUT_H: f32 = 156.0;
/// Oyen's column, and the gap to the right one.
const LEFT_W: f32 = 168.0;
const COL_GAP: f32 = 18.0;
const HEAD_W: f32 = 96.0;
const MIC_R: f32 = 25.0;
const FOOT_H: f32 = 14.0;
/// The ladder's rows.
const ROW_H: f32 = 30.0;
/// Oyen's orange: his page's accent in every theme.
const ACCENT: Color = oyen::ORANGE;
const REC: Color = palette::RED;
const DISCLAIMER: &str = "IELTS\u{ae} is a trademark of the IELTS partners. Not official IELTS material, not endorsed by them.";
/// Where the microphone switch is: Windows Settings › Privacy & security ›
/// Microphone, or System Settings › Privacy & Security › Microphone.
const MIC_SETTINGS: &str =
    if cfg!(target_os = "macos") { "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone" } else { "ms-settings:privacy-microphone" };
const SIGN_IN_TITLE: &str = "Latihan IELTS Speaking bareng Oyen";
const SIGN_IN_BODY: &str = "Oyen jadi examiner IELTS kamu: nanya Part 1 sampai 3, dengerin jawabanmu, kasih band score, terus nge-roast. Masuk ke Oyenza dulu buat mulai.";

pub fn height(st: &AppState) -> f32 {
    if !signed_in(st) {
        SIGNED_OUT_H
    } else {
        HEIGHT
    }
}

/// The session's next deadline: a line's reading time, Part 2's minute,
/// the recording's limit (the app ticks the coach then).
pub fn next_wake(st: &AppState) -> Option<f64> {
    st.ielts.deadline()
}

fn signed_in(st: &AppState) -> bool {
    matches!(st.oyenza.link, OyenzaLink::Connected { .. })
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if !signed_in(st) {
        let card = Rect::new(area.x, area.y, area.w, SIGNED_OUT_H);
        super::oyenza::sign_in_card(ui, st, card, SIGN_IN_TITLE, SIGN_IN_BODY, "ielts.sign-in");
        return;
    }
    // Fixed layout height: the island's spring may still be opening.
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    let main = Rect::new(area.x, area.y, area.w, area.h - FOOT_H - 6.0);
    let left = Rect::new(main.x, main.y, LEFT_W, main.h);
    let right = Rect::new(main.x + LEFT_W + COL_GAP, main.y, (main.w - LEFT_W - COL_GAP).max(0.0), main.h);
    match st.ielts.phase {
        Phase::Home => draw_home(ui, st, left, right),
        Phase::Result => draw_result(ui, st, left, right),
        Phase::Stuck => draw_stuck(ui, st, left, right),
        _ => draw_session(ui, st, left, right),
    }
    ui.c.text(DISCLAIMER, Rect::new(area.x, area.bottom() - FOOT_H, area.w, FOOT_H), &TextStyle::caption().color(palette::TEXT_FAINT));
}

/// Oyen, `w` wide at `center`; frames keep coming while he moves.
fn head(ui: &mut Ui, center: Point, w: f32, what: Oyen) {
    if oyen::coach(ui.c, center, w, ui.now, what) {
        ui.request_live();
    }
}

/// Where the pointer is beside Oyen (-1 .. 1) while it's over the page.
fn look(ui: &Ui, at: Point, page: Rect) -> Option<f32> {
    ui.input.mouse.filter(|m| page.contains(*m)).map(|m| ((m.x - at.x) / 220.0).clamp(-1.0, 1.0))
}

/// `label` as a capsule ending at `right`; returns the click and its left edge.
fn pill_left_of(ui: &mut Ui, id: &str, right: f32, y: f32, label: &str, icon: Option<Icon>, style: PillStyle) -> (bool, f32) {
    let w = ui.pill_width(label, icon.is_some(), true);
    let r = Rect::new(right - w, y, w, PILL_SMALL);
    (ui.pill(wid(id), r, label, icon, style).clicked, r.x)
}

/// A capsule that can't be pressed now: drawn faint, no click.
fn pill_off(ui: &mut Ui, r: Rect, label: &str) {
    ui.c.fill_rrect(r, r.h / 2.0, palette::SURFACE);
    ui.c.text(label, r, &TextStyle::footnote().weight(Weight::Semibold).color(palette::DISABLED).center());
}

// ---------------------------------------------------------------- home

fn draw_home(ui: &mut Ui, st: &mut AppState, left: Rect, right: Rect) {
    let page = Rect::new(left.x, left.y, right.right() - left.x, left.h);
    let at = Point::new(left.center().x, left.y + 56.0);
    let what = if st.ielts.talking() { Oyen::Talk { since: st.ielts.said_at } } else { Oyen::Rest { look: look(ui, at, page) } };
    head(ui, at, HEAD_W, what);
    ui.c.text("Latihan IELTS", Rect::new(left.x, left.y + 110.0, left.w, 20.0), &TextStyle::headline().center());
    ui.c.text("Oyen jadi examiner kamu", Rect::new(left.x, left.y + 131.0, left.w, 16.0), &TextStyle::footnote().color(palette::TEXT_DIM).center());
    if let Some(me) = &st.ielts.me {
        let today = format!("Hari ini {} dari {} sesi", me.today.used.min(me.today.cap), me.today.cap);
        ui.c.text(&today, Rect::new(left.x, left.y + 151.0, left.w, 15.0), &TextStyle::caption().color(palette::TEXT_FAINT).center());
    }

    match (st.ielts.me.clone(), st.ielts.me_problem) {
        (Some(me), _) => {
            for (i, level) in me.levels.iter().take(ielts::LEVELS as usize).enumerate() {
                let row = Rect::new(right.x - 6.0, right.y + i as f32 * ROW_H, right.w + 6.0, ROW_H - 2.0);
                if ladder_row(ui, st, row, level) {
                    st.ielts.pick = level.n;
                }
            }
        }
        (None, Some(p)) => {
            let r = Rect::new(right.x, right.y + 20.0, right.w, 120.0);
            problem_text(ui, r, p);
            let w = ui.pill_width("Coba lagi", false, true);
            if ui.pill(wid("ielts.home-retry"), Rect::new(right.x, right.y + 110.0, w, PILL_SMALL), "Coba lagi", None, PillStyle::Neutral).clicked {
                st.cmd(Command::Ielts(IeltsCmd::Retry));
            }
        }
        (None, None) => {
            let r = Rect::new(right.x, right.y, right.w, 5.0 * ROW_H);
            ui.c.text("Ngambil level kamu\u{2026}", r, &TextStyle::footnote().color(palette::TEXT_DIM));
        }
    }

    // How Oyen talks, and Mulai.
    let y = right.y + 5.0 * ROW_H + 8.0;
    let names: Vec<&str> = Spice::ALL.iter().map(|s| s.name()).collect();
    let sel = Spice::ALL.iter().position(|s| *s == st.settings.ielts_spice).unwrap_or(2);
    if let Some(i) = super::settings::picker(ui, "ielts.spice", right.x, y + 1.0, 26.0, &names, sel) {
        st.settings.ielts_spice = Spice::ALL[i];
        st.cmd(Command::SaveSettings);
    }
    let label = "Mulai";
    let w = ui.pill_width(label, true, true);
    let go = Rect::new(right.right() - w, y, w, PILL_SMALL);
    if st.ielts.startable(st.ielts.pick) {
        if ui.pill(wid("ielts.start"), go, label, Some(Icon::Mic), PillStyle::Primary(ACCENT)).clicked {
            st.cmd(Command::Ielts(IeltsCmd::Start));
        }
    } else {
        pill_off(ui, go, label);
    }
    // Oyen's own line about a refused Mulai (a capped day, a locked level)
    // takes the two lines under the choice.
    if let Some(said) = st.ielts.say.as_ref().filter(|_| st.ielts.problem.is_some()) {
        let r = Rect::new(right.x, y + 34.0, right.w, 32.0);
        ui.c.clip(r);
        ui.c.text(&said.text, r, &TextStyle::footnote().color(palette::ORANGE).wrap().top());
        ui.c.unclip();
        return;
    }
    let dim = TextStyle::caption().color(palette::TEXT_DIM);
    ui.c.text(st.settings.ielts_spice.line(), Rect::new(right.x, y + 34.0, right.w, 15.0), &dim);
    // Why Mulai can't go, or what the picked level holds.
    let capped = st.ielts.me.as_ref().is_some_and(|m| m.capped());
    let note = match (st.ielts.problem, capped) {
        (Some(p), _) => Some((p.title().to_string(), palette::ORANGE)),
        (None, true) => Some((Problem::DailyCap.title().to_string(), palette::ORANGE)),
        _ => st.ielts.me.as_ref().and_then(|m| m.rung(st.ielts.pick)).map(|l| (l.blurb.clone(), palette::TEXT_FAINT)),
    };
    if let Some((text, color)) = note {
        ui.c.text(&text, Rect::new(right.x, y + 51.0, right.w, 15.0), &TextStyle::caption().color(color));
    }
}

/// One level: its number, name and target, the best band or a lock. Returns
/// true when an open level was clicked.
fn ladder_row(ui: &mut Ui, st: &AppState, r: Rect, level: &ielts::Level) -> bool {
    let picked = st.ielts.pick == level.n;
    let resp = if level.unlocked { ui.button(wid_n("ielts.level", level.n as u64), r) } else { Resp::default() };
    if picked {
        ui.c.fill_rrect(r, 9.0, palette::SURFACE);
    } else if resp.hovered || resp.pressed {
        ui.c.fill_rrect(r, 9.0, Color::white(if resp.pressed { 0.10 } else { 0.05 }));
    }
    let cy = r.center().y;
    let badge = Point::new(r.x + 18.0, cy);
    let (fill, ink) = match (picked, level.unlocked) {
        (true, _) => (ACCENT, palette::ON_ACCENT),
        (false, true) => (Color::white(0.12), palette::TEXT),
        (false, false) => (Color::white(0.06), palette::TEXT_FAINT),
    };
    ui.c.fill_circle(badge, 10.0, fill);
    ui.c.text(&level.n.to_string(), Rect::from_center(badge, 20.0, 20.0), &TextStyle::caption().weight(Weight::Bold).color(ink).center());
    let name_st = TextStyle::body().weight(Weight::Semibold).color(if level.unlocked { palette::TEXT } else { palette::TEXT_FAINT });
    let x = r.x + 36.0;
    let nw = ui.c.measure(&level.name, &name_st, 200.0).0.ceil();
    ui.c.text(&level.name, Rect::new(x, r.y, nw + 2.0, r.h), &name_st);
    let target = format!("target {}", ielts::fmt_band(level.target));
    ui.c.text(&target, Rect::new(x + nw + 8.0, r.y, 90.0, r.h), &TextStyle::caption().color(palette::TEXT_FAINT));
    let right = r.right() - 10.0;
    let side = TextStyle::caption().color(palette::TEXT_DIM).right();
    if level.unlocked {
        let best = level.best.map_or("belum ada skor".to_string(), |b| format!("terbaik {}", ielts::fmt_band(b)));
        ui.c.text(&best, Rect::new(right - 120.0, r.y, 120.0, r.h), &side);
    } else {
        let w = ui.c.measure("kekunci", &side, 120.0).0.ceil();
        ui.c.text("kekunci", Rect::new(right - w - 1.0, r.y, w + 1.0, r.h), &side.color(palette::TEXT_FAINT));
        ui.c.icon(Icon::Lock, Rect::new(right - w - 17.0, cy - 6.5, 13.0, 13.0), palette::TEXT_FAINT);
    }
    resp.clicked
}

// ---------------------------------------------------------------- a session

fn draw_session(ui: &mut Ui, st: &mut AppState, left: Rect, right: Rect) {
    let c = &st.ielts;
    let phase = c.phase;
    let page = Rect::new(left.x, left.y, right.right() - left.x, left.h);
    let at = Point::new(left.center().x, left.y + 54.0);
    let what = match phase {
        // Talking, or one of his thinking lines while the answer is heard.
        Phase::Talking | Phase::Thinking if c.talking() => Oyen::Talk { since: c.said_at },
        Phase::Recording { .. } => Oyen::Listen { level: c.meter },
        Phase::Thinking | Phase::Starting => Oyen::Think,
        _ => Oyen::Rest { look: look(ui, at, page) },
    };
    head(ui, at, HEAD_W, what);

    // The mic under him, and its time.
    let mc = Point::new(left.center().x, left.y + 146.0);
    if draw_mic(ui, st, mc) {
        st.cmd(Command::Ielts(IeltsCmd::Mic));
    }
    let now = ui.now;
    let under = Rect::new(left.x, mc.y + MIC_R + 8.0, left.w, 18.0);
    let tab = TextStyle::footnote().weight(Weight::Semibold).tabular().center();
    match phase {
        Phase::Recording { since, until } => {
            let t = format!("{} / {}", ielts::fmt_secs(now - since), ielts::fmt_secs(until - since));
            ui.c.text(&t, under, &tab.color(REC));
            ui.request_live();
        }
        Phase::Prep { until } => {
            ui.c.text(&ielts::fmt_secs(until - now), Rect::new(under.x, under.y - 2.0, under.w, 22.0), &TextStyle::value().tabular().color(ACCENT).center());
            ui.c.text("waktu persiapan", Rect::new(under.x, under.y + 20.0, under.w, 14.0), &TextStyle::caption().color(palette::TEXT_FAINT).center());
            ui.request_live();
        }
        Phase::Ready => ui.c.text("Ketuk buat jawab", under, &TextStyle::caption().color(palette::TEXT_DIM).center()),
        Phase::Talking if answer_now(st) => ui.c.text("Ketuk buat langsung jawab", under, &TextStyle::caption().color(palette::TEXT_FAINT).center()),
        _ => {}
    }

    // The Part, then the line Oyen says (or Part 2's card).
    let c = &st.ielts;
    let part = c.turn.as_ref().map_or("Intro", ielts::part_label);
    ui.c.text(&part.to_uppercase(), Rect::new(right.x, right.y, 120.0, 16.0), &TextStyle::caption().weight(Weight::Bold).color(ACCENT));
    if let Some(l) = c.me.as_ref().and_then(|m| m.rung(c.level)) {
        let lv = format!("Level {} \u{b7} {}", l.n, l.name);
        ui.c.text(&lv, Rect::new(right.x + 120.0, right.y, right.w - 120.0, 16.0), &TextStyle::caption().color(palette::TEXT_FAINT).right());
    }
    let buttons_y = right.bottom() - PILL_SMALL;
    let card = c.turn.as_ref().and_then(|t| t.cue_card.clone()).filter(|_| matches!(phase, Phase::Prep { .. } | Phase::Ready | Phase::Recording { .. }));
    match (card, c.say.as_ref()) {
        // The card takes the room the heard line has otherwise.
        (Some(card), _) => draw_card(ui, Rect::new(right.x, right.y + 22.0, right.w, buttons_y - 30.0 - (right.y + 22.0)), &card),
        (None, Some(s)) => draw_line(ui, Rect::new(right.x, right.y + 24.0, right.w, 122.0), &s.text),
        (None, None) => {}
    }

    // What he heard, what's happening, and the way out.
    let c = &st.ielts;
    if !c.heard.is_empty() && matches!(phase, Phase::Talking | Phase::Thinking) && body_free(c) {
        let heard = format!("Oyen dengar: \u{201c}{}\u{201d}", c.heard);
        ui.c.text(&heard, Rect::new(right.x, buttons_y - 44.0, right.w, 16.0), &TextStyle::caption().color(palette::TEXT_FAINT));
    }
    // While the answer is heard (7 to 9 s), the dots keep going.
    let thinking = ["Oyen mikir.", "Oyen mikir..", "Oyen mikir..."];
    let status = match phase {
        Phase::Starting => "Oyen siap-siap\u{2026}",
        Phase::Talking => "Oyen lagi ngomong\u{2026}",
        Phase::Prep { .. } => "Siapin jawabanmu. Rekaman mulai sendiri pas waktunya habis.",
        Phase::Ready => c.hint.map_or("Giliran kamu. Ketuk mic buat jawab.", |h| h.line()),
        Phase::Recording { .. } => "Ngerekam\u{2026} ketuk lagi kalau udah.",
        Phase::Thinking => {
            ui.request_live();
            thinking[(now * 2.5) as usize % thinking.len()]
        }
        _ => "",
    };
    let status_st = TextStyle::footnote().color(if c.hint.is_some() && phase == Phase::Ready { palette::ORANGE } else { palette::TEXT_DIM });
    ui.c.text(status, Rect::new(right.x, buttons_y - 26.0, right.w, 18.0), &status_st);
    let skippable = c.id.is_some() && matches!(phase, Phase::Talking | Phase::Prep { .. } | Phase::Ready | Phase::Recording { .. });
    let (quit, x) = pill_left_of(ui, "ielts.quit", right.right(), buttons_y, "Udahan", None, PillStyle::Utility);
    if quit {
        st.cmd(Command::Ielts(IeltsCmd::Quit));
    }
    if skippable && pill_left_of(ui, "ielts.skip", x - PILL_GAP, buttons_y, "Lewati", None, PillStyle::Utility).0 {
        st.cmd(Command::Ielts(IeltsCmd::Skip));
    }
}

/// The person can answer now (Oyen may still be talking).
fn answer_now(st: &AppState) -> bool {
    st.ielts.turn.as_ref().is_some_and(|t| t.wants_answer())
}

/// No card takes the room the heard line sits in.
fn body_free(c: &ielts::Coach) -> bool {
    c.turn.as_ref().is_none_or(|t| t.cue_card.is_none())
}

/// The mic button: answer, or stop the answer (red, with the voice's level
/// around it). Returns true when clicked.
fn draw_mic(ui: &mut Ui, st: &AppState, c: Point) -> bool {
    let phase = st.ielts.phase;
    let rec = matches!(phase, Phase::Recording { .. });
    let can = rec || matches!(phase, Phase::Ready | Phase::Prep { .. }) || (phase == Phase::Talking && answer_now(st));
    let r = Rect::from_center(c, 2.0 * MIC_R, 2.0 * MIC_R);
    let resp = if can { ui.button(wid("ielts.mic"), r) } else { Resp::default() };
    let lift = |col: Color| if resp.pressed { Color::lerp(col, Color::white(1.0), 0.2) } else if resp.hovered { Color::lerp(col, Color::white(1.0), 0.1) } else { col };
    if rec {
        let level = st.ielts.meter.clamp(0.0, 1.0);
        ui.c.fill_circle(c, MIC_R + 4.0 + 10.0 * level, REC.with_alpha(0.22));
        ui.c.fill_circle(c, MIC_R, lift(REC));
        ui.c.icon(Icon::Stop, Rect::from_center(c, 22.0, 22.0), palette::ON_ACCENT);
    } else {
        let fill = match (can, phase) {
            (false, _) => palette::SURFACE,
            (true, Phase::Talking) => ACCENT.with_alpha(0.35),
            (true, _) => ACCENT,
        };
        ui.c.fill_circle(c, MIC_R, lift(fill));
        ui.c.icon(Icon::Mic, Rect::from_center(c, 22.0, 22.0), if can { palette::ON_ACCENT } else { palette::TEXT_FAINT });
    }
    resp.clicked
}

/// What Oyen says: large while it fits in three lines, a size down when longer.
fn draw_line(ui: &mut Ui, r: Rect, text: &str) {
    let big = TextStyle::value().wrap().top();
    let st = if ui.c.measure(text, &big, r.w).1 <= 3.0 * 25.0 { big } else { TextStyle::headline().wrap().top() };
    ui.c.clip(r);
    ui.c.text(text, r, &st);
    ui.c.unclip();
}

/// Part 2's card: the topic, what to cover, and how to round it off.
fn draw_card(ui: &mut Ui, r: Rect, card: &ielts::CueCard) {
    ui.c.fill_rrect(r, 12.0, palette::SURFACE);
    let inner = r.inset(14.0, 10.0);
    ui.c.clip(r);
    let topic = TextStyle::headline().wrap().top();
    let (_, th) = ui.c.measure(&card.topic, &topic, inner.w);
    ui.c.text(&card.topic, Rect::new(inner.x, inner.y, inner.w, th + 1.0), &topic);
    let mut y = inner.y + th + 4.0;
    ui.c.text("You should say:", Rect::new(inner.x, y, inner.w, 15.0), &TextStyle::caption().color(palette::TEXT_FAINT));
    y += 15.0;
    for b in &card.bullets {
        ui.c.text(&format!("\u{2022}  {b}"), Rect::new(inner.x + 4.0, y, inner.w - 4.0, 15.0), &TextStyle::footnote());
        y += 15.0;
    }
    ui.c.text(&card.closing, Rect::new(inner.x, y + 1.0, inner.w, 15.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    ui.c.unclip();
}

// ---------------------------------------------------------------- the result

fn draw_result(ui: &mut Ui, st: &mut AppState, left: Rect, right: Rect) {
    let Some(v) = st.ielts.result.clone() else { return };
    let c = &st.ielts;
    let level = c.me.as_ref().and_then(|m| m.rung(c.level));
    let reached = level.is_some_and(|l| v.band >= l.target);
    ui.c.text("BAND", Rect::new(left.x, left.y, left.w, 14.0), &TextStyle::caption().weight(Weight::Bold).color(palette::TEXT_FAINT).center());
    let band = TextStyle::new(46.0).weight(Weight::Bold).tabular().color(if reached { ACCENT } else { palette::TEXT }).center();
    ui.c.text(&ielts::fmt_band(v.band), Rect::new(left.x, left.y + 14.0, left.w, 54.0), &band);
    if let Some(l) = level {
        let line = format!("Level {} \u{b7} {}", l.n, l.name);
        ui.c.text(&line, Rect::new(left.x, left.y + 70.0, left.w, 16.0), &TextStyle::footnote().color(palette::TEXT_DIM).center());
    }
    if v.level.unlocked {
        let text = format!("Level {} kebuka!", v.level.after);
        let st_b = TextStyle::footnote().weight(Weight::Semibold).color(ACCENT).center();
        let w = ui.c.measure(&text, &st_b, left.w).0.ceil() + 24.0;
        let r = Rect::new(left.center().x - w / 2.0, left.y + 94.0, w, 24.0);
        ui.c.fill_rrect(r, 12.0, ACCENT.with_alpha(0.2));
        ui.c.text(&text, r, &st_b);
    }
    if let Some(best) = v.best {
        let text = format!("terbaik {}", ielts::fmt_band(best));
        ui.c.text(&text, Rect::new(left.x, left.y + 124.0, left.w, 15.0), &TextStyle::caption().color(palette::TEXT_FAINT).center());
    }
    // Again (at the picked level: the new one, once it opened), or done.
    let y = left.bottom() - PILL_SMALL;
    let (again, done) = ("Lagi", "Selesai");
    let (wa, wd) = (ui.pill_width(again, false, true), ui.pill_width(done, false, true));
    let x = left.center().x - (wa + PILL_GAP + wd) / 2.0;
    let again_r = Rect::new(x, y, wa, PILL_SMALL);
    if st.ielts.startable(st.ielts.pick) {
        if ui.pill(wid("ielts.again"), again_r, again, None, PillStyle::Primary(ACCENT)).clicked {
            st.cmd(Command::Ielts(IeltsCmd::Start));
        }
    } else {
        pill_off(ui, again_r, again);
    }
    if ui.pill(wid("ielts.done"), Rect::new(x + wa + PILL_GAP, y, wd, PILL_SMALL), done, None, PillStyle::Utility).clicked {
        st.cmd(Command::Ielts(IeltsCmd::Quit));
    }

    // The criteria with Oyen's notes.
    let name_st = TextStyle::footnote().weight(Weight::Semibold);
    let band_st = TextStyle::footnote().weight(Weight::Bold).tabular().right();
    let note_st = TextStyle::caption().color(palette::TEXT_DIM);
    for (i, (name, b)) in v.criteria.rows().into_iter().enumerate() {
        let y = right.y + i as f32 * 30.0;
        ui.c.text(name, Rect::new(right.x, y, right.w - 40.0, 15.0), &name_st);
        ui.c.text(&ielts::fmt_band(b.band), Rect::new(right.right() - 40.0, y, 40.0, 15.0), &band_st);
        ui.c.text(&b.note, Rect::new(right.x, y + 15.0, right.w, 14.0), &note_st);
    }
    // The roast, with Oyen saying it.
    let c = &st.ielts;
    let y = right.y + 124.0;
    let what = if c.talking() { Oyen::Talk { since: c.said_at } } else if reached { Oyen::Proud { since: c.said_at } } else { Oyen::Rest { look: None } };
    head(ui, Point::new(right.x + 13.0, y + 15.0), 26.0, what);
    // While Oyen talks, the line he says (closing, roast, the level); then the roast.
    let line = c.say.as_ref().filter(|_| c.talking()).map_or(v.roast.text.as_str(), |s| s.text.as_str());
    let roast = Rect::new(right.x + 34.0, y, right.w - 34.0, 34.0);
    ui.c.clip(roast);
    ui.c.text(line, roast, &TextStyle::footnote().wrap().top());
    ui.c.unclip();
    // Three tips.
    let y = right.y + 164.0;
    ui.c.text("TIPS DARI OYEN", Rect::new(right.x, y, right.w, 14.0), &TextStyle::caption().weight(Weight::Bold).color(palette::TEXT_FAINT));
    for (i, tip) in v.tips.iter().take(3).enumerate() {
        let r = Rect::new(right.x, y + 16.0 + i as f32 * 15.0, right.w, 15.0);
        ui.c.text(&format!("\u{2022}  {tip}"), r, &TextStyle::caption().color(palette::TEXT_DIM));
    }
}

// ---------------------------------------------------------------- stuck

fn draw_stuck(ui: &mut Ui, st: &mut AppState, left: Rect, right: Rect) {
    let p = st.ielts.problem.unwrap_or(Problem::Server);
    head(ui, Point::new(left.center().x, left.y + 74.0), HEAD_W, Oyen::Sorry { since: f64::NEG_INFINITY });
    let r = Rect::new(right.x, right.y + 34.0, right.w.min(400.0), 120.0);
    let bottom = problem_text(ui, r, p);
    let y = bottom + 14.0;
    let mut x = right.x;
    let mut button = |ui: &mut Ui, id: &str, label: &str, icon: Option<Icon>, style: PillStyle| {
        let w = ui.pill_width(label, icon.is_some(), true);
        let clicked = ui.pill(wid(id), Rect::new(x, y, w, PILL_SMALL), label, icon, style).clicked;
        x += w + PILL_GAP;
        clicked
    };
    if p == Problem::Mic(MicIssue::Denied) {
        if button(ui, "ielts.mic-settings", "Buka pengaturan mikrofon", Some(Icon::Open), PillStyle::Primary(ACCENT)) {
            st.cmd(Command::OpenUrl(MIC_SETTINGS.into()));
        }
        if button(ui, "ielts.retry", "Coba lagi", None, PillStyle::Neutral) {
            st.cmd(Command::Ielts(IeltsCmd::Retry));
        }
    } else {
        let label = if p.retryable() { "Coba lagi" } else { "Mulai lagi" };
        if button(ui, "ielts.retry", label, None, PillStyle::Primary(ACCENT)) {
            st.cmd(Command::Ielts(IeltsCmd::Retry));
        }
    }
    if button(ui, "ielts.quit", "Udahan", None, PillStyle::Utility) {
        st.cmd(Command::Ielts(IeltsCmd::Quit));
    }
}

/// A problem's title and line, top-down in `r`; returns where they end.
fn problem_text(ui: &mut Ui, r: Rect, p: Problem) -> f32 {
    let title = TextStyle::headline().wrap().top();
    let body = TextStyle::footnote().color(palette::TEXT_DIM).wrap().top();
    let line = p.line(cfg!(target_os = "macos"));
    let (_, th) = ui.c.measure(p.title(), &title, r.w);
    let (_, bh) = ui.c.measure(&line, &body, r.w);
    ui.c.text(p.title(), Rect::new(r.x, r.y, r.w, th + 1.0), &title);
    ui.c.text(&line, Rect::new(r.x, r.y + th + 4.0, r.w, bh + 1.0), &body);
    r.y + th + 4.0 + bh
}
