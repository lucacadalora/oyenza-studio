//! Stocks page (the Stocks add-on): the IHSG and up to ten IDX stocks, laid
//! out as Apple's Stocks is on a Mac. On the left the list: each row's code
//! and name, the day's line against the previous close (dotted), the price
//! and a change pill (a click on any pill switches all of them between
//! percent, points and market cap). On the right the row picked: its price,
//! a chart over 1D, 1M, 6M or 1Y that morphs from one line into the next,
//! the price and time under the pointer, and the day's numbers.
//!
//! "Add a stock" turns the list into a search through every listed stock's
//! code and name (type, then Up/Down and Enter, or a click); Edit gives each
//! row a remove button and a handle to drag it to another place. Rows glide
//! to their places as they come, go and move. The list lives in data.json
//! (`store.stocks`); the prices come from oyenza.trade through
//! `services::stocks`, asked only while this page is on screen ([`watch`]),
//! and run 10 minutes behind the exchange, which the page says.

use crate::gfx::{TextStyle, Weight};
use crate::icons::Icon;
use crate::services::stocks::{Trouble, Watch};
use crate::state::AppState;
use crate::ui::{wid, wid_n, KeyInput, PillStyle, Ui, WidgetId, PILL_SMALL};
use poni_core::geom::{lerp, palette, Color, Point, Rect};
use poni_core::stocks::{self, Board, Quote, Range, Trend, INDEX};
use poni_core::svgpath::Seg;

/// The System page's height: the window never grows for this page.
const HEIGHT: f32 = 256.0;
/// The list's share of the width stays between these.
const LIST_MIN: f32 = 232.0;
const LIST_MAX: f32 = 300.0;
const COL_GAP: f32 = 20.0;
const ROW_H: f32 = 48.0;
/// The row under the list: Add a stock, Edit, and where the prices come from.
const ADD_H: f32 = 46.0;
/// Text inside a row starts this far in (its highlight reaches the edge).
const ROW_PAD: f32 = 8.0;
const PILL_W: f32 = 66.0;
const PILL_H: f32 = 20.0;
const SPARK_W: f32 = 50.0;
/// Edit mode moves each row's text right for its remove button.
const EDIT_W: f32 = 26.0;
/// Glides (the scroll, rows moving, the picked row's highlight): about 95%
/// of the way in 0.19 s, eased like a flick coming to rest.
const GLIDE: f32 = 16.0;
/// A chart morphing into another stock's or range's line.
const MORPH_S: f64 = 0.45;
/// Every chart line is resampled to this many points, so one morphs into
/// the next point for point.
const SAMPLES: usize = 160;
const FIELD_H: f32 = 34.0;
const RESULT_H: f32 = 36.0;
const RESULTS: usize = 5;
/// The right side, from its top: title, price, chart, the axis's two
/// labels, then three rows of the day's numbers.
const PRICE_Y: f32 = 28.0;
const CHART_Y: f32 = 62.0;
const CHART_H: f32 = 106.0;
const AXIS_H: f32 = 16.0;
const STATS_Y: f32 = 192.0;
const STAT_H: f32 = 32.0;
const SEG_W: f32 = 32.0;
const SEG_H: f32 = 22.0;

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// What the page wants from `services::stocks`: nothing unless `open` (on
/// screen with someone at the computer); then the list's prices, the picked
/// stock's year while a longer range shows, and every name while the search
/// is open, each until it has come.
pub fn watch(st: &AppState, open: bool) -> Watch {
    let open = open && st.stocks_on();
    let w = &st.store.stocks;
    Watch {
        open,
        codes: w.codes.clone(),
        chart: (open && w.range != Range::Day && !st.stocks.years.contains_key(&w.pick)).then(|| w.pick.clone()),
        names: open && st.ui.stocks.adding && st.stocks.names.is_none(),
    }
}

/// The page's own state between frames (`st.ui.stocks`).
#[derive(Clone, Debug, Default)]
pub struct StocksUi {
    /// The search is open, what's typed in it and the highlighted result.
    pub adding: bool,
    pub query: String,
    hit: usize,
    /// The search field takes the keyboard on its first frame.
    focus: bool,
    /// Edit mode (remove buttons, handles) and how far its slide has come.
    pub editing: bool,
    edit_k: f32,
    /// The list or the search fading in after a switch between them.
    swap_k: f32,
    /// The row dragged in edit mode: its code and its top as the press began.
    drag: Option<(String, f32)>,
    /// The list's scroll: where the wheel sent it, and where it shows on its way.
    scroll: f32,
    shown_scroll: f32,
    /// Each row's shown top in the list, gliding to its place.
    rows: Vec<(String, f32)>,
    /// The picked row's highlight, gliding from row to row.
    pick_y: Option<f32>,
    /// When the page last drew (mono seconds): the glides' step.
    drawn: f64,
    chart: Morph,
    /// Still pictures (the self-test, the Mac renders): every glide and
    /// morph is at its end.
    still: bool,
}

impl StocksUi {
    /// From the next frame on, every glide and morph shows at its end, so
    /// the self-test's and the Mac renders' pictures hold still.
    pub fn settle(&mut self, now: f64) {
        self.still = true;
        self.drawn = now;
        self.swap_k = 1.0;
        self.edit_k = if self.editing { 1.0 } else { 0.0 };
    }
}

/// The chart's line, morphing from what it showed into what it shows now.
#[derive(Clone, Debug, Default)]
struct Morph {
    /// What the line is of: the stock, the range and the answer (a new one moves it on).
    key: String,
    /// `SAMPLES` points across the chart, 0..1 both ways (y up).
    from: Vec<(f32, f32)>,
    to: Vec<(f32, f32)>,
    /// The dotted previous close (1D only), 0..1 up the chart.
    base_from: Option<f32>,
    base_to: Option<f32>,
    color_from: Option<Color>,
    color_to: Option<Color>,
    t0: f64,
}

/// What a still picture of the page shows (the self-test's, the Mac renders').
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// The IHSG's day.
    Day,
    /// Bank BCA over a year.
    Year,
    /// "Add a stock" open, "bank" typed.
    Search,
    /// Edit mode.
    Edit,
}

