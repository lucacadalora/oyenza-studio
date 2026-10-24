//! System page: CPU, memory, drives, network and battery, Stats-style.
//!
//! Left, every module with its live value; right, the chosen module: the big
//! number, a minute of history, the split, and the apps using the most (or
//! the drives, the connection, the battery's health). Readings arrive once a
//! second from `services::sysmon`, which runs only while this page is open;
//! the page itself never asks for continuous frames.

use crate::commands::Command;
use crate::gfx::{Image, ImageData, TextStyle, Weight};
use crate::icons::Icon;
use crate::state::AppState;
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_SMALL};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::svgpath::Seg;
use poni_core::sysmon::{
    fmt_app_cpu, fmt_bytes, fmt_pct, fmt_rate, fmt_speed, fmt_time_left, fmt_uptime, fmt_watts, load_word, AppUse, Module, Series,
    HISTORY_LEN,
};

const HEIGHT: f32 = 172.0;
/// The module list on the left, and the gap to the detail.
const LIST_W: f32 = 172.0;
const LIST_GAP: f32 = 12.0;
const ROW_GAP: f32 = 2.0;
const ROW_MAX: f32 = 34.0;
/// Inside the detail tile.
const IN_X: f32 = 14.0;
const IN_Y: f32 = 12.0;
/// The column beside the chart: top apps, drives or facts, three rows.
const SIDE_W: f32 = 158.0;
const SIDE_GAP: f32 = 14.0;
/// App icons arrive as shell thumbnails: the icon fills this share of the
/// image (`services::imaging` centres icons with some air around them).
const ICON_FILL: f32 = 0.72;
const APP_ICON: f32 = 16.0;
const PLACEHOLDER: &str = "–";

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// The modules this PC has (Battery only with a battery).
fn modules(st: &AppState) -> Vec<Module> {
    let battery = match &st.system.latest {
        Some(r) => r.battery.is_some(),
        None => st.power.has_battery,
    };
    Module::ALL.into_iter().filter(|m| *m != Module::Battery || battery).collect()
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    // Fixed layout height: the island's spring may still be opening.
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    let mods = modules(st);
    if !mods.contains(&st.ui.system_module) {
        st.ui.system_module = Module::Cpu;
    }
    draw_list(ui, st, &mods, Rect::new(area.x, area.y, LIST_W, area.h));
    let x = area.x + LIST_W + LIST_GAP;
    draw_detail(ui, st, Rect::new(x, area.y, (area.right() - x).max(0.0), area.h));
}

// ------------------------------------------------------------------ list

fn draw_list(ui: &mut Ui, st: &mut AppState, mods: &[Module], r: Rect) {
    let n = mods.len().max(1) as f32;
    let row_h = ((r.h + ROW_GAP) / n - ROW_GAP).min(ROW_MAX);
    let name_st = TextStyle::body();
    for (i, &m) in mods.iter().enumerate() {
        let row = Rect::new(r.x, r.y + i as f32 * (row_h + ROW_GAP), r.w, row_h);
        let resp = ui.button(wid_n("system-module", i as u64), row);
        let selected = m == st.ui.system_module;
        if selected {
            ui.c.fill_rrect(row, 10.0, Color::white(0.14));
        } else if resp.hovered {
            ui.c.fill_rrect(row, 10.0, palette::SURFACE);
        }
        if resp.clicked && !selected {
            st.ui.system_module = m;
        }
        let cy = row.center().y;
        let ink = if selected || resp.hovered { palette::TEXT } else { palette::TEXT_DIM };
        ui.c.icon(module_icon(m), Rect::new(row.x + 10.0, cy - 7.5, 15.0, 15.0), ink);
        let value = list_value(st, m);
        let value_st = TextStyle::footnote().tabular().right().color(if selected { palette::TEXT } else { palette::TEXT_DIM });
        let vw = ui.c.measure(&value, &value_st, 200.0).0;
        let vr = Rect::new(row.right() - 10.0 - vw - 2.0, row.y, vw + 2.0, row.h);
        ui.c.text(&value, vr, &value_st);
        let nx = row.x + 33.0;
        ui.c.text(m.title(), Rect::new(nx, row.y, (vr.x - 6.0 - nx).max(0.0), row.h), &name_st.weight(if selected { Weight::Semibold } else { Weight::Regular }));
    }
}

