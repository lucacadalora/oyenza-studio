//! Caffeine page, the Caffeine add-on's (off until switched on): the day's
//! drinks and how much caffeine is still in the body, hour by hour
//! (`poni_core::caffeine`).
//!
//! Across the top: the day's drinks and caffeine against the day's limit,
//! and the last call for the drink the person usually has. On the left the
//! day from 06:00 to 06:00 as a field of dots: each drink a band of them,
//! stacked oldest at the bottom over what is left of the days before,
//! bright while it goes in and grey once it settles, blue while picked and
//! orange when it keeps too much in the body for bed; the column a drink
//! went in shows the level before it as rings. Above the field the day's
//! peak and what is left at bedtime, on lines down to them; across it the
//! 50 mg line and last call; under it each of the day's drinks as a dot to
//! drag to another time, and the hours. On the right the day's drinks as a
//! list, and Add a drink: a sheet over the page that picks the kind, then
//! how many (shots for an espresso drink, with its beans) or Your own's
//! name and mg, and when, and says what it leaves at bedtime.
//!
//! The chart follows the Caffeine Clock, a web page of the same dots ("the
//! web chart" below): the dots' sizes and shades, the rings, the y scale
//! and where the labels go, in the island's look.
//!
//! Nothing here moves by itself, and the page draws only when the app
//! does. The field's dots (72 columns by 22 rows) are painted into one
//! picture when what they show changes (a drink added, moved, removed or
//! picked, the settings, the day, the theme or the display's scale), so a
//! frame draws them as one image.

use crate::commands::Command;
use crate::gfx::{Image, ImageData, TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, KeyInput, PillStyle, Ui, PILL_GAP, PILL_SMALL};
use poni_core::caffeine::{bed_of, day_start, freshness, hhmm, in_body, last_call_hours, late_for_bed, local_minute, tmax, Bean, DAY, DAY_START, Drink, Kind, LocalMinute, Log, MAX_AMOUNT, Prefs, Today};
use poni_core::datetime::LocalDateTime;
use poni_core::geom::{lerp, palette, Color, Point, Rect, Size};
use poni_core::look::{Look, Material};
use poni_core::svgpath::Seg;

/// The System page's height: the window never grows for this page.
const HEIGHT: f32 = 256.0;
/// The chart's column, then the gap, then the list. The chart is this wide
/// at the island's usual width (620) and wider; on a narrower island it
/// gives way, down to `CHART_MIN`, so the list keeps `LIST_MIN`.
const CHART_W: f32 = 370.0;
const CHART_MIN: f32 = 250.0;
const COL_GAP: f32 = 24.0;
const LIST_MIN: f32 = 160.0;
/// The line across the top: the day's caffeine and its last call.
const TOP_H: f32 = 20.0;
/// The field: a column of dots for every 20 minutes of the day, in square
/// cells `CELL` wide (narrower where the chart gives way).
const COLS: usize = 72;
const ROWS: usize = 22;
const CELL: f32 = 4.75;
/// Minutes a column of dots covers.
const COL_MIN: i64 = DAY / COLS as i64;
/// The field starts this far into the chart's column, so the "06" under
/// its left edge, in a box `HOUR_W` wide, is centred on it.
const FIELD_X: f32 = 10.0;
const HOUR_W: f32 = 18.0;
/// Right of the field: the "50" beside the 50 mg line.
const FIFTY_W: f32 = 18.0;
/// The PEAK and BED labels: two lines each, on one of two levels above the
/// field, `LABEL_GAP` apart; a label sits `LABEL_SIDE` beside its line.
const LABEL_LINE: f32 = 14.0;
const LABEL_GAP: f32 = 6.0;
const LABEL_SIDE: f32 = 9.0;
/// Under the axis, the day's drinks as dots: the picked one a little larger.
const CUP_R: f32 = 4.5;
const CUP_PICKED_R: f32 = 6.0;
/// A time under a dot moves at most this far aside to fit, else it waits.
const NUDGE: f32 = 10.0;
/// A press on a dot moves the drink once the pointer has gone this far
/// (DIPs), so a click with a shaky hand doesn't.
const DRAG_START: f32 = 4.0;
/// The list's rows.
const ROW_H: f32 = 38.0;
const ROW_PAD: f32 = 6.0;
/// The sheet: its title, then rows this far apart, their labels in a
/// column this wide; the kinds five to a row (four on a narrow island);
/// Back and Add at the bottom.
const SHEET_TOP: f32 = 34.0;
const SHEET_ROW: f32 = 38.0;
const LABEL_W: f32 = 100.0;
const KIND_COLS: usize = 5;
const KIND_COLS_NARROW: usize = 4;
const NARROW: f32 = 540.0;
const BUTTON_W: f32 = 96.0;
/// A stepper's value between its buttons, the time between −15 and +15,
/// those buttons, Now, and Your own's name field.
const STEP_VALUE_W: f32 = 84.0;
const TIME_W: f32 = 64.0;
const TIME_BTN_W: f32 = 48.0;
const NOW_W: f32 = 56.0;
const NAME_W: f32 = 280.0;
/// Your own drink's caffeine: 5 mg steps from 5 to 800, starting at 100.
const OWN_STEP: u16 = 5;
const OWN_MIN: u16 = 5;
const OWN_MAX: u16 = 800;
const OWN_START: u16 = 100;
/// The sheet holds the keyboard under this id; Your own's name field under its own.
const SHEET_ID: &str = "caffeine.sheet";
const NAME_ID: &str = "caffeine.name";

/// The dots' shades: white over the black island (each theme paints them
/// its way): an empty cell, what is left from the days before, a drink
/// settled, a drink going in.
const EMPTY: Color = Color::white(0.13);
const OLD: Color = Color::white(0.24);
const REST: Color = Color::white(0.40);
const FRESH: Color = Color::white(0.94);

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// The page's own state between frames (`st.ui.caffeine`).
#[derive(Clone, Debug, Default)]
pub struct CaffeineUi {
    /// The drink clicked, blue in the chart and lit in the list; while it
    /// isn't one of the day's, the day's latest is.
    pick: Option<u32>,
    /// The drink whose dot is being dragged along the day.
    drag: Option<Drag>,
    /// How far the list is scrolled (DIPs); `reveal` scrolls the picked
    /// row into view on the next frame (a drink just added).
    scroll: f32,
    reveal: bool,
    /// Add a drink, while it is open.
    sheet: Option<Sheet>,
    /// Still pictures (the self-test's, the Mac renders'): the sheet leaves
    /// the keyboard alone.
    still: bool,
    /// The chart worked out for what it shows, and its dots as painted.
    kept: Kept,
    /// When the page last drew (`ui.now`), so [`put_away`] can tell it's gone.
    drawn: f64,
}

/// How long the page may be off screen before [`put_away`] lets its chart
/// go, seconds: past a page switch or the island's closing, whose frames
/// still draw it.
const KEEP_OFF_SCREEN_S: f64 = 1.0;

/// Called as the island draws something else (another page, or closed):
/// once the page has been off screen [`KEEP_OFF_SCREEN_S`], its chart and
/// painted dots go, the renderer's copy too (about 144 KB at 100 %, 572 KB
/// at 200 %), and the next visit works them out again. Free when there is
/// nothing kept.
pub fn put_away(ui: &mut Ui, st: &mut AppState) {
    let s = &mut st.ui.caffeine;
    if (s.kept.painted.is_none() && s.kept.chart.is_none()) || ui.now - s.drawn < KEEP_OFF_SCREEN_S {
        return;
    }
    if let Some(p) = s.kept.painted.take() {
        ui.c.forget_image(&p.image);
    }
    s.kept.chart = None;
}

/// What a still picture of the page shows (the self-test's, the Mac renders').
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// A day at 15:42: a kopi sachet at 07:30, a 2-shot es kopi susu
    /// (Blend) at 10:15 and a 2-shot Americano at 12:00.
    Day,
    /// That day with Add a drink open on its kinds.
    Kinds,
    /// That day with Add a drink on a 2-shot es kopi susu (Blend), now.
    Add,
    /// No drink yet, at 09:10.
    Empty,
    /// A late day at 21:05, after yesterday's evening cold brew: strong
    /// lattes, tea, cola and an evening es kopi susu, over the day's limit.
    Late,
}

impl Picture {
    /// The time of day it shows.
    fn clock(self) -> (u8, u8) {
        match self {
            Picture::Day | Picture::Kinds | Picture::Add => (15, 42),
            Picture::Empty => (9, 10),
            Picture::Late => (21, 5),
        }
    }
}

/// The page as a still picture shows it: the picture's drinks on the date
/// `st.local` has, its clock set to the picture's time (the self-test holds
/// it there), the settings as they start (with the Blend for the sheet's
/// beans), the day's latest drink picked.
pub fn picture(st: &mut AppState, shows: Picture) {
    let (hour, minute) = shows.clock();
    st.local = LocalDateTime { hour, minute, second: 0, ..st.local };
    let now = local_minute(&st.local);
    let start = day_start(now);
    let at = |h: i64, m: i64| start - DAY_START + h * 60 + m;
    let mut log = Log::default();
    let mut had = |at: LocalMinute, kind: Kind, amount: u8, bean: Option<Bean>| {
        log.add(Drink { id: 0, at, kind, amount, bean, mg: kind.mg(amount, bean.unwrap_or_default()), name: String::new() });
    };
    match shows {
        Picture::Day | Picture::Kinds | Picture::Add => {
            had(at(7, 30), Kind::Sachet, 1, None);
            had(at(10, 15), Kind::KopiSusu, 2, Some(Bean::Blend));
            had(at(12, 0), Kind::Americano, 2, Some(Bean::Arabica));
        }
        Picture::Empty => {}
        Picture::Late => {
            had(at(-3, 30), Kind::ColdBrew, 1, None);
            had(at(7, 45), Kind::Latte, 3, Some(Bean::Robusta));
            had(at(10, 30), Kind::Tubruk, 2, None);
            had(at(13, 30), Kind::Latte, 3, Some(Bean::Robusta));
            had(at(15, 15), Kind::Tea, 1, None);
            had(at(17, 45), Kind::Cola, 1, None);
            had(at(20, 30), Kind::KopiSusu, 2, Some(Bean::Blend));
        }
    }
    st.store.caffeine = log;
    st.settings.caffeine = Prefs { bean: Bean::Blend, ..Prefs::default() };
    let sheet = match shows {
        Picture::Kinds => Some(Sheet::new(now, Bean::Blend)),
        Picture::Add => {
            let mut s = Sheet::new(now, Bean::Blend);
            s.pick(Kind::KopiSusu);
            Some(s)
        }
        _ => None,
    };
    // The painting stays: it is painted again only if this picture's dots differ.
    let kept = std::mem::take(&mut st.ui.caffeine.kept);
    st.ui.caffeine = CaffeineUi { sheet, still: true, reveal: true, kept, ..CaffeineUi::default() };
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    st.ui.caffeine.drawn = ui.now;
    let now = local_minute(&st.local);
    let start = day_start(now);
    // Add a drink adds to its own day: left open past 06:00, it closes.
    if st.ui.caffeine.sheet.as_ref().is_some_and(|s| s.day != start) {
        st.ui.caffeine.sheet = None;
    }
    if st.ui.caffeine.sheet.is_some() {
        sheet(ui, st, area, now);
        return;
    }
    let lay = Layout::of(area);
    let pick = picked(&st.store.caffeine, start, st.ui.caffeine.pick);
    let mut kept = std::mem::take(&mut st.ui.caffeine.kept);
    kept.update(&st.store.caffeine, &st.settings.caffeine, now, pick);
    let scale = ui.c.scale().max(0.5);
    let field = snap_rect(lay.field, scale);
    let image = kept.image(ui, ((field.w * scale).round() as u32, (field.h * scale).round() as u32), scale, lay.cell);
    if let Some(chart) = kept.chart.as_ref() {
        top_row(ui, lay.top, &chart.today, &st.settings.caffeine, now);
        draw_chart(ui, st, &lay, field, chart, image.as_ref(), pick);
        list(ui, st, &lay, chart, pick, now);
    }
    st.ui.caffeine.kept = kept;
}