/// The page as a still picture shows it: the starter list on a made-up
/// market at 13:40 on the exchange's clock (`stocks::sample`), Bank BCA's
/// made-up year and a few names for the search, every glide at its end.
pub fn picture(st: &mut AppState, shows: Picture, now: f64) {
    st.store.stocks = stocks::Watchlist::default();
    st.stocks = crate::state::StocksState::default();
    st.ui.stocks = StocksUi::default();
    let mut board = stocks::sample(&st.day_key(), 13 * 60 + 40);
    board.at = st.unix - 8;
    board.t = board.at - board.delay_s;
    let today = poni_core::agenda::day_num(&st.day_key()).unwrap_or(0);
    let bbca = board.quote("BBCA").map_or(4_940.0, |q| q.price);
    st.stocks.years.insert("BBCA".into(), stocks::sample_history("BBCA", today, bbca));
    st.stocks.board = Some(board);
    st.stocks.checked = Some(st.unix - 8);
    st.stocks.names = Some(std::sync::Arc::new(stocks::sample_names()));
    if shows == Picture::Year {
        st.store.stocks.pick = "BBCA".into();
        st.store.stocks.range = Range::Year;
    }
    let ui = &mut st.ui.stocks;
    ui.adding = shows == Picture::Search;
    if ui.adding {
        ui.query = "bank".into();
    }
    ui.editing = shows == Picture::Edit;
    ui.settle(now);
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    let now = ui.now;
    let s = &mut st.ui.stocks;
    // After an idle gap (the frame clock was parked) step as if one frame
    // passed; still pictures jump to the end.
    let dt = if s.still {
        1.0
    } else if now > s.drawn {
        (now - s.drawn).min(1.0 / 30.0) as f32
    } else {
        0.0
    };
    s.drawn = now;
    let mut moving = false;
    s.swap_k = glide(s.swap_k, 1.0, dt, 0.01, &mut moving);
    let list_w = (area.w * 0.44).clamp(LIST_MIN, LIST_MAX).round();
    let left = Rect::new(area.x, area.y, list_w, area.h.min(HEIGHT));
    let right = Rect::new(left.right() + COL_GAP, area.y, (area.w - list_w - COL_GAP).max(0.0), left.h);
    let base = ui.c.alpha();
    ui.c.set_alpha(base * ease_out(st.ui.stocks.swap_k));
    if st.ui.stocks.adding {
        search(ui, st, left);
    } else {
        list(ui, st, left, dt, &mut moving);
    }
    ui.c.set_alpha(base);
    detail(ui, st, right);
    if moving {
        ui.request_anim();
    }
}