fn module_icon(m: Module) -> Icon {
    match m {
        Module::Cpu => Icon::Cpu,
        Module::Memory => Icon::Memory,
        Module::Drives => Icon::Disk,
        Module::Network => Icon::Network,
        Module::Battery => Icon::Battery,
    }
}

/// The short value beside each module's name.
fn list_value(st: &AppState, m: Module) -> String {
    let Some(r) = &st.system.latest else { return PLACEHOLDER.into() };
    let v = match m {
        Module::Cpu => r.cpu.as_ref().map(|c| fmt_pct(c.load)),
        Module::Memory => r.memory.as_ref().map(|mem| fmt_pct(mem.used_frac())),
        Module::Drives => r.system_drive().map(|d| format!("{} free", fmt_bytes(d.free))),
        Module::Network => r.network.as_ref().map(|n| if n.online { fmt_rate(n.down_bps) } else { "Offline".into() }),
        Module::Battery => r.battery.as_ref().map(|b| format!("{}%", b.percent.min(100))),
    };
    v.unwrap_or_else(|| PLACEHOLDER.into())
}

// ------------------------------------------------------------------ detail

/// What one module shows in the detail tile.
struct Detail {
    /// Caption pieces, joined with " · " as far as they fit.
    caption: Vec<String>,
    big: String,
    qualifier: String,
    /// Right side of the big number's row: (dot colour, text) items.
    legend: Vec<(Option<Color>, String)>,
    chart: Chart,
    side: Side,
}

enum Chart {
    /// Shares of 0..=1 (CPU, memory).
    Share(Series, Color),
    /// Byte rates, scaled to the minute's peak (drives, network).
    Rates(Series, Color, Series, Color),
    /// The battery's charge as a bar.
    Battery { percent: u8, color: Color },
    /// Nothing to chart: a line of text in its place (may be empty).
    Note(String),
}

/// The three rows beside the chart.
enum Side {
    Apps(Vec<AppUse>, fn(&AppUse) -> String),
    Drives,
    Facts(Vec<String>),
}

fn draw_detail(ui: &mut Ui, st: &mut AppState, r: Rect) {
    ui.c.fill_rrect(r, 16.0, palette::SURFACE);
    let inner = r.inset(IN_X, IN_Y);
    let m = st.ui.system_module;
    let d = detail(st, m);

    // Caption and the Task Manager button.
    let head = Rect::new(inner.x, inner.y, inner.w, PILL_SMALL);
    let label = "Task Manager";
    let pw = ui.pill_width(label, false, true);
    let pill = Rect::new(head.right() - pw, head.y, pw, PILL_SMALL);
    if ui.pill(wid("system-taskmgr"), pill, label, None, PillStyle::Utility).clicked {
        st.cmd(Command::OpenTaskManager);
    }
    let cap_st = TextStyle::caption().weight(Weight::Semibold).color(palette::TEXT_DIM);
    let cap_w = (pill.x - 10.0 - head.x).max(0.0);
    let caption = fit_pieces(ui, &d.caption, &cap_st, cap_w);
    ui.c.text(&caption, Rect::new(head.x, head.y, cap_w, head.h), &cap_st);

    // The big number, what it means, and the split on the right.
    let row = Rect::new(inner.x, head.bottom() + 4.0, inner.w, 36.0);
    let big_st = TextStyle::display().tabular();
    let bw = ui.c.measure(&d.big, &big_st, 400.0).0;
    let big_col = if d.big == PLACEHOLDER { palette::TEXT_DIM } else { palette::TEXT };
    ui.c.text(&d.big, Rect::new(row.x - 1.0, row.y, bw + 4.0, row.h), &big_st.color(big_col));
    let legend_w = draw_legend(ui, &d.legend, row.right(), row.y + 21.0);
    let qx = row.x + bw + 8.0;
    let q_w = (row.right() - legend_w - 12.0 - qx).max(0.0);
    ui.c.text(&d.qualifier, Rect::new(qx, row.y + 12.0, q_w, 18.0), &TextStyle::footnote().color(palette::TEXT_DIM));

    // A minute of history on the left, three rows of detail on the right.
    let band = Rect::new(inner.x, row.bottom() + 8.0, inner.w, (inner.bottom() - row.bottom() - 8.0).max(0.0));
    let side_w = SIDE_W.min(band.w * 0.45);
    draw_chart(ui, &d.chart, Rect::new(band.x, band.y + 2.0, (band.w - side_w - SIDE_GAP).max(0.0), (band.h - 4.0).max(0.0)));
    let side = Rect::new(band.right() - side_w, band.y, side_w, band.h);
    let rows = side.rows(3, 0.0);
    match d.side {
        Side::Apps(apps, value) => {
            for (a, row) in apps.iter().zip(rows) {
                draw_app_row(ui, st, a, &value(a), row);
            }
        }
        Side::Drives => draw_drives(ui, st, &rows),
        Side::Facts(facts) => {
            for (f, row) in facts.iter().zip(rows) {
                ui.c.text(f, row, &TextStyle::footnote().color(palette::TEXT_DIM));
            }
        }
    }
}

