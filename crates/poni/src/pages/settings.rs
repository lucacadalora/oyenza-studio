//! Settings page: the gear on the right of the header opens the Settings
//! window (`settings_window`), which has every setting, so this page shows
//! only the Update sheet (a new version's notice); the window draws this
//! page's account row, switch, picker and Check now.
//!
//! The Oyenza account comes first: the Oyenza app's icon, then signing in
//! (the code itself shows on the Oyenza page), who is signed in, and
//! Disconnect. Under a line, what the tray menu also changes: the theme and
//! the idle look as pickers, four switches, and Hide or Stay on top in
//! full-screen apps; this copy's version; and which of the island's own pages
//! (Music, Today, Shelf, Tasks, Focus) have a tab.
//! A change saves at once and applies the way the tray's does
//! (`Command::SaveSettings`). New versions are always looked for; beside
//! the version, Check now reads the update file at once and says what it
//! found in its place. While a later version is out, a last row across the
//! page says so, with Update:
//! that opens the Update sheet, which takes the rows' place (as the island's
//! notice and the menu's "Update to" line do) until the island closes or
//! Settings does.

use crate::activities::{text_with_dim, text_with_dim_width};
use crate::commands::{Command, OyenzaCmd};
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::pages::PageKind;
use crate::pill::status;
use crate::state::{AppState, OyenzaLink, Said};
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_GAP, PILL_REGULAR, PILL_SMALL};
use crate::version::VERSION;
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::oyenza;
use poni_core::phonedrop::fmt_bytes;
use poni_core::settings::{IdleStyle, Theme};
use poni_core::update::Release;

/// Account row, a line, the pickers under their labels (or a row each on a
/// narrow island), then the switches and choices (three rows), the tabs, and
/// the new version's row while there is one. Shorter than the System page
/// (256), so the window keeps its size.
const TABS_Y: f32 = SWITCHES_Y + ROWS as f32 * ROW_H;
const HEIGHT: f32 = TABS_Y + ROW_H;
const HEIGHT_OFFER: f32 = HEIGHT + ROW_H;
const ACCOUNT_H: f32 = 44.0;
/// The Oyenza app's icon, and the text column right of it.
const ICON: f32 = 36.0;
const TEXT_X: f32 = 48.0;
const RULE_Y: f32 = 54.0;
const LABELS_Y: f32 = 62.0;
const PICKERS_Y: f32 = 80.0;
const SWITCHES_Y: f32 = 114.0;
const ROW_H: f32 = 28.0;
const COL_GAP: f32 = 24.0;
/// A row's hover wash reaches this far past its text and switch.
const ROW_BLEED: f32 = 8.0;
/// Pickers: the selection's inset in the track, and each choice's padding.
/// Side by side, the theme and idle look pickers may close their choices up
/// to SEG_PAD_MIN to fit their columns (the Pill theme's five idle looks at
/// the default width).
const SEG_INSET: f32 = 2.0;
const SEG_PAD: f32 = 11.0;
const SEG_PAD_MIN: f32 = 7.5;
/// Where they still don't fit (a narrow island), each takes a row, its
/// label on the left, in the room the labels-above layout uses, so the page
/// keeps its height.
const STACK_Y: [f32; 2] = [58.0, 86.0];
const STACK_GAP: f32 = 16.0;
/// The full-screen choice's picker, inside its row.
const ROW_PICKER_H: f32 = 24.0;
const SWITCH_W: f32 = 30.0;
/// Check now (or its answer): the room around its words.
const CHECK_PAD: f32 = 5.0;
/// Its hover wash, inside the row's.
const CHECK_H: f32 = 22.0;
const SWITCH_H: f32 = 18.0;
const KNOB_INSET: f32 = 2.0;
/// Oyenza's colour, as on the Oyenza page.
const OYENZA: Color = palette::ORANGE;
/// The tabs row's chips, like the Record page's inputs.
const CHIP_H: f32 = 24.0;
const CHIP_PAD: f32 = 8.0;
const CHIP_ICON: f32 = 14.0;
const CHIP_ICON_GAP: f32 = 5.0;
const CHIP_GAP: f32 = 6.0;

// Glass: on Windows from a copy of the screen, on a Mac macOS's own glass.
const THEMES: [(Theme, &str); 4] = [(Theme::Classic, "Classic"), (Theme::Oyen, "Oyen"), (Theme::Glass, "Glass"), (Theme::Pill, "Pill")];