/// `pos` moved toward `target` for `dt` seconds; `moving` is set while it
/// isn't there yet (within `eps`).
fn glide(pos: f32, target: f32, dt: f32, eps: f32, moving: &mut bool) -> f32 {
    let p = pos + (target - pos) * (1.0 - (-dt * GLIDE).exp());
    if (target - p).abs() <= eps {
        target
    } else {
        *moving = true;
        p
    }
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A row's widgets are keyed by its code, so hover and press follow it when
/// it moves.
fn code_n(code: &str) -> u64 {
    code.bytes().fold(0, |h, b| (h << 8) | u64::from(b))
}

fn handle_id(code: &str) -> WidgetId {
    wid_n("stocks.handle", code_n(code))
}

fn trend_color(t: Trend) -> Color {
    match t {
        Trend::Up => palette::GREEN,
        Trend::Down => palette::RED,
        Trend::Flat => Color::white(0.36),
    }
}

// ---------------------------------------------------------------- the list

fn list(ui: &mut Ui, st: &mut AppState, r: Rect, dt: f32, moving: &mut bool) {
    let codes = st.store.stocks.codes.clone();
    let n = codes.len() + 1;
    let content_h = n as f32 * ROW_H + ADD_H;
    let max_scroll = (content_h - r.h).max(0.0);
    let card = super::card::pill_card(st, r);
    let accent = st.accent;
    let (board, names) = (st.stocks.board.as_ref(), st.stocks.names.as_deref());
    let (wl, s) = (&mut st.store.stocks, &mut st.ui.stocks);
    let mut save = false;

    let notches = ui.wheel_in(r);
    s.scroll = (s.scroll - notches * ROW_H).clamp(0.0, max_scroll);
    s.shown_scroll = glide(s.shown_scroll.clamp(0.0, max_scroll), s.scroll, dt, 0.25, moving);
    s.edit_k = glide(s.edit_k, if s.editing { 1.0 } else { 0.0 }, dt, 0.005, moving);

    // A drag in edit mode: the row follows the pointer and the list makes
    // room where it would land; letting go keeps the new order.
    let drag_dy = s.drag.as_ref().and_then(|(c, _)| {
        let held = ui.input.down && ui.input.pressed_id == Some(handle_id(c));
        held.then(|| ui.input.mouse.zip(ui.input.press_pos).map_or(0.0, |(m, p)| m.y - p.y))
    });
    if s.drag.is_some() && drag_dy.is_none() {
        s.drag = None;
        save = true;
    }
    if let (Some((code, y0)), Some(dy), false) = (s.drag.clone(), drag_dy, codes.is_empty()) {
        let to = ((y0 + dy) / ROW_H).round().clamp(1.0, codes.len() as f32) as usize - 1;
        if wl.codes.get(to) != Some(&code) {
            wl.move_to(&code, to);
        }
    }

    // Each row glides to its place; one new to the list starts there.
    let order: Vec<String> = std::iter::once(INDEX.to_string()).chain(wl.codes.iter().cloned()).collect();
    let mut shown = Vec::with_capacity(order.len());
    for (i, code) in order.iter().enumerate() {
        let target = i as f32 * ROW_H;
        let y = match (&s.drag, drag_dy) {
            (Some((c, y0)), Some(dy)) if c == code => (y0 + dy).clamp(ROW_H, (n - 1).max(1) as f32 * ROW_H),
            _ => match s.rows.iter().find(|(c, _)| c == code) {
                Some((_, y)) => glide(*y, target, dt, 0.25, moving),
                None => target,
            },
        };
        shown.push((code.clone(), y));
    }
    s.rows = shown;
    let top = r.y - s.shown_scroll;
    let pick_target = s.rows.iter().find(|(c, _)| *c == wl.pick).map_or(0.0, |p| p.1);
    let pick_y = match s.pick_y {
        Some(y) if s.drag.is_none() => glide(y, pick_target, dt, 0.25, moving),
        _ => pick_target,
    };
    s.pick_y = Some(pick_y);

    ui.c.clip(Rect::new(r.x - 8.0, r.y, r.w + 16.0, r.h));
    // The picked row's highlight (none in edit mode, where rows move instead).
    let hl = Rect::new(r.x, top + pick_y + 2.0, r.w, ROW_H - 4.0);
    if s.edit_k < 0.99 {
        let c = Color::white(0.10 * (1.0 - s.edit_k));
        super::card::fill_box(ui, card, hl, 10.0, c);
    }
    let span = board.map_or((540, 960), |b| b.span);
    let mut picked = None;
    let mut removed = None;
    let mut cycle = false;
    // The dragged row draws last, over the others.
    let mut draw_order: Vec<usize> = (0..s.rows.len()).collect();
    if let Some((dc, _)) = &s.drag {
        if let Some(i) = s.rows.iter().position(|(c, _)| c == dc) {
            draw_order.retain(|j| *j != i);
            draw_order.push(i);
        }
    }
    for i in draw_order {
        let (code, y) = (&s.rows[i].0, s.rows[i].1);
        let row = Rect::new(r.x, top + y, r.w, ROW_H);
        if row.bottom() < r.y || row.y > r.bottom() {
            continue;
        }
        let q = board.and_then(|b| b.quote(code));
        let name = q.map(|q| q.name.as_str()).or_else(|| names.and_then(|n| n.name(code))).unwrap_or(if code == INDEX { "IDX Composite" } else { "" });
        let dragged = s.drag.as_ref().is_some_and(|(c, _)| c == code);
        let act = draw_row(ui, RowView { code, name, q, span, show: wl.show, edit_k: s.edit_k, dragged, picked: *code == wl.pick }, row, card);
        match act {
            RowAct::Pick => picked = Some(code.clone()),
            RowAct::Cycle => cycle = true,
            RowAct::Remove => removed = Some(code.clone()),
            RowAct::Drag => {
                if s.drag.is_none() {
                    s.drag = Some((code.clone(), y));
                }
            }
            RowAct::None => {}
        }
    }

    // Under the rows: Add a stock (or the list is full), Edit or Done, and
    // where the prices come from.
    let add = Rect::new(r.x, top + n as f32 * ROW_H, r.w, ADD_H);
    let line = Rect::new(add.x + ROW_PAD, add.y + 4.0, add.w - 2.0 * ROW_PAD, 22.0);
    let edit_label = if s.editing { "Done" } else { "Edit" };
    let edit_st = TextStyle::body().weight(Weight::Semibold).right();
    let ew = ui.c.measure(edit_label, &edit_st, 100.0).0.ceil();
    let edit_r = Rect::new(line.right() - ew - 8.0, line.y - 3.0, ew + 16.0, line.h + 6.0);
    let mut start_add = false;
    if wl.full() {
        ui.c.text("Your list is full: 10 stocks", line, &TextStyle::footnote().color(palette::TEXT_FAINT));
    } else if !s.editing {
        let add_r = Rect::new(add.x, line.y - 3.0, (edit_r.x - add.x - 8.0).max(0.0), line.h + 6.0);
        let resp = ui.button(wid("stocks.add"), add_r);
        if resp.hovered || resp.pressed {
            super::card::fill_box(ui, card, add_r, 9.0, Color::white(if resp.pressed { 0.10 } else { 0.06 }));
        }
        let cy = line.center().y;
        ui.c.fill_circle(Point::new(line.x + 9.0, cy), 9.0, accent.with_alpha(0.20));
        ui.c.icon(Icon::Plus, Rect::from_center(Point::new(line.x + 9.0, cy), 13.0, 13.0), accent);
        ui.c.text("Add a stock", Rect::new(line.x + 26.0, line.y, line.w - 26.0 - ew - 20.0, line.h), &TextStyle::body().weight(Weight::Semibold).color(accent));
        start_add = resp.clicked;
    }
    if !wl.codes.is_empty() || s.editing {
        let resp = ui.button(wid("stocks.edit"), edit_r);
        let c = if resp.hovered { palette::TEXT } else { palette::TEXT_DIM };
        ui.c.text(edit_label, Rect::new(edit_r.x, line.y, edit_r.w - 8.0, line.h), &edit_st.color(c));
        if resp.clicked {
            s.editing = !s.editing;
            s.drag = None;
            save |= !s.editing;
        }
    }
    let credit = "Prices from oyenza.trade, 10 min behind IDX";
    ui.c.text(credit, Rect::new(line.x, add.y + 28.0, line.w, 14.0), &TextStyle::caption().color(palette::TEXT_FAINT));

    // The list fades out at an edge that scrolls further.
    if s.shown_scroll > 0.5 {
        fade(ui, Rect::new(r.x - 8.0, r.y, r.w + 16.0, 14.0), true);
    }
    if s.shown_scroll < max_scroll - 0.5 {
        fade(ui, Rect::new(r.x - 8.0, r.bottom() - 18.0, r.w + 16.0, 18.0), false);
    }
    ui.c.unclip();

    if let Some(code) = picked {
        wl.pick = code;
        save = true;
    }
    if cycle {
        wl.show = wl.show.next();
        save = true;
    }
    if let Some(code) = removed {
        wl.remove(&code);
        save = true;
        if wl.codes.is_empty() {
            s.editing = false;
        }
    }
    if start_add {
        s.adding = true;
        s.focus = true;
        s.query.clear();
        s.hit = 0;
        s.swap_k = 0.0;
    }
    if save {
        st.save();
    }
}

struct RowView<'a> {
    code: &'a str,
    name: &'a str,
    q: Option<&'a Quote>,
    span: (u16, u16),
    show: stocks::Show,
    edit_k: f32,
    dragged: bool,
    picked: bool,
}