/// Second colour of two-line charts: one that stands apart from the accent.
fn second_color(st: &AppState) -> Color {
    if st.accent.r > st.accent.b {
        palette::BLUE
    } else {
        palette::ORANGE
    }
}

fn detail(st: &AppState, m: Module) -> Detail {
    let accent = st.accent;
    let empty = |qualifier: &str| Detail {
        caption: vec![m.title().into()],
        big: PLACEHOLDER.into(),
        qualifier: qualifier.into(),
        legend: Vec::new(),
        chart: Chart::Note(String::new()),
        side: Side::Facts(Vec::new()),
    };
    let Some(r) = &st.system.latest else { return empty("Reading…") };
    let missing = || empty("Windows doesn't report this on this PC");
    let h = &st.system.history;
    match m {
        Module::Cpu => {
            let Some(c) = &r.cpu else { return missing() };
            let mut caption = vec!["CPU".to_string(), format!("{} threads", c.logical)];
            if let Some(mhz) = c.mhz {
                caption.push(fmt_speed(mhz));
            }
            caption.push(format!("up {}", fmt_uptime(c.uptime_s)));
            Detail {
                caption,
                big: fmt_pct(c.load),
                qualifier: load_word(c.load).into(),
                legend: vec![(None, format!("Apps {} · Windows {}", fmt_pct(c.user), fmt_pct(c.kernel)))],
                chart: Chart::Share(h.cpu.clone(), accent),
                side: Side::Apps(r.top_cpu.clone(), |a| fmt_app_cpu(a.cpu)),
            }
        }
        Module::Memory => {
            let Some(mem) = &r.memory else { return missing() };
            let mut caption = vec!["Memory".to_string(), fmt_bytes(mem.total)];
            if mem.cached > 0 {
                caption.push(format!("{} cached", fmt_bytes(mem.cached)));
            }
            let legend = if mem.commit_limit > 0 {
                vec![(None, format!("Committed {} of {}", fmt_bytes(mem.committed), fmt_bytes(mem.commit_limit)))]
            } else {
                Vec::new()
            };
            Detail {
                caption,
                big: fmt_pct(mem.used_frac()),
                qualifier: format!("{} in use", fmt_bytes(mem.used)),
                legend,
                chart: Chart::Share(h.memory.clone(), accent),
                side: Side::Apps(r.top_memory.clone(), |a| fmt_bytes(a.memory)),
            }
        }
        Module::Drives => {
            let Some(sys) = r.system_drive() else { return missing() };
            let mut caption = vec!["Drives".to_string(), if r.drives.len() == 1 { "1 drive".into() } else { format!("{} drives", r.drives.len()) }];
            if let Some(a) = r.disk.and_then(|d| d.active) {
                caption.push(format!("active {}", fmt_pct(a)));
            }
            let second = second_color(st);
            let (legend, chart) = match r.disk {
                Some(d) => (
                    vec![(Some(accent), format!("Read {}", fmt_rate(d.read_bps))), (Some(second), format!("Write {}", fmt_rate(d.write_bps)))],
                    Chart::Rates(h.disk_read.clone(), accent, h.disk_write.clone(), second),
                ),
                None => (Vec::new(), Chart::Note("No disk activity counters".into())),
            };
            Detail {
                caption,
                big: fmt_bytes(sys.free),
                qualifier: format!("free on {}", sys.root),
                legend,
                chart,
                side: Side::Drives,
            }
        }
        Module::Network => {
            let Some(n) = &r.network else { return missing() };
            if !n.online {
                return Detail {
                    caption: vec!["Network".into()],
                    big: "Offline".into(),
                    qualifier: String::new(),
                    legend: Vec::new(),
                    chart: Chart::Note("No network adapter is connected".into()),
                    side: Side::Facts(Vec::new()),
                };
            }
            let mut caption = vec!["Network".to_string()];
            if !n.adapter.is_empty() {
                caption.push(n.adapter.clone());
            }
            if let Some(ip) = &n.ipv4 {
                caption.push(ip.clone());
            }
            let second = second_color(st);
            Detail {
                caption,
                big: fmt_rate(n.down_bps),
                qualifier: "down".into(),
                legend: vec![(Some(accent), "Down".into()), (Some(second), format!("Up {}", fmt_rate(n.up_bps)))],
                chart: Chart::Rates(h.net_down.clone(), accent, h.net_up.clone(), second),
                side: Side::Facts(vec![format!("Received {}", fmt_bytes(n.received)), format!("Sent {}", fmt_bytes(n.sent)), "since connecting".into()]),
            }
        }
        Module::Battery => {
            let Some(b) = &r.battery else { return missing() };
            let percent = b.percent.min(100);
            let (state, color) = if b.charging {
                ("Charging", palette::GREEN)
            } else if b.on_ac {
                ("Plugged in", palette::GREEN)
            } else if percent <= 20 {
                ("Low battery", palette::RED)
            } else if b.saver {
                ("Battery saver", palette::YELLOW)
            } else {
                ("On battery", Color::white(0.92))
            };
            let qualifier = match (b.on_ac, b.secs_left) {
                (false, Some(s)) => fmt_time_left(s),
                (true, _) if !b.charging && percent >= 99 => "Fully charged".into(),
                _ => String::new(),
            };
            let legend = match b.rate_mw.filter(|r| *r != 0) {
                Some(r) if r > 0 => vec![(None, format!("Charging at {}", fmt_watts(r)))],
                Some(r) => vec![(None, format!("Using {}", fmt_watts(r)))],
                None => Vec::new(),
            };
            let mut facts = Vec::new();
            if let Some(hl) = b.health() {
                facts.push(format!("Health {}", fmt_pct(hl.min(1.0))));
            }
            if let Some(c) = b.cycles {
                facts.push(if c == 1 { "1 cycle".into() } else { format!("{c} cycles") });
            }
            if let Some(d) = b.design_mwh {
                facts.push(format!("Built for {:.0} Wh", f64::from(d) / 1000.0));
            }
            if facts.is_empty() {
                facts.push("Health not reported".into());
            }
            Detail {
                caption: vec!["Battery".into(), state.into()],
                big: format!("{percent}%"),
                qualifier,
                legend,
                chart: Chart::Battery { percent, color },
                side: Side::Facts(facts),
            }
        }
    }
}