/// An idle look's name in the picker (`eyes` is Oyen in his theme).
fn idle_name(s: IdleStyle, eyes: &'static str) -> &'static str {
    match s {
        IdleStyle::Clock => "Clock",
        IdleStyle::Eyes => eyes,
        IdleStyle::Minimal => "Minimal",
        IdleStyle::Hidden => "Hidden",
        IdleStyle::Status => "Status",
    }
}

/// The switches, as the tray menu names them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Switch {
    Lyrics,
    Pronunciation,
    Sounds,
    Autostart,
}

/// What a row of the two columns holds: a switch, the full-screen choice
/// (the tray menu's "In full-screen apps" submenu), or this copy's version
/// with Check now.
#[derive(Clone, Copy)]
enum Row {
    Switch(Switch, &'static str),
    Fullscreen,
    Version,
}

const LEFT: &[Row] = &[
    Row::Switch(Switch::Lyrics, "Synced lyrics"),
    Row::Switch(Switch::Pronunciation, "Lyrics pronunciation"),
    Row::Switch(Switch::Sounds, "Sounds"),
];
// The version took the row of the new versions' switch (Clipboard history's
// before it), so the columns stay even.
const RIGHT: &[Row] = &[Row::Version, Row::Fullscreen, Row::Switch(Switch::Autostart, "Start with Windows")];
const COLUMNS: [&[Row]; 2] = [LEFT, RIGHT];
/// Rows in the taller column.
const ROWS: usize = if LEFT.len() > RIGHT.len() { LEFT.len() } else { RIGHT.len() };

/// The full-screen choices as the tray menu names them, and the
/// `hide_in_fullscreen` each one sets.
const FULLSCREEN: [(bool, &str); 2] = [(true, "Hide"), (false, "Stay on top")];

pub fn height(st: &AppState) -> f32 {
    match st.update.offer {
        Some(_) if st.update.sheet => SHEET_H,
        Some(_) => HEIGHT_OFFER,
        None => HEIGHT,
    }
}

/// While a sign-in code waits, its time left counts down each second.
pub fn next_wake(st: &AppState) -> Option<f64> {
    matches!(st.oyenza.link, OyenzaLink::Connecting { .. }).then(|| st.now.floor() + 1.0)
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    if let Some(rel) = st.update.offer.clone().filter(|_| st.update.sheet) {
        draw_sheet(ui, st, area, &rel);
        return;
    }
    draw_account(ui, st, Rect::new(area.x, area.y, area.w, ACCOUNT_H));
    let px = 1.0 / ui.c.scale().max(0.5);
    ui.c.fill_rect(Rect::new(area.x, area.y + RULE_Y, area.w, px), Color::white(0.10));

    let col_w = ((area.w - COL_GAP) / 2.0).max(0.0);
    let cols = [area.x, area.x + col_w + COL_GAP];
    let label_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_FAINT);

    // Pickers.
    let theme_names: Vec<&str> = THEMES.iter().map(|t| t.1).collect();
    let theme_sel = THEMES.iter().position(|t| t.0 == st.settings.theme).unwrap_or(0);
    let looks = status::looks(st.settings.theme);
    // In the Oyen theme the eyes are Oyen's own (as in the tray menu).
    let eyes = if st.settings.theme == Theme::Oyen { "Oyen" } else { "Eyes" };
    let look_names: Vec<&str> = looks.iter().map(|s| idle_name(*s, eyes)).collect();
    let look_sel = looks.iter().position(|s| *s == status::shown(st.settings.theme, st.settings.idle_style)).unwrap_or(0);
    // Side by side under their labels when both fit their columns, each
    // closed up only as far as it needs; else a row each. Whether they fit is
    // asked of this theme's own looks, so Classic, Oyen and Glass sit side by
    // side exactly where they always did (at a narrow island picking Pill,
    // with its fifth look, can stack them).
    let theme_pad = pad_to_fit(ui, &theme_names, col_w);
    let side_by_side = theme_pad >= SEG_PAD_MIN && pad_to_fit(ui, &look_names, col_w) >= SEG_PAD_MIN;
    let (theme_pick, look_pick) = if side_by_side {
        ui.c.text("THEME", Rect::new(cols[0], area.y + LABELS_Y, col_w, 14.0), &label_st);
        ui.c.text("IDLE LOOK", Rect::new(cols[1], area.y + LABELS_Y, col_w, 14.0), &label_st);
        let y = area.y + PICKERS_Y;
        let look_pad = pad_to_fit(ui, &look_names, col_w);
        (
            picker_padded(ui, "settings.theme", cols[0], y, PILL_SMALL, &theme_names, theme_sel, theme_pad),
            picker_padded(ui, "settings.idle", cols[1], y, PILL_SMALL, &look_names, look_sel, look_pad),
        )
    } else {
        // A row each, the labels in a column of their own and both pickers
        // starting after it.
        let labels = ["THEME", "IDLE LOOK"];
        let label_w = labels.iter().map(|l| ui.c.measure(l, &label_st, 300.0).0.ceil()).fold(0.0f32, f32::max);
        let x = area.x + label_w + STACK_GAP;
        let pick = |ui: &mut Ui, i: usize, id: &str, names: &[&str], sel: usize| {
            let row = Rect::new(area.x, area.y + STACK_Y[i], area.w, ROW_H);
            ui.c.text(labels[i], Rect::new(row.x, row.y, label_w + 2.0, row.h), &label_st);
            let pad = pad_to_fit(ui, names, row.right() - x).max(0.0);
            picker_padded(ui, id, x, row.center().y - ROW_PICKER_H / 2.0, ROW_PICKER_H, names, sel, pad)
        };
        (pick(ui, 0, "settings.theme", &theme_names, theme_sel), pick(ui, 1, "settings.idle", &look_names, look_sel))
    };
    if let Some(i) = theme_pick {
        st.settings.theme = THEMES[i].0;
        st.cmd(Command::SaveSettings);
    }
    if let Some(i) = look_pick {
        st.settings.idle_style = looks[i];
        st.cmd(Command::SaveSettings);
    }