/// The drink the page has picked: the one clicked while it is one of the
/// day's (from `start`), else the day's latest.
fn picked(log: &Log, start: LocalMinute, pick: Option<u32>) -> Option<u32> {
    match pick {
        Some(id) if log.day(start).any(|d| d.id == id) => Some(id),
        _ => log.day(start).last().map(|d| d.id),
    }
}

/// `r` on the device's pixel grid.
fn snap_rect(r: Rect, scale: f32) -> Rect {
    let px = |v: f32| (v * scale).round() / scale;
    Rect::new(px(r.x), px(r.y), px(r.w), px(r.h))
}

// ---------------------------------------------------------------- layout

/// Where everything on the page goes, from the page's `area`.
#[derive(Clone, Copy, Debug)]
struct Layout {
    area: Rect,
    top: Rect,
    chart: Rect,
    /// The dots, `cell` DIPs apart.
    field: Rect,
    cell: f32,
    /// The bottoms of the labels' two levels above the field, the lower first.
    levels: [f32; 2],
    axis: f32,
    /// The centre line of the drinks' dots under the axis.
    cups: f32,
    /// The row of their times, and the row of the hours under it.
    times: Rect,
    hours: Rect,
    list: Rect,
    add: Rect,
}

impl Layout {
    fn of(area: Rect) -> Layout {
        let top = Rect::new(area.x, area.y, area.w, TOP_H);
        let chart = Rect::new(area.x, area.y, (area.w - COL_GAP - LIST_MIN).clamp(CHART_MIN, CHART_W).min(area.w), HEIGHT);
        let label_h = 2.0 * LABEL_LINE;
        let high = top.bottom() + 4.0 + label_h;
        let low = high + LABEL_GAP + label_h;
        let cell = ((chart.w - FIELD_X - FIFTY_W) / COLS as f32).clamp(1.0, CELL);
        let field = Rect::new(chart.x + FIELD_X, low + 6.0, COLS as f32 * cell, ROWS as f32 * cell);
        let axis = field.bottom() + 4.5;
        let cups = axis + 12.0;
        let times = Rect::new(chart.x, cups + CUP_PICKED_R + 3.0, chart.w, 14.0);
        let hours = Rect::new(chart.x, times.bottom() + 2.0, chart.w, 14.0);
        let list_x = chart.right() + COL_GAP;
        let add = Rect::new(list_x, area.y + HEIGHT - PILL_SMALL, (area.right() - list_x).max(0.0), PILL_SMALL);
        let list = Rect::new(list_x, top.bottom() + 6.0, add.w, (add.y - 8.0 - top.bottom() - 6.0).max(0.0));
        Layout { area, top, chart, field, cell, levels: [low, high], axis, cups, times, hours, list, add }
    }

    /// The "50" beside the 50 mg line at `y`.
    fn fifty(&self, field: Rect, y: f32) -> Rect {
        Rect::new(field.right() + 4.0, y - 7.0, (self.chart.right() - field.right() - 4.0).max(0.0), 14.0)
    }
}

// ---------------------------------------------------------------- the chart's arithmetic

/// What the chart is of: it is worked out again when one of these changes.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    drinks: Vec<Drink>,
    prefs: Prefs,
    start: LocalMinute,
    pick: Option<u32>,
}

/// One cell of the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dot {
    Empty,
    /// Caffeine from the days before, still in the body.
    Old,
    /// The level before a drink, in the column it went in (bright for the picked drink).
    Ring { picked: bool },
    /// A drink's own band: `level` 5 while it goes in, settling to 0.
    Band { tone: Tone, level: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Plain,
    Picked,
    /// More of it than the bedtime limit is left at bedtime (`Today::late`).
    Late,
}

impl Dot {
    /// Its radius (DIPs) in cells `cell` wide, its line's width for a ring
    /// (`None` for a disc) and its colour, as the web chart has them.
    fn style(self, cell: f32) -> (f32, Option<f32>, Color) {
        match self {
            Dot::Empty => ((cell * 0.1).max(0.55), None, EMPTY),
            Dot::Old => (cell * 0.27, None, OLD),
            Dot::Ring { picked } => (cell * 0.25, Some(1.0), if picked { FRESH } else { REST }),
            Dot::Band { tone, level } => {
                let l = f32::from(level) / 5.0;
                match tone {
                    Tone::Plain => (cell * (0.29 + 0.13 * l), None, Color::white(lerp(REST.a, FRESH.a, l))),
                    Tone::Picked => (cell * (0.24 + 0.18 * l), None, palette::BLUE.with_alpha(lerp(0.45, 1.0, l))),
                    Tone::Late => (cell * (0.29 + 0.13 * l), None, palette::ORANGE.with_alpha(lerp(0.45, 1.0, l))),
                }
            }
        }
    }
}

/// The chart of a day, worked out once for what it is of.
#[derive(Clone, Debug)]
struct Chart {
    key: Key,
    today: Today,
    /// The y scale's top, mg.
    top_mg: f64,
    /// `COLS` columns of `ROWS` cells, each column from the bottom up.
    dots: Vec<Dot>,
    /// The day's drinks that are late for bed.
    late: Vec<u32>,
    /// The picked drink's high point (mg): its guide reaches up to it.
    pick_high: Option<f64>,
}

/// The y scale's top for a day peaking at `peak` mg: room above the peak,
/// in steps of 40 mg, and at least 180 (the web chart's).
fn scale_top(peak: f64) -> f64 {
    ((peak * 1.15 / 40.0).ceil() * 40.0).max(180.0)
}

/// The chart of the day that holds `now`, with `pick` picked. Every drink
/// kept counts: yesterday's evening coffee is still there in the morning.
fn chart_of(log: &Log, prefs: &Prefs, now: LocalMinute, pick: Option<u32>) -> Chart {
    let k = prefs.k();
    let today = Today::of(log, prefs, now);
    let start = today.start;
    let top_mg = scale_top(today.peak.map_or(0.0, |p| p.1));
    let mg_row = top_mg / ROWS as f64;
    let before: Vec<&Drink> = log.drinks.iter().filter(|d| d.at < start).collect();
    let day: Vec<&Drink> = log.day(start).collect();
    let late: Vec<bool> = day.iter().map(|d| today.late(prefs, d.mg, d.at)).collect();
    let mut dots = Vec::with_capacity(COLS * ROWS);
    let mut own = vec![0.0; day.len()];
    for col in 0..COLS {
        let t = (start + col as i64 * COL_MIN) as f64 + COL_MIN as f64 / 2.0;
        let hours = |at: LocalMinute| (t - at as f64) / 60.0;
        let carry: f64 = before.iter().map(|d| in_body(d.mg, hours(d.at), k)).sum();
        for (o, d) in own.iter_mut().zip(&day) {
            *o = in_body(d.mg, hours(d.at), k);
        }
        let total = carry + own.iter().sum::<f64>();
        // The column a drink went in shows the level before it as rings:
        // the picked drink's where two went in together.
        let ring = (0..day.len()).filter(|&i| (day[i].at - start).div_euclid(COL_MIN) == col as i64).fold(None, |r: Option<usize>, i| if r.is_none() || Some(day[i].id) == pick { Some(i) } else { r });
        let below = ring.map_or(0.0, |i| total - own[i]);
        let ring_picked = ring.is_some_and(|i| Some(day[i].id) == pick);
        for row in 0..ROWS {
            let v = (row as f64 + 0.5) * mg_row;
            let dot = if v >= total {
                Dot::Empty
            } else if v < below {
                Dot::Ring { picked: ring_picked }
            } else if v < carry || day.is_empty() {
                Dot::Old
            } else {
                // The band this height falls in, the oldest drink lowest.
                let mut acc = carry;
                let i = own
                    .iter()
                    .position(|o| {
                        acc += o;
                        v < acc
                    })
                    .unwrap_or(day.len() - 1);
                let d = day[i];
                let level = (freshness(hours(d.at), k) * 5.0).round().clamp(0.0, 5.0) as u8;
                let tone = if Some(d.id) == pick {
                    Tone::Picked
                } else if late[i] {
                    Tone::Late
                } else {
                    Tone::Plain
                };
                Dot::Band { tone, level }
            };
            dots.push(dot);
        }
    }
    let pick_high = pick.and_then(|id| log.get(id)).map(|d| {
        let end = d.at + (tmax(k) * 60.0).ceil() as i64 + 20;
        (d.at..=end).step_by(5).map(|t| log.in_body_at(t, k)).fold(0.0, f64::max)
    });
    let late = day.iter().zip(&late).filter(|(_, l)| **l).map(|(d, _)| d.id).collect();
    Chart { key: Key { drinks: log.drinks.clone(), prefs: prefs.sane(), start, pick }, today, top_mg, dots, late, pick_high }
}

/// The chart kept between frames, and its dots as painted.
#[derive(Clone, Debug, Default)]
struct Kept {
    chart: Option<Chart>,
    /// Counts the charts worked out, so the painting knows which one it is of.
    gen: u64,
    painted: Option<Painted>,
}

/// The field's dots as one picture: of chart `gen`, for a look, a scale,
/// cells `cell` wide and a size in pixels.
#[derive(Clone, Debug)]
struct Painted {
    gen: u64,
    look: usize,
    scale: f32,
    cell: f32,
    size: (u32, u32),
    image: Image,
}

impl Kept {
    /// The chart for the log as it is: worked out again only when what it
    /// shows changed.
    fn update(&mut self, log: &Log, prefs: &Prefs, now: LocalMinute, pick: Option<u32>) {
        let start = day_start(now);
        let prefs_now = prefs.sane();
        let same = self.chart.as_ref().is_some_and(|c| c.key.start == start && c.key.pick == pick && c.key.prefs == prefs_now && c.key.drinks == log.drinks);
        if !same {
            self.chart = Some(chart_of(log, prefs, now, pick));
            self.gen += 1;
        }
    }

    /// The dots as one picture, `size` pixels at `scale` with cells `cell`
    /// wide, in this frame's look: painted again only when the chart, the
    /// look, the scale or the size changed. The renderer's copy of the one
    /// before goes with it.
    fn image(&mut self, ui: &mut Ui, size: (u32, u32), scale: f32, cell: f32) -> Option<Image> {
        let chart = self.chart.as_ref()?;
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        let look = ui.c.look();
        let look_id = look as *const Look as usize;
        if let Some(p) = self.painted.as_ref().filter(|p| p.gen == self.gen && p.look == look_id && p.scale == scale && p.cell == cell && p.size == size) {
            return Some(p.image.clone());
        }
        let image = ImageData::new(size.0, size.1, paint(&chart.dots, size.0, size.1, scale, cell, look));
        if let Some(old) = self.painted.replace(Painted { gen: self.gen, look: look_id, scale, cell, size, image: image.clone() }) {
            ui.c.forget_image(&old.image);
        }
        Some(image)
    }
}

