//! Notices (`Mode::Notice`): a phone paired, money in, a prayer's time,
//! a focus session done: an icon disc, a title and a line, and an inline
//! action pill.
//!
//! In Pill a notice is a 48 DIP capsule as wide as its words (up to
//! Classic's width): the disc sits in the left end and the action pill in
//! the right one, each concentric with the end it sits in, the words in
//! between. What the notice does is Classic's, to the widget: the same
//! inline actions, the check once one is done, and the same widget ids in
//! the same order, since the app acts on them at mouse-down.

use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::island::Geom;
use crate::state::{AppState, InlineAction, Notice};
use crate::ui::{wid, PillStyle, Ui, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use std::cell::{Cell, RefCell};

/// Two lines (title and subtitle) in a capsule: r = h / 2.
const H: f32 = 48.0;
/// As wide as the words, between these. The widest is Classic's notice, so
/// Pill never covers more of the screen and always leaves the words more
/// room than Classic does.
const MIN_W: f32 = 200.0;
const MAX_W: f32 = 372.0;
/// The disc, concentric with the left end (inset 8 at r 24), and its glyph.
const DISC: f32 = 32.0;
const GLYPH: f32 = 16.0;
/// Where the words start, and how far they keep from the right end.
const TEXT_X: f32 = 8.0 + DISC + 10.0;
const TEXT_END: f32 = 20.0;
/// The action pill: Classic's size, its round end concentric with the
/// notice's (24 - 14), and Classic's gap before it.
const ACT_INSET: f32 = H / 2.0 - PILL_SMALL / 2.0;
const ACT_GAP: f32 = 12.0;
const ACT_MIN_W: f32 = 76.0;
/// Over a fill this light the glyph turns dark (white, yellow).
const LIGHT_FILL: f32 = 0.75;

thread_local! {
    /// The width the notice on screen measured, keyed by its words. `geom`
    /// has no canvas to measure with, so `draw` leaves it here.
    static FIT: Cell<(u64, f32)> = const { Cell::new((0, 0.0)) };
    /// The notice last drawn. A notice that leaves is gone from the state
    /// before the app draws its fade, so the fade draws this one: it fades
    /// as the pill shrinks instead of vanishing on the first frame.
    static LAST: RefCell<Option<Notice>> = const { RefCell::new(None) };
}

fn title_style() -> TextStyle {
    TextStyle::body().weight(Weight::Semibold)
}

fn subtitle_style() -> TextStyle {
    TextStyle::footnote().color(palette::TEXT_DIM)
}

/// What the width depends on: the words on screen and whether a pill sits
/// beside them. Never the hidden sum (`reveal`), so the pill's size gives
/// nothing away before Show.
fn key(n: &Notice) -> u64 {
    wid(&n.title) ^ wid(&n.title_dim).rotate_left(42) ^ wid(&n.subtitle).rotate_left(21) ^ n.inline.map_or(0, |a| a as u64 + 1)
}

/// Whole even DIPs, so the centred body's edges stay on whole DIPs.
fn fit(content: f32) -> f32 {
    ((content.clamp(MIN_W, MAX_W) / 2.0).ceil() * 2.0).min(MAX_W)
}

pub fn geom(st: &AppState) -> Option<Geom> {
    let w = match st.notice.as_ref() {
        Some(n) => {
            let (k, w) = FIT.with(|f| f.get());
            if k == key(n) {
                w
            } else {
                guess_width(n)
            }
        }
        None => MIN_W,
    };
    Some(Geom::new(w, H, H / 2.0))
}

/// The width before the words have been measured (a new notice's first
/// frame; the next frame has the measured one while the morph runs).
fn guess_width(n: &Notice) -> f32 {
    let words = guess(&format!("{} {}", n.title, n.title_dim), 13.0 * 1.05).max(guess(&n.subtitle, 12.0));
    let tail = if n.inline.is_some() { ACT_GAP + ACT_MIN_W + ACT_INSET } else { TEXT_END };
    fit(TEXT_X + words + tail)
}

/// About what the UI font's letters take at `size`, erring wide (also how
/// long an AI notice's second line may grow, `state::usage_notice`).
pub(crate) fn guess(s: &str, size: f32) -> f32 {
    let em: f32 = s
        .chars()
        .map(|c| match c {
            ' ' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' | '\u{b7}' | 'i' | 'j' | 'l' | 'I' | 'f' | 't' | 'r' => 0.30,
            'm' | 'w' | 'M' | 'W' | '%' | '@' | '\u{2014}' => 0.85,
            'A'..='Z' => 0.64,
            c if c as u32 >= 0x2E80 => 1.0,
            _ => 0.56,
        })
        .sum();
    em * size
}

/// The action already happened: the pill shows a check until the notice
/// leaves (the app gives it 0.6 s more). As `activities::inline_done`, with
/// the notice drawn standing in for `st.notice` (the same one while it
/// shows; the last one while it fades).
fn inline_done(st: &AppState, n: &Notice, a: InlineAction) -> bool {
    match a {
        InlineAction::StartFocus => st.pomodoro.is_active(),
        // Oyenza: the sum is in place once Show took it.
        InlineAction::ShowAmount => n.reveal.is_none(),
        InlineAction::StopAdzan => !st.prayer.adzan,
        // Done once clicked: the app puts the check in the notice's icon.
        InlineAction::WhatsNew => n.icon == Icon::Check,
        // Hide and Open take the notice away at once.
        InlineAction::Hide | InlineAction::Open => false,
    }
}

/// A glyph that reads on `fill` (seen over the black island).
fn ink_on(fill: Color) -> Color {
    if fill.luminance() * fill.a > LIGHT_FILL {
        palette::ON_ACCENT_DARK
    } else {
        palette::ON_ACCENT
    }
}

pub fn draw(ui: &mut Ui, st: &mut AppState, body: Rect) -> bool {
    let st = &*st;
    match st.notice.as_ref() {
        Some(n) => {
            LAST.with(|l| {
                let mut l = l.borrow_mut();
                if l.as_ref() != Some(n) {
                    *l = Some(n.clone());
                }
            });
            draw_notice(ui, st, n, body);
        }
        // The notice just left and this is its fade (no input reaches it).
        None => LAST.with(|l| {
            if let Some(n) = l.borrow().as_ref() {
                draw_notice(ui, st, n, body);
            }
        }),
    }
    true
}

fn draw_notice(ui: &mut Ui, st: &AppState, n: &Notice, body: Rect) {
    let act = n.inline.map(|a| {
        let (label, icon, color) = crate::activities::inline_look(st, n, a);
        (a, label, icon, color, ui.pill_width(label, icon.is_some(), true).max(if icon.is_some() { ACT_MIN_W } else { 0.0 }))
    });

    // New words: measure them once for `geom` (a DIP of slack so they never
    // trim). A change at rest (Show put the sum in place) asks for one more
    // frame, which follows the new width.
    let (title_st, sub_st) = (title_style(), subtitle_style());
    let k = key(n);
    let (fk, mut fw) = FIT.with(|f| f.get());
    if fk != k {
        let words = crate::activities::notice_title_width(ui, n, &title_st).max(ui.c.measure(&n.subtitle, &sub_st, 2000.0).0);
        let tail = act.as_ref().map_or(TEXT_END, |&(.., w)| ACT_GAP + w + ACT_INSET);
        fw = fit(TEXT_X + words.ceil() + 1.0 + tail);
        FIT.with(|f| f.set((k, fw)));
        ui.request_repaint();
    }

    // Laid out at its own size from the outline's top centre, as pages are:
    // while the outline grows into it, it uncovers the notice instead of
    // squeezing the words into ever-changing ellipses. A wider or taller
    // outline (a notch, a lift, a longer notice before it) takes the ends
    // and the middle with it, so the disc and the pill stay concentric.
    let (w, h) = (body.w.max(fw), body.h.clamp(H, H + 8.0));
    let lay = Rect::new(body.center().x - w / 2.0, body.y, w, h);
    let cy = lay.center().y;
    let c = Point::new(lay.x + h / 2.0, cy);
    ui.c.fill_circle(c, DISC / 2.0, n.color);
    ui.c.icon(n.icon, Rect::from_center(c, GLYPH, GLYPH), ink_on(n.color));

    let inset = h / 2.0 - PILL_SMALL / 2.0;
    let act = act.map(|(a, label, icon, color, w)| (a, label, icon, color, Rect::new(lay.right() - inset - w, cy - PILL_SMALL / 2.0, w, PILL_SMALL)));
    let tx = lay.x + TEXT_X;
    let text_right = act.as_ref().map_or(lay.right() - TEXT_END, |(.., r)| r.x - ACT_GAP);
    let tw = (text_right - tx).max(0.0);
    // Baselines about 22 and 37 DIP down the 48, as the spec has them.
    crate::activities::notice_title(ui, n, Rect::new(tx, cy - 17.0, tw, 20.0), title_st);
    ui.c.text(&n.subtitle, Rect::new(tx, cy, tw, 16.0), &sub_st);

    // The whole notice is a button (handled on mouse-down by the app); the
    // inline pill is registered after it so it wins the hit test.
    ui.button(wid("notice"), body);
    if let Some((a, label, icon, color, r)) = act {
        if inline_done(st, n, a) {
            ui.c.fill_rrect(r, r.h / 2.0, color.with_alpha(0.20));
            ui.c.icon(Icon::Check, Rect::from_center(r.center(), 14.0, 14.0), color);
        } else {
            let style = if icon.is_some() { PillStyle::Primary(color) } else { PillStyle::Utility };
            ui.pill(wid("notice.act"), r, label, icon, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(subtitle: &str, reveal: Option<&str>) -> Notice {
        Notice {
            icon: Icon::MoneyIn,
            color: palette::GREEN,
            title: "Money in \u{b7} BCA".into(),
            title_dim: String::new(),
            subtitle: subtitle.into(),
            until: 0.0,
            action: None,
            inline: Some(InlineAction::ShowAmount),
            reveal: reveal.map(Into::into),
        }
    }

    #[test]
    fn width_stays_between_its_bounds_on_even_dips() {
        assert_eq!(fit(10.0), MIN_W);
        assert_eq!(fit(1000.0), MAX_W);
        assert_eq!(fit(251.2), 252.0);
        assert_eq!(fit(253.0), 254.0);
    }

    #[test]
    fn a_hidden_sum_never_sizes_the_pill() {
        let long = money("Amount and sender hidden", Some("Rp 1.250.000 \u{b7} Gaji Oktober"));
        let short = money("Amount and sender hidden", Some("Rp 9"));
        assert_eq!(key(&long), key(&short));
        assert_eq!(guess_width(&long), guess_width(&short));
        // Once shown, the words are new and the width follows them.
        assert_ne!(key(&long), key(&money("Rp 1.250.000 \u{b7} Gaji Oktober", None)));
    }

    #[test]
    fn glyphs_read_on_their_disc() {
        for c in [palette::GREEN, palette::ORANGE, palette::BLUE, palette::TEXT_DIM] {
            assert_eq!(ink_on(c), palette::ON_ACCENT, "{c:?}");
        }
        for c in [palette::TEXT, palette::YELLOW] {
            assert_eq!(ink_on(c), palette::ON_ACCENT_DARK, "{c:?}");
        }
    }
}