    // Switches and the two choices, in two columns.
    let accent = st.accent;
    let card = super::card::pill_card(st, area);
    for (c, column) in COLUMNS.iter().enumerate() {
        for (r, &kind) in column.iter().enumerate() {
            let row = Rect::new(cols[c], area.y + SWITCHES_Y + r as f32 * ROW_H, col_w, ROW_H);
            match kind {
                Row::Switch(sw, label) => {
                    // A Mac starts apps at login, as its menu bar says.
                    let label = if cfg!(target_os = "macos") && sw == Switch::Autostart { "Start at login" } else { label };
                    let on = switch_on(st, sw);
                    let id = wid_n("settings.switch", (c * ROWS + r) as u64);
                    if switch_row_on(ui, id, row, label, on, accent, card) {
                        flip(st, sw, on);
                    }
                }
                Row::Version => {
                    let check = check_slot(ui, st, row);
                    let right = check.map_or(row.right(), |b| b.x - 10.0);
                    text_with_dim(ui, "Version", VERSION, Rect::new(row.x, row.y, (right - row.x).max(0.0), row.h), TextStyle::body());
                    if let Some(b) = check {
                        draw_check(ui, st, b);
                    }
                }
                Row::Fullscreen => {
                    let sel = FULLSCREEN.iter().position(|f| f.0 == st.settings.hide_in_fullscreen).unwrap_or(0);
                    if let Some(i) = choice_row(ui, row, "In full-screen apps", "settings.fullscreen", &FULLSCREEN.map(|f| f.1), sel) {
                        st.settings.hide_in_fullscreen = FULLSCREEN[i].0;
                        st.cmd(Command::SaveSettings);
                    }
                }
            }
        }
    }
    draw_tabs(ui, st, Rect::new(area.x, area.y + TABS_Y, area.w, ROW_H));
    if let Some(rel) = st.update.offer.clone() {
        draw_offer(ui, st, Rect::new(area.x, area.y + HEIGHT, area.w, ROW_H), &rel);
    }
}