enum RowAct {
    None,
    Pick,
    Cycle,
    Remove,
    Drag,
}

/// One row of the list: code and name, the day's line, the price and its
/// change pill; in edit mode a remove button and a handle instead.
fn draw_row(ui: &mut Ui, v: RowView, r: Rect, card: Option<(Rect, f32)>) -> RowAct {
    let n = code_n(v.code);
    let index = v.code == INDEX;
    let editing = v.edit_k > 0.01;
    let mut act = RowAct::None;
    let hit = Rect::new(r.x, r.y + 2.0, r.w, r.h - 4.0);
    let resp = ui.button(wid_n("stocks.row", n), hit);
    if v.dragged {
        super::card::fill_box(ui, card, hit, 10.0, Color::white(0.14));
    } else if (resp.hovered || resp.pressed) && !v.picked && !editing {
        super::card::fill_box(ui, card, hit, 10.0, Color::white(if resp.pressed { 0.08 } else { 0.05 }));
    }
    if resp.clicked && !editing {
        act = RowAct::Pick;
    }
    let shift = EDIT_W * ease_out(v.edit_k);
    let x0 = r.x + ROW_PAD + shift;
    let right = r.right() - ROW_PAD;
    let pill = Rect::new(right - PILL_W, r.y + 25.0, PILL_W, PILL_H);
    let spark = Rect::new(pill.x - 10.0 - SPARK_W, r.y + 11.0, SPARK_W, 26.0);
    let text_w = (spark.x - 8.0 - x0).max(0.0);
    ui.c.text(v.code, Rect::new(x0, r.y + 6.0, text_w, 19.0), &TextStyle::headline().weight(Weight::Semibold));
    ui.c.text(v.name, Rect::new(x0, r.y + 25.0, text_w, 17.0), &TextStyle::footnote().color(palette::TEXT_DIM));

    // The price side fades out as edit mode comes in.
    let base = ui.c.alpha();
    let k = 1.0 - v.edit_k;
    if k > 0.01 {
        ui.c.set_alpha(base * k);
        match v.q {
            Some(q) => {
                mini_chart(ui, q, v.span, spark);
                let price = stocks::fmt_price(q.price, index);
                ui.c.text(&price, Rect::new(right - 110.0, r.y + 5.0, 110.0, 19.0), &TextStyle::body().weight(Weight::Semibold).tabular().right());
                let pr = ui.button(wid_n("stocks.pill", n), pill);
                let mut fill = trend_color(q.trend());
                if pr.pressed {
                    fill = fill.mul_alpha(0.75);
                }
                ui.c.fill_rrect(pill, 6.0, fill);
                let label = q.pill(v.show);
                ui.c.text(&label, Rect::new(pill.x + 6.0, pill.y, pill.w - 12.0, pill.h), &TextStyle::footnote().weight(Weight::Semibold).tabular().right().color(Color::white(1.0)));
                if pr.clicked && !editing {
                    act = RowAct::Cycle;
                }
            }
            None => {
                ui.c.text("\u{2014}", Rect::new(right - 80.0, r.y + 5.0, 80.0, 19.0), &TextStyle::body().color(palette::TEXT_FAINT).right());
                ui.c.fill_rrect(pill, 6.0, Color::white(0.10));
            }
        }
        ui.c.set_alpha(base);
    }
    if editing && !index {
        ui.c.set_alpha(base * v.edit_k);
        // Remove: a red disc with a white bar, in the room the text left.
        let c = Point::new(r.x + ROW_PAD + 9.0, r.center().y);
        let rm = ui.button(wid_n("stocks.remove", n), Rect::from_center(c, 26.0, 26.0));
        ui.c.fill_circle(c, 9.0, if rm.pressed { palette::RED.mul_alpha(0.7) } else { palette::RED });
        ui.c.fill_rrect(Rect::from_center(c, 9.0, 2.0), 1.0, Color::white(1.0));
        if rm.clicked {
            act = RowAct::Remove;
        }
        // The handle: three bars to drag the row by.
        let h = Rect::new(right - 36.0, r.y + 4.0, 36.0, r.h - 8.0);
        let hr = ui.button(handle_id(v.code), h);
        let hc = if hr.hovered || hr.pressed || v.dragged { palette::TEXT } else { palette::TEXT_FAINT };
        for j in 0..3 {
            ui.c.fill_rrect(Rect::new(h.right() - 22.0, h.center().y - 5.0 + j as f32 * 5.0 - 0.75, 16.0, 1.5), 0.75, hc);
        }
        if hr.pressed {
            act = RowAct::Drag;
        }
        ui.c.set_alpha(base);
    }
    act
}

/// A row's small chart: the day's line from the first session's open, the
/// previous close dotted across.
fn mini_chart(ui: &mut Ui, q: &Quote, span: (u16, u16), r: Rect) {
    let Some((lo, hi)) = stocks::bounds(q.spark.iter().map(|p| p.1), Some(q.prev)) else { return };
    let y = |v: f64| r.bottom() - ((v - lo) / (hi - lo)) as f32 * r.h;
    dotted(ui, r.x, r.right(), y(q.prev).round() + 0.5, Color::white(0.30));
    if q.spark.len() < 2 {
        return;
    }
    let color = trend_color(q.trend());
    let pts: Vec<(f32, f32)> = q.spark.iter().map(|p| (r.x + stocks::day_x(p.0, span) * r.w, y(p.1))).collect();
    draw_line(ui, &pts, color, r.y, r.bottom(), 1.25, None);
}

