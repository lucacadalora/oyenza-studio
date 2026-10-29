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

pub mod addons;
pub mod card;
pub mod downloads;
pub mod downloads_offer;
pub mod focus;
pub mod health;
pub mod ielts;
pub mod media;
pub mod notes;
pub mod notifications;
pub mod oyenza;
pub mod prayer;
pub mod record;
pub mod script;
pub mod settings;
pub mod shelf;
pub mod stocks;
pub mod system;
pub mod tasks;
pub mod today;
pub mod usage;

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
    Today,
    Shelf,
    Tasks,
    Focus,
    /// AI usage: how much of each AI coding plan is left (`pages::usage`).
    /// A core tab, always in the row.
    Ai,
    /// Oyenza, with Oyen himself as its tab (`oyen::tab`): signed out, the
    /// way to sign in; signed in, the money by day.
    Oyenza,
    /// The Health add-on's page, while it is on ([`tabs`]): the steps
    /// Oyenza sends, so its tab sits beside Oyenza's.
    Health,
    /// The IELTS add-on's page, while it is on ([`tabs`]): Oyen as an IELTS
    /// Speaking examiner, through the Oyenza account, so its tab follows
    /// Oyenza's and Health's.
    Ielts,
    /// The Meeting notes add-on's page, while it is on ([`tabs`]; Windows):
    /// Oyen listens to a meeting and the Oyenza account writes its notes, so
    /// its tab follows IELTS'.
    Notes,
    /// The System add-on's page, while it is on ([`tabs`]).
    System,
    /// The teleprompter: a script read under the webcam.
    Script,
    /// Screen recording with Oyenza Studio Recorder, while its add-on is on
    /// ([`tabs`]): what to record, then the recording under way.
    Record,
    /// Prayer times, while that add-on is on ([`tabs`]).
    Prayer,
    /// The IHSG and the person's stocks, while the Stocks add-on is on ([`tabs`]).
    Stocks,
    /// Links to save as video or sound, while the Downloader add-on is on ([`tabs`]).
    Downloads,
    /// Which apps' notifications show on the island, while the
    /// Notifications add-on is on ([`tabs`]; Windows only).
    Notifications,
    /// The "+" at the end of the tab row: features to switch on, each of
    /// which downloads only then (`poni_core::addons`).
    Addons,
    /// Behind the gear on the right of the header, never in the tab row.
    Settings,
}

impl PageKind {
    /// The tab row's order: the island's own pages, Oyenza, then the
    /// add-ons' pages and the "+".
    pub const ALL: [PageKind; 19] = [
        PageKind::Media,
        PageKind::Today,
        PageKind::Shelf,
        PageKind::Tasks,
        PageKind::Focus,
        PageKind::Ai,
        PageKind::Oyenza,
        PageKind::Health,
        PageKind::Ielts,
        PageKind::Notes,
        PageKind::System,
        PageKind::Script,
        PageKind::Record,
        PageKind::Prayer,
        PageKind::Stocks,
        PageKind::Downloads,
        PageKind::Notifications,
        PageKind::Addons,
        PageKind::Settings,
    ];