/// "Tabs", then a chip for each of the island's own pages: on, its icon and
/// name; off (no tab in the row), the icon struck through and dim, as the
/// Record page marks an input that's off. A click flips it. Icons only
/// where the names don't fit.
fn draw_tabs(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let body = TextStyle::body();
    let label_w = ui.c.measure("Tabs", &body, 200.0).0.ceil();
    ui.c.text("Tabs", Rect::new(r.x, r.y, label_w + 1.0, r.h), &body);
    let chip_st = TextStyle::footnote().weight(Weight::Semibold);
    let pages = PageKind::HIDEABLE;
    let gaps = CHIP_GAP * (pages.len() - 1) as f32;
    let full: Vec<f32> =
        pages.iter().map(|p| (CHIP_PAD + CHIP_ICON + CHIP_ICON_GAP + ui.c.measure(p.title(), &chip_st, 200.0).0 + CHIP_PAD).ceil()).collect();
    let names = full.iter().sum::<f32>() + gaps <= r.w - label_w - 12.0;
    let widths: Vec<f32> = if names { full } else { vec![CHIP_H + 8.0; pages.len()] };
    let mut x = r.right() - widths.iter().sum::<f32>() - gaps;
    let y = r.center().y - CHIP_H / 2.0;
    let mut flip = None;
    for (i, (&p, w)) in pages.iter().zip(widths).enumerate() {
        let c = Rect::new(x, y, w, CHIP_H);
        x += w + CHIP_GAP;
        let on = !st.tab_hidden(p);
        let resp = ui.button(wid_n("settings.tab", i as u64), c);
        if resp.clicked {
            flip = Some(p);
        }
        let fill = match (on, resp.pressed, resp.hovered) {
            (_, true, _) => Color::white(0.22),
            (true, _, true) => Color::white(0.20),
            (true, _, false) => Color::white(0.16),
            (false, _, true) => Color::white(0.12),
            (false, _, false) => palette::SURFACE,
        };
        ui.c.fill_rrect(c, CHIP_H / 2.0, fill);
        let color = if on { palette::TEXT } else { palette::TEXT_DIM };
        let ix = if names { c.x + CHIP_PAD } else { c.center().x - CHIP_ICON / 2.0 };
        let ir = Rect::new(ix, c.center().y - CHIP_ICON / 2.0, CHIP_ICON, CHIP_ICON);
        ui.c.icon(p.icon(), ir, color);
        if !on {
            ui.c.line(Point::new(ir.x, ir.bottom()), Point::new(ir.right(), ir.y), color, 1.5);
        }
        if names {
            let tx = ix + CHIP_ICON + CHIP_ICON_GAP;
            ui.c.text(p.title(), Rect::new(tx, c.y, (c.right() - tx).max(0.0), c.h), &chip_st.color(color));
        }
    }
    if let Some(p) = flip {
        let hidden = &mut st.settings.hidden_tabs;
        match hidden.iter().position(|k| k == p.key()) {
            Some(i) => {
                hidden.remove(i);
            }
            None => hidden.push(p.key().to_string()),
        }
        st.cmd(Command::SaveSettings);
    }
}

/// Where Check now goes in the version's row, at its right end: while this
/// copy looks for new versions itself and none is offered (then the last
/// row has Update). Words with a wash on hover, not a pill.
pub(crate) fn check_slot(ui: &mut Ui, st: &AppState, row: Rect) -> Option<Rect> {
    if !st.update.looks || st.update.offer.is_some() {
        return None;
    }
    let (text, _) = check_look(st.update.said, st.accent);
    // Checking… and Up to date are words only: no room for a wash, so they
    // take less of the label's, ending where Check now's words end.
    let pad = if answered(st.update.said) { 0.0 } else { CHECK_PAD };
    let w = (ui.c.measure(text, &check_text(), 300.0).0 + 2.0 * pad).ceil();
    let right = row.right() - (CHECK_PAD - pad);
    Some(Rect::new(right - w, row.center().y - CHECK_H / 2.0, w, CHECK_H))
}

/// Check now said something that isn't a button: checking, or up to date.
fn answered(said: Option<Said>) -> bool {
    matches!(said, Some(Said::Checking | Said::UpToDate))
}

/// Check now, or what it found: checking, up to date, or (it couldn't
/// look) Try again, which looks again.
pub(crate) fn draw_check(ui: &mut Ui, st: &mut AppState, b: Rect) {
    let said = st.update.said;
    let (text, color) = check_look(said, st.accent);
    if answered(said) {
        ui.c.text(text, b, &check_text().color(color).right());
        return;
    }
    let resp = ui.button(wid("settings.check"), b);
    if resp.hovered || resp.pressed {
        ui.c.fill_rrect(b, b.h / 2.0, Color::white(if resp.pressed { 0.14 } else { 0.08 }));
    }
    ui.c.text(text, b, &check_text().color(color).center());
    if resp.clicked {
        st.update.said = Some(Said::Checking);
        st.cmd(Command::CheckForUpdates);
    }
}

/// Check now's words and colour for what it has said so far.
fn check_look(said: Option<Said>, accent: Color) -> (&'static str, Color) {
    match said {
        None => ("Check now", accent),
        Some(Said::Checking) => ("Checking…", palette::TEXT_DIM),
        Some(Said::UpToDate) => ("Up to date", palette::GREEN),
        Some(Said::Failed) => ("Try again", palette::ORANGE),
    }
}

fn check_text() -> TextStyle {
    TextStyle::footnote().weight(Weight::Semibold)
}