/// A line through `pts` with the area under it washed in its colour: from
/// `fade` at `top` to clear at `bottom` (the chart's), or one faint wash (a
/// row's, which draws without making a gradient each frame).
fn draw_line(ui: &mut Ui, pts: &[(f32, f32)], color: Color, top: f32, bottom: f32, width: f32, fade: Option<f32>) {
    if pts.len() < 2 {
        return;
    }
    let line: Vec<Seg> = pts.iter().enumerate().map(|(i, &(x, y))| if i == 0 { Seg::MoveTo(x, y) } else { Seg::LineTo(x, y) }).collect();
    let mut area = line.clone();
    area.push(Seg::LineTo(pts[pts.len() - 1].0, bottom));
    area.push(Seg::LineTo(pts[0].0, bottom));
    area.push(Seg::Close);
    match fade {
        Some(alpha) => ui.c.fill_path_fade(&area, color, top, bottom, alpha, 0.0),
        None => ui.c.path(&area, Some(color.mul_alpha(0.16)), None),
    }
    ui.c.path(&line, None, Some((color, width)));
}

/// A dotted line across at `y`, as Stocks marks the previous close: one
/// path of short dashes (round caps make them dots).
fn dotted(ui: &mut Ui, x0: f32, x1: f32, y: f32, c: Color) {
    let mut segs = Vec::with_capacity(((x1 - x0) / 2.0).max(0.0) as usize + 2);
    let mut x = x0 + 0.5;
    while x + 0.5 < x1 {
        segs.push(Seg::MoveTo(x, y));
        segs.push(Seg::LineTo(x + 0.5, y));
        x += 4.0;
    }
    ui.c.path(&segs, None, Some((c, 1.2)));
}

/// The island's ground over `r`, from solid at the outer edge to clear:
/// rows fade out where the list is cut off (Glass shows its own backdrop).
fn fade(ui: &mut Ui, r: Rect, top: bool) {
    if ui.c.look().material == poni_core::look::Material::Glass {
        return;
    }
    const STEPS: usize = 8;
    let step = r.h / STEPS as f32;
    for k in 0..STEPS {
        // k = 0 is the strip at the edge.
        let a = 0.92 * (1.0 - (k as f32 + 0.5) / STEPS as f32).powf(1.4);
        let y = if top { r.y + k as f32 * step } else { r.bottom() - (k as f32 + 1.0) * step };
        ui.c.fill_rect(Rect::new(r.x, y, r.w, step), palette::ISLAND.with_alpha(a));
    }
}

// ---------------------------------------------------------------- the search

fn search(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let accent = st.accent;
    let id = wid("stocks.search");
    let cancel_w = ui.pill_width("Cancel", false, true);
    let cancel = Rect::new(r.right() - cancel_w, r.y + (FIELD_H - PILL_SMALL) / 2.0, cancel_w, PILL_SMALL);
    let field = Rect::new(r.x, r.y, (cancel.x - 8.0 - r.x).max(0.0), FIELD_H);
    let card = super::card::pill_card(st, r);
    let s = &mut st.ui.stocks;
    if std::mem::take(&mut s.focus) {
        ui.focus_text(id, s.query.chars().count());
    }
    // Up and Down walk the results; the field has no use for them.
    let mut moves = 0i32;
    if ui.input.focused == Some(id) {
        ui.input.keys.retain(|k| match k {
            KeyInput::Up => {
                moves -= 1;
                false
            }
            KeyInput::Down => {
                moves += 1;
                false
            }
            _ => true,
        });
    }
    let resp = ui.text_input_row(id, field, &mut s.query, "Code or name, like BBCA or Telkom", 32, accent, Some(Icon::Search));
    if resp.changed {
        s.hit = 0;
    }
    let names = st.stocks.names.clone();
    let results = names.as_ref().map(|n| n.search(&s.query, &st.store.stocks.codes, RESULTS)).unwrap_or_default();
    if !results.is_empty() {
        s.hit = (s.hit as i32 + moves).clamp(0, results.len() as i32 - 1) as usize;
    }
    // A typed code goes in as it is only when the list of names isn't there
    // to look in (oyenza.trade then says whether it lists it).
    let code = stocks::code_of(&s.query);
    let typed = code.clone().filter(|c| names.is_none() && !st.store.stocks.has(c));
    let mut chosen = None;
    if resp.submitted {
        chosen = results.get(s.hit).map(|p| p.0.clone()).or(typed.clone());
    }
    let cancelled = ui.pill(wid("stocks.cancel"), cancel, "Cancel", None, PillStyle::Utility).clicked || resp.cancelled;

    let top = r.y + FIELD_H + 8.0;
    let hint_r = Rect::new(r.x + 4.0, top + 2.0, r.w - 8.0, 34.0);
    let hint = TextStyle::footnote().color(palette::TEXT_DIM).wrap().top();
    if results.is_empty() {
        let line = match (&names, st.stocks.names_failed) {
            (_, _) if st.store.stocks.full() => "Your list is full: remove a stock first.".to_string(),
            (None, false) => "Getting every listed stock from oyenza.trade\u{2026}".to_string(),
            (_, _) if s.query.trim().is_empty() => "Type a code or a company's name.".to_string(),
            (_, _) if code.as_ref().is_some_and(|c| st.store.stocks.has(c)) => format!("{} is on your list already.", code.unwrap_or_default()),
            (None, true) => match &typed {
                Some(c) => format!("Couldn't get the list of stocks. Press Enter to add {c}."),
                None => "Couldn't get the list of stocks. Type a code, like BBCA, and press Enter.".to_string(),
            },
            _ => "No listed stock by that code or name.".to_string(),
        };
        ui.c.text(&line, hint_r, &hint);
    }
    for (i, (code, name)) in results.iter().enumerate() {
        let row = Rect::new(r.x, top + i as f32 * RESULT_H, r.w, RESULT_H);
        let b = ui.button(wid_n("stocks.result", code_n(code)), row);
        if b.clicked {
            chosen = Some(code.clone());
        }
        let lit = i == s.hit || b.hovered;
        if lit {
            super::card::fill_box(ui, card, row.inset(0.0, 1.0), 9.0, Color::white(if b.pressed { 0.12 } else if i == s.hit { 0.09 } else { 0.06 }));
        }
        let tx = row.x + ROW_PAD;
        let code_st = TextStyle::body().weight(Weight::Semibold);
        let cw = ui.c.measure(code, &code_st, 100.0).0.ceil().max(40.0);
        ui.c.text(code, Rect::new(tx, row.y, cw + 2.0, row.h), &code_st);
        ui.c.text(name, Rect::new(tx + cw + 12.0, row.y, row.right() - tx - cw - 20.0, row.h), &TextStyle::footnote().color(palette::TEXT_DIM));
    }

    if let Some(code) = chosen {
        if st.store.stocks.add(&code) {
            // The new row shows at the end of the list.
            st.ui.stocks.scroll = f32::MAX;
            st.save();
        }
        close_search(ui, &mut st.ui.stocks);
    } else if cancelled {
        close_search(ui, &mut st.ui.stocks);
    }
}