/// The dots as pixels: premultiplied BGRA, `w` by `h`, the columns from
/// the left and each from the bottom up, `cell` DIPs apart at `scale`.
/// Each dot's centre sits on a pixel's, so every dot of a kind is the same
/// pixels: each kind is worked out once (its colour as `look` paints it)
/// and stamped where its dots go.
fn paint(dots: &[Dot], w: u32, h: u32, scale: f32, cell: f32, look: &Look) -> Vec<u8> {
    let (wi, hi) = (w as i32, h as i32);
    let mut px = vec![0u8; w as usize * h as usize * 4];
    let pitch = cell * scale;
    let mut stamps: Vec<(Dot, Stamp)> = Vec::new();
    for (i, dot) in dots.iter().enumerate() {
        let k = match stamps.iter().position(|(d, _)| d == dot) {
            Some(k) => k,
            None => {
                stamps.push((*dot, Stamp::of(*dot, scale, cell, look)));
                stamps.len() - 1
            }
        };
        let s = &stamps[k].1;
        let (col, row) = ((i / ROWS) as f32, (i % ROWS) as f32);
        let (x0, y0) = (((col + 0.5) * pitch).floor() as i32 - s.n, (h as f32 - (row + 0.5) * pitch).floor() as i32 - s.n);
        let side = 2 * s.n + 1;
        for sy in 0..side {
            let y = y0 + sy;
            if !(0..hi).contains(&y) {
                continue;
            }
            for sx in 0..side {
                let x = x0 + sx;
                let src = s.px[(sy * side + sx) as usize];
                if src[3] == 0 || !(0..wi).contains(&x) {
                    continue;
                }
                // Over what is there already: a dot's soft edge can reach its neighbour's.
                let o = (y as usize * w as usize + x as usize) * 4;
                let keep = 255 - u32::from(src[3]);
                for (j, v) in src.into_iter().enumerate() {
                    px[o + j] = (u32::from(v) + (u32::from(px[o + j]) * keep + 127) / 255) as u8;
                }
            }
        }
    }
    px
}

/// One kind of dot as pixels: premultiplied BGRA for the square of pixels
/// `n` each way round the one its centre is on, rows from the top.
#[derive(Debug)]
struct Stamp {
    n: i32,
    px: Vec<[u8; 4]>,
}

impl Stamp {
    fn of(dot: Dot, scale: f32, cell: f32, look: &Look) -> Stamp {
        let (r, ring, color) = dot.style(cell);
        let c = look.paint(color);
        let rad = r * scale;
        let half = ring.map(|lw| lw * scale / 2.0);
        let n = (rad + half.unwrap_or(0.0) + 1.0).ceil() as i32;
        let mut px = Vec::with_capacity(((2 * n + 1) * (2 * n + 1)) as usize);
        for dy in -n..=n {
            for dx in -n..=n {
                let d = (dx as f32).hypot(dy as f32);
                // How much of the pixel the dot covers, from its distance to the edge.
                let cover = match half {
                    None => rad + 0.5 - d,
                    Some(hw) => hw + 0.5 - (d - rad).abs(),
                }
                .clamp(0.0, 1.0);
                let a = c.a.clamp(0.0, 1.0) * cover;
                let byte = |v: f32| (v.clamp(0.0, 1.0) * a * 255.0).round() as u8;
                px.push([byte(c.b), byte(c.g), byte(c.r), (a * 255.0).round() as u8]);
            }
        }
        Stamp { n, px }
    }
}

// ---------------------------------------------------------------- the top line

/// The line across the top: the day's drinks and caffeine against the
/// day's limit on the left; on the right last call for the usual drink,
/// and that drink where there is room.
fn top_row(ui: &mut Ui, r: Rect, today: &Today, prefs: &Prefs, now: LocalMinute) {
    let (sum, rest, over) = summary(today, prefs);
    let strong = TextStyle::body().weight(Weight::Semibold);
    let x = run(ui, r.x, r, &sum, &strong);
    let x = run(ui, x, r, &rest, &TextStyle::body().color(if over { palette::ORANGE } else { palette::TEXT_DIM }));
    let (call, warn) = last_call_line(today, now);
    let call_st = strong.color(if warn { palette::ORANGE } else { palette::TEXT });
    let usual = format!(" \u{b7} {}", today.usual.phrase);
    let dim = TextStyle::body().color(palette::TEXT_DIM);
    let cw = ui.c.measure(&call, &call_st, r.w).0.ceil();
    let uw = ui.c.measure(&usual, &dim, r.w).0.ceil();
    let room = (r.right() - x - 24.0).max(0.0);
    let uw = if cw + uw <= room { uw } else { 0.0 };
    if uw > 0.0 {
        ui.c.text(&usual, Rect::new(r.right() - uw, r.y, uw, r.h), &dim);
    }
    let cw = cw.min(room);
    ui.c.text(&call, Rect::new(r.right() - uw - cw, r.y, cw + 1.0, r.h), &call_st);
}

/// The top line's left: "3 drinks · 285 mg", then " of 400 today" or, past
/// the day's limit, " · over your 400 mg" (true).
fn summary(today: &Today, prefs: &Prefs) -> (String, String, bool) {
    let limit = prefs.sane().day_limit_mg;
    let sum = match today.count {
        0 => "0 mg".to_string(),
        1 => format!("1 drink \u{b7} {} mg", mg_text(today.total_mg)),
        n => format!("{n} drinks \u{b7} {} mg", mg_text(today.total_mg)),
    };
    if today.over_day_limit {
        (sum, format!(" \u{b7} over your {limit} mg"), true)
    } else {
        (sum, format!(" of {limit} today"), false)
    }
}

/// The top line's right: last call for the usual drink, and true where it
/// warns (it has passed, or no time today is early enough).
fn last_call_line(today: &Today, now: LocalMinute) -> (String, bool) {
    match today.last_call {
        None => ("No last call".into(), false),
        Some(call) if call < today.start => (format!("Too strong for a {} bed", hhmm(today.bed)), true),
        Some(call) if call > now => (format!("Last call {}", hhmm(call)), false),
        Some(call) => (format!("Last call was {}", hhmm(call)), true),
    }
}

/// `s` drawn from `x` on the line `r`; the x after it.
fn run(ui: &mut Ui, x: f32, r: Rect, s: &str, st: &TextStyle) -> f32 {
    let room = (r.right() - x).max(0.0);
    if room <= 0.0 || s.is_empty() {
        return x;
    }
    let w = ui.c.measure(s, st, room).0.min(room);
    ui.c.text(s, Rect::new(x, r.y, w + 1.0, r.h), st);
    x + w
}

// ---------------------------------------------------------------- the chart

/// The field (`field`, on the pixel grid) with its lines and labels, the
/// day's drinks under it, and the hours.
fn draw_chart(ui: &mut Ui, st: &mut AppState, lay: &Layout, field: Rect, c: &Chart, image: Option<&Image>, pick: Option<u32>) {
    let today = &c.today;
    let start = today.start;
    let x_of = |t: LocalMinute| field.x + (t - start) as f32 / DAY as f32 * field.w;
    let y_of = |mg: f64| field.bottom() - (mg / c.top_mg) as f32 * field.h;
    let scale = ui.c.scale().max(0.5);
    // A one-pixel line's centre, on the pixel grid.
    let px = |v: f32| (v * scale).floor() / scale + 0.5 / scale;
    if let Some(image) = image {
        ui.c.image_exact(image, field, 0.0);
    }

    // The 50 mg line, dashed, with its "50".
    let y50 = px(y_of(50.0));
    dashed(ui, Point::new(field.x - 6.0, y50), Point::new(field.right() + 2.0, y50), 3.0, 5.0, Color::white(0.30), 1.0);
    ui.c.text("50", lay.fifty(field, y50), &TextStyle::caption().tabular().color(palette::TEXT_FAINT));

    // The axis with a tick every three hours, orange from last call to bedtime.
    let ay = px(lay.axis);
    let axis_c = Color::white(0.32);
    ui.c.line(Point::new(field.x, ay), Point::new(field.right(), ay), axis_c, 1.0);
    for h in 0..=8 {
        let x = px(field.x + h as f32 * field.w / 8.0);
        ui.c.line(Point::new(x, ay), Point::new(x, ay + if h % 2 == 0 { 5.0 } else { 3.0 }), axis_c, 1.0);
    }
    if let Some(call) = today.last_call {
        ui.c.line(Point::new(x_of(call.max(start)), ay), Point::new(x_of(today.bed), ay), palette::ORANGE, 2.0);
        if call >= start {
            let x = px(x_of(call));
            dashed(ui, Point::new(x, field.y), Point::new(x, ay - 2.0), 3.0, 3.0, palette::ORANGE, 1.0);
        }
    }

    // PEAK and BED above the field, on lines down to the peak and to the axis.
    let first = TextStyle::caption().weight(Weight::Semibold).tabular();
    let second = TextStyle::caption().tabular();
    let bed_text = (format!("BED {}", hhmm(today.bed)), format!("{} MG STILL IN YOU", mg_text(today.at_bed_mg)));
    let peak_text = today.peak.map(|(t, mg)| (format!("PEAK {} MG", mg_text(mg)), hhmm(t)));
    let mut size = |(a, b): &(String, String)| Size::new(ui.c.measure(a, &first, 400.0).0.max(ui.c.measure(b, &second, 400.0).0).ceil() + 1.0, 2.0 * LABEL_LINE);
    let bed = Mark { x: x_of(today.bed), y: lay.axis, size: size(&bed_text) };
    let peak = today.peak.zip(peak_text.as_ref()).map(|((t, mg), text)| Mark { x: x_of(t), y: y_of(mg), size: size(text) });
    let (peak_at, bed_at) = place_labels(lay.chart.x, lay.chart.right(), lay.levels, peak, bed);
    if let (Some(p), Some(at)) = (peak, peak_at) {
        let x = px(p.x);
        ui.c.line(Point::new(x, at.r.bottom() + 3.0), Point::new(x, (p.y - 4.0).max(at.r.bottom() + 3.0)), Color::white(0.50), 1.0);
    }
    let x = px(bed.x);
    ui.c.line(Point::new(x, bed_at.r.bottom() + 3.0), Point::new(x, lay.axis - 1.0), Color::white(0.55), 1.0);
    if let (Some(at), Some((a, b))) = (peak_at, &peak_text) {
        label(ui, at, a, b, &first, &second.color(palette::TEXT_DIM));
    }
    label(ui, bed_at, &bed_text.0, &bed_text.1, &first, &second);

    // Under the axis, the day's drinks: dots to drag along the day, each
    // with a dotted guide up to the field (the picked one's up to the high
    // point it causes), the picked one on top.
    let day: Vec<(u32, LocalMinute)> = st.store.caffeine.day(start).map(|d| (d.id, d.at)).collect();
    let mut order: Vec<usize> = (0..day.len()).collect();
    order.sort_by_key(|&i| Some(day[i].0) == pick);
    for &i in &order {
        let (id, at) = day[i];
        let picked = Some(id) == pick;
        let x = px(x_of(at));
        let top = if picked { c.pick_high.map_or(field.bottom() + 2.0, y_of) } else { field.bottom() + 2.0 };
        let (from, color) = if picked { (CUP_PICKED_R + 3.0, palette::BLUE.with_alpha(0.95)) } else { (CUP_R + 3.0, Color::white(0.42)) };
        dashed(ui, Point::new(x, lay.cups - from), Point::new(x, top), 0.5, 3.0, color, 1.4);
    }
    let s = &mut st.ui.caffeine;
    let mut moved = None;
    for &i in &order {
        let (id, at) = day[i];
        let picked = Some(id) == pick;
        let centre = Point::new(x_of(at), lay.cups);
        let resp = ui.button(cup_id(id), Rect::from_center(centre, 16.0, 24.0));
        // A tap (a touchpad's, down and up between two frames) picks it too.
        if resp.clicked {
            s.pick = Some(id);
        }
        if resp.pressed {
            if s.drag.is_none_or(|d| d.id != id) {
                s.drag = Some(Drag { id, from: at, moved: false });
                s.pick = Some(id);
            }
            if let (Some(d), Some((dx, _))) = (s.drag.as_mut(), resp.drag) {
                if let Some(t) = d.to(dx, field.w, start).filter(|t| *t != at) {
                    moved = Some((id, t));
                }
            }
        }
        let color = if picked {
            palette::BLUE
        } else if c.late.contains(&id) {
            palette::ORANGE
        } else if resp.hovered || resp.pressed {
            Color::white(0.95)
        } else {
            Color::white(0.75)
        };
        ui.c.fill_circle(centre, if picked { CUP_PICKED_R } else { CUP_R }, color);
    }
    // The drag ends with the press; a drink that moved is saved then.
    let mut save = false;
    if let Some(d) = s.drag {
        if !(ui.input.down && ui.input.pressed_id == Some(cup_id(d.id))) {
            s.drag = None;
            save = d.moved;
        }
    }

    // The times under the dots: the picked one's always, the others where they fit.
    let tst = TextStyle::caption().tabular();
    let tw = ui.c.measure("00:00", &tst, 100.0).0.ceil();
    let xs: Vec<(f32, bool)> = day.iter().map(|&(id, at)| (x_of(at), Some(id) == pick)).collect();
    for (&(id, at), x) in day.iter().zip(place_times(&xs, tw, lay.chart.x, lay.chart.right())) {
        if let Some(x) = x {
            let col = if Some(id) == pick { palette::BLUE } else { palette::TEXT_FAINT };
            ui.c.text(&hhmm(at), Rect::new(x - tw / 2.0 - 1.0, lay.times.y, tw + 2.0, lay.times.h), &tst.color(col).center());
        }
    }
    let hst = TextStyle::caption().tabular().color(palette::TEXT_FAINT).center();
    for i in 0..=4 {
        let x = field.x + i as f32 * field.w / 4.0;
        ui.c.text(&format!("{:02}", (6 + 6 * i) % 24), Rect::new(x - HOUR_W / 2.0, lay.hours.y, HOUR_W, lay.hours.h), &hst);
    }

    if let Some((id, t)) = moved {
        st.store.caffeine.move_to(id, t);
        ui.request_repaint();
    }
    if save {
        st.save();
    }
}