/// The last row while a later version is out, across the page: "Update
/// available 0.2.5", its headline, and Update (the sheet).
fn draw_offer(ui: &mut Ui, st: &mut AppState, r: Rect, rel: &Release) {
    let cy = r.center().y;
    let w = ui.pill_width("Update", false, true);
    let b = Rect::new(r.right() - w, cy - PILL_SMALL / 2.0, w, PILL_SMALL);
    if ui.pill(wid("settings.update"), b, "Update", None, PillStyle::Primary(st.accent)).clicked {
        st.update.sheet = true;
    }
    let right = b.x - 12.0;
    let body = TextStyle::body();
    let title = "Update available";
    let tw = text_with_dim_width(ui, title, &rel.version, &body).ceil().min((right - r.x).max(0.0));
    text_with_dim(ui, title, &rel.version, Rect::new(r.x, r.y, tw + 1.0, r.h), body);
    let x = r.x + tw + 12.0;
    if right > x {
        ui.c.text(&rel.headline, Rect::new(x, r.y, right - x, r.h), &TextStyle::footnote().color(palette::TEXT_DIM));
    }
}

// ---------------------------------------------------------------- the Update sheet

/// The Update sheet: a disc, "Update available 0.2.5" over what the
/// download is, Not now; a line; the release's headline under What's new;
/// a card that gets it; and its notes. Once Windows has the Setup it reads
/// "Update ready to install", with Install now and Install on next start,
/// as Screen Studio's does. Shorter than the rows it replaces.
const SHEET_CAPTION_Y: f32 = 66.0;
const SHEET_HEADLINE_Y: f32 = 82.0;
const CARDS_Y: f32 = 114.0;
const CARD_H: f32 = 58.0;
const CARD_GAP: f32 = 10.0;
const CARD_DISC: f32 = 30.0;
const NOTES_Y: f32 = CARDS_Y + CARD_H + 12.0;
const SHEET_H: f32 = NOTES_Y + PILL_SMALL;

/// What a card of the sheet does when clicked.
#[derive(Clone, Copy)]
enum Get {
    /// The release page (Windows, until the Setup is in) or the DMG (Mac).
    Download,
    /// Run the downloaded Setup now (Windows).
    InstallNow,
    /// Run it before the app opens next time (Windows).
    NextStart,
}

fn draw_sheet(ui: &mut Ui, st: &mut AppState, area: Rect, rel: &Release) {
    let head = Rect::new(area.x, area.y, area.w, ACCOUNT_H);
    let cy = head.center().y;
    let accent = st.accent;
    let c = Point::new(head.x + ICON / 2.0, cy);
    ui.c.fill_circle(c, ICON / 2.0, accent.with_alpha(0.22));
    ui.c.icon(Icon::Download, Rect::from_center(c, 20.0, 20.0), accent);
    let later_w = ui.pill_width("Not now", false, true);
    let later = Rect::new(head.right() - later_w, cy - PILL_SMALL / 2.0, later_w, PILL_SMALL);
    if ui.pill(wid("update.later"), later, "Not now", None, PillStyle::Utility).clicked {
        st.cmd(Command::Collapse);
    }
    let x = head.x + TEXT_X;
    let w = (later.x - 16.0 - x).max(0.0);
    let ready = st.update.ready;
    let title = if ready { "Update ready to install" } else { "Update available" };
    text_with_dim(ui, title, &rel.version, Rect::new(x, cy - 19.0, w, 20.0), TextStyle::headline());
    let size = rel.download(cfg!(target_os = "macos")).map(|d| fmt_bytes(d.size)).unwrap_or_default();
    let line = if ready {
        "A new version of Oyenza Studio is ready to be installed.".to_string()
    } else {
        format!("A new version of Oyenza Studio is ready to download ({size}).")
    };
    ui.c.text(&line, Rect::new(x, cy + 1.0, w, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));

    let px = 1.0 / ui.c.scale().max(0.5);
    ui.c.fill_rect(Rect::new(area.x, area.y + RULE_Y, area.w, px), Color::white(0.10));
    let label_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_FAINT);
    ui.c.text("WHAT'S NEW", Rect::new(area.x, area.y + SHEET_CAPTION_Y, area.w, 14.0), &label_st);
    ui.c.text(&rel.headline, Rect::new(area.x, area.y + SHEET_HEADLINE_Y, area.w, 20.0), &TextStyle::body());

    // Windows with the Setup in: now or next start. Otherwise the DMG on a
    // Mac, and the release page on Windows (the Setup couldn't download).
    let cards: Vec<(Get, Icon, String, &str)> = if ready {
        let next = if st.update.next_start {
            (Get::NextStart, Icon::Check, "Installs on next start".to_string(), "Set: before it next opens")
        } else {
            (Get::NextStart, Icon::Reset, "Install on next start".to_string(), "Before it next opens")
        };
        vec![(Get::InstallNow, Icon::Download, "Install now".to_string(), "Closes, updates and opens again"), next]
    } else if cfg!(target_os = "macos") {
        vec![(Get::Download, Icon::Download, format!("Download {}", rel.version), "Downloads the DMG: open it and drag Oyenza Studio to Applications.")]
    } else {
        vec![(Get::Download, Icon::Download, format!("Download {}", rel.version), "Opens its release page: run the Setup there, your settings stay.")]
    };
    let n = cards.len() as f32;
    let card_w = ((area.w - (n - 1.0) * CARD_GAP) / n).max(0.0);
    for (i, (get, icon, title, line)) in cards.iter().enumerate() {
        let r = Rect::new(area.x + i as f32 * (card_w + CARD_GAP), area.y + CARDS_Y, card_w, CARD_H);
        if card(ui, wid_n("update.card", i as u64), r, *icon, title, line, accent) {
            match get {
                Get::Download | Get::InstallNow => st.cmd(Command::Update),
                Get::NextStart => st.cmd(Command::InstallOnNextStart),
            }
        }
    }

    let notes = format!("See what's new in Oyenza Studio {}", rel.version);
    let nw = ui.pill_width(&notes, true, true).min(area.w);
    let nr = Rect::new(area.x, area.y + NOTES_Y, nw, PILL_SMALL);
    if ui.pill(wid("update.notes"), nr, &notes, Some(Icon::Open), PillStyle::Utility).clicked {
        st.cmd(Command::OpenUrl(rel.notes.clone()));
    }
}