/// `pieces` joined with " · ", dropping pieces from the end until it fits.
fn fit_pieces(ui: &mut Ui, pieces: &[String], st: &TextStyle, w: f32) -> String {
    for n in (1..=pieces.len()).rev() {
        let s = pieces[..n].join(" · ");
        if n == 1 || ui.c.measure(&s, st, 10_000.0).0 <= w {
            return s;
        }
    }
    String::new()
}

/// Right-aligned legend items ending at `right`, centred on `cy`; returns the width used.
fn draw_legend(ui: &mut Ui, items: &[(Option<Color>, String)], right: f32, cy: f32) -> f32 {
    let st = TextStyle::footnote().color(palette::TEXT_DIM);
    let mut x = right;
    for (i, (dot, text)) in items.iter().enumerate().rev() {
        let tw = ui.c.measure(text, &st, 400.0).0;
        x -= tw;
        ui.c.text(text, Rect::new(x - 1.0, cy - 9.0, tw + 2.0, 18.0), &st);
        if let Some(c) = dot {
            x -= 11.0;
            ui.c.fill_circle(Point::new(x + 3.0, cy), 3.0, *c);
        }
        if i > 0 {
            x -= 12.0;
        }
    }
    right - x
}

// ------------------------------------------------------------------ charts

fn draw_chart(ui: &mut Ui, chart: &Chart, r: Rect) {
    if let Chart::Battery { percent, color } = chart {
        let bar = Rect::new(r.x, r.center().y - 12.0, r.w - 8.0, 24.0);
        ui.c.fill_rrect(bar, 8.0, palette::TRACK);
        let w = (bar.w * f32::from(*percent) / 100.0).max(if *percent > 0 { 16.0 } else { 0.0 });
        if w > 0.0 {
            ui.c.fill_rrect(Rect::new(bar.x, bar.y, w, bar.h), 8.0, *color);
        }
        // The battery's terminal.
        ui.c.fill_rrect(Rect::new(bar.right() + 3.0, bar.center().y - 5.0, 4.0, 10.0), 2.0, palette::TRACK);
        return;
    }
    let grid = Color::white(0.06);
    for i in 0..3 {
        let y = r.y + r.h * i as f32 / 2.0;
        ui.c.line(Point::new(r.x, y), Point::new(r.right(), y), grid, 1.0);
    }
    match chart {
        Chart::Share(s, c) => plot(ui, s, 1.0, *c, r, true),
        Chart::Rates(a, ca, b, cb) => {
            let peak = nice_ceiling(a.max().max(b.max()));
            plot(ui, a, peak, *ca, r, true);
            plot(ui, b, peak, *cb, r, false);
            let label = fmt_rate(f64::from(peak));
            ui.c.text(&label, Rect::new(r.x, r.y + 2.0, r.w, 13.0), &TextStyle::caption().color(palette::TEXT_FAINT).top());
        }
        Chart::Note(text) if !text.is_empty() => ui.c.text(text, r, &TextStyle::footnote().color(palette::TEXT_FAINT).center()),
        _ => {}
    }
}

