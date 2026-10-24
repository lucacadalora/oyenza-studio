//! Expanded-island pages. Each page module exposes:
//!
//! ```ignore
//! pub fn height(st: &AppState) -> f32;                        // content height (DIPs)
//! pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect);    // draw + handle input
//! ```
//!
//! Pages mutate `AppState` directly for local state and push
//! [`crate::commands::Command`]s for side effects (media control, opening
//! files, saving, …).

pub mod clipboard;
pub mod focus;
pub mod health;
pub mod media;
pub mod shelf;
pub mod tasks;
pub mod today;

use crate::commands::Command;
use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, Input, Ui};
use poni_core::datetime::{fmt_clock, fmt_date_short};
use poni_core::geom::{lerp, palette, Color, Point, Rect};

/// Height of the header band (tabs + clock) above the page content. The
/// header row itself is a fixed 36 DIPs ([`header_rect`]); the rest is the
/// 12 DIP gap between the tab pills and the first line of content.
pub const HEADER_H: f32 = 50.0;
/// Height of the header row (the 28 DIP tab pills sit centred in it).
const HEADER_ROW_H: f32 = 36.0;
/// Horizontal padding inside the expanded island, also used below the content.
pub const PAD: f32 = 18.0;

/// The header row inside the expanded island `body`. Fixed size, so changing
/// [`HEADER_H`] only moves the content, never the tabs.
pub fn header_rect(body: Rect) -> Rect {
    Rect::new(body.x + PAD - 6.0, body.y + 6.0, body.w - 2.0 * PAD + 12.0, HEADER_ROW_H)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PageKind {
    Media,
    Focus,
    Health,
    Shelf,
    Tasks,
    Clipboard,
    Today,
}

impl PageKind {
    pub const ALL: [PageKind; 7] =
        [PageKind::Media, PageKind::Focus, PageKind::Health, PageKind::Shelf, PageKind::Tasks, PageKind::Clipboard, PageKind::Today];

    pub fn key(self) -> &'static str {
        match self {
            PageKind::Media => "media",
            PageKind::Focus => "focus",
            PageKind::Health => "health",
            PageKind::Shelf => "shelf",
            PageKind::Tasks => "tasks",
            PageKind::Clipboard => "clipboard",
            PageKind::Today => "today",
        }
    }

    pub fn from_key(s: &str) -> PageKind {
        // Health took the Water page's place in the tab row.
        let s = if s.trim().eq_ignore_ascii_case("water") { "health" } else { s.trim() };
        Self::ALL.into_iter().find(|p| p.key().eq_ignore_ascii_case(s)).unwrap_or(PageKind::Media)
    }

    pub fn title(self) -> &'static str {
        match self {
            PageKind::Media => "Music",
            PageKind::Focus => "Focus",
            PageKind::Health => "Health",
            PageKind::Shelf => "Shelf",
            PageKind::Tasks => "Tasks",
            PageKind::Clipboard => "Clipboard",
            PageKind::Today => "Today",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            PageKind::Media => Icon::Music,
            PageKind::Focus => Icon::Timer,
            PageKind::Health => Icon::Steps,
            PageKind::Shelf => Icon::Tray,
            PageKind::Tasks => Icon::ListChecks,
            PageKind::Clipboard => Icon::Clipboard,
            PageKind::Today => Icon::Calendar,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn next(self) -> PageKind {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> PageKind {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    /// Position in the tab row (for slide direction).
    pub fn tab_index(self) -> usize {
        self.index()
    }
}

pub fn content_height(p: PageKind, st: &AppState) -> f32 {
    match p {
        PageKind::Media => media::height(st),
        PageKind::Focus => focus::height(st),
        PageKind::Health => health::height(st),
        PageKind::Shelf => shelf::height(st),
        PageKind::Tasks => tasks::height(st),
        PageKind::Clipboard => clipboard::height(st),
        PageKind::Today => today::height(st),
    }
}

/// The next time (mono seconds) the expanded `page` changes on its own and
/// needs a frame without input, if any (e.g. an Undo pill expiring). Meant
/// for the app's wake timer; until it asks, such changes show on the next
/// repaint.
pub fn next_wake(page: PageKind, st: &AppState) -> Option<f64> {
    let wake = match page {
        PageKind::Clipboard => clipboard::next_wake(st),
        PageKind::Tasks => tasks::next_wake(st),
        PageKind::Shelf => shelf::next_wake(st),
        _ => None,
    };
    // Oyen in the header blinks and flicks an ear now and then.
    let oyen = crate::oyen::on(st.settings.theme).then(|| crate::oyen::next_rest_move(st.now));
    match (wake, oyen) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// A page switch in progress: the page we came from, 0..1 progress, and the
/// slide direction (+1 = moving to a tab on the right).
#[derive(Clone, Copy, Debug)]
pub struct PageTransition {
    pub from: PageKind,
    pub t: f32,
    pub dir: f32,
}

/// How far (DIPs) pages slide during a switch.
const SLIDE: f32 = 20.0;
/// The old page is gone by this much of the switch...
const OUT_END: f32 = 0.4;
/// ...while the new one fades in over this window.
const IN_START: f32 = 0.1;
const IN_LEN: f32 = 0.5;
/// The header's selection pill and labels settle by this much of the switch.
const TABS_END: f32 = 0.6;

/// Draw the header and the active page inside the expanded island `body`.
/// During a page switch the old page fades out sliding away while the new
/// one fades in; the old page is drawn without input (it can't be clicked).
/// `body` is the island at its final size, so nothing reflows while it opens.
pub fn draw_expanded(ui: &mut Ui, st: &mut AppState, body: Rect, page: PageKind, transition: Option<PageTransition>) {
    let tr = transition.filter(|tr| tr.t < 1.0 && tr.from != page);
    let header = header_rect(body);
    let tip = draw_header(ui, st, header, page, tr);
    let area = Rect::new(body.x + PAD, body.y + HEADER_H, body.w - 2.0 * PAD, (body.h - HEADER_H - PAD).max(0.0));
    // Content may reach a little above `area` (text boxes whose cap height,
    // not their line box, sits on the content top) but never into the tabs.
    let clip_top = header.bottom() + 2.0;
    ui.c.clip(Rect::new(body.x, clip_top, body.w, (body.bottom() - clip_top).max(0.0)));
    match tr {
        Some(tr) => {
            // Overlapping smoothstep fades: the summed opacity stays at or
            // above about 0.5, so dense pages don't dip into a muddy grey.
            let out_a = 1.0 - smoothstep(tr.t / OUT_END);
            let in_a = smoothstep((tr.t - IN_START) / IN_LEN);
            let base = ui.c.alpha();
            // Restore (never reset) the transform: the app may be scaling the
            // whole island content while it opens.
            let m0 = ui.c.save_transform();
            if out_a > 0.0 {
                // Outgoing page: ghost UI (no input, throwaway hit list).
                let mut ghost_input = Input::default();
                let mut ghost_hits = Vec::new();
                let now = ui.now;
                let mut ghost = Ui::new(&mut *ui.c, &mut ghost_input, &mut ghost_hits, now);
                ghost.c.set_alpha(base * out_a);
                ghost.c.translate(-tr.dir * SLIDE * (1.0 - out_a), 0.0);
                draw_page(&mut ghost, st, area, tr.from);
                ghost.c.restore_transform(m0);
            }
            // The new page always draws (even fully transparent) so its hit
            // regions and animation state stay continuous.
            ui.c.set_alpha(base * in_a);
            ui.c.translate(tr.dir * SLIDE * (1.0 - in_a), 0.0);
            draw_page(ui, st, area, page);
            ui.c.restore_transform(m0);
            ui.c.set_alpha(base);
            ui.request_anim();
        }
        None => draw_page(ui, st, area, page),
    }
    ui.c.unclip();
    // Hover labels sit on top of the page, outside the content clip.
    if let Some(tip) = tip {
        draw_tip(ui, header, &tip);
    }
}

fn draw_page(ui: &mut Ui, st: &mut AppState, area: Rect, page: PageKind) {
    match page {
        PageKind::Media => media::draw(ui, st, area),
        PageKind::Focus => focus::draw(ui, st, area),
        PageKind::Health => health::draw(ui, st, area),
        PageKind::Shelf => shelf::draw(ui, st, area),
        PageKind::Tasks => tasks::draw(ui, st, area),
        PageKind::Clipboard => clipboard::draw(ui, st, area),
        PageKind::Today => today::draw(ui, st, area),
    }
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Round to the device-pixel grid.
fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

// ---------------------------------------------------------------- header

const TAB_H: f32 = 28.0;
/// Inactive tabs are bare icons in a 30 DIP slot.
const TAB_W: f32 = 30.0;
const TAB_GAP: f32 = 2.0;
const TAB_ICON: f32 = 16.0;
/// The active tab's label starts this far in and ends `TAB_LABEL_END` before its edge.
const TAB_LABEL_X: f32 = 28.0;
const TAB_LABEL_END: f32 = 12.0;
/// Clock to date, and date (or clock) to the pin button.
const DATE_GAP: f32 = 12.0;
const PIN_GAP: f32 = 10.0;
const PIN: f32 = 28.0;
/// His theme: Oyen peeks up beside the pin, as he does beside the clock when
/// the island is closed.
const OYEN_W: f32 = 24.0;
const OYEN_GAP: f32 = 6.0;
/// Hover labels: dwell, fade-in, capsule metrics.
const TIP_DELAY: f64 = 0.4;
const TIP_FADE: f64 = 0.12;
const TIP_H: f32 = 22.0;
const TIP_PAD: f32 = 8.0;
const TIP_RADIUS: f32 = 8.0;
/// White 0.16 over the black island, made opaque so page text can't show through.
const TIP_FILL: Color = Color::rgba(0.16, 0.16, 0.16, 1.0);
/// `tab_hover` slot of the pin button (tabs use their index).
const PIN_SLOT: usize = PageKind::ALL.len();

/// A hover label waiting to be painted above the page.
struct Tip {
    anchor: Rect,
    text: &'static str,
    alpha: f32,
}

/// Tab slots (x, width) with `active` showing its label.
fn tab_layout(ui: &mut Ui, x0: f32, active: PageKind, label_st: &TextStyle, scale: f32) -> [(f32, f32); 7] {
    let mut out = [(0.0, 0.0); 7];
    let mut x = x0;
    for (i, p) in PageKind::ALL.into_iter().enumerate() {
        let w = if p == active { snap(TAB_LABEL_X + ui.c.measure(p.title(), label_st, 200.0).0 + TAB_LABEL_END, scale) } else { TAB_W };
        out[i] = (snap(x, scale), w);
        x += w + TAB_GAP;
    }
    out
}

/// Tabs on the left; pin, date and clock on the right. During a page switch
/// the tabs re-flow, the selection pill slides and the labels cross-fade.
/// Returns the hover label to paint once the page is drawn.
fn draw_header(ui: &mut Ui, st: &mut AppState, r: Rect, page: PageKind, tr: Option<PageTransition>) -> Option<Tip> {
    let scale = ui.c.scale().max(0.5);
    let cy = r.center().y;
    let label_st = TextStyle::body().weight(Weight::Semibold);
    let from = tr.map_or(page, |t| t.from);
    let e = tr.map_or(1.0, |t| ease_out_cubic(t.t / TABS_END));
    let lay_from = tab_layout(ui, r.x, from, &label_st, scale);
    let lay_to = if from == page { lay_from } else { tab_layout(ui, r.x, page, &label_st, scale) };
    let tabs: [Rect; 7] =
        std::array::from_fn(|i| Rect::new(lerp(lay_from[i].0, lay_to[i].0, e), cy - TAB_H / 2.0, lerp(lay_from[i].1, lay_to[i].1, e), TAB_H));
    let (fi, pi) = (from.tab_index(), page.tab_index());
    let tabs_end = tabs[tabs.len() - 1].right();

    // Selection pill, gliding from the old tab to the new one.
    let pill = Rect::new(lerp(lay_from[fi].0, lay_to[pi].0, e), cy - TAB_H / 2.0, lerp(lay_from[fi].1, lay_to[pi].1, e), TAB_H);
    ui.c.fill_rrect(pill, TAB_H / 2.0, Color::white(0.14));

    let mut hover: Option<(usize, Rect)> = None;
    for (i, p) in PageKind::ALL.into_iter().enumerate() {
        let tr = tabs[i];
        let resp = ui.button(wid_n("tab", i as u64), tr);
        let active = p == page;
        if resp.hovered && !active {
            ui.c.fill_rrect(tr, TAB_H / 2.0, palette::SURFACE);
            hover = Some((i, tr));
        }
        let k = if active { e } else if i == fi { 1.0 - e } else { 0.0 };
        let ic = if resp.hovered { palette::TEXT } else { Color::lerp(palette::TEXT_FAINT, palette::TEXT, k) };
        let ix = snap(tr.x + (TAB_W - TAB_ICON) / 2.0, scale);
        ui.c.icon(p.icon(), Rect::new(ix, snap(cy - TAB_ICON / 2.0, scale), TAB_ICON, TAB_ICON), ic);
        if resp.clicked && !active {
            st.cmd(Command::SetPage(p));
        }
    }
    if fi != pi {
        draw_tab_label(ui, from, tabs[fi], &label_st, 1.0 - e);
    }
    draw_tab_label(ui, page, tabs[pi], &label_st, e);

    // Right side: the clock ends on the content column's right edge (where
    // every page's trailing control ends). Today already shows the date
    // large, so the header leaves it out there.
    let right = r.right() - 6.0;
    let time = fmt_clock(&st.local, st.settings.use_24h);
    let time_st = TextStyle::body().weight(Weight::Semibold).tabular().right();
    let tw = ui.c.measure(&time, &time_st, 200.0).0;
    ui.c.text(&time, Rect::new(right - tw - 4.0, r.y, tw + 4.0, r.h), &time_st);
    let mut left = right - tw;

    let shows_date = |p: PageKind| if p == PageKind::Today { 0.0 } else { 1.0 };
    let date_k = lerp(shows_date(from), shows_date(page), e);
    let date = fmt_date_short(&st.local);
    let date_st = TextStyle::footnote().color(palette::TEXT_DIM).right();
    let dw = ui.c.measure(&date, &date_st, 200.0).0;
    let date_left = left - DATE_GAP - dw;
    if date_k > 0.0 && date_left > tabs_end + 40.0 {
        let date_r = Rect::new(date_left - 2.0, r.y, dw + 2.0, r.h);
        ui.c.with_alpha(date_k, |c| c.text(&date, date_r, &date_st));
        left = lerp(left, date_left, date_k);
    }

    let pin_r = Rect::new(snap(left - PIN_GAP - PIN, scale), cy - PIN / 2.0, PIN, PIN);
    let oyen_cx = pin_r.x - OYEN_GAP - OYEN_W / 2.0;
    if crate::oyen::on(st.settings.theme) && oyen_cx - OYEN_W / 2.0 > tabs_end + 12.0 {
        // Eyes on the cursor while it is over the island; asleep at night.
        let look = ui.input.mouse.map(|p| ((p.x - r.center().x) / (r.w / 2.0)).clamp(-1.0, 1.0));
        let (frame, moving) = crate::oyen::rest(st.now, st.local.hour < 5, look);
        if moving {
            ui.request_anim();
        }
        crate::oyen::peek(ui.c, &frame, oyen_cx, cy + 9.0, OYEN_W, r.y);
    }
    if pin_r.x > tabs_end + 8.0 {
        let pinned = st.pinned;
        let (icon, color) = if pinned { (Icon::PinFill, st.accent) } else { (Icon::Pin, palette::TEXT_FAINT) };
        let resp = ui.icon_button(wid("pin"), pin_r, icon, color, 15.0);
        if resp.hovered {
            hover = Some((PIN_SLOT, pin_r));
        }
        if resp.clicked {
            st.cmd(Command::SetPinned(!pinned));
        }
    }

    // Hover labels: after a short dwell over an inactive tab or the pin.
    let now = ui.now;
    st.ui.tab_hover = match (hover, st.ui.tab_hover) {
        (Some((i, _)), Some((j, t0))) if i == j => Some((j, t0)),
        (Some((i, _)), _) => Some((i, now)),
        (None, _) => None,
    };
    let (slot, anchor) = hover?;
    let since = now - st.ui.tab_hover.map_or(now, |h| h.1);
    if since < TIP_DELAY + TIP_FADE {
        ui.request_anim();
    }
    let alpha = ((since - TIP_DELAY) / TIP_FADE).clamp(0.0, 1.0) as f32;
    if alpha <= 0.0 {
        return None;
    }
    let text = match PageKind::ALL.get(slot) {
        Some(p) => p.title(),
        None if st.pinned => "Unpin",
        None => "Keep open",
    };
    Some(Tip { anchor, text, alpha })
}

/// A tab's label, faded by `alpha` and cut (not ellipsized) at the tab's
/// current right edge while the tab is narrower than its settled width.
fn draw_tab_label(ui: &mut Ui, p: PageKind, tab: Rect, st: &TextStyle, alpha: f32) {
    if alpha <= 0.01 {
        return;
    }
    let lx = tab.x + TAB_LABEL_X;
    let avail = tab.right() - TAB_LABEL_END + 2.0 - lx;
    if avail <= 0.5 {
        return;
    }
    let lw = ui.c.measure(p.title(), st, 200.0).0;
    let cut = avail < lw + 1.0;
    if cut {
        ui.c.clip(Rect::new(lx, tab.y, avail, tab.h));
    }
    ui.c.with_alpha(alpha, |c| c.text(p.title(), Rect::new(lx, tab.y, lw + 2.0, tab.h), st));
    if cut {
        ui.c.unclip();
    }
}

/// A small capsule centred under the hovered tab (kept within the header's width).
fn draw_tip(ui: &mut Ui, header: Rect, tip: &Tip) {
    let scale = ui.c.scale().max(0.5);
    let st = TextStyle::footnote().center();
    let tw = ui.c.measure(tip.text, &st, 300.0).0;
    let w = snap((tw + 2.0 * TIP_PAD).ceil(), scale);
    let x = (tip.anchor.center().x - w / 2.0).clamp(header.x, (header.right() - w).max(header.x));
    let r = Rect::new(snap(x, scale), snap(header.bottom() + 4.0, scale), w, TIP_H);
    ui.c.with_alpha(tip.alpha, |c| {
        c.fill_rrect(r, TIP_RADIUS, TIP_FILL);
        c.text(tip.text, r, &st);
    });
}

/// Shared helper: a centred, dim placeholder message with an icon.
pub fn empty_state(ui: &mut Ui, area: Rect, icon: Icon, title: &str, hint: &str) {
    let c = area.center();
    ui.c.icon(icon, Rect::from_center(Point::new(c.x, c.y - 22.0), 26.0, 26.0), Color::white(0.35));
    ui.c.text(title, Rect::new(area.x, c.y - 4.0, area.w, 20.0), &TextStyle::headline().center());
    ui.c.text(hint, Rect::new(area.x, c.y + 16.0, area.w, 18.0), &TextStyle::footnote().color(palette::TEXT_DIM).center());
}