fn close_search(ui: &mut Ui, s: &mut StocksUi) {
    s.adding = false;
    s.query.clear();
    s.hit = 0;
    s.swap_k = 0.0;
    ui.blur();
}

// ---------------------------------------------------------------- the stock

fn detail(ui: &mut Ui, st: &mut AppState, r: Rect) {
    let pick = st.store.stocks.pick.clone();
    let range = st.store.stocks.range;
    let index = pick == INDEX;
    let board = st.stocks.board.as_ref();
    let q = board.and_then(|b| b.quote(&pick));

    // Title: the code and the name, the ranges on the right.
    let code_st = TextStyle::value().weight(Weight::Bold);
    let cw = ui.c.measure(&pick, &code_st, 200.0).0.ceil();
    let title = Rect::new(r.x, r.y, r.w, 22.0);
    let seg_x = r.right() - SEG_W * Range::ALL.len() as f32;
    ui.c.text(&pick, Rect::new(title.x, title.y, cw + 2.0, title.h), &code_st);
    let name = q.map(|q| q.name.clone()).or_else(|| st.stocks.names.as_ref().and_then(|n| n.name(&pick).map(str::to_string))).unwrap_or_else(|| if index { "IDX Composite".into() } else { String::new() });
    let name_r = Rect::new(title.x + cw + 8.0, title.y + 1.0, (seg_x - title.x - cw - 18.0).max(0.0), title.h);
    ui.c.text(&name, name_r, &TextStyle::headline().color(palette::TEXT_DIM));
    if let Some(pick_range) = range_control(ui, Rect::new(seg_x, title.y, r.right() - seg_x, SEG_H), range) {
        st.store.stocks.range = pick_range;
        st.save();
    }

    let board = st.stocks.board.as_ref();
    let q = board.and_then(|b| b.quote(&pick));
    let Some(q) = q else {
        let line = match (board, st.stocks.trouble) {
            (None, Some(t)) => trouble_text(t).to_string(),
            (None, None) => "Getting prices from oyenza.trade\u{2026}".to_string(),
            (Some(b), _) if b.missing.contains(&pick) => format!("oyenza.trade doesn't list {pick}."),
            (Some(_), _) => format!("Getting {pick}\u{2026}"),
        };
        let m = Rect::new(r.x, r.y + CHART_Y, r.w, CHART_H);
        ui.c.text(&line, m, &TextStyle::footnote().color(palette::TEXT_DIM).center().wrap());
        return;
    };
    let q = q.clone();
    let span = board.map_or((540, 960), |b| b.span);

    // The chart's line: the day from the opening bell, or the range's closes.
    let chart = Rect::new(r.x, r.y + CHART_Y, r.w, CHART_H);
    let line = chart_line(st, &q, span, range);
    let now = ui.now;
    let still = st.ui.stocks.still;
    let m = &mut st.ui.stocks.chart;
    let target: Vec<(f32, f32)> = stocks::resample(&line.pts, SAMPLES);
    if line.key != m.key {
        // From wherever the line shows now; the first one rises from its base.
        let (from, base_from, color_from) = if still {
            (target.clone(), line.base, Some(line.color))
        } else if m.to.is_empty() {
            let flat = line.base.unwrap_or_else(|| target.first().map_or(0.5, |p| p.1));
            (target.iter().map(|p| (p.0, flat)).collect(), line.base, Some(line.color))
        } else {
            let e = morph_k(m.t0, now);
            (lerp_pts(&m.from, &m.to, e), lerp_base(m.base_from, m.base_to, e), Some(lerp_color(m.color_from, m.color_to, e)))
        };
        *m = Morph { key: line.key.clone(), from, to: target.clone(), base_from, base_to: line.base, color_from, color_to: Some(line.color), t0: now };
    }
    let e = morph_k(m.t0, now);
    if e < 1.0 {
        ui.request_anim();
    }
    let shown = lerp_pts(&m.from, &m.to, e);
    let color = lerp_color(m.color_from, m.color_to, e);
    let base = lerp_base(m.base_from, m.base_to, e);
    let to_px = |p: (f32, f32)| (chart.x + p.0 * chart.w, chart.bottom() - p.1 * chart.h);
    if let Some(b) = base {
        dotted(ui, chart.x, chart.right(), (chart.bottom() - b * chart.h).round() + 0.5, Color::white(0.30));
    }
    let px: Vec<(f32, f32)> = shown.iter().map(|p| to_px(*p)).collect();
    if line.pts.len() >= 2 {
        draw_line(ui, &px, color, chart.y, chart.bottom() + 6.0, 2.0, Some(0.34));
    } else {
        let msg = match range {
            // Before the bell, or (the day the feed starts keeping lines) a first point.
            Range::Day if line.values.is_empty() => "The day's line starts at the opening bell.",
            Range::Day => "The day's line fills in as IDX trades.",
            _ if st.stocks.no_year.contains(&pick) => "No chart for this range yet.",
            _ => "Getting the year from oyenza.trade\u{2026}",
        };
        ui.c.text(msg, chart, &TextStyle::footnote().color(palette::TEXT_FAINT).center());
    }

    // Under the pointer: that point's price and time, as Stocks scrubs.
    let scrub = ui.input.mouse.filter(|p| chart.inset(-4.0, -6.0).contains(*p) && line.pts.len() >= 2).map(|p| {
        let x = ((p.x - chart.x) / chart.w).clamp(0.0, 1.0);
        let i = line.pts.iter().enumerate().min_by(|a, b| (a.1 .0 - x).abs().total_cmp(&(b.1 .0 - x).abs())).map_or(0, |(i, _)| i);
        i
    });
    if let Some(i) = scrub.filter(|_| e >= 1.0) {
        let (sx, sy) = to_px(line.pts[i]);
        ui.c.line(Point::new(sx, chart.y - 2.0), Point::new(sx, chart.bottom()), Color::white(0.32), 1.0);
        ui.c.fill_circle(Point::new(sx, sy), 5.0, palette::ISLAND);
        ui.c.fill_circle(Point::new(sx, sy), 3.5, color);
    }

    // The price (or the scrubbed one) and its change, and the ranges.
    let (value, when) = match scrub.filter(|_| e >= 1.0) {
        Some(i) => (line.values[i], Some(line.label(i))),
        None => (q.price, None),
    };
    // The day's change is from the previous close; a range's from its first close.
    let from = if range == Range::Day { q.prev } else { line.values.first().copied().unwrap_or(q.prev) };
    let price = stocks::fmt_price(value, index);
    let price_st = TextStyle::value().weight(Weight::Semibold).tabular();
    let pw = ui.c.measure(&price, &price_st, 200.0).0.ceil();
    let py = r.y + PRICE_Y;
    ui.c.text(&price, Rect::new(r.x, py, pw + 2.0, 24.0), &price_st);
    let d = value - from;
    let pct = if from > 0.0 { d / from * 100.0 } else { 0.0 };
    let change = format!("{} ({})", stocks::fmt_change(d, index), stocks::fmt_pct(pct));
    let ccol = trend_color(stocks::trend_of(value, from));
    let change_st = TextStyle::body().weight(Weight::Semibold).tabular().color(ccol);
    let cw2 = ui.c.measure(&change, &change_st, 200.0).0.ceil();
    ui.c.text(&change, Rect::new(r.x + pw + 10.0, py + 2.0, (r.right() - r.x - pw - 10.0).max(0.0), 22.0), &change_st);
    if let Some(when) = when {
        let wx = r.x + pw + 10.0 + cw2 + 8.0;
        ui.c.text(&when, Rect::new(wx, py + 3.0, (r.right() - wx).max(0.0), 22.0), &TextStyle::footnote().color(palette::TEXT_FAINT));
    }

    // The axis: the day's open and close, or the range's first and last
    // dates; between them, where the exchange stands.
    let axis = Rect::new(chart.x, chart.bottom() + 4.0, chart.w, AXIS_H);
    let (a, b) = line.ends();
    let axis_st = TextStyle::caption().color(palette::TEXT_FAINT);
    let aw = ui.c.measure(&a, &axis_st, 200.0).0.ceil();
    let bw = ui.c.measure(&b, &axis_st, 200.0).0.ceil();
    ui.c.text(&a, axis, &axis_st);
    ui.c.text(&b, axis, &axis_st.right());
    let (status, warn) = status_line(st, st.stocks.board.as_ref());
    let room = Rect::new(axis.x + aw + 10.0, axis.y, (axis.w - aw - bw - 20.0).max(0.0), axis.h);
    ui.c.text(&status, room, &TextStyle::caption().color(if warn { palette::ORANGE } else { palette::TEXT_FAINT }).center());

    stats(ui, &q, Rect::new(r.x, r.y + STATS_Y, r.w, (r.h - STATS_Y).max(2.0 * STAT_H)));
}