    pub fn key(self) -> &'static str {
        match self {
            PageKind::Media => "media",
            PageKind::Focus => "focus",
            PageKind::Ai => "ai",
            PageKind::Health => "health",
            PageKind::Ielts => "ielts",
            PageKind::Notes => "notes",
            PageKind::Shelf => "shelf",
            PageKind::Tasks => "tasks",
            PageKind::Today => "today",
            PageKind::System => "system",
            PageKind::Script => "script",
            PageKind::Record => "record",
            PageKind::Oyenza => "oyenza",
            PageKind::Prayer => "prayer",
            PageKind::Stocks => "stocks",
            PageKind::Downloads => "downloads",
            PageKind::Notifications => "notifications",
            PageKind::Addons => "addons",
            PageKind::Settings => "settings",
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
            PageKind::Ai => "AI",
            PageKind::Health => "Health",
            PageKind::Ielts => "IELTS",
            PageKind::Notes => "Notes",
            PageKind::Shelf => "Shelf",
            PageKind::Tasks => "Tasks",
            PageKind::Today => "Today",
            PageKind::System => "System",
            PageKind::Script => "Script",
            PageKind::Record => "Record",
            PageKind::Oyenza => "Oyenza",
            PageKind::Prayer => "Prayer",
            PageKind::Stocks => "Stocks",
            PageKind::Downloads => "Downloads",
            PageKind::Notifications => "Notifications",
            PageKind::Addons => "Add-ons",
            PageKind::Settings => "Settings",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            PageKind::Media => Icon::Music,
            PageKind::Focus => Icon::Timer,
            PageKind::Ai => Icon::Ai,
            PageKind::Health => Icon::Steps,
            PageKind::Ielts => Icon::Mic,
            PageKind::Notes => Icon::Notes,
            PageKind::Shelf => Icon::Tray,
            PageKind::Tasks => Icon::ListChecks,
            PageKind::Today => Icon::Calendar,
            PageKind::System => Icon::Gauge,
            PageKind::Script => Icon::Script,
            PageKind::Record => Icon::Recorder,
            PageKind::Oyenza => Icon::Wallet,
            PageKind::Prayer => Icon::Moon,
            PageKind::Stocks => Icon::Stocks,
            PageKind::Downloads => Icon::Download,
            PageKind::Notifications => Icon::Bell,
            PageKind::Addons => Icon::Addons,
            PageKind::Settings => Icon::Gear,
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// Position in the tab row (for slide direction). Settings comes last:
    /// its gear sits right of the tabs.
    pub fn tab_index(self) -> usize {
        self.index()
    }

    /// Has a tab in the header's row (Settings has the gear instead).
    pub fn tabbed(self) -> bool {
        self != PageKind::Settings
    }

    /// The island's own pages, which Settings can take off the row. AI and
    /// Oyenza stay; the add-ons' pages come and go with their switches.
    pub const HIDEABLE: [PageKind; 5] = [PageKind::Media, PageKind::Today, PageKind::Shelf, PageKind::Tasks, PageKind::Focus];

    pub fn hideable(self) -> bool {
        Self::HIDEABLE.contains(&self)
    }
}

/// The tabs in the header: every page but Settings, and Health, IELTS, Notes,
/// System, Script, Record, Prayer, Stocks and Notifications only while their
/// add-ons are on. Music, Today, Shelf, Tasks and Focus unless Settings took
/// them off the row; one of those still shows while its page is open (a file
/// dropped on the island opens the Shelf). AI and Oyenza are always there
/// (signed out, Oyenza is where to sign in). Add-ons is last, in both apps.
pub fn tabs(st: &AppState) -> Vec<PageKind> {
    row(st, Some(st.page))
}

/// Where the island goes when its page leaves the row (an add-on switched
/// off, a tab hidden): the first tab, Music unless it's hidden.
pub fn first_tab(st: &AppState) -> PageKind {
    // Never empty: AI, Oyenza and Add-ons always have tabs.
    row(st, None)[0]
}

fn row(st: &AppState, open: Option<PageKind>) -> Vec<PageKind> {
    PageKind::ALL
        .into_iter()
        .filter(|&p| match p {
            PageKind::Health => st.health_on(),
            PageKind::Ielts => st.ielts_on(),
            PageKind::Notes => st.notes_on(),
            PageKind::System => st.system_on(),
            PageKind::Prayer => st.prayer_on(),
            PageKind::Script => st.script_on(),
            PageKind::Stocks => st.stocks_on(),
            PageKind::Downloads => st.downloader_on(),
            PageKind::Record => st.record_on(),
            PageKind::Notifications => st.notifications_on(),
            p if p.hideable() => !st.tab_hidden(p) || open == Some(p),
            p => p.tabbed(),
        })
        .collect()
}

/// The tab after (or before) `p` among [`tabs`], wrapping around.
pub fn step_tab(p: PageKind, st: &AppState, forward: bool) -> PageKind {
    let tabs = tabs(st);
    let n = tabs.len();
    let i = tabs.iter().position(|t| *t == p).unwrap_or(0);
    tabs[if forward { (i + 1) % n } else { (i + n - 1) % n }]
}

pub fn content_height(p: PageKind, st: &AppState) -> f32 {
    match p {
        PageKind::Media => media::height(st),
        PageKind::Focus => focus::height(st),
        PageKind::Ai => usage::height(st),
        PageKind::Health => health::height(st),
        PageKind::Ielts => ielts::height(st),
        PageKind::Notes => notes::height(st),
        PageKind::Shelf => shelf::height(st),
        PageKind::Tasks => tasks::height(st),
        PageKind::Today => today::height(st),
        PageKind::System => system::height(st),
        PageKind::Script => script::height(st),
        PageKind::Record => record::height(st),
        PageKind::Oyenza => oyenza::height(st),
        PageKind::Prayer => prayer::height(st),
        PageKind::Stocks => stocks::height(st),
        PageKind::Downloads => downloads::height(st),
        PageKind::Notifications => notifications::height(st),
        PageKind::Addons => addons::height(st),
        PageKind::Settings => settings::height(st),
    }
}

/// The next time (mono seconds) the expanded `page` changes on its own and
/// needs a frame without input, if any (e.g. an Undo pill expiring). Meant
/// for the app's wake timer; until it asks, such changes show on the next
/// repaint.
pub fn next_wake(page: PageKind, st: &AppState) -> Option<f64> {
    let wake = match page {
        PageKind::Tasks => tasks::next_wake(st),
        PageKind::Shelf => shelf::next_wake(st),
        PageKind::Oyenza => oyenza::next_wake(st),
        PageKind::Settings => settings::next_wake(st),
        PageKind::Script => script::next_wake(st),
        PageKind::Record => record::next_wake(st),
        PageKind::Prayer => prayer::next_wake(st),
        PageKind::Today => today::next_wake(st),
        PageKind::Downloads => downloads::next_wake(st),
        PageKind::Ielts => ielts::next_wake(st),
        PageKind::Notes => notes::next_wake(st),
        _ => None,
    };
    // Oyen in the Oyenza tab (and beside the pin in his theme) blinks and
    // flicks an ear now and then; asleep at night, he stays still.
    let oyen = (!crate::activities::night(st)).then(|| crate::oyen::next_rest_move(st.now));
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
    if crate::pill::expanded::draw(ui, st, body, page, transition) {
        return;
    }
    let tr = transition.filter(|tr| tr.t < 1.0 && tr.from != page);
    let header = header_rect(body);
    let tip = draw_header(ui, st, header, page, tr);
    draw_pages(ui, st, body, header, page, tr);
    // Hover labels sit on top of the page, outside the content clip.
    if let Some(tip) = tip {
        draw_tip(ui, header, &tip);
    }
}

/// The page content's rect inside the expanded island `body`: below the
/// header band, `PAD` in from the sides and the bottom.
pub(crate) fn content_area(body: Rect) -> Rect {
    Rect::new(body.x + PAD, body.y + HEADER_H, body.w - 2.0 * PAD, (body.h - HEADER_H - PAD).max(0.0))
}

/// The active page in [`content_area`], clipped to the island below the
/// `header` row. During the page switch `tr` (one in progress, as
/// [`draw_expanded`] filters it) the old page fades out sliding away while
/// the new one fades in. Every theme's frame draws its pages with this, so
/// a switch moves the same in all of them.
pub(crate) fn draw_pages(ui: &mut Ui, st: &mut AppState, body: Rect, header: Rect, page: PageKind, tr: Option<PageTransition>) {
    let area = content_area(body);
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
}

pub(crate) fn draw_page(ui: &mut Ui, st: &mut AppState, area: Rect, page: PageKind) {
    match page {
        PageKind::Media => media::draw(ui, st, area),
        PageKind::Focus => focus::draw(ui, st, area),
        PageKind::Ai => usage::draw(ui, st, area),
        PageKind::Health => health::draw(ui, st, area),
        PageKind::Ielts => ielts::draw(ui, st, area),
        PageKind::Notes => notes::draw(ui, st, area),
        PageKind::Shelf => shelf::draw(ui, st, area),
        PageKind::Tasks => tasks::draw(ui, st, area),
        PageKind::Today => today::draw(ui, st, area),
        PageKind::System => system::draw(ui, st, area),
        PageKind::Script => script::draw(ui, st, area),
        PageKind::Record => record::draw(ui, st, area),
        PageKind::Oyenza => oyenza::draw(ui, st, area),
        PageKind::Prayer => prayer::draw(ui, st, area),
        PageKind::Stocks => stocks::draw(ui, st, area),
        PageKind::Downloads => downloads::draw(ui, st, area),
        PageKind::Notifications => notifications::draw(ui, st, area),
        PageKind::Addons => addons::draw(ui, st, area),
        PageKind::Settings => settings::draw(ui, st, area),
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
pub(crate) fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

// ---------------------------------------------------------------- header

const TAB_H: f32 = 28.0;
/// Inactive tabs are bare icons in a 30 DIP slot.
const TAB_W: f32 = 30.0;
const TAB_GAP: f32 = 2.0;
const TAB_ICON: f32 = 16.0;
/// Oyen's head in the Oyenza tab (a little wider than the icons: his ears
/// take the top of it).
const OYEN_TAB_W: f32 = 18.0;
/// The active tab's label starts this far in and ends `TAB_LABEL_END` before its edge.
const TAB_LABEL_X: f32 = 28.0;
const TAB_LABEL_END: f32 = 12.0;
/// Clock to date, and date (or clock) to the Settings button, which sits
/// right of the pin.
const DATE_GAP: f32 = 12.0;
const PIN_GAP: f32 = 10.0;
const PIN: f32 = 28.0;
const GEAR: f32 = 28.0;
const GEAR_GAP: f32 = 2.0;
/// His theme: Oyen peeks up beside the pin, as he does beside the clock when
/// the island is closed.
const OYEN_W: f32 = 24.0;
const OYEN_GAP: f32 = 6.0;
/// Hover labels: dwell, fade-in, capsule metrics.
const TIP_DELAY: f64 = 0.4;
const TIP_FADE: f64 = 0.12;
pub(crate) const TIP_H: f32 = 22.0;
pub(crate) const TIP_PAD: f32 = 8.0;
const TIP_RADIUS: f32 = 8.0;
/// White 0.16 over the black island, made opaque so page text can't show through.
pub(crate) const TIP_FILL: Color = Color::rgba(0.16, 0.16, 0.16, 1.0);
/// `tab_hover` slots of the pin and Settings buttons (tabs use their place in the row).
const PIN_SLOT: usize = PageKind::ALL.len();
const GEAR_SLOT: usize = PIN_SLOT + 1;
/// The Settings button's widget id (the self-test clicks it).
pub const GEAR_ID: &str = "header.gear";

/// A hover label waiting to be painted above the page.
pub(crate) struct Tip {
    pub(crate) anchor: Rect,
    pub(crate) text: &'static str,
    pub(crate) alpha: f32,
}

/// Tab slots (x, width) of `tabs`, with `active` showing its label.
fn tab_layout(ui: &mut Ui, x0: f32, tabs: &[PageKind], active: PageKind, label_st: &TextStyle, scale: f32) -> Vec<(f32, f32)> {
    let mut x = x0;
    tabs.iter()
        .map(|&p| {
            let w = if p == active { snap(TAB_LABEL_X + ui.c.measure(p.title(), label_st, 200.0).0 + TAB_LABEL_END, scale) } else { TAB_W };
            let slot = (snap(x, scale), w);
            x += w + TAB_GAP;
            slot
        })
        .collect()
}

/// Tabs on the left; pin, date and clock on the right. During a page switch
/// the tabs re-flow, the selection pill slides and the labels cross-fade.
/// Returns the hover label to paint once the page is drawn.
pub(crate) fn draw_header(ui: &mut Ui, st: &mut AppState, r: Rect, page: PageKind, tr: Option<PageTransition>) -> Option<Tip> {
    let scale = ui.c.scale().max(0.5);
    let cy = r.center().y;
    let label_st = TextStyle::body().weight(Weight::Semibold);
    let from = tr.map_or(page, |t| t.from);
    let e = tr.map_or(1.0, |t| ease_out_cubic(t.t / TABS_END));
    let row = self::tabs(st);
    let lay_from = tab_layout(ui, r.x, &row, from, &label_st, scale);
    let lay_to = if from == page { lay_from.clone() } else { tab_layout(ui, r.x, &row, page, &label_st, scale) };
    let tabs: Vec<Rect> =
        (0..row.len()).map(|i| Rect::new(lerp(lay_from[i].0, lay_to[i].0, e), cy - TAB_H / 2.0, lerp(lay_from[i].1, lay_to[i].1, e), TAB_H)).collect();
    // A page that isn't in the row (Oyenza right after signing out) counts as the first tab.
    let place = |p: PageKind| row.iter().position(|t| *t == p).unwrap_or(0);
    let (fi, pi) = (place(from), place(page));
    let tabs_end = tabs[tabs.len() - 1].right();
    // How much the selection sits on the gear (Settings) rather than on a tab.
    let on_gear = |p: PageKind| if p.tabbed() { 0.0 } else { 1.0 };
    let gear_k = lerp(on_gear(from), on_gear(page), e);

    // Selection pill, gliding from the old tab to the new one. To or from
    // Settings it stays on its tab and fades while the gear takes it.
    let (a, b) = (if from.tabbed() { fi } else { pi }, if page.tabbed() { pi } else { fi });
    let pill = Rect::new(lerp(lay_from[a].0, lay_to[b].0, e), cy - TAB_H / 2.0, lerp(lay_from[a].1, lay_to[b].1, e), TAB_H);
    if gear_k < 1.0 {
        ui.c.with_alpha(1.0 - gear_k, |c| c.fill_rrect(pill, TAB_H / 2.0, Color::white(0.14)));
    }

    // Each opening of the island is Oyen's arrival in his tab.
    if st.ui.oyen_tab.is_none() {
        st.ui.oyen_tab = Some((crate::oyen::TabMove::Arrive, ui.now));
    }
    let mut hover: Option<(usize, Rect)> = None;
    for (i, &p) in row.iter().enumerate() {
        let tr = tabs[i];
        let resp = ui.button(wid_n("tab", i as u64), tr);
        let active = p == page;
        if resp.hovered && !active {
            ui.c.fill_rrect(tr, TAB_H / 2.0, palette::SURFACE);
            hover = Some((i, tr));
        }
        if p == PageKind::Oyenza {
            draw_oyen_tab(ui, st, r, tr);
        } else {
            let k = if active { e } else if from.tabbed() && i == fi { 1.0 - e } else { 0.0 };
            let ic = if resp.hovered { palette::TEXT } else { Color::lerp(palette::TEXT_FAINT, palette::TEXT, k) };
            let ix = snap(tr.x + (TAB_W - TAB_ICON) / 2.0, scale);
            ui.c.icon(p.icon(), Rect::new(ix, snap(cy - TAB_ICON / 2.0, scale), TAB_ICON, TAB_ICON), ic);
        }
        if resp.clicked && !active {
            st.cmd(Command::SetPage(p));
            if p == PageKind::Oyenza {
                st.ui.oyen_tab = Some((crate::oyen::TabMove::Picked, ui.now));
            }
        }
    }
    if from.tabbed() && (fi != pi || !page.tabbed()) {
        draw_tab_label(ui, from, tabs[fi], &label_st, 1.0 - e);
    }
    if page.tabbed() {
        draw_tab_label(ui, page, tabs[pi], &label_st, e);
    }

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

    let gear_r = Rect::new(snap(left - PIN_GAP - GEAR, scale), cy - GEAR / 2.0, GEAR, GEAR);
    let pin_r = Rect::new(snap(gear_r.x - GEAR_GAP - PIN, scale), cy - PIN / 2.0, PIN, PIN);
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
    // The gear opens the Settings window. The Settings page shows only the
    // Update sheet (a new version's notice): the gear is lit like a selected
    // tab while it shows, and a press then goes back to the page it was on.
    if gear_r.x > tabs_end + 8.0 {
        if gear_k > 0.0 {
            ui.c.with_alpha(gear_k, |c| c.fill_rrect(gear_r, GEAR / 2.0, Color::white(0.14)));
        }
        let color = Color::lerp(palette::TEXT_FAINT, palette::TEXT, gear_k);
        let resp = ui.icon_button(wid(GEAR_ID), gear_r, Icon::Gear, color, 16.0);
        if resp.hovered {
            hover = Some((GEAR_SLOT, gear_r));
        }
        if resp.clicked {
            if page == PageKind::Settings {
                let back = st.ui.before_settings.take().filter(|p| row.contains(p)).unwrap_or(row[0]);
                st.cmd(Command::SetPage(back));
            } else {
                st.cmd(Command::OpenSettings);
            }
        }
    }

    // Hover labels: after a short dwell over an inactive tab, the pin or the gear.
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
    let text = match row.get(slot) {
        Some(p) => p.title(),
        None if slot == GEAR_SLOT && page == PageKind::Settings => "Close settings",
        None if slot == GEAR_SLOT => "Settings",
        None if st.pinned => "Unpin",
        None => "Keep open",
    };
    Some(Tip { anchor, text, alpha })
}

/// The Oyenza tab's icon: Oyen himself, the Oyenza mark, in his own colours
/// whether the tab is picked or not (`oyen::tab`). His eyes follow the pointer
/// while it is over the tab `row`.
fn draw_oyen_tab(ui: &mut Ui, st: &AppState, row: Rect, tab: Rect) {
    let c = Point::new(tab.x + TAB_W / 2.0, row.center().y);
    let look = ui.input.mouse.filter(|m| row.contains(*m)).map(|m| ((m.x - c.x) / (row.w / 2.0)).clamp(-1.0, 1.0));
    let clip = Rect::new(tab.x, tab.y, TAB_W, tab.h);
    if crate::oyen::tab(ui.c, c, OYEN_TAB_W, clip, ui.now, st.ui.oyen_tab, crate::activities::night(st), look) {
        ui.request_anim();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::OyenzaLink;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    fn state() -> AppState {
        AppState::new(Settings::default(), Store::default())
    }

    #[test]
    fn the_row_is_the_island_s_own_pages_then_oyenza_then_the_plus() {
        // Music, then Today beside it; Focus where Today was; AI after it;
        // Oyenza last, signed in or not.
        use PageKind::*;
        let mut st = state();
        assert_eq!(tabs(&st), [Media, Today, Shelf, Tasks, Focus, Ai, Oyenza, Addons]);
        st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
        assert_eq!(tabs(&st), [Media, Today, Shelf, Tasks, Focus, Ai, Oyenza, Addons]);
        // The add-ons' pages come after Oyenza (Health, Oyenza's steps, first), before the "+".
        for id in ["system", "script", "health"] {
            st.addons.insert(id, poni_core::addons::Status::Ready);
        }
        assert_eq!(&tabs(&st)[6..], [Oyenza, Health, System, Script, Addons]);
        // Clipboard left the app: an island last open there opens on Music.
        assert_eq!(PageKind::from_key("clipboard"), Media);
    }

    #[test]
    fn hidden_tabs_leave_the_row_but_not_while_their_page_is_open() {
        use PageKind::*;
        let mut st = state();
        st.settings.hidden_tabs = vec!["today".into(), "tasks".into(), "oyenza".into(), "addons".into()];
        st.page = Focus;
        // AI, Oyenza and the "+" can't be hidden.
        assert_eq!(tabs(&st), [Media, Shelf, Focus, Ai, Oyenza, Addons]);
        assert_eq!(first_tab(&st), Media);
        // Opened from elsewhere (a meeting's notice opens Today): its tab
        // shows while it's open, and stepping on leaves it behind.
        st.page = Today;
        assert_eq!(tabs(&st), [Media, Today, Shelf, Focus, Ai, Oyenza, Addons]);
        st.page = step_tab(Today, &st, true);
        assert_eq!(st.page, Shelf);
        assert!(!tabs(&st).contains(&Today));
        // Every one of them hidden: the island opens on AI.
        st.settings.hidden_tabs = PageKind::HIDEABLE.iter().map(|p| p.key().to_string()).collect();
        st.page = Ai;
        assert_eq!(tabs(&st), [Ai, Oyenza, Addons]);
        assert_eq!(first_tab(&st), Ai);
        // A start on a hidden page opens on the first tab instead.
        let mut settings = poni_core::settings::Settings::default();
        settings.start_page = "today".into();
        settings.remember_page = false;
        settings.hidden_tabs = vec!["media".into(), "today".into()];
        assert_eq!(AppState::new(settings, Store::default()).page, Shelf);
    }

    #[test]
    fn settings_has_the_gear_not_a_tab() {
        let mut st = state();
        assert!(!tabs(&st).contains(&PageKind::Settings));
        assert!(tabs(&st).contains(&PageKind::Oyenza), "signed out it's where to sign in");
        st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
        let row = tabs(&st);
        assert!(row.contains(&PageKind::Oyenza) && !row.contains(&PageKind::Settings));
        // Stepping through the tabs never lands on Settings.
        let mut p = PageKind::Media;
        for _ in 0..2 * PageKind::ALL.len() {
            p = step_tab(p, &st, true);
            assert!(p.tabbed(), "stepped onto {p:?}");
        }
    }

    #[test]
    fn settings_keeps_its_key_and_slides_in_from_the_right() {
        assert_eq!(PageKind::from_key("settings"), PageKind::Settings);
        assert!(PageKind::ALL.iter().all(|p| p.tab_index() <= PageKind::Settings.tab_index()));
    }

    #[test]
    fn addons_is_the_last_tab_and_never_makes_the_window_taller() {
        let mut st = state();
        assert_eq!(tabs(&st).last(), Some(&PageKind::Addons));
        st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
        assert_eq!(tabs(&st).last(), Some(&PageKind::Addons));
        assert_eq!(PageKind::from_key("addons"), PageKind::Addons);
        // The window is as tall as the tallest page (System, laid out for
        // whether it shows or not); the add-ons' rows and Stocks stay under it.
        assert!(content_height(PageKind::Addons, &st) <= content_height(PageKind::System, &st));
        assert!(content_height(PageKind::Stocks, &st) <= content_height(PageKind::System, &st));
        // Notifications with as many apps as it lists.
        for i in 0..poni_core::notifications::MAX_APPS {
            st.settings.notifications.remember(&format!("App.{i}"), &format!("App {i}"));
        }
        assert!(content_height(PageKind::Notifications, &st) <= content_height(PageKind::System, &st));
        // Signed out, Oyenza's sign-in is as tall as its code, so starting to
        // sign in doesn't move the island.
        st.oyenza.link = OyenzaLink::Off;
        let signed_out = content_height(PageKind::Oyenza, &st);
        st.oyenza.link = OyenzaLink::Connecting { code: "ABCD-EFGH".into(), url: String::new(), until: 600.0 };
        assert_eq!(signed_out, content_height(PageKind::Oyenza, &st));
    }

    #[test]
    fn ai_is_always_a_tab_and_never_makes_the_window_taller() {
        let st = state();
        assert!(tabs(&st).contains(&PageKind::Ai), "a core tab: no add-on to switch on");
        assert_eq!(PageKind::from_key("ai"), PageKind::Ai);
        // An island last open on the AI tab opens there again.
        let store = Store { last_page: "ai".into(), ..Store::default() };
        assert_eq!(AppState::new(Settings { remember_page: true, ..Settings::default() }, store).page, PageKind::Ai);
        assert!(content_height(PageKind::Ai, &st) <= content_height(PageKind::System, &st));
    }

    #[test]
    fn system_health_and_stocks_have_a_tab_only_while_their_add_ons_are_on() {
        for (page, addon) in [
            (PageKind::System, poni_core::addons::SYSTEM),
            (PageKind::Health, poni_core::addons::HEALTH),
            (PageKind::Stocks, poni_core::addons::STOCKS),
        ] {
            let mut st = state();
            assert!(!tabs(&st).contains(&page), "{} is off for everyone until switched on", addon.id);
            st.settings.set_addon(addon.id, true);
            st.addons.insert(addon.id, poni_core::addons::Status::Ready);
            assert!(tabs(&st).contains(&page));
            // An island last open on its page opens on the first tab while it's off.
            let store = Store { last_page: page.key().into(), ..Store::default() };
            let mut settings = Settings { remember_page: true, ..Settings::default() };
            assert_eq!(AppState::new(settings.clone(), store.clone()).page, PageKind::ALL[0]);
            settings.set_addon(addon.id, true);
            assert_eq!(AppState::new(settings, store).page, page);
        }
    }

    #[test]
    fn downloads_has_a_tab_only_with_studio_pro() {
        // Notifications is free (Luca, 28 Oct): its tab needs no sign-in.
        let mut st = state();
        st.settings.set_addon(poni_core::addons::NOTIFICATIONS.id, true);
        st.addons.insert(poni_core::addons::NOTIFICATIONS.id, poni_core::addons::Status::Ready);
        assert_eq!(tabs(&st).contains(&PageKind::Notifications), cfg!(windows), "signed out, no Pro");
        for (page, addon) in [(PageKind::Downloads, poni_core::addons::DOWNLOADER)] {
            let mut st = state();
            st.downloader_offered = true;
            st.settings.set_addon(addon.id, true);
            st.addons.insert(addon.id, poni_core::addons::Status::Ready);
            assert!(!tabs(&st).contains(&page), "{}: signed out", addon.id);
            st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
            assert!(!tabs(&st).contains(&page), "{}: before the server's answer", addon.id);
            st.oyenza.pro = Some(false);
            assert!(!tabs(&st).contains(&page), "{}: an account without Pro", addon.id);
            st.oyenza.pro = Some(true);
            assert!(tabs(&st).contains(&page), "{}", addon.id);
            // Signed out again: gone, whatever the server said before.
            st.oyenza.link = OyenzaLink::Off;
            assert!(!tabs(&st).contains(&page), "{}", addon.id);
            // Pro isn't known yet at start: an island last open on its page opens on the first tab.
            let store = Store { last_page: page.key().into(), ..Store::default() };
            let mut settings = Settings { remember_page: true, ..Settings::default() };
            settings.set_addon(addon.id, true);
            assert_eq!(AppState::new(settings, store).page, PageKind::ALL[0], "{}", addon.id);
        }
    }

    #[test]
    fn today_keeps_its_tiles_only_for_a_server_without_the_day() {
        use crate::state::AgendaState;
        let mut st = state();
        // Signed out: the sign-in card, so no stats are read for tiles.
        assert!(!today::shows_tiles(&st));
        st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
        assert!(!today::shows_tiles(&st), "getting the day");
        st.oyenza.agenda_state = AgendaState::Unavailable;
        assert!(today::shows_tiles(&st));
        // The day for today, Calendar switched on: the day, and a look at the next minute.
        let today = st.day_key();
        st.oyenza.agenda = Some(poni_core::agenda::sample(&today, 600));
        st.oyenza.agenda_state = AgendaState::Ready;
        assert!(!today::shows_tiles(&st));
        assert!(today::next_wake(&st).is_some_and(|t| t > st.now && t <= st.now + 60.0));
    }

    #[test]
    fn prayer_has_a_tab_only_while_its_add_on_is_on_and_ready() {
        let mut st = state();
        assert!(!tabs(&st).contains(&PageKind::Prayer));
        st.addons.insert("sholat", poni_core::addons::Status::Ready);
        assert!(!tabs(&st).contains(&PageKind::Prayer), "no places read yet");
        let list = poni_core::prayer::PlaceList::parse(r#"{"places":[]}"#).unwrap();
        st.prayer.places = Some(std::sync::Arc::new(list));
        let row = tabs(&st);
        assert_eq!(&row[row.len() - 2..], &[PageKind::Prayer, PageKind::Addons]);
        assert!(content_height(PageKind::Prayer, &st) <= content_height(PageKind::Tasks, &st));
        st.addons.insert("sholat", poni_core::addons::Status::Downloading { done: 1, total: 2 });
        assert!(!tabs(&st).contains(&PageKind::Prayer));
    }

    #[test]
    fn ielts_has_a_tab_only_while_its_add_on_is_on_after_health() {
        use PageKind::{Addons, Health, Ielts, Oyenza};
        let mut st = state();
        assert!(!tabs(&st).contains(&Ielts), "off for everyone until switched on");
        st.addons.insert("ielts", poni_core::addons::Status::Ready);
        st.addons.insert("health", poni_core::addons::Status::Ready);
        assert_eq!(&tabs(&st)[6..], [Oyenza, Health, Ielts, Addons]);
        // Signed out it shows the sign-in card; signed in, every state is as
        // tall, and never taller than System: the window keeps its size.
        let signed_out = content_height(Ielts, &st);
        st.oyenza.link = OyenzaLink::Connected { account: "a•••@example.com".into() };
        let h = content_height(Ielts, &st);
        assert!(signed_out < h && h <= content_height(PageKind::System, &st), "{signed_out} {h}");
        for s in [poni_core::ielts::Sample::Talking, poni_core::ielts::Sample::Result, poni_core::ielts::Sample::MicDenied] {
            st.ielts = poni_core::ielts::Coach::sample(s, 10.0);
            assert_eq!(content_height(Ielts, &st), h);
        }
        // An island last open on it opens on the first tab once it's off.
        let store = Store { last_page: "ielts".into(), ..Store::default() };
        let mut settings = Settings { remember_page: true, ..Settings::default() };
        assert_eq!(AppState::new(settings.clone(), store.clone()).page, PageKind::Media);
        settings.set_addon("ielts", true);
        assert_eq!(AppState::new(settings, store).page, Ielts);
    }

    #[test]
    fn script_has_a_tab_only_while_its_add_on_is_on() {
        let mut st = state();
        assert!(!tabs(&st).contains(&PageKind::Script), "off for everyone until switched on");
        st.addons.insert("script", poni_core::addons::Status::Ready);
        assert!(tabs(&st).contains(&PageKind::Script));
        assert_eq!(tabs(&st).last(), Some(&PageKind::Addons));
    }

    #[test]
    fn record_has_a_tab_once_the_recorder_is_installed() {
        let mut st = state();
        assert!(!tabs(&st).contains(&PageKind::Record), "no recorder: no tab");
        st.recorder = true;
        assert!(!tabs(&st).contains(&PageKind::Record), "a recorder that never wrote (1.0.0) can't read Record's links");
        st.store.recorder_island = true;
        let row = tabs(&st);
        if cfg!(target_os = "macos") {
            assert!(!row.contains(&PageKind::Record), "Windows only for now");
            return;
        }
        assert!(row.contains(&PageKind::Record), "installing the recorder is the switch");
        assert_eq!(row.last(), Some(&PageKind::Addons));
        st.settings.set_addon("recorder", false);
        assert!(!tabs(&st).contains(&PageKind::Record), "switched off: gone");
        st.settings.set_addon("recorder", true);
        st.addons.insert("script", poni_core::addons::Status::Ready);
        let row = tabs(&st);
        let at = |p| row.iter().position(|t| *t == p).unwrap();
        assert_eq!(at(PageKind::Record), at(PageKind::Script) + 1, "beside the Script page");
    }

    #[test]
    fn neither_the_record_page_nor_six_add_ons_make_the_window_taller() {
        let mut st = state();
        st.recorder = true;
        st.store.recorder_island = true;
        let tallest = content_height(PageKind::System, &st);
        assert!(content_height(PageKind::Record, &st) <= tallest);
        assert!(content_height(PageKind::Addons, &st) <= tallest, "{} > {tallest}", content_height(PageKind::Addons, &st));
    }

    #[test]
    fn an_island_last_on_the_script_page_opens_on_the_first_tab_once_it_is_off() {
        let store = Store { last_page: "script".into(), ..Store::default() };
        let mut settings = Settings { remember_page: true, ..Settings::default() };
        assert_eq!(AppState::new(settings.clone(), store.clone()).page, PageKind::ALL[0]);
        settings.set_addon("script", true);
        assert_eq!(AppState::new(settings, store).page, PageKind::Script);
    }

    #[test]
    fn settings_never_makes_the_window_taller() {
        // The window fits the tallest page; System is always 256 DIPs (Settings
        // has three rows of switches since Clipboard history left).
        let mut st = state();
        assert!(content_height(PageKind::Settings, &st) <= content_height(PageKind::System, &st));
        // Nor its new version's row, nor the Update sheet in its place.
        let rel = poni_core::update::Release {
            version: "9.9.9".into(),
            date: String::new(),
            headline: "Something new".into(),
            notes: String::new(),
            windows: None,
            mac: None,
        };
        st.update.offer = Some(rel);
        let row = content_height(PageKind::Settings, &st);
        st.update.sheet = true;
        let sheet = content_height(PageKind::Settings, &st);
        assert!(row <= content_height(PageKind::System, &st) && sheet <= row, "{row} {sheet}");
    }
}