/// Milligrams as the page writes them: whole, never "-0" (a sum of no
/// drinks is -0.0).
fn mg_text(mg: f64) -> String {
    format!("{:.0}", mg.max(0.0) + 0.0)
}

fn cup_id(id: u32) -> u64 {
    wid_n("caffeine.cup", u64::from(id))
}

/// A two-line label at `at`, its lines right-aligned when it sits left of its line.
fn label(ui: &mut Ui, at: Spot, a: &str, b: &str, first: &TextStyle, second: &TextStyle) {
    let (first, second) = if at.left { (first.right(), second.right()) } else { (*first, *second) };
    ui.c.text(a, Rect::new(at.r.x, at.r.y, at.r.w, LABEL_LINE), &first);
    ui.c.text(b, Rect::new(at.r.x, at.r.y + LABEL_LINE, at.r.w, LABEL_LINE), &second);
}

/// A dashed line from `a` to `b`, `on` DIPs drawn and `off` left out in
/// turn: one path (short dashes with round caps are dots).
fn dashed(ui: &mut Ui, a: Point, b: Point, on: f32, off: f32, c: Color, width: f32) {
    let len = a.dist(b);
    if len <= 0.0 {
        return;
    }
    let (dx, dy) = ((b.x - a.x) / len, (b.y - a.y) / len);
    let mut segs = Vec::with_capacity(2 * (len / (on + off)) as usize + 2);
    let mut t = 0.0;
    while t < len {
        let t1 = (t + on).min(len);
        segs.push(Seg::MoveTo(a.x + dx * t, a.y + dy * t));
        segs.push(Seg::LineTo(a.x + dx * t1, a.y + dy * t1));
        t += on + off;
    }
    ui.c.path(&segs, None, Some((c, width)));
}

/// A label's size and its line: at `x`, from under the label down to `y`.
#[derive(Clone, Copy, Debug)]
struct Mark {
    x: f32,
    y: f32,
    size: Size,
}

/// Where a label goes: its box, and whether it sits left of its line.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Spot {
    r: Rect,
    left: bool,
}

/// The PEAK and BED labels' places, as the web chart's `placeLabels` finds
/// them: each beside its line on either side, on one of the two levels
/// that end at `bottoms` (the lower first), inside `x0..x1`; never over
/// each other or over the other's line. Of the places that do, the lower
/// levels win, then a label right of its line, then one kept nearer it.
fn place_labels(x0: f32, x1: f32, bottoms: [f32; 2], peak: Option<Mark>, bed: Mark) -> (Option<Spot>, Spot) {
    // The label of `m` on that side and level, moved inside the bounds, and how far it moved.
    let at = |m: Mark, left: bool, level: usize| {
        let want = if left { m.x - LABEL_SIDE - m.size.w } else { m.x + LABEL_SIDE };
        let x = want.clamp(x0, (x1 - m.size.w).max(x0));
        (Spot { r: Rect::new(x, bottoms[level] - m.size.h, m.size.w, m.size.h), left }, (x - want).abs())
    };
    let over = |a: Rect, b: Rect| a.x < b.right() + 12.0 && b.x < a.right() + 12.0 && a.y < b.bottom() + 4.0 && b.y < a.bottom() + 4.0;
    // The line at `x` from `y1` down to `y2` runs under the box `r`.
    let under = |x: f32, y1: f32, y2: f32, r: Rect| x > r.x - 6.0 && x < r.right() + 6.0 && y2 > r.y && y1 < r.bottom();
    let mut best: Option<(f32, Option<Spot>, Spot)> = None;
    for bed_level in 0..2 {
        for bed_left in [false, true] {
            let (b, b_off) = at(bed, bed_left, bed_level);
            let base = b_off + 10.0 * bed_level as f32 + 1.2 * f32::from(u8::from(bed_left));
            let mut tries: Vec<(f32, Option<Spot>)> = Vec::with_capacity(4);
            match peak {
                None => tries.push((base, None)),
                Some(pk) => {
                    for peak_level in 0..2 {
                        for peak_left in [false, true] {
                            let (p, p_off) = at(pk, peak_left, peak_level);
                            let bad = [over(p.r, b.r), under(pk.x, p.r.bottom(), pk.y, b.r), under(bed.x, b.r.bottom(), bed.y, p.r)].into_iter().filter(|x| *x).count();
                            let cost = base + 100.0 * bad as f32 + p_off + 10.0 * peak_level as f32 + f32::from(u8::from(peak_left));
                            tries.push((cost, Some(p)));
                        }
                    }
                }
            }
            for (cost, p) in tries {
                if best.as_ref().is_none_or(|(c, _, _)| cost < *c) {
                    best = Some((cost, p, b));
                }
            }
        }
    }
    let (_, p, b) = best.unwrap_or((0.0, None, at(bed, false, 0).0));
    (p, b)
}

/// Where the time under each dot goes (its centre), in the dots' order,
/// for times `w` wide inside `x0..x1`: the picked one's (true) always,
/// under its dot as far as the edges let it; another where it fits beside
/// those already placed, moved aside at most [`NUDGE`]; `None` where one
/// doesn't fit.
fn place_times(cups: &[(f32, bool)], w: f32, x0: f32, x1: f32) -> Vec<Option<f32>> {
    let mut order: Vec<usize> = (0..cups.len()).collect();
    order.sort_by(|&a, &b| cups[b].1.cmp(&cups[a].1).then(cups[a].0.total_cmp(&cups[b].0)));
    let room = w + 8.0;
    let (lo, hi) = (x0 + w / 2.0, (x1 - w / 2.0).max(x0 + w / 2.0));
    let mut placed: Vec<f32> = Vec::with_capacity(cups.len());
    let mut out = vec![None; cups.len()];
    for i in order {
        let want = cups[i].0.clamp(lo, hi);
        let free = |x: f32| x >= lo - 0.01 && x <= hi + 0.01 && placed.iter().all(|p| (p - x).abs() >= room - 0.01);
        let tries = std::iter::once(want).chain(placed.iter().flat_map(|p| [p + room, p - room]));
        let best = tries.filter(|x| (x - want).abs() <= NUDGE + 0.01 && free(*x)).min_by(|a, b| (a - want).abs().total_cmp(&(b - want).abs()));
        if let Some(x) = best {
            placed.push(x);
            out[i] = Some(x);
        }
    }
    out
}

/// A drink dragged by its dot under the axis.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    id: u32,
    /// Its time when the press began.
    from: LocalMinute,
    /// The pointer went far enough: the press is a drag, not a click, and
    /// the log is saved when it ends.
    moved: bool,
}

impl Drag {
    /// Where the drag has the drink with the pointer `dx` DIPs along a field
    /// `w` wide over the day from `start`: in 5-minute steps, inside the
    /// day; `None` until the pointer has gone [`DRAG_START`].
    fn to(&mut self, dx: f32, w: f32, start: LocalMinute) -> Option<LocalMinute> {
        if !self.moved && dx.abs() < DRAG_START {
            return None;
        }
        self.moved = true;
        let t = self.from as f64 + f64::from(dx) / f64::from(w.max(1.0)) * DAY as f64;
        Some((((t / 5.0).round() * 5.0) as LocalMinute).clamp(start, start + DAY - 5))
    }
}

// ---------------------------------------------------------------- the list

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAct {
    Pick,
    Remove,
}