/// The four ranges as a small segmented control; the one clicked, if any.
fn range_control(ui: &mut Ui, r: Rect, now: Range) -> Option<Range> {
    let mut out = None;
    for (i, range) in Range::ALL.into_iter().enumerate() {
        let seg = Rect::new(r.x + i as f32 * SEG_W, r.y, SEG_W, r.h);
        let resp = ui.button(wid_n("stocks.range", i as u64), seg);
        let on = range == now;
        if on {
            ui.c.fill_rrect(seg.inset(1.0, 0.0), r.h / 2.0, Color::white(0.16));
        } else if resp.hovered {
            ui.c.fill_rrect(seg.inset(1.0, 0.0), r.h / 2.0, Color::white(0.07));
        }
        let c = if on || resp.hovered { palette::TEXT } else { palette::TEXT_DIM };
        ui.c.text(range.label(), seg, &TextStyle::footnote().weight(Weight::Semibold).center().color(c));
        if resp.clicked && !on {
            out = Some(range);
        }
    }
    out
}

/// The day's numbers under the chart: four to a row, each label over its value.
fn stats(ui: &mut Ui, q: &Quote, r: Rect) {
    let index = q.is_index();
    let dash = || "\u{2014}".to_string();
    let p = |v: Option<f64>| v.map_or_else(dash, |v| stocks::fmt_price(v, index));
    let cells: Vec<(&str, String)> = if index {
        vec![
            ("Open", p(q.open)),
            ("High", p(q.high)),
            ("Low", p(q.low)),
            ("Prev close", p(Some(q.prev))),
            ("52W high", p(q.w52.map(|w| w.1))),
            ("52W low", p(q.w52.map(|w| w.0))),
        ]
    } else {
        // Stocks' own set (the previous close is the dotted line and the
        // change), with the rupiah traded where Stocks has the P/E: the
        // answers carry no P/E, as they need no sign-in on oyenza.trade.
        vec![
            ("Open", p(q.open)),
            ("High", p(q.high)),
            ("Low", p(q.low)),
            ("Volume", q.volume.map_or_else(dash, stocks::fmt_big)),
            ("Mkt cap", q.mcap.map_or_else(dash, stocks::fmt_cap)),
            ("Value", q.value.map_or_else(dash, stocks::fmt_cap)),
            ("52W high", p(q.w52.map(|w| w.1))),
            ("52W low", p(q.w52.map(|w| w.0))),
        ]
    };
    let gap = 8.0;
    let cw = (r.w - 3.0 * gap) / 4.0;
    let row_h = r.h / 2.0;
    let label_st = TextStyle::caption().color(palette::TEXT_FAINT);
    let value_st = TextStyle::footnote().weight(Weight::Semibold).tabular();
    for (i, (label, value)) in cells.iter().enumerate() {
        let (row, col) = (i / 4, i % 4);
        let cell = Rect::new(r.x + col as f32 * (cw + gap), r.y + row as f32 * row_h, cw, row_h);
        ui.c.line(Point::new(cell.x, cell.y + 0.5), Point::new(cell.right(), cell.y + 0.5), Color::white(0.08), 1.0);
        ui.c.text(label, Rect::new(cell.x, cell.y + 3.0, cell.w, 13.0), &label_st);
        ui.c.text(value, Rect::new(cell.x, cell.y + 15.0, cell.w, 15.0), &value_st);
    }
}