/// A round top for a rate chart at or above `v`, in the units the page
/// shows (1, 2 or 5 × 10ⁿ KB/s, MB/s, GB/s); never below 1 KB/s.
fn nice_ceiling(v: f32) -> f32 {
    let v = if v.is_finite() { v.max(1024.0) } else { 1024.0 };
    let mut unit = 1024.0f32;
    while v / unit > 1000.0 && unit < 1024.0 * 1024.0 * 1024.0 {
        unit *= 1024.0;
    }
    let x = v / unit;
    let step = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0].into_iter().find(|s| x <= *s).unwrap_or(1000.0);
    step * unit
}

/// One series, newest at the right edge; gaps (NaN) split the line.
fn plot(ui: &mut Ui, s: &Series, top: f32, color: Color, r: Rect, fill: bool) {
    if s.is_empty() || top <= 0.0 {
        return;
    }
    let step = r.w / (HISTORY_LEN - 1) as f32;
    let x0 = r.right() - (s.len() - 1) as f32 * step;
    let y = |v: f32| r.bottom() - (v / top).clamp(0.0, 1.0) * (r.h - 2.0) - 1.0;
    let values: Vec<f32> = s.values().collect();
    let mut i = 0;
    while i < values.len() {
        if !values[i].is_finite() {
            i += 1;
            continue;
        }
        let start = i;
        while i < values.len() && values[i].is_finite() {
            i += 1;
        }
        let run = &values[start..i];
        let xs = |k: usize| x0 + (start + k) as f32 * step;
        if run.len() == 1 {
            ui.c.fill_circle(Point::new(xs(0), y(run[0])), 1.5, color);
            continue;
        }
        let line: Vec<Seg> = run.iter().enumerate().map(|(k, v)| if k == 0 { Seg::MoveTo(xs(k), y(*v)) } else { Seg::LineTo(xs(k), y(*v)) }).collect();
        if fill {
            let mut area = line.clone();
            area.push(Seg::LineTo(xs(run.len() - 1), r.bottom()));
            area.push(Seg::LineTo(xs(0), r.bottom()));
            area.push(Seg::Close);
            ui.c.path(&area, Some(color.with_alpha(0.22)), None);
        }
        ui.c.path(&line, None, Some((color, 1.6)));
    }
    if let Some(v) = values.last().filter(|v| v.is_finite()) {
        ui.c.fill_circle(Point::new(r.right(), y(*v)), 2.6, color);
    }
}

// ------------------------------------------------------------------ side rows