/// One of the sheet's choices: a card with an icon, what it does in bold
/// and how under it. True when clicked.
fn card(ui: &mut Ui, id: u64, r: Rect, icon: Icon, title: &str, line: &str, accent: Color) -> bool {
    let resp = ui.button(id, r);
    let fill = if resp.pressed {
        Color::white(0.14)
    } else if resp.hovered {
        Color::white(0.10)
    } else {
        Color::white(0.06)
    };
    ui.c.fill_rrect(r, 12.0, fill);
    let c = Point::new(r.x + 14.0 + CARD_DISC / 2.0, r.center().y);
    ui.c.fill_circle(c, CARD_DISC / 2.0, accent.with_alpha(0.22));
    ui.c.icon(icon, Rect::from_center(c, 16.0, 16.0), accent);
    let x = c.x + CARD_DISC / 2.0 + 12.0;
    let w = (r.right() - 14.0 - x).max(0.0);
    let cy = r.center().y;
    ui.c.text(title, Rect::new(x, cy - 18.0, w, 20.0), &TextStyle::body().weight(Weight::Semibold));
    ui.c.text(line, Rect::new(x, cy + 1.0, w, 16.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    resp.clicked
}

/// A choice's label with its picker where the other rows have their
/// switch. Returns the choice clicked, if any.
fn choice_row(ui: &mut Ui, r: Rect, label: &str, id: &str, names: &[&str], sel: usize) -> Option<usize> {
    let x = r.right() - picker_width(ui, names);
    ui.c.text(label, Rect::new(r.x, r.y, (x - 10.0 - r.x).max(0.0), r.h), &TextStyle::body());
    picker(ui, id, x, r.center().y - ROW_PICKER_H / 2.0, ROW_PICKER_H, names, sel)
}

/// Where a switch reads from. Start with Windows is read off the UI thread
/// when the page opens; until then it shows off, dimmed.
fn switch_on(st: &AppState, sw: Switch) -> Option<bool> {
    let s = &st.settings;
    match sw {
        Switch::Lyrics => Some(s.lyrics),
        Switch::Pronunciation => Some(s.lyrics_pronunciation),
        Switch::Sounds => Some(s.sounds),
        Switch::Autostart => st.ui.autostart,
    }
}

fn flip(st: &mut AppState, sw: Switch, on: Option<bool>) {
    let s = &mut st.settings;
    match sw {
        Switch::Lyrics => s.lyrics = !s.lyrics,
        Switch::Pronunciation => s.lyrics_pronunciation = !s.lyrics_pronunciation,
        Switch::Sounds => s.sounds = !s.sounds,
        Switch::Autostart => {
            if let Some(on) = on {
                st.cmd(Command::SetAutostart(!on));
            }
            return;
        }
    }
    st.cmd(Command::SaveSettings);
}

// ---------------------------------------------------------------- account

pub(crate) fn draw_account(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let cy = r.center().y;
    crate::oyen::app_icon(ui.c, Point::new(r.x + ICON / 2.0, cy), ICON);
    match st.oyenza.link.clone() {
        OyenzaLink::Off => {
            let label = "Sign in with Oyenza";
            let w = ui.pill_width(label, false, false);
            let b = Rect::new(r.right() - w, cy - PILL_REGULAR / 2.0, w, PILL_REGULAR);
            if ui.pill(wid("settings.sign-in"), b, label, None, PillStyle::Primary(OYENZA)).clicked {
                st.cmd(Command::Oyenza(OyenzaCmd::SignIn));
            }
            account_text(ui, r, b.x, "Oyenza", "Sign in to see money in and bills due here.");
        }
        OyenzaLink::Connecting { code, until, .. } => {
            let cancel_w = ui.pill_width("Cancel", false, true);
            let cancel = Rect::new(r.right() - cancel_w, cy - PILL_SMALL / 2.0, cancel_w, PILL_SMALL);
            if ui.pill(wid("settings.cancel"), cancel, "Cancel", None, PillStyle::Utility).clicked {
                st.cmd(Command::Oyenza(OyenzaCmd::Cancel));
            }
            let show_w = ui.pill_width("Show code", false, true);
            let show = Rect::new(cancel.x - PILL_GAP - show_w, cancel.y, show_w, PILL_SMALL);
            if ui.pill(wid("settings.show-code"), show, "Show code", None, PillStyle::Primary(OYENZA)).clicked {
                st.cmd(Command::SetPage(PageKind::Oyenza));
            }
            let line = format!("Code {code} · works for {}", oyenza::code_time_left(until - st.now));
            account_text(ui, r, show.x, "Signing in to Oyenza", &line);
        }
        OyenzaLink::Connected { account } => {
            let dis_w = ui.pill_width("Disconnect", false, true);
            let dis = Rect::new(r.right() - dis_w, cy - PILL_SMALL / 2.0, dis_w, PILL_SMALL);
            if ui.pill(wid("settings.disconnect"), dis, "Disconnect", None, PillStyle::Utility).clicked {
                st.cmd(Command::Oyenza(OyenzaCmd::Disconnect));
            }
            // Sums and senders straight away on the island (off: they wait for Show).
            let label = "Show amounts";
            let lw = ui.c.measure(label, &TextStyle::body(), 300.0).0.ceil();
            let row = Rect::new(dis.x - 2.0 * ROW_BLEED - lw - 10.0 - SWITCH_W, cy - ROW_H / 2.0, lw + 10.0 + SWITCH_W, ROW_H);
            let on = st.settings.oyenza_show_amounts;
            if switch_row(ui, wid("settings.amounts"), row, label, Some(on), st.accent) {
                st.settings.oyenza_show_amounts = !on;
                st.cmd(Command::SaveSettings);
            }
            account_text(ui, r, row.x - ROW_BLEED, "Oyenza", &format!("Signed in as {account}"));
        }
    }
}

/// The account's name over one dim line, between Oyen and `right` (where
/// the row's buttons start).
fn account_text(ui: &mut Ui, r: Rect, right: f32, title: &str, line: &str) {
    let x = r.x + TEXT_X;
    let w = (right - 16.0 - x).max(0.0);
    let cy = r.center().y;
    ui.c.text(title, Rect::new(x, cy - 19.0, w, 20.0), &TextStyle::headline());
    ui.c.text(line, Rect::new(x, cy + 1.0, w, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));
}

// ---------------------------------------------------------------- widgets

/// The widest padding (up to SEG_PAD) that fits a picker of `names` into `w`.
fn pad_to_fit(ui: &mut Ui, names: &[&str], w: f32) -> f32 {
    let n = names.len().max(1) as f32;
    let text: f32 = names.iter().map(|s| ui.c.measure(s, &picker_text(), 300.0).0).sum();
    // Each choice's width is rounded up: up to a DIP each.
    ((w - 2.0 * SEG_INSET - text - n) / (2.0 * n)).min(SEG_PAD)
}

/// Choices side by side in a track `h` tall, the chosen one lifted (the
/// Focus page's picker, a size smaller). Returns the choice clicked, if any.
pub(crate) fn picker(ui: &mut Ui, id: &str, x: f32, y: f32, h: f32, names: &[&str], sel: usize) -> Option<usize> {
    picker_padded(ui, id, x, y, h, names, sel, SEG_PAD)
}

/// [`picker`] with `pad` around each choice's name.
#[allow(clippy::too_many_arguments)]
fn picker_padded(ui: &mut Ui, id: &str, x: f32, y: f32, h: f32, names: &[&str], sel: usize, pad: f32) -> Option<usize> {
    let st = picker_text();
    let inner_h = h - 2.0 * SEG_INSET;
    let mut cx = x + SEG_INSET;
    let cells: Vec<Rect> = names
        .iter()
        .map(|n| {
            let w = choice_width(ui, n, pad);
            let cell = Rect::new(cx, y + SEG_INSET, w, inner_h);
            cx += w;
            cell
        })
        .collect();
    ui.c.fill_rrect(Rect::new(x, y, cx + SEG_INSET - x, h), h / 2.0, palette::SURFACE);
    let mut picked = None;
    for (i, (name, cell)) in names.iter().zip(&cells).enumerate() {
        let color = if i == sel {
            ui.c.fill_rrect(*cell, inner_h / 2.0, Color::white(0.16));
            palette::TEXT
        } else {
            let resp = ui.button(wid_n(id, i as u64), Rect::new(cell.x, y, cell.w, h));
            if resp.clicked {
                picked = Some(i);
            }
            if resp.pressed || resp.hovered {
                ui.c.fill_rrect(*cell, inner_h / 2.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
                palette::TEXT
            } else {
                palette::TEXT_DIM
            }
        };
        ui.c.text(name, *cell, &st.color(color).center());
    }
    picked
}

fn picker_text() -> TextStyle {
    TextStyle::footnote().weight(Weight::Semibold)
}

/// One choice in a picker: its name and the padding around it.
fn choice_width(ui: &mut Ui, name: &str, pad: f32) -> f32 {
    (2.0 * pad + ui.c.measure(name, &picker_text(), 300.0).0).ceil()
}

/// A whole picker: its choices and the track's inset.
pub(crate) fn picker_width(ui: &mut Ui, names: &[&str]) -> f32 {
    2.0 * SEG_INSET + names.iter().map(|n| choice_width(ui, n, SEG_PAD)).sum::<f32>()
}

/// A label with a switch at the row's right end; the whole row toggles.
/// `None` = not known yet: drawn off and dimmed, and it doesn't click.
/// Returns true when clicked.
pub(crate) fn switch_row(ui: &mut Ui, id: u64, r: Rect, label: &str, on: Option<bool>, accent: Color) -> bool {
    switch_row_on(ui, id, r, label, on, accent, None)
}

/// [`switch_row`] on the open card (`card` from `card::pill_card`): in
/// Pill, a bottom row's hover wash follows the card's rounder corners.
fn switch_row_on(ui: &mut Ui, id: u64, r: Rect, label: &str, on: Option<bool>, accent: Color, card: Option<(Rect, f32)>) -> bool {
    let hit = Rect::new(r.x - ROW_BLEED, r.y, r.w + 2.0 * ROW_BLEED, r.h);
    let resp = if on.is_some() { ui.button(id, hit) } else { Default::default() };
    if resp.hovered || resp.pressed {
        super::card::fill_box(ui, card, hit.inset(0.0, 1.0), 8.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
    }
    let sw = Rect::new(r.right() - SWITCH_W, r.center().y - SWITCH_H / 2.0, SWITCH_W, SWITCH_H);
    ui.c.text(label, Rect::new(r.x, r.y, (sw.x - 10.0 - r.x).max(0.0), r.h), &TextStyle::body());
    let alpha = if on.is_some() { 1.0 } else { 0.5 };
    ui.c.with_alpha(alpha, |c| draw_switch(c, sw, on == Some(true), accent));
    resp.clicked
}

/// A switch's track and knob (also the Add-ons page's).
pub(crate) fn draw_switch(c: &mut crate::gfx::Canvas, r: Rect, on: bool, accent: Color) {
    c.fill_rrect(r, r.h / 2.0, if on { accent } else { palette::TRACK });
    let d = r.h - 2.0 * KNOB_INSET;
    let x = if on { r.right() - KNOB_INSET - d / 2.0 } else { r.x + KNOB_INSET + d / 2.0 };
    c.fill_circle(Point::new(x, r.center().y), d / 2.0, palette::ON_ACCENT);
}