/// The line under the title: where the exchange stands, or why the prices
/// stopped; true when it's a warning.
fn status_line(st: &AppState, board: Option<&Board>) -> (String, bool) {
    match (board, st.stocks.trouble) {
        (Some(b), Some(t)) => (format!("{} \u{b7} prices from {}", trouble_word(t), stocks::wib_clock(b.t)), true),
        (Some(b), None) => (b.status(st.unix), b.stale),
        (None, _) => (String::new(), false),
    }
}

fn trouble_word(t: Trouble) -> &'static str {
    match t {
        Trouble::Offline => "Offline",
        Trouble::Paused => "Paused on oyenza.trade",
        Trouble::Server => "oyenza.trade didn't answer",
    }
}

fn trouble_text(t: Trouble) -> &'static str {
    match t {
        Trouble::Offline => "Can't reach oyenza.trade. The prices come once you're back online.",
        Trouble::Paused => "oyenza.trade has its live prices paused for now. Trying again in a few minutes.",
        Trouble::Server => "oyenza.trade didn't answer properly. Trying again soon.",
    }
}

/// One chart's line, in the chart's own 0..1 (y up), with its real values
/// for the scrub.
struct Line {
    key: String,
    pts: Vec<(f32, f32)>,
    values: Vec<f64>,
    /// Day lines: the minute of each point; longer ranges: its day.
    minutes: Vec<u16>,
    days: Vec<i64>,
    /// The previous close's height, dotted across (1D only).
    base: Option<f32>,
    color: Color,
    span: (u16, u16),
}

impl Line {
    /// The scrubbed point's time ("10:35") or date ("24 Oct 2026").
    fn label(&self, i: usize) -> String {
        match (self.minutes.get(i), self.days.get(i)) {
            (Some(m), _) => stocks::minute_label(*m),
            (_, Some(d)) => stocks::date_label(*d),
            _ => String::new(),
        }
    }

    /// The axis's two ends.
    fn ends(&self) -> (String, String) {
        match (self.days.first(), self.days.last()) {
            (Some(a), Some(b)) => (stocks::date_label(*a), stocks::date_label(*b)),
            _ => (stocks::minute_label(self.span.0), stocks::minute_label(self.span.1)),
        }
    }
}

fn chart_line(st: &AppState, q: &Quote, span: (u16, u16), range: Range) -> Line {
    let tag = st.stocks.board.as_ref().map_or("", |b| b.tag.as_str());
    let mut line = Line { key: String::new(), pts: Vec::new(), values: Vec::new(), minutes: Vec::new(), days: Vec::new(), base: None, color: trend_color(q.trend()), span };
    match range {
        Range::Day => {
            line.key = format!("{}/1d/{tag}", q.code);
            line.values = q.spark.iter().map(|p| p.1).collect();
            line.minutes = q.spark.iter().map(|p| p.0).collect();
            if let Some((lo, hi)) = stocks::bounds(line.values.iter().copied(), Some(q.prev)) {
                let y = |v: f64| ((v - lo) / (hi - lo)) as f32;
                line.pts = q.spark.iter().map(|p| (stocks::day_x(p.0, span), y(p.1))).collect();
                line.base = Some(y(q.prev));
            }
        }
        _ => {
            let year = st.stocks.years.get(&q.code);
            // A new answer moves its last point: the key moves with it.
            line.key = format!("{}/{}/{}/{tag}", q.code, range.label(), year.map_or(0, |h| h.days.len()));
            if let Some(h) = year {
                let (days, closes) = h.window(range);
                line.days = days.to_vec();
                line.values = closes.to_vec();
                // The year ends at the build's last close: today's price takes
                // that day's place, or comes after it.
                let today = st.stocks.board.as_ref().and_then(|b| stocks::day_of(&b.day));
                match (today, line.days.last().copied()) {
                    (Some(t), Some(last)) if t == last => {
                        if let Some(v) = line.values.last_mut() {
                            *v = q.price;
                        }
                    }
                    (Some(t), Some(last)) if t > last => {
                        line.days.push(t);
                        line.values.push(q.price);
                    }
                    _ => {}
                }
                let vals = &line.values;
                if let (Some((lo, hi)), Some(first), Some(last)) = (stocks::bounds(vals.iter().copied(), None), vals.first(), vals.last()) {
                    let n = (vals.len() - 1).max(1) as f32;
                    line.pts = vals.iter().enumerate().map(|(i, c)| (i as f32 / n, ((c - lo) / (hi - lo)) as f32)).collect();
                    line.color = trend_color(stocks::trend_of(*last, *first));
                }
            }
        }
    }
    line
}

fn morph_k(t0: f64, now: f64) -> f32 {
    ease_out(((now - t0) / MORPH_S) as f32)
}

fn lerp_pts(a: &[(f32, f32)], b: &[(f32, f32)], t: f32) -> Vec<(f32, f32)> {
    if a.len() != b.len() {
        return b.to_vec();
    }
    a.iter().zip(b).map(|(p, q)| (lerp(p.0, q.0, t), lerp(p.1, q.1, t))).collect()
}

fn lerp_base(a: Option<f32>, b: Option<f32>, t: f32) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(lerp(a, b, t)),
        (_, b) => b.filter(|_| t >= 0.5),
    }
}

fn lerp_color(a: Option<Color>, b: Option<Color>, t: f32) -> Color {
    match (a, b) {
        (Some(a), Some(b)) => Color::lerp(a, b, t),
        (_, Some(b)) => b,
        (Some(a), None) => a,
        _ => Color::white(0.36),
    }
}