fn draw_app_row(ui: &mut Ui, st: &mut AppState, a: &AppUse, value: &str, row: Rect) {
    let icon = Rect::new(row.x, row.center().y - APP_ICON / 2.0, APP_ICON, APP_ICON);
    draw_app_icon(ui, st, a.path.as_deref(), icon);
    let value_st = TextStyle::footnote().tabular().right().color(palette::TEXT_DIM);
    let vw = ui.c.measure(value, &value_st, 200.0).0;
    ui.c.text(value, Rect::new(row.right() - vw - 2.0, row.y, vw + 2.0, row.h), &value_st);
    let nx = icon.right() + 7.0;
    ui.c.text(&a.name, Rect::new(nx, row.y, (row.right() - vw - 8.0 - nx).max(0.0), row.h), &TextStyle::footnote());
}

fn draw_app_icon(ui: &mut Ui, st: &mut AppState, path: Option<&str>, r: Rect) {
    let img = path.and_then(|p| match st.app_icons.get(p) {
        Some(found) => found.clone(),
        None => {
            st.cmd(Command::LoadAppIcon(p.to_string()));
            None
        }
    });
    match img {
        // The icon fills ICON_FILL of the picture: draw the picture that much larger.
        Some(img) => ui.c.image(&img, Rect::from_center(r.center(), r.w / ICON_FILL, r.h / ICON_FILL), 0.0),
        None => ui.c.icon(Icon::App, r, palette::TEXT_DIM),
    }
}

fn draw_drives(ui: &mut Ui, st: &AppState, rows: &[Rect]) {
    let Some(reading) = &st.system.latest else { return };
    let root_st = TextStyle::footnote().weight(Weight::Semibold);
    let free_st = TextStyle::footnote().tabular().right().color(palette::TEXT_DIM);
    for (d, row) in reading.drives.iter().zip(rows) {
        let free = format!("{} free", fmt_bytes(d.free));
        let fw = ui.c.measure(&free, &free_st, 200.0).0;
        ui.c.text(&free, Rect::new(row.right() - fw - 2.0, row.y, fw + 2.0, row.h), &free_st);
        let rw = ui.c.measure(&d.root, &root_st, 100.0).0;
        ui.c.text(&d.root, Rect::new(row.x, row.y, rw + 2.0, row.h), &root_st);
        let bx = row.x + rw.max(18.0) + 6.0;
        let bar = Rect::new(bx, row.center().y - 2.5, (row.right() - fw - 10.0 - bx).clamp(0.0, 56.0), 5.0);
        if bar.w >= 12.0 {
            ui.c.fill_rrect(bar, 2.5, palette::TRACK);
            let used = d.used_frac();
            let fill = if used >= 0.9 { palette::RED } else if used >= 0.75 { palette::ORANGE } else { Color::white(0.85) };
            ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * used).max(bar.h), bar.h), 2.5, fill);
        }
    }
}

/// An app icon picture shrunk to `px` square by averaging (the shell hands
/// out large pictures; drawing them scaled down by a lot would shimmer).
pub fn shrink_icon(img: &Image, px: u32) -> Image {
    let (w, h) = (img.width.max(1), img.height.max(1));
    let px = px.clamp(1, w.max(h));
    if w <= px && h <= px {
        return img.clone();
    }
    let mut out = vec![0u8; (px * px * 4) as usize];
    for oy in 0..px {
        let (y0, y1) = (oy * h / px, ((oy + 1) * h / px).max(oy * h / px + 1).min(h));
        for ox in 0..px {
            let (x0, x1) = (ox * w / px, ((ox + 1) * w / px).max(ox * w / px + 1).min(w));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += u32::from(img.pixels[i + c]);
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            let o = ((oy * px + ox) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    ImageData::new(px, px, out)
}

/// Pixel size app icons are kept at for the island's scale (the picture
/// around a 16 DIP icon, see [`ICON_FILL`]).
pub fn icon_px(scale: f32) -> u32 {
    (APP_ICON / ICON_FILL * scale.max(1.0)).ceil() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_chart_tops_are_round() {
        const K: f32 = 1024.0;
        // An idle minute still gets a scale.
        assert_eq!(nice_ceiling(0.0), K);
        assert_eq!(nice_ceiling(f32::NAN), K);
        assert_eq!(nice_ceiling(3.0 * K), 5.0 * K);
        assert_eq!(nice_ceiling(6.5 * K * K), 10.0 * K * K);
        assert_eq!(nice_ceiling(900.0 * K), 1000.0 * K);
        assert_eq!(nice_ceiling(1500.0 * K), 2.0 * K * K);
    }
}