/// The day's drinks, newest last, with the wheel to scroll them; Add a
/// drink under them.
fn list(ui: &mut Ui, st: &mut AppState, lay: &Layout, c: &Chart, pick: Option<u32>, now: LocalMinute) {
    let r = lay.list;
    let day: Vec<Drink> = st.store.caffeine.day(c.today.start).cloned().collect();
    let card = super::card::pill_card(st, lay.area);
    let glass = ui.c.look().material == Material::Glass;
    let mut act = None;
    if day.is_empty() {
        ui.c.text("Nothing yet today.", Rect::new(r.x + ROW_PAD, r.y, (r.w - 2.0 * ROW_PAD).max(0.0), 20.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    } else {
        let s = &mut st.ui.caffeine;
        let max = (day.len() as f32 * ROW_H - r.h).max(0.0);
        s.scroll = (s.scroll - ui.wheel_in(r) * ROW_H).clamp(0.0, max);
        if std::mem::take(&mut s.reveal) {
            if let Some(i) = day.iter().position(|d| Some(d.id) == pick) {
                let top = i as f32 * ROW_H;
                s.scroll = s.scroll.clamp((top + ROW_H - r.h).max(0.0), top).clamp(0.0, max);
            }
        }
        let clip = Rect::new(r.x - 2.0, r.y, r.w + 4.0, r.h);
        ui.c.clip(clip);
        for (i, d) in day.iter().enumerate() {
            let row = Rect::new(r.x, r.y + i as f32 * ROW_H - s.scroll, r.w, ROW_H);
            if row.bottom() <= r.y || row.y >= r.bottom() {
                continue;
            }
            if let Some(a) = draw_row(ui, card, row, r, d, Some(d.id) == pick, c.late.contains(&d.id)) {
                act = Some((d.id, a));
            }
        }
        // The list fades out at an edge it scrolls on past.
        if !glass && s.scroll > 0.5 {
            fade(ui, Rect::new(clip.x, r.y, clip.w, 12.0), true);
        }
        if !glass && s.scroll < max - 0.5 {
            fade(ui, Rect::new(clip.x, r.bottom() - 14.0, clip.w, 14.0), false);
        }
        ui.c.unclip();
    }
    if ui.pill(wid("caffeine.add"), lay.add, "Add a drink", Some(Icon::Plus), PillStyle::Primary(st.accent)).clicked {
        st.ui.caffeine.sheet = Some(Sheet::new(now, st.settings.caffeine.bean));
    }
    match act {
        Some((id, RowAct::Pick)) => st.ui.caffeine.pick = Some(id),
        Some((id, RowAct::Remove)) => remove(st, id),
        None => {}
    }
}

/// One of the day's drinks in the list (`r`, cut to the list's `clip`):
/// its time, its title over its detail, its mg and a remove button; lit
/// while picked. Its time is blue while picked, orange while late for bed.
fn draw_row(ui: &mut Ui, card: Option<(Rect, f32)>, r: Rect, clip: Rect, d: &Drink, picked: bool, late: bool) -> Option<RowAct> {
    let n = u64::from(d.id);
    let bg = r.inset(0.0, 1.0);
    let resp = ui.button(wid_n("caffeine.row", n), intersect(bg, clip));
    if picked {
        super::card::fill_box(ui, card, bg, 9.0, Color::white(0.10));
    } else if resp.hovered || resp.pressed {
        super::card::fill_box(ui, card, bg, 9.0, Color::white(if resp.pressed { 0.08 } else { 0.05 }));
    }
    let rm_r = Rect::from_center(Point::new(r.right() - 11.0, r.center().y), 20.0, 20.0);
    let rm = ui.button(wid_n("caffeine.remove", n), intersect(rm_r, clip));
    if rm.hovered || rm.pressed {
        ui.c.fill_circle(rm_r.center(), 10.0, if rm.pressed { palette::SURFACE_PRESSED } else { palette::SURFACE_HOVER });
    }
    ui.c.icon(Icon::Close, Rect::from_center(rm_r.center(), 10.0, 10.0), if rm.hovered { palette::TEXT } else { palette::TEXT_FAINT });
    let small = TextStyle::footnote().tabular();
    let time_c = if picked {
        palette::BLUE
    } else if late {
        palette::ORANGE
    } else {
        palette::TEXT_DIM
    };
    ui.c.text(&hhmm(d.at), Rect::new(r.x + ROW_PAD, r.y, 36.0, r.h), &small.color(time_c));
    // The title and the mg on the first line, the detail under them both
    // (a drink with none has its one line in the middle).
    let mg = format!("{} mg", mg_text(d.mg));
    let mw = ui.c.measure(&mg, &small, 100.0).0.ceil();
    let right = rm_r.x - 2.0;
    let tx = r.x + ROW_PAD + 38.0;
    let detail = d.detail();
    let line = if detail.is_empty() { Rect::new(tx, r.y, right - tx, r.h) } else { Rect::new(tx, r.y + 3.0, right - tx, 18.0) };
    ui.c.text(&mg, Rect::new(right - mw - 1.0, line.y, mw + 1.0, line.h), &small.right());
    ui.c.text(d.title(), Rect::new(tx, line.y, (line.w - mw - 6.0).max(0.0), line.h), &TextStyle::body());
    if !detail.is_empty() {
        ui.c.text(&detail, Rect::new(tx, r.y + 20.0, (right - tx).max(0.0), 15.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    }
    if rm.clicked {
        Some(RowAct::Remove)
    } else if resp.clicked {
        Some(RowAct::Pick)
    } else {
        None
    }
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// The island's ground over `r`, solid at the list's cut edge (its top, or
/// its bottom) and clear inward.
fn fade(ui: &mut Ui, r: Rect, top: bool) {
    const STEPS: usize = 8;
    let step = r.h / STEPS as f32;
    for k in 0..STEPS {
        let a = 0.92 * (1.0 - (k as f32 + 0.5) / STEPS as f32).powf(1.4);
        let y = if top { r.y + k as f32 * step } else { r.bottom() - (k as f32 + 1.0) * step };
        ui.c.fill_rect(Rect::new(r.x, y, r.w, step), palette::ISLAND.with_alpha(a));
    }
}

/// Takes a drink off the log and saves; the picked one falls back to the day's latest.
fn remove(st: &mut AppState, id: u32) {
    if st.store.caffeine.remove(id) {
        if st.ui.caffeine.pick == Some(id) {
            st.ui.caffeine.pick = None;
        }
        st.save();
    }
}

// ---------------------------------------------------------------- Add a drink

/// Add a drink: its first step picks the kind, the second the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    /// The page's day it adds to (its 06:00): the time stays inside it, and
    /// the sheet closes once the day is over.
    day: LocalMinute,
    /// The kind picked; `None` on the first step.
    kind: Option<Kind>,
    /// Shots or servings.
    amount: u8,
    bean: Bean,
    /// Your own's name and caffeine.
    name: String,
    mg: u16,
    /// The time set with −15 and +15; `None` = now.
    at: Option<LocalMinute>,
    /// The keyboard comes to the sheet (Your own: to its name) on its next
    /// frame, the kind just picked.
    focus: bool,
}

/// What a frame of the sheet asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Stay,
    Close,
    Add,
}

impl Sheet {
    /// The sheet as Add a drink opens it at `now`, with the beans the last
    /// espresso drink had.
    fn new(now: LocalMinute, bean: Bean) -> Sheet {
        Sheet { day: day_start(now), kind: None, amount: 1, bean, name: String::new(), mg: OWN_START, at: None, focus: false }
    }

    /// On to the second step for `kind`, at its usual amount.
    fn pick(&mut self, kind: Kind) {
        self.kind = Some(kind);
        self.amount = kind.default_amount();
        self.focus = true;
    }

    /// When the drink went in: the time set, else `now`; inside the day.
    fn at(&self, now: LocalMinute) -> LocalMinute {
        self.at.unwrap_or(now).clamp(self.day, self.day + DAY - 1)
    }

    /// −15 and +15: the time shown, moved, inside the day.
    fn shift(&mut self, now: LocalMinute, minutes: i64) {
        self.at = Some((self.at(now) + minutes).clamp(self.day, self.day + DAY - 1));
    }

    fn step_amount(&mut self, d: i32) {
        self.amount = (i32::from(self.amount) + d).clamp(1, i32::from(MAX_AMOUNT)) as u8;
    }

    fn step_mg(&mut self, d: i32) {
        self.mg = (i32::from(self.mg) + d * i32::from(OWN_STEP)).clamp(i32::from(OWN_MIN), i32::from(OWN_MAX)) as u16;
    }

    /// The drink's caffeine, mg.
    fn mg(&self) -> f64 {
        match self.kind {
            Some(Kind::Custom) => f64::from(self.mg),
            Some(k) => k.mg(self.amount, self.bean),
            None => 0.0,
        }
    }

    /// The drink to add (`None` until a kind is picked).
    fn drink(&self, now: LocalMinute) -> Option<Drink> {
        let kind = self.kind?;
        let own = kind == Kind::Custom;
        Some(Drink {
            id: 0,
            at: self.at(now),
            kind,
            amount: if own { 1 } else { self.amount },
            bean: kind.shots().then_some(self.bean),
            mg: self.mg(),
            name: if own { self.name.trim().to_string() } else { String::new() },
        })
    }

    /// A key while the sheet has the keyboard: Enter adds; Esc goes back to
    /// the kinds, and from there closes the sheet.
    fn key(&mut self, k: KeyInput) -> Act {
        match (k, self.kind) {
            (KeyInput::Enter, Some(_)) => Act::Add,
            (KeyInput::Escape, Some(_)) => {
                self.kind = None;
                Act::Stay
            }
            (KeyInput::Escape, None) => Act::Close,
            _ => Act::Stay,
        }
    }
}

/// Adds the sheet's drink: into the log (tidied, saved), picked and in view,
/// the sheet closed. Returns its id.
fn add(st: &mut AppState, sh: &Sheet, now: LocalMinute) -> Option<u32> {
    let d = sh.drink(now)?;
    let log = &mut st.store.caffeine;
    let id = log.add(d);
    log.tidy(now);
    st.save();
    let s = &mut st.ui.caffeine;
    s.pick = Some(id);
    s.reveal = true;
    s.sheet = None;
    Some(id)
}

/// Add a drink, over the whole page.
fn sheet(ui: &mut Ui, st: &mut AppState, area: Rect, now: LocalMinute) {
    let Some(mut sh) = st.ui.caffeine.sheet.take() else { return };
    let still = st.ui.caffeine.still;
    let id = wid(SHEET_ID);
    let mut act = Act::Stay;
    // The keyboard comes to the sheet as a kind is picked: Enter adds and
    // Esc goes back. Your own's name field takes it instead (`details`).
    if sh.kind != Some(Kind::Custom) {
        let take = std::mem::take(&mut sh.focus) && !still;
        for k in ui.key_focus(id, take) {
            let back = sh.kind.is_some() && k == KeyInput::Escape;
            if let a @ (Act::Add | Act::Close) = sh.key(k) {
                act = a;
            }
            // Back on the kinds, the sheet keeps the keyboard for another Esc.
            if back {
                ui.key_focus(id, true);
            }
        }
    }
    let mut drawn = Act::Stay;
    match sh.kind {
        None => kinds(ui, &mut sh, area, &mut drawn),
        Some(kind) => details(ui, st, &mut sh, kind, area, now, still, &mut drawn),
    }
    if act == Act::Stay {
        act = drawn;
    }
    match act {
        Act::Stay => st.ui.caffeine.sheet = Some(sh),
        Act::Close => ui.blur(),
        Act::Add => {
            add(st, &sh, now);
            ui.blur();
        }
    }
}

/// The kinds' chips under the title, five to a row (four on a narrow island).
fn kind_grid(area: Rect) -> Vec<Rect> {
    let cols = if area.w >= NARROW { KIND_COLS } else { KIND_COLS_NARROW };
    let w = (area.w - (cols - 1) as f32 * PILL_GAP) / cols as f32;
    (0..Kind::PICK.len()).map(|i| Rect::new(area.x + (i % cols) as f32 * (w + PILL_GAP), area.y + SHEET_TOP + (i / cols) as f32 * (PILL_SMALL + PILL_GAP), w, PILL_SMALL)).collect()
}

/// Back and Add, at the sheet's bottom right.
fn buttons(area: Rect) -> (Rect, Rect) {
    let y = area.y + HEIGHT - PILL_SMALL;
    let add = Rect::new(area.right() - BUTTON_W, y, BUTTON_W, PILL_SMALL);
    (Rect::new(add.x - PILL_GAP - BUTTON_W, y, BUTTON_W, PILL_SMALL), add)
}

/// The sheet's first step: what kind of drink.
fn kinds(ui: &mut Ui, sh: &mut Sheet, area: Rect, act: &mut Act) {
    ui.c.text("Add a drink", Rect::new(area.x, area.y, area.w, 22.0), &TextStyle::headline());
    for (i, (kind, r)) in Kind::PICK.iter().zip(kind_grid(area)).enumerate() {
        if ui.pill(wid_n("caffeine.kind", i as u64), r, kind.name(), None, PillStyle::Neutral).clicked {
            sh.pick(*kind);
        }
    }
    // Its one button at the right edge, where the next step has Add.
    if ui.pill(wid("caffeine.back"), buttons(area).1, "Back", None, PillStyle::Utility).clicked {
        *act = Act::Close;
    }
}

/// The sheet's second step for `kind`: how many (and the beans), or Your
/// own's name and mg; when; what it leaves at bedtime; Back and Add.
#[allow(clippy::too_many_arguments)]
fn details(ui: &mut Ui, st: &mut AppState, sh: &mut Sheet, kind: Kind, area: Rect, now: LocalMinute, still: bool, act: &mut Act) {
    let title_st = TextStyle::headline();
    let tw = ui.c.measure(kind.name(), &title_st, area.w).0.ceil();
    ui.c.text(kind.name(), Rect::new(area.x, area.y, tw + 2.0, 22.0), &title_st);
    let each = each_line(kind, sh.bean);
    ui.c.text(&each, Rect::new(area.x + tw + 10.0, area.y + 1.0, (area.w - tw - 10.0).max(0.0), 22.0), &TextStyle::footnote().color(palette::TEXT_DIM));
    let label_st = TextStyle::body().color(palette::TEXT_DIM);
    let label = |ui: &mut Ui, y: f32, s: &str| ui.c.text(s, Rect::new(area.x, y, LABEL_W - 8.0, PILL_SMALL), &label_st);
    let cx = area.x + LABEL_W;
    let mut y = area.y + SHEET_TOP;
    if kind == Kind::Custom {
        label(ui, y, "Name");
        let field_id = wid(NAME_ID);
        if std::mem::take(&mut sh.focus) && !still {
            ui.focus_text(field_id, sh.name.chars().count());
        }
        let resp = ui.text_input(field_id, Rect::new(cx, y, NAME_W, PILL_SMALL), &mut sh.name, "What you had", 40);
        if resp.submitted {
            *act = Act::Add;
        } else if resp.cancelled {
            // Esc goes back to the kinds, which keep the keyboard for another Esc.
            sh.kind = None;
            ui.key_focus(wid(SHEET_ID), true);
        }
        y += SHEET_ROW;
        label(ui, y, "Caffeine");
        let d = stepper(ui, "caffeine.mg", cx, y, &format!("{} mg", sh.mg), sh.mg > OWN_MIN, sh.mg < OWN_MAX);
        sh.step_mg(d);
    } else {
        label(ui, y, "How many");
        let d = stepper(ui, "caffeine.amount", cx, y, &kind.unit().count(sh.amount), sh.amount > 1, sh.amount < MAX_AMOUNT);
        sh.step_amount(d);
        if kind.shots() {
            y += SHEET_ROW;
            label(ui, y, "Beans");
            let names: Vec<String> = Bean::ALL.iter().map(|b| format!("{} {:.0}", b.name(), b.mg_per_shot())).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let sel = Bean::ALL.iter().position(|b| *b == sh.bean).unwrap_or(0);
            if let Some(i) = super::settings::picker(ui, "caffeine.bean", cx, y, PILL_SMALL, &names, sel) {
                // The beans picked are where the next espresso drink starts.
                sh.bean = Bean::ALL[i];
                st.settings.caffeine.bean = sh.bean;
                st.cmd(Command::SaveSettings);
            }
        }
    }
    y += SHEET_ROW;
    label(ui, y, "Time");
    let at = sh.at(now);
    let earlier = Rect::new(cx, y, TIME_BTN_W, PILL_SMALL);
    let shown = Rect::new(earlier.right(), y, TIME_W, PILL_SMALL);
    let later = Rect::new(shown.right(), y, TIME_BTN_W, PILL_SMALL);
    if pill_if(ui, "caffeine.earlier", earlier, "\u{2212}15", None, at > sh.day) {
        sh.shift(now, -15);
    }
    ui.c.text(&hhmm(at), shown, &TextStyle::value().tabular().center());
    if pill_if(ui, "caffeine.later", later, "+15", None, at < sh.day + DAY - 1) {
        sh.shift(now, 15);
    }
    if ui.pill(wid("caffeine.now"), Rect::new(later.right() + PILL_GAP, y, NOW_W, PILL_SMALL), "Now", None, PillStyle::Utility).clicked {
        sh.at = None;
    }
    y += SHEET_ROW + 4.0;
    let (line, warn) = effect(sh.mg(), sh.at(now), sh.day, &st.settings.caffeine);
    ui.c.text(&line, Rect::new(area.x, y, area.w, 18.0), &TextStyle::body());
    if let Some(warn) = warn {
        ui.c.text(&warn, Rect::new(area.x, y + 20.0, area.w, 18.0), &TextStyle::body().color(palette::ORANGE));
    }
    let (back, add_r) = buttons(area);
    if ui.pill(wid("caffeine.back"), back, "Back", None, PillStyle::Utility).clicked {
        sh.kind = None;
    }
    if ui.pill(wid("caffeine.sheet.add"), add_r, "Add", None, PillStyle::Primary(st.accent)).clicked {
        *act = Act::Add;
    }
}

/// What one of a kind holds, beside its name: "1 shot is 105 mg" (with the
/// beans picked), "1 sachet is 40 mg"; nothing for Your own.
fn each_line(kind: Kind, bean: Bean) -> String {
    match kind {
        Kind::Custom | Kind::Other => String::new(),
        k => format!("{} is {:.0} mg", k.unit().count(1), k.mg(1, bean)),
    }
}

/// What adding `mg` at `at` to the day from `day` does, in a line, and the
/// warning under it when the drink is late for bed (`caffeine::late_for_bed`,
/// as the chart and the list call it): at or after bedtime, after its own
/// last call, or too strong for any time of the day.
fn effect(mg: f64, at: LocalMinute, day: LocalMinute, prefs: &Prefs) -> (String, Option<String>) {
    let p = prefs.sane();
    let (k, limit) = (p.k(), f64::from(p.bed_limit_mg));
    let bed = bed_of(day, p.bedtime);
    if at >= bed {
        return (format!("Adds {} mg", mg_text(mg)), Some(format!("After bedtime ({})", hhmm(bed))));
    }
    let left = in_body(mg, (bed - at) as f64 / 60.0, k);
    let line = format!("Adds {} mg \u{b7} {} mg of it left at bedtime ({})", mg_text(mg), mg_text(left), hhmm(bed));
    let warn = late_for_bed(&p, bed, mg, at).then(|| match last_call_hours(mg, k, limit).map(|h| bed - (h * 60.0).ceil() as i64) {
        Some(call) if call >= day => format!("After this drink's last call ({})", hhmm(call)),
        _ => format!("Too strong for a {} bed", hhmm(bed)),
    });
    (line, warn)
}

/// A pill that does nothing, dimmed, while `on` is false; true when clicked.
fn pill_if(ui: &mut Ui, id: &str, r: Rect, label: &str, icon: Option<Icon>, on: bool) -> bool {
    let base = ui.c.alpha();
    if !on {
        ui.c.set_alpha(base * 0.35);
    }
    let clicked = ui.pill(wid(id), r, label, icon, PillStyle::Neutral).clicked;
    ui.c.set_alpha(base);
    clicked && on
}

/// − value +: the value between two round buttons, each dimmed at its end
/// of the range; the step clicked (−1 or +1), else 0.
fn stepper(ui: &mut Ui, id: &str, x: f32, y: f32, value: &str, less: bool, more: bool) -> i32 {
    let minus = Rect::new(x, y, PILL_SMALL, PILL_SMALL);
    let shown = Rect::new(minus.right(), y, STEP_VALUE_W, PILL_SMALL);
    let plus = Rect::new(shown.right(), y, PILL_SMALL, PILL_SMALL);
    ui.c.text(value, shown, &TextStyle::body().weight(Weight::Semibold).tabular().center());
    let mut step = 0;
    if pill_if(ui, &format!("{id}.less"), minus, "", Some(Icon::Minus), less) {
        step = -1;
    }
    if pill_if(ui, &format!("{id}.more"), plus, "", Some(Icon::Plus), more) {
        step = 1;
    }
    step
}

#[cfg(test)]
mod tests {
    use super::*;
    use poni_core::caffeine::BEDTIMES;
    use poni_core::settings::Settings;
    use poni_core::store::Store;

    /// The page's area at the island's usual width (620), where the app
    /// puts it; and the narrowest and widest the settings allow (480, 900).
    const AREA: Rect = Rect::new(18.0, 50.0, 584.0, HEIGHT);
    const WIDTHS: [f32; 3] = [444.0, 584.0, 864.0];
    /// The widest labels the chart writes, at the caption size (with room to spare).
    const PEAK: Size = Size::new(84.0, 2.0 * LABEL_LINE);
    const BED: Size = Size::new(132.0, 2.0 * LABEL_LINE);
    /// 10 Nov 2026, 00:00.
    const D0: LocalMinute = 20_767 * DAY;

    fn at(h: i64, m: i64) -> LocalMinute {
        D0 + h * 60 + m
    }

    fn state() -> AppState {
        AppState::new(Settings::default(), Store::default())
    }

    fn drink(t: LocalMinute, kind: Kind, amount: u8, bean: Option<Bean>) -> Drink {
        Drink { id: 0, at: t, kind, amount, bean, mg: kind.mg(amount, bean.unwrap_or_default()), name: String::new() }
    }

    fn area(w: f32) -> Rect {
        Rect::new(AREA.x, AREA.y, w, AREA.h)
    }

    fn inside(a: Rect, r: Rect, what: &str) {
        assert!(r.x >= a.x - 0.01 && r.y >= a.y - 0.01 && r.right() <= a.right() + 0.01 && r.bottom() <= a.bottom() + 0.01, "{what} {r:?} leaves the area {a:?}");
    }

    /// The labels as the chart places them for `peak` and `bed` (their x and
    /// the bottom of their lines), checked: inside the chart's column, clear
    /// of each other, and neither under the other's line.
    fn check_labels(a: Rect, peak: Option<(f32, f32)>, bed: f32, what: &str) {
        let lay = Layout::of(a);
        let bed = Mark { x: bed, y: lay.axis, size: BED };
        let peak = peak.map(|(x, y)| Mark { x, y, size: PEAK });
        let (p, b) = place_labels(lay.chart.x, lay.chart.right(), lay.levels, peak, bed);
        let fits = |r: Rect| r.x >= lay.chart.x - 0.01 && r.right() <= lay.chart.right() + 0.01 && r.y >= lay.top.bottom();
        assert!(fits(b.r), "{what}: BED {:?} leaves the chart", b.r);
        let (Some(p), Some(pk)) = (p, peak) else { return };
        assert!(fits(p.r), "{what}: PEAK {:?} leaves the chart", p.r);
        let apart = p.r.right() <= b.r.x || b.r.right() <= p.r.x || p.r.bottom() <= b.r.y || b.r.bottom() <= p.r.y;
        assert!(apart, "{what}: PEAK {:?} over BED {:?}", p.r, b.r);
        let under = |x: f32, y1: f32, y2: f32, r: Rect| x > r.x - 1.0 && x < r.right() + 1.0 && y2 > r.y && y1 < r.bottom();
        assert!(!under(pk.x, p.r.bottom(), pk.y, b.r), "{what}: the peak's line at {} runs under BED {:?}", pk.x, b.r);
        assert!(!under(bed.x, b.r.bottom(), bed.y, p.r), "{what}: the bed line at {} runs under PEAK {:?}", bed.x, p.r);
    }

    /// The labels of a day with these drinks, as `check_labels` checks them.
    fn day_labels(a: Rect, drinks: &[Drink], bedtime: u16, what: &str) {
        let mut log = Log::default();
        for d in drinks {
            log.add(d.clone());
        }
        let prefs = Prefs { bedtime, ..Prefs::default() };
        let c = chart_of(&log, &prefs, at(12, 0), None);
        let field = Layout::of(a).field;
        let x_of = |t: LocalMinute| field.x + (t - c.today.start) as f32 / DAY as f32 * field.w;
        let peak = c.today.peak.map(|(t, mg)| (x_of(t), field.bottom() - (mg / c.top_mg) as f32 * field.h));
        check_labels(a, peak, x_of(c.today.bed), &format!("{what}, {} wide", a.w));
    }

    #[test]
    fn every_rect_stays_inside_the_area() {
        for a in WIDTHS.map(area) {
            let lay = Layout::of(a);
            let inside = |r: Rect, what: &str| inside(a, r, what);
            for (r, what) in [(lay.top, "the top line"), (lay.chart, "the chart"), (lay.field, "the field"), (lay.times, "the times"), (lay.hours, "the hours"), (lay.list, "the list"), (lay.add, "Add a drink")] {
                inside(r, what);
            }
            // The labels' levels sit between the top line and the field.
            assert!(lay.levels[1] - 2.0 * LABEL_LINE >= lay.top.bottom() && lay.levels[0] <= lay.field.y && lay.levels[1] + LABEL_GAP <= lay.levels[0] - 2.0 * LABEL_LINE);
            // The field's square cells, the "50" right of it and the hours under its ends.
            assert!((lay.field.w - COLS as f32 * lay.cell).abs() < 0.01 && (lay.field.h - ROWS as f32 * lay.cell).abs() < 0.01 && lay.cell <= CELL);
            assert!(lay.field.right() + FIFTY_W <= lay.chart.right() + 0.01);
            inside(lay.fifty(lay.field, lay.field.y), "the 50");
            inside(lay.fifty(lay.field, lay.field.bottom()), "the 50");
            for i in 0..=4 {
                inside(Rect::new(lay.field.x + i as f32 * lay.field.w / 4.0 - HOUR_W / 2.0, lay.hours.y, HOUR_W, lay.hours.h), "an hour");
            }
            // The dots under the axis at the day's ends, and the axis's ticks, above the times.
            for x in [lay.field.x, lay.field.right()] {
                inside(Rect::from_center(Point::new(x, lay.cups), 16.0, 24.0), "a drink's dot");
            }
            assert!(lay.axis + 5.0 < lay.cups - CUP_PICKED_R && lay.cups + CUP_PICKED_R < lay.times.y && lay.times.bottom() <= lay.hours.y);
            // The chart and the list side by side, the list over Add a drink.
            assert!(lay.chart.right() + COL_GAP <= lay.list.x + 0.01 && lay.list.bottom() < lay.add.y && lay.list.y >= lay.top.bottom());
            assert!(lay.list.w >= LIST_MIN - 0.01 && lay.list.h >= 5.0 * ROW_H, "five rows show, {} wide", lay.list.w);
            // The sheet: the kinds, its rows (the widest: four beans), its lines and Back and Add.
            let (back, add) = buttons(a);
            for r in kind_grid(a) {
                inside(r, "a kind");
                assert!(r.bottom() < back.y && r.w >= 100.0, "{r:?}: a kind's name fits");
            }
            inside(back, "Back");
            inside(add, "Add");
            let cx = a.x + LABEL_W;
            let top = a.y + SHEET_TOP;
            inside(Rect::new(cx, top, 2.0 * PILL_SMALL + STEP_VALUE_W, PILL_SMALL), "a stepper");
            inside(Rect::new(cx, top, NAME_W, PILL_SMALL), "the name");
            inside(Rect::new(cx, top + SHEET_ROW, 4.0 * 84.0 + 4.0, PILL_SMALL), "the beans");
            let time = Rect::new(cx, top + 2.0 * SHEET_ROW, 2.0 * TIME_BTN_W + TIME_W + PILL_GAP + NOW_W, PILL_SMALL);
            inside(time, "the time");
            let warn = Rect::new(a.x, time.bottom() + 14.0 + 20.0, a.w, 18.0);
            inside(warn, "the warning");
            assert!(warn.bottom() <= back.y, "the lines end above Back and Add");
        }
        // At the usual width the chart is the full 370 with 4.75 DIP cells.
        let lay = Layout::of(AREA);
        assert_eq!((lay.chart.w, lay.cell, lay.list.w), (CHART_W, CELL, 190.0));
    }

    #[test]
    fn the_labels_never_overlap() {
        for a in [area(444.0), AREA] {
            let field = Layout::of(a).field;
            let x_of = |m: i64| field.x + m as f32 / DAY as f32 * field.w;
            // The worst days: the peak next to bedtime (with a late bed too), a
            // drink at 05:55, the last minute's peak, and ten drinks.
            let late = drink(at(21, 50), Kind::Americano, 2, Some(Bean::Arabica));
            day_labels(a, std::slice::from_ref(&late), 23 * 60, "the peak at bedtime");
            day_labels(a, &[drink(at(25, 50), Kind::Latte, 2, Some(Bean::Robusta))], 3 * 60, "the peak at a 03:00 bed");
            day_labels(a, &[drink(at(29, 55), Kind::Espresso, 1, Some(Bean::Arabica))], 23 * 60, "a drink at 05:55");
            day_labels(a, &[drink(at(29, 55), Kind::Espresso, 1, Some(Bean::Arabica))], 3 * 60, "a drink at 05:55, a 03:00 bed");
            let ten: Vec<Drink> = (0..10).map(|i| drink(at(7 + 2 * i, 10 * (i % 3)), Kind::Americano, 1 + (i % 3) as u8, Some(Bean::Blend))).collect();
            day_labels(a, &ten, 23 * 60, "ten drinks");
            day_labels(a, &ten, 2 * 60, "ten drinks, a 02:00 bed");
            day_labels(a, &[], 23 * 60, "no drink");
            // And every bedtime the settings offer against a peak anywhere in
            // the day, high or low.
            for bedtime in BEDTIMES.step_by(15) {
                let bed = x_of((bedtime as i64 - DAY_START).rem_euclid(DAY));
                for m in (0..DAY).step_by(5) {
                    for y in [field.y + 2.0, field.bottom() - 2.0] {
                        check_labels(a, Some((x_of(m), y)), bed, &format!("bed {} and a peak at {}, {} wide", hhmm(bedtime as i64), hhmm(DAY_START + m), a.w));
                    }
                }
            }
        }
    }

    #[test]
    fn the_times_under_the_dots_keep_apart() {
        // The picked time always shows; crowded ones wait; none moves more than the nudge.
        let w = 30.0;
        let cups = [(100.0, false), (110.0, true), (120.0, false), (160.0, false), (360.0, false)];
        let got = place_times(&cups, w, 0.0, 370.0);
        assert_eq!(got[1], Some(110.0));
        assert_eq!((got[0], got[2]), (None, None), "too close to the picked one");
        assert!(got[3].is_some_and(|x| (x - 160.0).abs() <= NUDGE));
        assert_eq!(got[4], Some(370.0 - w / 2.0), "kept inside at the edge");
        let shown: Vec<f32> = got.iter().flatten().copied().collect();
        for (i, a) in shown.iter().enumerate() {
            assert!(shown[i + 1..].iter().all(|b| (a - b).abs() >= w + 8.0 - 0.01));
        }
    }

    #[test]
    fn the_top_line_says_the_day_and_its_last_call() {
        let prefs = Prefs::default();
        let mut log = Log::default();
        let t = Today::of(&log, &prefs, at(9, 10));
        assert_eq!(summary(&t, &prefs), ("0 mg".into(), " of 400 today".into(), false));
        // No drink yet: a 2-shot Americano's last call, 12:07, is ahead.
        let (line, warn) = last_call_line(&t, at(9, 10));
        assert!(line.starts_with("Last call 12:0") && !warn, "{line}");
        assert_eq!(t.usual.phrase, "a 2-shot Americano");
        log.add(drink(at(7, 30), Kind::Sachet, 1, None));
        log.add(drink(at(10, 15), Kind::KopiSusu, 2, Some(Bean::Blend)));
        log.add(drink(at(12, 0), Kind::Americano, 2, Some(Bean::Arabica)));
        let t = Today::of(&log, &prefs, at(15, 42));
        assert_eq!(summary(&t, &prefs), ("3 drinks \u{b7} 400 mg".into(), " of 400 today".into(), false));
        let (line, warn) = last_call_line(&t, at(15, 42));
        assert!(line.starts_with("Last call was 12:0") && warn, "{line}");
        log.add(drink(at(16, 0), Kind::Tea, 1, None));
        let t = Today::of(&log, &prefs, at(16, 5));
        assert_eq!(summary(&t, &prefs).1, " \u{b7} over your 400 mg");
        // Green tea most often: it never leaves 35 mg at bedtime.
        let mut tea = Log::default();
        for h in [8, 11, 14] {
            tea.add(drink(at(h, 0), Kind::GreenTea, 1, None));
        }
        let t = Today::of(&tea, &prefs, at(15, 0));
        assert_eq!(summary(&t, &prefs).0, "3 drinks \u{b7} 84 mg");
        assert_eq!(last_call_line(&t, at(15, 0)), ("No last call".into(), false));
        // Strong lattes most often: no time today is early enough.
        let mut strong = Log::default();
        for h in [8, 13] {
            strong.add(drink(at(h, 0), Kind::Latte, 3, Some(Bean::Robusta)));
        }
        let t = Today::of(&strong, &prefs, at(15, 0));
        assert_eq!(last_call_line(&t, at(15, 0)), ("Too strong for a 23:00 bed".into(), true));
        let one = Today::of(&{ let mut l = Log::default(); l.add(drink(at(8, 0), Kind::Tea, 1, None)); l }, &prefs, at(9, 0));
        assert_eq!(summary(&one, &prefs).0, "1 drink \u{b7} 47 mg");
        // Nothing in the body is 0 mg, not "-0" (an empty sum is -0.0).
        assert_eq!(mg_text(Log::default().in_body_at(at(23, 0), prefs.k())), "0");
        assert_eq!((mg_text(-0.3), mg_text(34.6)), ("0".into(), "35".into()));
    }

    #[test]
    fn the_chart_stacks_bands_with_rings_where_drinks_went_in() {
        let mut log = Log::default();
        let yesterday = log.add(drink(at(-3, 30), Kind::ColdBrew, 1, None));
        let sachet = log.add(drink(at(7, 30), Kind::Sachet, 1, None));
        let kopi = log.add(drink(at(10, 15), Kind::KopiSusu, 2, Some(Bean::Blend)));
        let americano = log.add(drink(at(12, 0), Kind::Americano, 2, Some(Bean::Arabica)));
        let prefs = Prefs::default();
        let pick = picked(&log, day_start(at(15, 42)), None);
        assert_eq!(pick, Some(americano), "the day's latest, until one is clicked");
        assert_eq!(picked(&log, day_start(at(15, 42)), Some(yesterday)), Some(americano), "yesterday's isn't the day's");
        assert_eq!(picked(&log, day_start(at(15, 42)), Some(sachet)), Some(sachet));
        let c = chart_of(&log, &prefs, at(15, 42), pick);
        assert_eq!(c.dots.len(), COLS * ROWS);
        let column = |t: LocalMinute| &c.dots[((t - c.today.start) / COL_MIN) as usize * ROWS..][..ROWS];
        // Yesterday's cold brew is at the bottom in the morning, with nothing above it before the sachet.
        let dawn = column(at(6, 0));
        assert_eq!(dawn[0], Dot::Old);
        assert!(dawn.iter().all(|d| matches!(d, Dot::Old | Dot::Empty)));
        // The Americano's column: the level before it as bright rings (it's
        // picked), then its own blue band going in.
        let noon = column(at(12, 0));
        assert!(noon[..3].iter().all(|d| *d == Dot::Ring { picked: true }), "{noon:?}");
        assert!(noon.iter().any(|d| matches!(d, Dot::Band { tone: Tone::Picked, level: 5 })));
        // An hour on, the bands stack oldest at the bottom: the sachet's, the
        // kopi susu's (late for bed: orange), the Americano's on top.
        let later = column(at(13, 0));
        let tones: Vec<Tone> = later.iter().filter_map(|d| if let Dot::Band { tone, .. } = d { Some(*tone) } else { None }).collect();
        assert_eq!(tones.first(), Some(&Tone::Plain));
        assert_eq!(tones.last(), Some(&Tone::Picked));
        assert!(tones.windows(2).all(|w| !(w[0] == Tone::Picked && w[1] != Tone::Picked)));
        assert!(tones.contains(&Tone::Late));
        assert_eq!(c.late, vec![kopi], "210 mg at 10:15 keeps more than 35 mg at 23:00; the 12:00 Americano just doesn't");
        // By evening every band has settled to grey (or its own colour), and the scale fits the peak.
        let evening = column(at(20, 0));
        assert!(evening.iter().all(|d| !matches!(d, Dot::Band { level: 5, .. })));
        assert!(c.top_mg >= c.today.peak.unwrap().1 * 1.15 && c.top_mg % 40.0 == 0.0);
        assert!(c.pick_high.unwrap() >= c.today.peak.unwrap().1 - 1.0, "the Americano makes the day's peak");
        // The painting: every dot inside the picture, opaque where a fresh dot sits.
        let (w, h) = ((COLS as f32 * CELL * 2.0) as u32, (ROWS as f32 * CELL * 2.0).round() as u32);
        let px = paint(&c.dots, w, h, 2.0, CELL, &poni_core::look::CLASSIC);
        assert_eq!(px.len(), (w * h * 4) as usize);
        assert!(px.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "premultiplied");
        let at_px = |x: usize, y: usize| &px[(y * w as usize + x) * 4..][..4];
        // The first column's top cell is empty: a faint dot, clear round it.
        assert_eq!((at_px(4, 4)[3], at_px(0, 0)[3]), (33, 0));
        // The Americano going in at noon: a blue dot, solid at its centre.
        let (col, row) = (((at(12, 0) - c.today.start) / COL_MIN) as usize, noon.iter().position(|d| matches!(d, Dot::Band { tone: Tone::Picked, level: 5 })).unwrap());
        let cell = CELL * 2.0;
        let (x, y) = (((col as f32 + 0.5) * cell).floor() as usize, (h as f32 - (row as f32 + 0.5) * cell).floor() as usize);
        assert_eq!(at_px(x, y), [255, 132, 10, 255], "palette::BLUE, premultiplied BGRA");
    }

    #[test]
    fn adding_a_drink_starts_now_and_stores_what_was_picked() {
        let mut st = state();
        let now = at(15, 42);
        let mut sh = Sheet::new(now, Bean::Blend);
        assert_eq!((sh.kind, sh.at(now)), (None, now), "the time starts at now");
        assert_eq!(sh.drink(now), None, "nothing to add before a kind");
        sh.pick(Kind::KopiSusu);
        assert_eq!(sh.amount, Kind::KopiSusu.default_amount());
        sh.shift(now, -15);
        sh.shift(now, -15);
        assert_eq!(sh.at(now), at(15, 12));
        sh.shift(now, 15);
        assert_eq!(sh.at(now), at(15, 27));
        assert_eq!(sh.at(now + 5), at(15, 27), "a time set stays as the clock goes on");
        sh.at = None;
        assert_eq!(sh.at(now + 5), now + 5, "Now follows the clock");
        sh.step_amount(1);
        sh.step_amount(-5);
        assert_eq!(sh.amount, 1);
        for _ in 0..20 {
            sh.step_amount(1);
        }
        assert_eq!(sh.amount, MAX_AMOUNT);
        sh.amount = 3;
        sh.shift(now, 15);
        let id = add(&mut st, &sh, now).expect("added");
        let d = st.store.caffeine.get(id).expect("in the log").clone();
        assert_eq!((d.kind, d.amount, d.bean, d.mg, d.at), (Kind::KopiSusu, 3, Some(Bean::Blend), 315.0, at(15, 57)));
        assert!(st.cmds.contains(&Command::SaveStore));
        assert_eq!((st.ui.caffeine.pick, st.ui.caffeine.sheet.is_none(), st.ui.caffeine.reveal), (Some(id), true, true));
        // A drink counted in servings has no beans; Your own has its name and mg.
        let mut tea = Sheet::new(now, Bean::Robusta);
        tea.pick(Kind::Tea);
        tea.step_amount(1);
        let d = tea.drink(now).unwrap();
        assert_eq!((d.kind, d.amount, d.bean, d.mg), (Kind::Tea, 2, None, 94.0));
        let mut own = Sheet::new(now, Bean::Arabica);
        own.pick(Kind::Custom);
        own.name = "  Teh botol ".into();
        own.step_mg(10);
        for _ in 0..400 {
            own.step_mg(-1);
        }
        assert_eq!(own.mg, OWN_MIN);
        own.mg = 150;
        let id = add(&mut st, &own, now).unwrap();
        let d = st.store.caffeine.get(id).unwrap();
        assert_eq!((d.kind, d.amount, d.bean, d.mg, d.name.as_str(), d.title()), (Kind::Custom, 1, None, 150.0, "Teh botol", "Teh botol"));
        // The time stays inside the day, 06:00 to 05:59.
        let mut early = Sheet::new(at(6, 5), Bean::Arabica);
        early.shift(at(6, 5), -15);
        assert_eq!(early.at(at(6, 5)), at(6, 0));
        let mut late = Sheet::new(at(29, 50), Bean::Arabica);
        late.shift(at(29, 50), 15);
        assert_eq!(late.at(at(29, 50)), at(29, 59));
        // The keyboard: Enter adds, Esc goes back to the kinds and then closes.
        let mut keys = Sheet::new(now, Bean::Arabica);
        assert_eq!(keys.key(KeyInput::Enter), Act::Stay);
        keys.pick(Kind::Americano);
        assert_eq!(keys.key(KeyInput::Enter), Act::Add);
        assert_eq!((keys.key(KeyInput::Escape), keys.kind), (Act::Stay, None));
        assert_eq!(keys.key(KeyInput::Escape), Act::Close);
    }

    #[test]
    fn the_sheet_says_what_the_drink_leaves_at_bedtime() {
        let prefs = Prefs::default();
        let day = day_start(at(15, 42));
        let (line, warn) = effect(150.0, at(11, 0), day, &prefs);
        assert!(line.starts_with("Adds 150 mg \u{b7} ") && line.ends_with(" mg of it left at bedtime (23:00)") && warn.is_none(), "{line}");
        let (_, warn) = effect(210.0, at(15, 42), day, &prefs);
        assert!(warn.as_deref().is_some_and(|w| w.starts_with("After this drink's last call (09:")), "{warn:?}");
        assert_eq!(effect(480.0, at(7, 0), day, &prefs).1.as_deref(), Some("Too strong for a 23:00 bed"));
        assert_eq!(effect(75.0, at(23, 30), day, &prefs), ("Adds 75 mg".into(), Some("After bedtime (23:00)".into())));
        // An espresso just before bed: little of it is in by then, but it's late.
        assert_eq!(effect(75.0, at(22, 50), day, &prefs).1.as_deref(), Some("After this drink's last call (17:07)"));
        assert_eq!(each_line(Kind::KopiSusu, Bean::Blend), "1 shot is 105 mg");
        assert_eq!(each_line(Kind::Sachet, Bean::Blend), "1 sachet is 40 mg");
        assert_eq!(each_line(Kind::Custom, Bean::Blend), "");
        // Late is what the chart and the top line call late, whatever the time.
        let t = Today::of(&Log::default(), &prefs, at(15, 42));
        for m in (0..DAY).step_by(10) {
            for mg in [47.0, 150.0, 210.0] {
                assert_eq!(effect(mg, day + m, day, &prefs).1.is_some(), t.late(&prefs, mg, day + m), "{mg} mg at {}", hhmm(day + m));
            }
        }
    }

    #[test]
    fn a_drag_moves_a_drink_in_5_minute_steps_and_remove_takes_it_off() {
        let mut st = state();
        let start = day_start(at(15, 0));
        let a = st.store.caffeine.add(drink(at(8, 2), Kind::Americano, 2, Some(Bean::Arabica)));
        let b = st.store.caffeine.add(drink(at(10, 0), Kind::Tea, 1, None));
        let w = COLS as f32 * CELL;
        let mut d = Drag { id: a, from: at(8, 2), moved: false };
        assert_eq!(d.to(1.0, w, start), None, "a shaky click doesn't move it");
        // 50 DIPs is 210.5 minutes: 11:32.5, in 5-minute steps 11:35.
        let t = d.to(50.0, w, start).unwrap();
        assert_eq!(t, at(11, 35));
        assert!(d.moved);
        assert_eq!(d.to(0.0, w, start), Some(at(8, 0)), "back where it was, on the 5-minute steps");
        assert_eq!(d.to(-1000.0, w, start), Some(start), "never before the day");
        assert_eq!(d.to(1000.0, w, start), Some(start + DAY - 5), "or after it");
        assert!(st.store.caffeine.move_to(a, t));
        assert_eq!(st.store.caffeine.drinks.iter().map(|d| (d.id, d.at)).collect::<Vec<_>>(), vec![(b, at(10, 0)), (a, at(11, 35))]);
        assert_eq!(picked(&st.store.caffeine, start, None), Some(a), "moved last, it is the day's latest");
        st.ui.caffeine.pick = Some(a);
        remove(&mut st, a);
        assert_eq!(st.store.caffeine.drinks.len(), 1);
        assert_eq!(st.ui.caffeine.pick, None);
        assert!(st.cmds.contains(&Command::SaveStore));
        assert_eq!(picked(&st.store.caffeine, start, None), Some(b));
    }

    #[test]
    fn the_pictures_hold_their_own_days() {
        let mut st = state();
        st.local = LocalDateTime { year: 2026, month: 11, day: 10, hour: 3, minute: 0, second: 0 };
        for p in [Picture::Day, Picture::Kinds, Picture::Add, Picture::Empty, Picture::Late] {
            picture(&mut st, p);
            let now = local_minute(&st.local);
            assert_eq!((st.local.hour, st.local.minute), p.clock());
            let n = st.store.caffeine.day(day_start(now)).count();
            assert_eq!(n, match p { Picture::Empty => 0, Picture::Late => 6, _ => 3 }, "{p:?}");
            assert_eq!(st.ui.caffeine.sheet.as_ref().map(|s| s.kind), match p { Picture::Kinds => Some(None), Picture::Add => Some(Some(Kind::KopiSusu)), _ => None });
            assert!(st.ui.caffeine.still);
        }
        let t = Today::of(&st.store.caffeine, &st.settings.caffeine, local_minute(&st.local));
        assert!(t.over_day_limit && last_call_line(&t, local_minute(&st.local)).0 == "Too strong for a 23:00 bed");
        assert!(st.store.caffeine.drinks.first().is_some_and(|d| d.at < t.start), "yesterday's cold brew counts");
    }
}
