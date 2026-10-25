//! System page: CPU, GPU, memory, drives, network and battery, laid out the
//! way Stats lays out its menu-bar popups.
//!
//! Along the top, a strip of every module's live value over its last minute
//! (Stats' menu-bar widgets); a click shows that module underneath, in three
//! columns: a dashboard of rings and gauges with the details under it
//! (coloured dots name each part), the usage history with a second chart
//! under it, and the processes using the most. Readings arrive once a second
//! from `services::sysmon` (Windows' or the Mac's), which keeps the minute
//! while the page is closed (on Windows, tray › Keep System page history);
//! the page itself never asks for frames. Where macOS names a thing
//! differently (swap, Low Power Mode, the GPU's renderer and tiler), the
//! Mac's page says it its way.
//!
//! On a desktop, Power is the whole PC's draw: its power supply's own
//! reading when it reports over USB, else an estimate (`poni_core::power`),
//! split into the CPU, the graphics card and the rest, with the source on
//! the right. On a Mac without a battery it's the whole Mac's draw from its
//! own sensor, which macOS doesn't split by part.
//!
//! The last module, Games (Windows only), holds Show over games: its switch,
//! whether the readout stays or peeks, whether Windows lets Poni read frames
//! (with the one-time OK), the frame rate's last minute and what's in front.

use crate::commands::Command;
use crate::gfx::{Image, ImageData, TextStyle, Weight};
use crate::icons::Icon;
use crate::state::{AppState, FpsAccess};
use crate::ui::{wid, wid_n, PillStyle, Ui, PILL_SMALL};
use poni_core::frames::{fmt_fps, fmt_frame_ms};
use poni_core::geom::{palette, Color, Point, Rect};
use poni_core::power::{fmt_w, watts_ceiling, PowerReading, REST_W};
use poni_core::svgpath::Seg;
use poni_core::sysmon::{
    fmt_app_cpu, fmt_bytes, fmt_link, fmt_pct, fmt_rate, fmt_time_left, fmt_uptime, fmt_watts, pressure_word, AppUse, BatteryReading, CoreKind,
    CpuReading, Drive, GpuReading, History, MemPart, MemoryReading, Module, NetReading, Reading, Series, HISTORY_LEN,
};

const HEIGHT: f32 = 256.0;
/// The module strip, and the gap under it.
const STRIP_H: f32 = 40.0;
const CHIP_GAP: f32 = 6.0;
const STRIP_GAP: f32 = 10.0;
/// The outer columns at the default width (a narrower island shrinks them);
/// the middle one takes the rest.
const DASH_W: f32 = 182.0;
const TOP_W: f32 = 172.0;
const COL_GAP: f32 = 16.0;
const FULL_W: f32 = 584.0;
/// Stats' proportions: the dashboard's height, its centre ring and side
/// gauges, and their arcs.
const DASH_H: f32 = 86.0;
const RING: f32 = 70.0;
const GAUGE: f32 = 50.0;
const RING_W: f32 = 7.0;
const GAUGE_W: f32 = 5.0;
/// Section titles (Stats' separators), detail rows and process rows.
const SEP_H: f32 = 20.0;
const DETAIL_H: f32 = 20.0;
const PROC_H: f32 = 22.0;
/// The second chart under the usage history.
const SUB_H: f32 = 56.0;
/// One drive: its name and free space, and a bar of the space used.
const DRIVE_H: f32 = 37.0;
const BOX_R: f32 = 6.0;
/// App icons arrive as shell thumbnails: the icon fills this share of the
/// image (`services::imaging` centres icons with some air around them).
const ICON_FILL: f32 = 0.72;
const APP_ICON: f32 = 16.0;
const PLACEHOLDER: &str = "–";
/// Built for a Mac: macOS's words where they differ from Windows'.
const MAC: bool = cfg!(target_os = "macos");
/// What the page says of a module the system gives no reading for.
const UNREPORTED: &str = if MAC { "macOS doesn't report this on this Mac" } else { "Windows doesn't report this on this PC" };

// Stats' colours: Windows' own time red, apps blue; performance cores indigo,
// efficiency cores teal; reads and downloads blue, writes and uploads red.
const SYSTEM_C: Color = palette::RED;
const USER_C: Color = palette::BLUE;
/// Stats' performance cores are indigo; a CPU with one kind of core is blue.
const P_CORE_C: Color = Color::rgba(0.369, 0.361, 0.902, 1.0);
const E_CORE_C: Color = palette::CYAN;
const CORE_C: Color = palette::BLUE;
const IN_C: Color = palette::BLUE;
const OUT_C: Color = palette::RED;
const APP_C: Color = palette::BLUE;
const WIRED_C: Color = palette::ORANGE;
const COMPRESSED_C: Color = palette::PINK;
const CACHED_C: Color = Color::white(0.42);
/// Idle and free: the part of a ring left as track, a little brighter as a dot.
const REST_C: Color = Color::white(0.26);
const ENGINE_3D_C: Color = palette::BLUE;
const COPY_C: Color = palette::ORANGE;
const DECODE_C: Color = palette::GREEN;
const ENCODE_C: Color = palette::PURPLE;
/// Power: the CPU in the orange of its own power gauge, the graphics card
/// blue, the rest of the PC grey.
const CPU_W_C: Color = palette::ORANGE;
const GPU_W_C: Color = palette::BLUE;
const REST_W_C: Color = Color::white(0.42);
const TILER_C: Color = palette::ORANGE;
const BOX_C: Color = Color::white(0.06);
const GRID_C: Color = Color::white(0.07);
const RULE_C: Color = Color::white(0.12);

pub fn height(_st: &AppState) -> f32 {
    HEIGHT
}

/// The modules this PC has: GPU with a graphics card the kernel reports on,
/// Battery with a battery, Power on a desktop that reads or estimates it.
/// Before the first reading, what the last one said.
fn modules(st: &AppState) -> Vec<Module> {
    let (battery, gpu) = match &st.system.latest {
        Some(r) => (r.battery.is_some(), r.gpu.is_some()),
        None => (st.power.has_battery, st.ui.system_gpu),
    };
    // Games is Windows' (it reads Windows' frame log).
    let games = cfg!(windows);
    let power = st.ui.system_power && !battery;
    Module::ALL
        .into_iter()
        .filter(|m| (*m != Module::Battery || battery) && (*m != Module::Gpu || gpu) && (*m != Module::Games || games) && (*m != Module::Power || power))
        .collect()
}

pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect) {
    // Fixed layout height: the island's spring may still be opening.
    let area = Rect::new(area.x, area.y, area.w, HEIGHT);
    if let Some(r) = &st.system.latest {
        st.ui.system_gpu = r.gpu.is_some();
        // Only the page's own (full) readings, the ones with app lists, say
        // whether there's a power reading; the kept minute never has one.
        if r.power.is_some() || !r.top_memory.is_empty() {
            st.ui.system_power = r.power.is_some();
        }
    }
    let mods = modules(st);
    // CPU stands in while the chosen module isn't there (no reading yet, a
    // battery that didn't answer once); the choice itself is kept.
    let module = if mods.contains(&st.ui.system_module) { st.ui.system_module } else { Module::Cpu };
    draw_strip(ui, st, &mods, module, Rect::new(area.x, area.y, area.w, STRIP_H));
    let body = Rect::new(area.x, area.y + STRIP_H + STRIP_GAP, area.w, HEIGHT - STRIP_H - STRIP_GAP);
    if module == Module::Games {
        draw_games(ui, st, &Cols::new(body));
        return;
    }
    let Some(r) = st.system.latest.clone() else {
        ui.c.text("Reading…", body, &TextStyle::footnote().color(palette::TEXT_FAINT).center());
        return;
    };
    let h = st.system.history.clone();
    let cols = Cols::new(body);
    let shown = match module {
        Module::Cpu => r.cpu.as_ref().map(|c| draw_cpu(ui, st, c, &r, &h, &cols)),
        Module::Gpu => r.gpu.as_ref().map(|g| draw_gpu(ui, st, g, &h, &cols)),
        Module::Memory => r.memory.as_ref().map(|m| draw_memory(ui, st, m, &r, &h, &cols)),
        Module::Drives => (!r.drives.is_empty() || r.disk.is_some()).then(|| draw_drives(ui, st, &r, &h, &cols)),
        Module::Network => r.network.as_ref().map(|n| draw_network(ui, n, &h, &cols)),
        Module::Battery => r.battery.as_ref().map(|b| draw_battery(ui, b, &cols)),
        Module::Power => {
            match &r.power {
                Some(p) => draw_power(ui, st, p, &r, &h, &cols),
                // The page's first reading is the kept minute's: the power comes with the next.
                None => ui.c.text("Reading…", body, &TextStyle::footnote().color(palette::TEXT_FAINT).center()),
            }
            Some(())
        }
        Module::Games => None,
    };
    if shown.is_none() {
        ui.c.text(UNREPORTED, body, &TextStyle::footnote().color(palette::TEXT_FAINT).center());
    }
}

/// The three columns under the strip.
struct Cols {
    dash: Rect,
    mid: Rect,
    top: Rect,
}

impl Cols {
    fn new(r: Rect) -> Cols {
        let k = (r.w / FULL_W).min(1.0);
        let dash = Rect::new(r.x, r.y, DASH_W * k, r.h);
        let top = Rect::new(r.right() - TOP_W * k, r.y, TOP_W * k, r.h);
        let mid = Rect::new(dash.right() + COL_GAP, r.y, (top.x - dash.right() - 2.0 * COL_GAP).max(0.0), r.h);
        Cols { dash, mid, top }
    }
}

// ------------------------------------------------------------------ strip

/// What a chip draws under its value.
enum Spark {
    /// Shares of 0..=1.
    Share(Series),
    /// Byte rates, scaled to the minute's peak.
    Rate(Series),
    /// A level (the battery's charge).
    Level(f32, Color),
    /// Frames per second, against the nearest common refresh rate above the minute's peak.
    Fps(Series),
    /// Watts, against a round top above the minute's peak.
    Watts(Series),
}

/// The widest value each chip can show, so its width never changes as
/// the value does.
fn widest_value(m: Module) -> &'static str {
    match m {
        Module::Drives | Module::Network => "999 MB/s",
        Module::Games => "999 fps",
        Module::Power => "1888 W",
        _ => "100%",
    }
}

/// Each chip's width: all the same when every chip's title and widest value
/// fit that way (six chips at the default width), else each gets what its
/// own needs and an even share of the rest.
fn chip_widths(ui: &mut Ui, mods: &[Module], avail: f32) -> Vec<f32> {
    let n = mods.len().max(1) as f32;
    let even = (avail / n).max(0.0);
    let title = TextStyle::caption().weight(Weight::Semibold);
    let value = TextStyle::body().weight(Weight::Semibold).tabular();
    let needs: Vec<f32> =
        mods.iter().map(|&m| ui.c.measure(m.title(), &title, 400.0).0.max(ui.c.measure(widest_value(m), &value, 400.0).0) + 22.0).collect();
    let total: f32 = needs.iter().sum();
    if needs.iter().all(|w| *w <= even) || total <= 0.0 {
        return vec![even; mods.len()];
    }
    if total > avail {
        return needs.iter().map(|w| w * avail / total).collect();
    }
    let share = (avail - total) / n;
    needs.iter().map(|w| w + share).collect()
}

fn draw_strip(ui: &mut Ui, st: &mut AppState, mods: &[Module], module: Module, r: Rect) {
    let n = mods.len().max(1) as f32;
    let widths = chip_widths(ui, mods, (r.w - CHIP_GAP * (n - 1.0)).max(0.0));
    let accent = st.accent;
    let mut x = r.x;
    for (i, (&m, &w)) in mods.iter().zip(&widths).enumerate() {
        let chip = Rect::new(x, r.y, w, r.h);
        x += w + CHIP_GAP;
        let resp = ui.button(wid_n("system-module", i as u64), chip);
        let selected = m == module;
        let fill = if selected {
            Color::white(0.15)
        } else if resp.hovered {
            Color::white(0.11)
        } else {
            palette::SURFACE
        };
        ui.c.fill_rrect(chip, 10.0, fill);
        if resp.clicked && !selected {
            st.ui.system_module = m;
        }
        let (value, spark) = chip_value(st, m);
        // The minute behind the value, faint under the text (inside the rounded corners).
        let band = Rect::new(chip.x + 6.0, chip.y + chip.h * 0.45, chip.w - 12.0, chip.h * 0.55 - 4.0);
        match spark {
            Some(Spark::Share(s)) => plot(ui, &s, 1.0, accent.with_alpha(if selected { 0.9 } else { 0.55 }), band, Fill::Faint),
            Some(Spark::Rate(s)) => plot(ui, &s, nice_ceiling(s.max()), accent.with_alpha(if selected { 0.9 } else { 0.55 }), band, Fill::Faint),
            Some(Spark::Fps(s)) => plot(ui, &s, fps_ceiling(s.max()), accent.with_alpha(if selected { 0.9 } else { 0.55 }), band, Fill::Faint),
            Some(Spark::Watts(s)) => plot(ui, &s, watts_ceiling(s.max()), accent.with_alpha(if selected { 0.9 } else { 0.55 }), band, Fill::Faint),
            Some(Spark::Level(f, c)) => {
                let bar = Rect::new(chip.x + 10.0, chip.bottom() - 9.0, chip.w - 20.0, 3.0);
                ui.c.fill_rrect(bar, 1.5, palette::TRACK);
                ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * f.clamp(0.0, 1.0)).max(bar.h), bar.h), 1.5, c);
            }
            None => {}
        }
        let ink = if selected { palette::TEXT } else { palette::TEXT_DIM };
        ui.c.text(m.title(), Rect::new(chip.x + 10.0, chip.y + 4.0, chip.w - 20.0, 14.0), &TextStyle::caption().weight(Weight::Semibold).color(ink));
        ui.c.text(&value, Rect::new(chip.x + 10.0, chip.y + 17.0, chip.w - 20.0, 18.0), &TextStyle::body().weight(Weight::Semibold).tabular());
    }
}

fn chip_value(st: &AppState, m: Module) -> (String, Option<Spark>) {
    if m == Module::Games {
        return match (st.settings.game_stats, st.game.active, st.game.rate) {
            (false, ..) => ("Off".into(), None),
            (true, true, Some(r)) => (format!("{} fps", fmt_fps(r.fps)), Some(Spark::Fps(st.game.history.clone()))),
            (true, ..) => ("On".into(), None),
        };
    }
    let Some(r) = &st.system.latest else { return (PLACEHOLDER.into(), None) };
    let h = &st.system.history;
    let v = match m {
        Module::Cpu => r.cpu.as_ref().map(|c| (fmt_pct(c.load), Some(Spark::Share(h.cpu.clone())))),
        Module::Gpu => r.gpu.as_ref().map(|g| (fmt_pct(g.util), Some(Spark::Share(h.gpu.clone())))),
        Module::Memory => r.memory.as_ref().map(|mem| (fmt_pct(mem.used_frac()), Some(Spark::Share(h.memory.clone())))),
        Module::Drives => r.disk.map(|d| (fmt_rate(d.read_bps + d.write_bps), Some(Spark::Rate(sum(&h.disk_read, &h.disk_write))))),
        Module::Network => r.network.as_ref().map(|n| {
            if n.online {
                (fmt_rate(n.down_bps), Some(Spark::Rate(sum(&h.net_down, &h.net_up))))
            } else {
                ("Offline".into(), None)
            }
        }),
        Module::Battery => r.battery.as_ref().map(|b| (format!("{}%", b.percent.min(100)), Some(Spark::Level(f32::from(b.percent.min(100)) / 100.0, battery_color(b))))),
        Module::Power => r.power.as_ref().map(|p| (fmt_w(p.total_w), Some(Spark::Watts(h.power.clone())))),
        Module::Games => None,
    };
    v.unwrap_or_else(|| (PLACEHOLDER.into(), None))
}

/// Two series added sample by sample (a gap in either is a gap).
fn sum(a: &Series, b: &Series) -> Series {
    let mut s = Series::default();
    for (x, y) in a.values().zip(b.values()) {
        s.push(Some(x + y));
    }
    s
}

// ------------------------------------------------------------------ CPU

fn draw_cpu(ui: &mut Ui, st: &mut AppState, c: &CpuReading, r: &Reading, h: &History, cols: &Cols) {
    let accent = st.accent;
    // Dashboard: temperature (or package power), the load split, the speed.
    let left = match (c.temp_c, c.power_w) {
        (Some(t), _) => temperature(t),
        (None, Some(w)) => Gauge { frac: Some(w / 125.0), value: format!("{w:.0} W"), label: "Power", color: palette::ORANGE },
        (None, None) => Gauge::unknown("Temperature"),
    };
    // The speed against one and a half times the base speed (where turbo usually tops out).
    let right = match c.mhz {
        Some(mhz) => Gauge { frac: c.base_mhz.map(|b| mhz as f32 / (b as f32 * 1.5)), value: ghz(mhz), label: "GHz", color: CORE_C },
        None => Gauge::unknown("GHz"),
    };
    let ring = Ring { parts: vec![(c.kernel, SYSTEM_C), (c.user, USER_C)], value: fmt_pct(c.load) };
    let (dash, rest) = cols.dash.split_top(DASH_H);
    dashboard(ui, dash, &left, &ring, &right);
    let mut rows = vec![
        Row::dot(SYSTEM_C, "System", fmt_pct(c.kernel)),
        Row::dot(USER_C, "User", fmt_pct(c.user)),
        Row::dot(REST_C, "Idle", fmt_pct(1.0 - c.load)),
    ];
    match (c.kind_load(CoreKind::Efficiency), c.kind_load(CoreKind::Performance)) {
        (Some(e), Some(p)) if c.hybrid().is_some() => {
            rows.push(Row::dot(E_CORE_C, "Efficiency cores", fmt_pct(e)));
            rows.push(Row::dot(P_CORE_C, "Performance cores", fmt_pct(p)));
        }
        _ => {
            rows.push(Row::plain("Threads", c.logical.to_string()));
            rows.push(Row::plain("Uptime", fmt_uptime(c.uptime_s)));
        }
    }
    details(ui, rest, "DETAILS", &rows);

    // Usage history, and each core's load under it.
    let (chart, sub) = history_column(ui, cols.mid, "USAGE HISTORY", "PER CORE");
    chart_box(ui, chart);
    plot(ui, &h.cpu, 1.0, accent, chart.inset(1.0, 1.0), Fill::Gradient);
    let kind_c = |k: CoreKind| match k {
        CoreKind::Efficiency => E_CORE_C,
        CoreKind::Performance => P_CORE_C,
        CoreKind::Any => CORE_C,
    };
    let bars: Vec<(f32, Color)> = c.cores.iter().map(|k| (k.load, kind_c(k.kind))).collect();
    if bars.is_empty() {
        ui.c.fill_rrect(sub, BOX_R, BOX_C);
        ui.c.text("Not reported", sub, &TextStyle::caption().color(palette::TEXT_FAINT).center());
    } else {
        bar_chart(ui, sub, &bars);
    }

    processes(ui, st, cols.top, &r.top_cpu, |a| fmt_app_cpu(a.cpu), "");
}

// ------------------------------------------------------------------ GPU

fn draw_gpu(ui: &mut Ui, st: &mut AppState, g: &GpuReading, h: &History, cols: &Cols) {
    let accent = st.accent;
    let mem_frac = (g.mem_total > 0).then(|| g.mem_used as f32 / g.mem_total as f32);
    let left = match (g.temp_c, g.mhz) {
        (Some(t), _) => temperature(t),
        (None, Some(mhz)) => Gauge { frac: None, value: ghz(mhz), label: "GHz", color: accent },
        (None, None) => Gauge::unknown("Temperature"),
    };
    let right = match mem_frac {
        Some(f) => Gauge { frac: Some(f), value: fmt_pct(f), label: "Memory", color: palette::PURPLE },
        None => Gauge::unknown("Memory"),
    };
    let ring = Ring { parts: vec![(g.util, accent)], value: fmt_pct(g.util) };
    let (dash, rest) = cols.dash.split_top(DASH_H);
    dashboard(ui, dash, &left, &ring, &right);
    let memory = Row::plain(if g.integrated { "Shared memory" } else { "Memory" }, format!("{} / {}", fmt_bytes(g.mem_used), fmt_bytes(g.mem_total)));
    // Apple's GPUs report their renderer (pixels) and tiler (geometry), as Stats shows them.
    let rows = if MAC {
        vec![Row::dot(ENGINE_3D_C, "Renderer", fmt_pct(g.engine_3d)), Row::dot(TILER_C, "Tiler", fmt_pct(g.tiler)), memory]
    } else {
        vec![
            Row::dot(ENGINE_3D_C, "3D", fmt_pct(g.engine_3d)),
            Row::dot(COPY_C, "Copy", fmt_pct(g.copy)),
            Row::dot(DECODE_C, "Video decode", fmt_pct(g.video_decode)),
            Row::dot(ENCODE_C, "Video encode", fmt_pct(g.video_encode)),
            memory,
        ]
    };
    details(ui, rest, "ENGINES", &rows);

    let (chart, sub) = history_column(ui, cols.mid, "USAGE HISTORY", "ENGINES");
    chart_box(ui, chart);
    plot(ui, &h.gpu, 1.0, accent, chart.inset(1.0, 1.0), Fill::Gradient);
    if MAC {
        labelled_bars(ui, sub, &[(g.engine_3d, ENGINE_3D_C, "Renderer"), (g.tiler, TILER_C, "Tiler")]);
    } else {
        labelled_bars(ui, sub, &[(g.engine_3d, ENGINE_3D_C, "3D"), (g.copy, COPY_C, "Copy"), (g.video_decode, DECODE_C, "Decode"), (g.video_encode, ENCODE_C, "Encode")]);
    }

    // The card itself.
    let (head, rest) = cols.top.split_top(SEP_H);
    separator(ui, head, "ADAPTER");
    let name_st = TextStyle::footnote().weight(Weight::Semibold).wrap().top().line_spacing(1.25);
    let (name_r, rest) = rest.split_top(36.0);
    ui.c.text(&g.name, name_r.inset(0.0, 2.0), &name_st);
    let built_in = if MAC { "Built into the chip" } else { "Built into the CPU" };
    let mut rows = vec![Row::plain("Type", if g.integrated { built_in.into() } else { "Dedicated".into() })];
    if let Some(w) = g.power_w {
        rows.push(Row::plain("Power", fmt_w(w)));
    }
    if let Some(rpm) = g.fan_rpm {
        rows.push(Row::plain("Fan", format!("{rpm} RPM")));
    }
    if let Some(mhz) = g.mhz {
        rows.push(Row::plain("Clock", format!("{mhz} MHz")));
    }
    if let Some(t) = g.temp_c {
        rows.push(Row::plain("Temperature", format!("{t:.0}°C")));
    }
    if g.temp_c.is_none() && g.fan_rpm.is_none() {
        rows.push(Row::plain("Sensors", "Not reported".into()));
    }
    for (row, rect) in rows.iter().zip(stack(rest, DETAIL_H)) {
        row.draw(ui, rect);
    }
}

// ------------------------------------------------------------------ memory

fn draw_memory(ui: &mut Ui, st: &mut AppState, m: &MemoryReading, r: &Reading, h: &History, cols: &Cols) {
    let accent = st.accent;
    let parts = m.parts();
    let share = |p: MemPart| parts.iter().find(|(k, _)| *k == p).map_or(0, |(_, v)| *v);
    let frac = |b: u64| if m.total == 0 { 0.0 } else { b as f32 / m.total as f32 };
    let pressure = m.pressure();
    // macOS swaps to files it adds as it needs them: none yet is 0 MB, never off.
    let page_label = if MAC { "Swap" } else { "Page file" };
    let right = if m.page_total > 0 {
        Gauge { frac: Some(m.page_used as f32 / m.page_total as f32), value: compact_bytes(m.page_used), label: page_label, color: palette::YELLOW }
    } else if MAC {
        Gauge { frac: Some(0.0), value: compact_bytes(0), label: page_label, color: palette::YELLOW }
    } else {
        Gauge { frac: None, value: "Off".into(), label: page_label, color: palette::YELLOW }
    };
    let ring = Ring {
        parts: vec![(frac(share(MemPart::App)), APP_C), (frac(share(MemPart::Wired)), WIRED_C), (frac(share(MemPart::Compressed)), COMPRESSED_C)],
        value: fmt_pct(m.used_frac()),
    };
    let (dash, rest) = cols.dash.split_top(DASH_H);
    let (d, g) = dashboard_sizes(dash);
    draw_ring(ui, dash.center(), d, &ring);
    let gy = dash.center().y - 6.0;
    if m.commit_limit > 0 {
        pressure_gauge(ui, Point::new(dash.x + g / 2.0 + 2.0, gy + 4.0), g, pressure);
    } else {
        draw_gauge(ui, Point::new(dash.x + g / 2.0 + 2.0, gy), g, &Gauge::unknown("Pressure"));
    }
    draw_gauge(ui, Point::new(dash.right() - g / 2.0 - 2.0, gy), g, &right);
    let rows = [
        Row::dot(APP_C, "App", fmt_bytes(share(MemPart::App))),
        Row::dot(WIRED_C, "Wired", fmt_bytes(share(MemPart::Wired))),
        Row::dot(COMPRESSED_C, "Compressed", fmt_bytes(share(MemPart::Compressed))),
        Row::dot(CACHED_C, "Cached", fmt_bytes(share(MemPart::Cached))),
        Row::dot(REST_C, "Free", fmt_bytes(share(MemPart::Free))),
    ];
    details(ui, rest, "DETAILS", &rows);

    let (chart, sub) = history_column(ui, cols.mid, "USAGE HISTORY", "USED");
    chart_box(ui, chart);
    plot(ui, &h.memory, 1.0, accent, chart.inset(1.0, 1.0), Fill::Gradient);
    let segments: Vec<(f32, Color)> = [(MemPart::App, APP_C), (MemPart::Wired, WIRED_C), (MemPart::Compressed, COMPRESSED_C), (MemPart::Cached, CACHED_C)]
        .iter()
        .map(|(p, c)| (frac(share(*p)), *c))
        .collect();
    // macOS has no commit charge to show (its gauge reads the kernel's own pressure level).
    let committed = if m.commit_limit > 0 && !MAC { format!("Committed {} of {}", fmt_bytes(m.committed), fmt_bytes(m.commit_limit)) } else { String::new() };
    stacked_meter(ui, sub, &segments, &fmt_bytes(m.used), &format!("of {}", fmt_bytes(m.total)), &committed);

    processes(ui, st, cols.top, &r.top_memory, |a| fmt_bytes(a.memory), "");
}

// ------------------------------------------------------------------ drives

fn draw_drives(ui: &mut Ui, st: &mut AppState, r: &Reading, h: &History, cols: &Cols) {
    // Every fixed drive and its space.
    let (head, rest) = cols.dash.split_top(SEP_H);
    separator(ui, head, "DRIVES");
    if r.drives.is_empty() {
        ui.c.text("Reading…", rest.split_top(DETAIL_H).0, &TextStyle::footnote().color(palette::TEXT_FAINT));
    }
    let slots = stack(rest, DRIVE_H);
    for (d, slot) in r.drives.iter().zip(&slots) {
        drive_row(ui, d, *slot);
    }
    // Reads and writes now: under the drives when there's room, else under the chart.
    let used = DRIVE_H * r.drives.len().min(slots.len()) as f32;
    let room = Rect::new(rest.x, rest.y + used + 6.0, rest.w, (rest.h - used - 6.0).max(0.0));
    let activity = r.disk.map(|d| {
        let mut rows = vec![Row::dot(IN_C, "Read", fmt_rate(d.read_bps)), Row::dot(OUT_C, "Write", fmt_rate(d.write_bps))];
        if let Some(a) = d.active {
            rows.push(Row::plain("Active time", fmt_pct(a)));
        }
        rows
    });
    let beside = activity.as_ref().is_some_and(|rows| room.h >= SEP_H + DETAIL_H * rows.len() as f32);
    if let (true, Some(rows)) = (beside, &activity) {
        details(ui, room, "ACTIVITY", rows);
    }

    // Reads up, writes down, over the minute.
    let (head, rest) = cols.mid.split_top(SEP_H);
    separator(ui, head, "USAGE HISTORY");
    let (chart, legend) = if beside { (rest, Rect::default()) } else { rest.split_bottom(DETAIL_H + 2.0) };
    chart_box(ui, chart);
    mirrored(ui, &h.disk_read, IN_C, &h.disk_write, OUT_C, chart);
    if let (false, Some(d)) = (beside, r.disk) {
        let mut items = vec![(Some(IN_C), format!("Read {}", fmt_rate(d.read_bps))), (Some(OUT_C), format!("Write {}", fmt_rate(d.write_bps)))];
        if let Some(a) = d.active {
            items.push((None, format!("Active {}", fmt_pct(a))));
        }
        legend_row(ui, legend.offset(0.0, 2.0), &items);
    }

    processes(ui, st, cols.top, &r.top_io, |a| fmt_rate(a.io), "No disk activity");
}

fn drive_row(ui: &mut Ui, d: &Drive, r: Rect) {
    let line = Rect::new(r.x, r.y + 2.0, r.w, 18.0);
    let free = format!("{} free", fmt_bytes(d.free));
    let free_st = TextStyle::footnote().tabular().right().color(palette::TEXT_DIM);
    let fw = ui.c.measure(&free, &free_st, r.w).0;
    ui.c.text(&free, Rect::new(line.right() - fw - 2.0, line.y, fw + 2.0, line.h), &free_st);
    let name = if d.label.is_empty() { d.root.clone() } else { format!("{}  {}", d.root, d.label) };
    ui.c.text(&name, Rect::new(line.x, line.y, (line.w - fw - 8.0).max(0.0), line.h), &TextStyle::footnote().weight(Weight::Semibold));
    let bar = Rect::new(r.x, line.bottom() + 4.0, r.w, 6.0);
    ui.c.fill_rrect(bar, 3.0, palette::TRACK);
    let used = d.used_frac();
    let fill = if used >= 0.9 {
        palette::RED
    } else if used >= 0.75 {
        palette::ORANGE
    } else {
        IN_C
    };
    ui.c.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * used).max(bar.h), bar.h), 3.0, fill);
}

// ------------------------------------------------------------------ network

fn draw_network(ui: &mut Ui, n: &NetReading, h: &History, cols: &Cols) {
    // Download and upload side by side, big, as Stats shows them.
    let (dash, rest) = cols.dash.split_top(DASH_H);
    for (half, (color, label, rate)) in dash.inset(0.0, 14.0).columns(2, 8.0).iter().zip([(IN_C, "Download", n.down_bps), (OUT_C, "Upload", n.up_bps)]) {
        let (value_r, label_r) = half.split_top(half.h * 0.62);
        let value = if n.online { fmt_rate(rate) } else { PLACEHOLDER.into() };
        ui.c.text(&value, value_r, &TextStyle::value().tabular().center());
        let lst = TextStyle::caption().color(palette::TEXT_DIM);
        let lw = ui.c.measure(label, &lst, label_r.w).0 + 11.0;
        let x = label_r.center().x - lw / 2.0;
        ui.c.fill_rrect(Rect::new(x, label_r.center().y - 3.5, 7.0, 7.0), 2.0, color);
        ui.c.text(label, Rect::new(x + 10.0, label_r.y, lw, label_r.h), &lst);
    }
    let mut rows = vec![Row::plain("Interface", if n.adapter.is_empty() { PLACEHOLDER.into() } else { n.adapter.clone() })];
    rows.push(Row::plain("Status", if n.online { "Connected".into() } else { "Offline".into() }));
    rows.push(Row::plain("Address", n.ipv4.clone().unwrap_or_else(|| PLACEHOLDER.into())));
    rows.push(Row::plain("MAC", if n.mac.is_empty() { PLACEHOLDER.into() } else { n.mac.clone() }));
    rows.push(Row::plain("Link speed", if n.link_bps > 0 { fmt_link(n.link_bps) } else { PLACEHOLDER.into() }));
    details(ui, rest, "DETAILS", &rows);

    // Downloads up, uploads down, over the minute.
    let (head, chart) = cols.mid.split_top(SEP_H);
    separator(ui, head, "USAGE HISTORY");
    chart_box(ui, chart);
    if n.online {
        mirrored(ui, &h.net_down, IN_C, &h.net_up, OUT_C, chart);
    } else {
        ui.c.text("No network adapter is connected", chart, &TextStyle::caption().color(palette::TEXT_FAINT).center());
    }

    let (head, rest) = cols.top.split_top(SEP_H);
    separator(ui, head, "SINCE STARTUP");
    let (totals, rest) = rest.split_top(2.0 * DETAIL_H + 4.0);
    let rows = [Row::dot(IN_C, "Received", fmt_bytes(n.received)), Row::dot(OUT_C, "Sent", fmt_bytes(n.sent))];
    for (row, rect) in rows.iter().zip(stack(totals, DETAIL_H)) {
        row.draw(ui, rect);
    }
    let (head, rest) = rest.split_top(SEP_H);
    separator(ui, head, "LAST MINUTE");
    let peak = |s: &Series| if s.is_empty() { PLACEHOLDER.into() } else { fmt_rate(f64::from(s.max())) };
    let rows = [Row::dot(IN_C, "Peak download", peak(&h.net_down)), Row::dot(OUT_C, "Peak upload", peak(&h.net_up))];
    for (row, rect) in rows.iter().zip(stack(rest, DETAIL_H)) {
        row.draw(ui, rect);
    }
}

// ------------------------------------------------------------------ battery

fn battery_color(b: &BatteryReading) -> Color {
    let percent = b.percent.min(100);
    if b.charging || b.on_ac {
        palette::GREEN
    } else if percent <= 20 {
        palette::RED
    } else if b.saver {
        palette::YELLOW
    } else {
        palette::GREEN
    }
}

fn draw_battery(ui: &mut Ui, b: &BatteryReading, cols: &Cols) {
    let percent = b.percent.min(100);
    let color = battery_color(b);
    let (dash, rest) = cols.dash.split_top(DASH_H);
    battery_glyph(ui, Point::new(dash.center().x - 26.0, dash.center().y), percent, color, b.on_ac);
    ui.c.text(&format!("{percent}%"), Rect::new(dash.center().x + 12.0, dash.y, dash.right() - dash.center().x - 12.0, dash.h), &TextStyle::display().tabular());
    let state = if b.charging {
        "Charging"
    } else if b.on_ac {
        if percent >= 99 {
            "Fully charged"
        } else {
            "Not charging"
        }
    } else {
        "Discharging"
    };
    let time = match (b.on_ac, b.secs_left) {
        (false, Some(s)) => fmt_time_left(s),
        _ => PLACEHOLDER.into(),
    };
    let rows = [
        Row::dot(color, "Level", format!("{percent}%")),
        Row::plain("Source", if b.on_ac { "Power adapter".into() } else { "Battery".into() }),
        Row::plain("State", state.into()),
        Row::plain("Time left", time),
        Row::plain(if MAC { "Low Power Mode" } else { "Battery saver" }, if b.saver { "On".into() } else { "Off".into() }),
    ];
    details(ui, rest, "DETAILS", &rows);

    // Health: today's full charge against the design.
    let (head, rest) = cols.mid.split_top(SEP_H);
    separator(ui, head, "HEALTH");
    let (dial, rest) = rest.split_top(DASH_H);
    match b.health() {
        Some(hl) => {
            let c = if hl >= 0.8 { palette::GREEN } else { palette::ORANGE };
            let ring = Ring { parts: vec![(hl.min(1.0), c)], value: fmt_pct(hl.min(1.0)) };
            draw_ring(ui, dial.center(), RING.min(dial.h - 8.0), &ring);
        }
        None => ui.c.text("Health not reported", dial, &TextStyle::footnote().color(palette::TEXT_FAINT).center()),
    }
    let wh = |mwh: Option<u32>| mwh.map_or(PLACEHOLDER.into(), |v| format!("{:.1} Wh", f64::from(v) / 1000.0));
    let rows = [Row::plain("Full charge", wh(b.full_mwh)), Row::plain("Design", wh(b.design_mwh))];
    for (row, rect) in rows.iter().zip(stack(rest.inset(0.0, 4.0), DETAIL_H)) {
        row.draw(ui, rect);
    }

    let (head, rest) = cols.top.split_top(SEP_H);
    separator(ui, head, "POWER");
    let rate = match b.rate_mw.filter(|r| *r != 0) {
        Some(r) if r > 0 => Row::dot(palette::GREEN, "Charging at", fmt_watts(r)),
        Some(r) => Row::dot(palette::ORANGE, "Using", fmt_watts(r)),
        None => Row::plain("Power", PLACEHOLDER.into()),
    };
    let rows = [rate, Row::plain("Cycles", b.cycles.map_or(PLACEHOLDER.into(), |c| c.to_string()))];
    for (row, rect) in rows.iter().zip(stack(rest, DETAIL_H)) {
        row.draw(ui, rect);
    }
}

/// Stats' battery: an outline with the charge in it, and a bolt while plugged in.
fn battery_glyph(ui: &mut Ui, center: Point, percent: u8, color: Color, plugged: bool) {
    let body = Rect::from_center(Point::new(center.x - 3.0, center.y), 78.0, 38.0);
    ui.c.stroke_rrect(body, 9.0, Color::white(0.45), 2.0);
    ui.c.fill_rrect(Rect::new(body.right() + 3.0, center.y - 6.0, 4.0, 12.0), 2.0, Color::white(0.45));
    let inner = body.inset(4.0, 4.0);
    let w = inner.w * f32::from(percent) / 100.0;
    if w > 0.5 {
        ui.c.fill_rrect(Rect::new(inner.x, inner.y, w.max(8.0), inner.h), 6.0, color);
    }
    if plugged {
        ui.c.icon(Icon::Bolt, Rect::from_center(inner.center(), 18.0, 18.0), palette::ON_ACCENT);
    }
}

// ------------------------------------------------------------------ power

/// The whole PC's draw: the CPU, the graphics card and the rest in the ring
/// and the rows on the left, the minute in the middle, where the number
/// comes from on the right.
fn draw_power(ui: &mut Ui, st: &mut AppState, p: &PowerReading, r: &Reading, h: &History, cols: &Cols) {
    if MAC {
        draw_power_mac(ui, st, p, r, h, cols);
        return;
    }
    let accent = st.accent;
    let total = p.total_w.max(0.1);
    let share = |w: f32| (w / total).clamp(0.0, 1.0);
    let gauge = |w: Option<f32>, label: &'static str, color: Color| match w {
        Some(w) => Gauge { frac: Some(share(w)), value: fmt_w(w), label, color },
        None => Gauge::unknown(label),
    };
    let ring = Ring {
        parts: vec![(share(p.cpu_w.unwrap_or(0.0)), CPU_W_C), (share(p.gpu_w.unwrap_or(0.0)), GPU_W_C), (share(p.rest_w), REST_W_C)],
        value: fmt_w(p.total_w),
    };
    let (dash, rest) = cols.dash.split_top(DASH_H);
    dashboard(ui, dash, &gauge(p.cpu_w, "CPU", CPU_W_C), &ring, &gauge(p.gpu_w, "GPU", GPU_W_C));
    let in_cpu = r.gpu.as_ref().is_none_or(|g| g.integrated);
    let gpu = match p.gpu_w {
        Some(w) => fmt_w(w),
        None if in_cpu => "In the CPU".into(),
        None => PLACEHOLDER.into(),
    };
    let rest_w = if p.estimated() { format!("about {}", fmt_w(p.rest_w)) } else { fmt_w(p.rest_w) };
    let rows = [
        Row::dot(CPU_W_C, "CPU", p.cpu_w.map_or_else(|| PLACEHOLDER.into(), fmt_w)),
        Row::dot(GPU_W_C, "Graphics card", gpu),
        Row::dot(REST_W_C, "Rest of the PC", rest_w),
        Row::plain(if p.estimated() { "Whole PC, estimated" } else { "Whole PC" }, fmt_w(p.total_w)),
    ];
    details(ui, rest, "WHERE IT GOES", &rows);

    // The minute, and the power supply's outputs (or the shares) under it.
    let rails = p.psu.as_ref().map(|s| s.rails.as_slice()).filter(|r| !r.is_empty());
    let (chart, sub) = history_column(ui, cols.mid, "POWER HISTORY", if rails.is_some() { "OUTPUTS" } else { "SHARE" });
    chart_box(ui, chart);
    let top = watts_ceiling(h.power.max());
    plot(ui, &h.power, top, accent, chart.inset(1.0, 1.0), Fill::Gradient);
    ui.c.text(&fmt_w(top), Rect::new(chart.x + 7.0, chart.y + 3.0, 60.0, 14.0), &TextStyle::caption().tabular().color(palette::TEXT_FAINT));
    match rails {
        Some(rails) => {
            let sum: f32 = rails.iter().map(|r| r.watts()).sum::<f32>().max(0.1);
            let names: Vec<String> = rails.iter().map(|r| format!("{} V", fmt_volts(r.nominal))).collect();
            let bars: Vec<(f32, Color, &str)> = rails.iter().zip(&names).map(|(r, n)| (r.watts() / sum, accent, n.as_str())).collect();
            labelled_bars(ui, sub, &bars);
        }
        None => labelled_bars(
            ui,
            sub,
            &[(share(p.cpu_w.unwrap_or(0.0)), CPU_W_C, "CPU"), (share(p.gpu_w.unwrap_or(0.0)), GPU_W_C, "Graphics"), (share(p.rest_w), REST_W_C, "Rest")],
        ),
    }

    // Where the number comes from.
    let (head, rest) = cols.top.split_top(SEP_H);
    separator(ui, head, "POWER SUPPLY");
    let name_st = TextStyle::footnote().weight(Weight::Semibold).wrap().top().line_spacing(1.25);
    let (name_r, rest) = rest.split_top(36.0);
    match &p.psu {
        Some(s) => {
            ui.c.text(&s.name, name_r.inset(0.0, 2.0), &name_st);
            let mut rows = Vec::new();
            if let Some(w) = s.in_w {
                rows.push(Row::plain("From the wall", fmt_w(w)));
            }
            if let Some(e) = s.efficiency {
                rows.push(Row::plain("Efficiency", fmt_pct(e)));
            }
            if let Some(v) = s.mains_v {
                rows.push(Row::plain("Mains", format!("{v:.0} V")));
            }
            if let Some(t) = s.temp_c {
                rows.push(Row::plain("Temperature", format!("{t:.0}\u{b0}C")));
            }
            if let Some(rpm) = s.fan_rpm {
                rows.push(Row::plain("Fan", if rpm == 0 { "Stopped".into() } else { format!("{rpm} RPM") }));
            }
            for (row, rect) in rows.iter().zip(stack(rest, DETAIL_H)) {
                row.draw(ui, rect);
            }
        }
        None => {
            ui.c.text("Doesn't report", name_r.inset(0.0, 2.0), &name_st.color(palette::TEXT_DIM));
            let note = format!(
                "So this adds about {:.0} W for the board, memory, drives and fans to what the CPU and the graphics card report. Corsair HXi and RMi, and MSI's Ai power supplies, report the real number over USB.",
                REST_W
            );
            ui.c.text(&note, rest.inset(0.0, 1.0), &TextStyle::caption().color(palette::TEXT_DIM).wrap().top().line_spacing(1.25));
        }
    }
}

/// The Mac's Power: the whole Mac's draw as its own sensor (the SMC) reads
/// it, with its minute. macOS doesn't split the watts by part, so the CPU's
/// and the graphics' load stand beside them instead.
fn draw_power_mac(ui: &mut Ui, st: &mut AppState, p: &PowerReading, r: &Reading, h: &History, cols: &Cols) {
    let accent = st.accent;
    let load = |v: Option<f32>, label: &'static str, color: Color| match v {
        Some(v) => Gauge { frac: Some(v.clamp(0.0, 1.0)), value: fmt_pct(v), label, color },
        None => Gauge::unknown(label),
    };
    let (cpu, gpu) = (r.cpu.as_ref().map(|c| c.load), r.gpu.as_ref().map(|g| g.util));
    let ring = Ring { parts: vec![(1.0, CPU_W_C)], value: fmt_w(p.total_w) };
    let (dash, rest) = cols.dash.split_top(DASH_H);
    dashboard(ui, dash, &load(cpu, "CPU", CPU_W_C), &ring, &load(gpu, "GPU", GPU_W_C));
    let rows = [
        Row::dot(CPU_W_C, "Whole Mac", fmt_w(p.total_w)),
        Row::plain("CPU load", cpu.map_or_else(|| PLACEHOLDER.into(), fmt_pct)),
        Row::plain("Graphics load", gpu.map_or_else(|| PLACEHOLDER.into(), fmt_pct)),
    ];
    details(ui, rest, "THIS MAC", &rows);

    // The minute, and its highest and lowest under it.
    let (chart, sub) = history_column(ui, cols.mid, "POWER HISTORY", "THIS MINUTE");
    chart_box(ui, chart);
    let top = watts_ceiling(h.power.max());
    plot(ui, &h.power, top, accent, chart.inset(1.0, 1.0), Fill::Gradient);
    ui.c.text(&fmt_w(top), Rect::new(chart.x + 7.0, chart.y + 3.0, 60.0, 14.0), &TextStyle::caption().tabular().color(palette::TEXT_FAINT));
    let low = h.power.values().filter(|v| v.is_finite()).fold(f32::INFINITY, f32::min);
    let line = if low.is_finite() { format!("Highest {} \u{b7} lowest {}", fmt_w(h.power.max()), fmt_w(low)) } else { PLACEHOLDER.into() };
    ui.c.text(&line, sub.inset(0.0, 3.0), &TextStyle::caption().tabular().color(palette::TEXT_DIM).top());

    // Where the number comes from.
    let (head, rest) = cols.top.split_top(SEP_H);
    separator(ui, head, "SOURCE");
    let name_st = TextStyle::footnote().weight(Weight::Semibold).wrap().top().line_spacing(1.25);
    let (name_r, rest) = rest.split_top(36.0);
    ui.c.text("The Mac's own sensor", name_r.inset(0.0, 2.0), &name_st);
    let note = "What its System Management Controller measures, the number Stats shows as System total. macOS doesn't split it by part.";
    ui.c.text(note, rest.inset(0.0, 1.0), &TextStyle::caption().color(palette::TEXT_DIM).wrap().top().line_spacing(1.25));
}

/// "12", "5", "3.3".
fn fmt_volts(v: f32) -> String {
    if v.fract().abs() < 0.05 {
        format!("{v:.0}")
    } else {
        format!("{v:.1}")
    }
}

// ------------------------------------------------------------------ games

/// Frame rate charts top out at the nearest common refresh rate above the peak.
const FPS_TOPS: [f32; 8] = [30.0, 60.0, 75.0, 120.0, 144.0, 165.0, 240.0, 360.0];

/// The readout's two ways over a game, and the `game_peek` each one sets.
const GAME_READOUT: [(bool, &str); 2] = [(false, "Stays"), (true, "Peeks")];

fn fps_ceiling(peak: f32) -> f32 {
    FPS_TOPS.into_iter().find(|t| peak <= *t).unwrap_or_else(|| (peak / 120.0).ceil().max(1.0) * 120.0)
}

/// Show over games: the switch and whether Windows lets Poni read frames on
/// the left, the frame rate's minute in the middle, what's in front on the
/// right.
fn draw_games(ui: &mut Ui, st: &mut AppState, cols: &Cols) {
    let accent = st.accent;
    let note = TextStyle::caption().color(palette::TEXT_DIM).wrap().top().line_spacing(1.2);

    // The switch, and the frames' access with its one button.
    let (head, rest) = cols.dash.split_top(SEP_H);
    separator(ui, head, "OVER GAMES");
    let (row, rest) = rest.split_top(28.0);
    let on = st.settings.game_stats;
    if crate::pages::settings::switch_row(ui, wid("system.games.switch"), row, "Show over games", Some(on), accent) {
        st.settings.game_stats = !on;
        st.cmd(Command::SaveSettings);
    }
    // The readout stays over the game, or peeks for a few seconds at a time
    // (between them nothing covers the game: no added delay).
    let (row, rest) = rest.split_top(28.0);
    let names = GAME_READOUT.map(|r| r.1);
    let x = row.right() - crate::pages::settings::picker_width(ui, &names);
    ui.c.text("Readout", Rect::new(row.x, row.y, (x - 10.0 - row.x).max(0.0), row.h), &TextStyle::body());
    let sel = GAME_READOUT.iter().position(|r| r.0 == st.settings.game_peek).unwrap_or(0);
    if let Some(i) = crate::pages::settings::picker(ui, "system.games.readout", x, row.center().y - 12.0, 24.0, &names, sel) {
        st.settings.game_peek = GAME_READOUT[i].0;
        st.cmd(Command::SaveSettings);
    }
    let (about, rest) = rest.split_top(40.0);
    let about_text = if st.settings.game_peek {
        "A few seconds as a game starts, when FPS drops and on Alt+Shift+F. No added delay in between."
    } else {
        "Stays over the game, which adds about a frame of delay. Alt+Shift+F hides it."
    };
    ui.c.text(about_text, about.inset(0.0, 3.0), &note);
    let (head, rest) = rest.split_top(SEP_H);
    separator(ui, head, "FPS");
    if st.game.access == FpsAccess::Unknown {
        st.cmd(Command::CheckFps);
    }
    let (text, action) = match &st.game.access {
        FpsAccess::Unknown | FpsAccess::Checking => ("Checking with Windows\u{2026}".to_string(), None),
        FpsAccess::Ready => ("Ready. Counted from Windows' own log of each frame; nothing loads into the game.".to_string(), None),
        FpsAccess::NeedsOk => ("Needs a one-time OK from Windows, as Xbox Game Bar does.".to_string(), Some(("Allow FPS", Command::AllowFps))),
        FpsAccess::Asking => ("Answer Windows' prompt to allow it.".to_string(), None),
        FpsAccess::SignOut => ("Allowed. Sign out of Windows and back in, and FPS shows.".to_string(), None),
        // Why goes to poni.log; the page says it plainly.
        FpsAccess::Failed(_) => ("Windows didn't allow it this time.".to_string(), Some(("Check again", Command::CheckFps))),
    };
    let (text_r, rest) = rest.split_top(44.0);
    ui.c.text(&text, text_r.inset(0.0, 3.0), &note);
    if let Some((label, cmd)) = action {
        let w = ui.pill_width(label, false, true);
        let r = Rect::new(rest.x, rest.y + 2.0, w, PILL_SMALL);
        if ui.pill(wid("system.games.fps"), r, label, None, PillStyle::Primary(accent)).clicked {
            st.cmd(cmd);
        }
    }

    // The frame rate's last minute, and the frame times under it.
    let (chart, sub) = history_column(ui, cols.mid, "FRAMES PER SECOND", "FRAME TIME");
    chart_box(ui, chart);
    let h = st.game.history.clone();
    if h.values().any(f32::is_finite) {
        let top = fps_ceiling(h.max());
        plot(ui, &h, top, accent, chart.inset(1.0, 1.0), Fill::Gradient);
        let scale = TextStyle::caption().tabular().color(palette::TEXT_FAINT);
        ui.c.text(&format!("{top:.0}"), Rect::new(chart.x + 7.0, chart.y + 3.0, 40.0, 14.0), &scale);
    } else {
        let waiting = if !on {
            "Off"
        } else if st.game.active {
            "No frames yet"
        } else {
            "Start a game in borderless full screen"
        };
        ui.c.text(waiting, chart.inset(8.0, 0.0), &TextStyle::caption().color(palette::TEXT_FAINT).center());
    }
    let rate = st.game.rate.filter(|_| st.game.active);
    let times = [
        Row::plain("Average", rate.map_or_else(|| PLACEHOLDER.into(), |r| fmt_frame_ms(r.frame_ms))),
        Row::plain("Slowest in the last second", rate.map_or_else(|| PLACEHOLDER.into(), |r| fmt_frame_ms(r.worst_ms))),
    ];
    for (row, rect) in times.iter().zip(stack(sub, DETAIL_H)) {
        row.draw(ui, rect);
    }

    // What's in front, and the island's numbers for it.
    let (head, rest) = cols.top.split_top(SEP_H);
    separator(ui, head, "IN FRONT");
    let (name_r, rest) = rest.split_top(36.0);
    let (name, ink) = match (st.game.active, &st.game.app) {
        (true, Some(app)) => (app.clone(), palette::TEXT),
        (true, None) => ("A full-screen app".to_string(), palette::TEXT),
        (false, _) => ("No game in front".to_string(), palette::TEXT_FAINT),
    };
    ui.c.text(&name, name_r.inset(0.0, 2.0), &TextStyle::footnote().weight(Weight::Semibold).wrap().top().line_spacing(1.25).color(ink));
    let reading = st.system.latest.as_ref().filter(|_| st.game.active);
    let gpu = reading.and_then(|r| r.gpu.as_ref());
    let rows = [
        Row::plain("FPS", rate.map_or_else(|| PLACEHOLDER.into(), |r| fmt_fps(r.fps))),
        Row::plain("GPU", gpu.map_or_else(|| PLACEHOLDER.into(), |g| fmt_pct(g.util))),
        Row::plain("GPU temperature", gpu.and_then(|g| g.temp_c).map_or_else(|| PLACEHOLDER.into(), |t| format!("{t:.0}\u{b0}C"))),
        Row::plain("Video memory", gpu.map_or_else(|| PLACEHOLDER.into(), |g| fmt_bytes(g.mem_used))),
        Row::plain("CPU", reading.and_then(|r| r.cpu.as_ref()).map_or_else(|| PLACEHOLDER.into(), |c| fmt_pct(c.load))),
    ];
    for (row, rect) in rows.iter().zip(stack(rest, DETAIL_H)) {
        row.draw(ui, rect);
    }
}

// ------------------------------------------------------------------ dashboard

/// One of the smaller rings beside the centre one: how full, the value
/// inside, what it is under it.
struct Gauge {
    frac: Option<f32>,
    value: String,
    label: &'static str,
    color: Color,
}

impl Gauge {
    fn unknown(label: &'static str) -> Gauge {
        Gauge { frac: None, value: PLACEHOLDER.into(), label, color: palette::TRACK }
    }
}

fn temperature(t: f32) -> Gauge {
    Gauge { frac: Some((t - 20.0) / 80.0), value: format!("{t:.0}°C"), label: "Temperature", color: heat_color(t) }
}

/// A speed in GHz as Stats writes it ("2.56").
fn ghz(mhz: u32) -> String {
    format!("{:.2}", mhz as f32 / 1000.0)
}

/// Stats' pie chart: parts in turn from twelve o'clock, the value inside.
struct Ring {
    parts: Vec<(f32, Color)>,
    value: String,
}

/// The centre ring's and side rings' diameters for a dashboard in `r`.
fn dashboard_sizes(r: Rect) -> (f32, f32) {
    let d = RING.min(r.h - 10.0).min(r.w * 0.4);
    (d, (GAUGE * d / RING).min((r.w - d) / 2.0 - 6.0))
}

fn dashboard(ui: &mut Ui, r: Rect, left: &Gauge, ring: &Ring, right: &Gauge) {
    let (d, g) = dashboard_sizes(r);
    let c = r.center();
    draw_ring(ui, c, d, ring);
    let gy = c.y - 6.0;
    draw_gauge(ui, Point::new(r.x + g / 2.0 + 2.0, gy), g, left);
    draw_gauge(ui, Point::new(r.right() - g / 2.0 - 2.0, gy), g, right);
}

fn draw_ring(ui: &mut Ui, c: Point, d: f32, ring: &Ring) {
    let radius = d / 2.0 - RING_W / 2.0;
    ui.c.stroke_circle(c, radius, palette::TRACK, RING_W);
    let mut start = 0.0f32;
    for &(f, color) in &ring.parts {
        let f = f.clamp(0.0, (1.0 - start).max(0.0));
        if f > 0.002 {
            ui.c.arc(c, radius, start, f, color, RING_W);
        }
        start += f;
    }
    let size = (d * 0.21).clamp(11.0, 15.0);
    ui.c.text(&ring.value, Rect::from_center(c, d - 2.0 * RING_W - 2.0, size + 6.0), &TextStyle::new(size).weight(Weight::Semibold).tabular().center());
}

fn draw_gauge(ui: &mut Ui, c: Point, d: f32, g: &Gauge) {
    if d < 20.0 {
        return;
    }
    let radius = d / 2.0 - GAUGE_W / 2.0;
    ui.c.stroke_circle(c, radius, palette::TRACK, GAUGE_W);
    if let Some(f) = g.frac.filter(|f| f.is_finite()) {
        ui.c.arc(c, radius, 0.0, f.clamp(0.0, 1.0), g.color, GAUGE_W);
    }
    let inner = d - 2.0 * GAUGE_W - 2.0;
    let mut size = if g.value.chars().count() > 4 { 10.0 } else { 11.0 };
    // The Mac's font runs wider than Windows': a value too wide for the ring ("307 MB") shrinks to fit it.
    if MAC {
        let w = ui.c.measure(&g.value, &TextStyle::new(size).weight(Weight::Semibold).tabular(), 400.0).0;
        if w > inner - 2.0 {
            size = (size * (inner - 2.0) / w).max(7.0);
        }
    }
    ui.c.text(&g.value, Rect::from_center(c, inner, 16.0), &TextStyle::new(size).weight(Weight::Semibold).tabular().center());
    gauge_label(ui, c, d, g.label, palette::TEXT_DIM);
}

fn gauge_label(ui: &mut Ui, c: Point, d: f32, label: &str, color: Color) {
    let r = Rect::new(c.x - d / 2.0 - 14.0, c.y + d / 2.0 + 1.0, d + 28.0, 13.0);
    ui.c.text(label, r, &TextStyle::new(10.0).color(color).center());
}

/// Stats' memory pressure gauge: a half circle green, yellow and red where
/// Windows' commit charge would read normal, warning and critical (on a Mac
/// the kernel's own level picks the zone), a needle at the pressure, and the
/// word under it.
fn pressure_gauge(ui: &mut Ui, c: Point, d: f32, p: f32) {
    if d < 20.0 {
        return;
    }
    let radius = d / 2.0 - GAUGE_W / 2.0;
    // From nine o'clock over the top to three o'clock.
    let zones = [(0.0, 0.75, palette::GREEN), (0.75, 0.9, palette::YELLOW), (0.9, 1.0, palette::RED)];
    for (a, b, color) in zones {
        ui.c.arc(c, radius, 0.75 + 0.5 * a, 0.5 * (b - a) - 0.012, color, GAUGE_W - 1.0);
    }
    let angle = (0.75 + 0.5 * p.clamp(0.0, 1.0)) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    let tip = Point::new(c.x + (radius - 4.0) * angle.cos(), c.y + (radius - 4.0) * angle.sin());
    ui.c.line(c, tip, palette::TEXT, 2.0);
    ui.c.fill_circle(c, 3.0, palette::TEXT);
    gauge_label(ui, Point::new(c.x, c.y - 6.0), d, pressure_word(p), pressure_color(p));
}

// ------------------------------------------------------------------ details

/// One line of details: an optional dot naming a part's colour, the label
/// and the value at the right.
struct Row {
    dot: Option<Color>,
    label: &'static str,
    value: String,
}

impl Row {
    fn dot(c: Color, label: &'static str, value: String) -> Row {
        Row { dot: Some(c), label, value }
    }

    fn plain(label: &'static str, value: String) -> Row {
        Row { dot: None, label, value }
    }

    fn draw(&self, ui: &mut Ui, r: Rect) {
        let mut x = r.x;
        if let Some(c) = self.dot {
            ui.c.fill_rrect(Rect::new(r.x, r.center().y - 4.0, 8.0, 8.0), 2.0, c);
            x += 14.0;
        }
        let vst = TextStyle::footnote().tabular().right();
        let vw = ui.c.measure(&self.value, &vst, r.w).0.min((r.right() - x) * 0.66);
        ui.c.text(&self.value, Rect::new(r.right() - vw - 2.0, r.y, vw + 2.0, r.h), &vst);
        ui.c.text(self.label, Rect::new(x, r.y, (r.right() - vw - 8.0 - x).max(0.0), r.h), &TextStyle::footnote().color(palette::TEXT_DIM));
    }
}

fn details(ui: &mut Ui, r: Rect, title: &str, rows: &[Row]) {
    let (head, rest) = r.split_top(SEP_H);
    separator(ui, head, title);
    for (row, rect) in rows.iter().zip(stack(rest, DETAIL_H)) {
        row.draw(ui, rect);
    }
}

/// Rows of height `h` down `r`, as many as fit.
fn stack(r: Rect, h: f32) -> Vec<Rect> {
    let n = ((r.h + 0.5) / h).floor().max(0.0) as usize;
    (0..n).map(|i| Rect::new(r.x, r.y + i as f32 * h, r.w, h)).collect()
}

/// Stats' section title: small capitals between two hairlines.
fn separator(ui: &mut Ui, r: Rect, title: &str) {
    let st = TextStyle::new(10.0).weight(Weight::Semibold).color(palette::TEXT_FAINT).center();
    let tw = ui.c.measure(title, &st, r.w).0;
    let px = 1.0 / ui.c.scale().max(0.5);
    let cy = (r.center().y).round();
    let lw = ((r.w - tw) / 2.0 - 8.0).max(0.0);
    if lw >= 6.0 {
        ui.c.fill_rect(Rect::new(r.x, cy, lw, px), RULE_C);
        ui.c.fill_rect(Rect::new(r.right() - lw, cy, lw, px), RULE_C);
    }
    ui.c.text(title, r, &st);
}

/// The middle column: a titled usage chart, and a titled second chart under it.
fn history_column(ui: &mut Ui, r: Rect, title: &str, sub_title: &str) -> (Rect, Rect) {
    let (head, rest) = r.split_top(SEP_H);
    separator(ui, head, title);
    let (chart, rest) = rest.split_top((rest.h - SEP_H - SUB_H - 4.0).max(0.0));
    let (sub_head, sub) = rest.split_top(SEP_H + 4.0);
    separator(ui, sub_head.offset(0.0, 2.0), sub_title);
    (chart, Rect::new(sub.x, sub.y, sub.w, SUB_H.min(sub.h)))
}

/// The processes using the most, with their icons.
fn processes(ui: &mut Ui, st: &mut AppState, r: Rect, apps: &[AppUse], value: impl Fn(&AppUse) -> String, empty: &str) {
    let (head, rest) = r.split_top(SEP_H);
    separator(ui, head, "TOP PROCESSES");
    if apps.is_empty() {
        let note = if empty.is_empty() { "Reading…" } else { empty };
        ui.c.text(note, rest.split_top(PROC_H).0, &TextStyle::footnote().color(palette::TEXT_FAINT));
        return;
    }
    let value_st = TextStyle::footnote().tabular().right().color(palette::TEXT_DIM);
    for (a, row) in apps.iter().zip(stack(rest, PROC_H)) {
        let icon = Rect::new(row.x, row.center().y - APP_ICON / 2.0, APP_ICON, APP_ICON);
        draw_app_icon(ui, st, a, icon);
        let v = value(a);
        let vw = ui.c.measure(&v, &value_st, 200.0).0;
        ui.c.text(&v, Rect::new(row.right() - vw - 2.0, row.y, vw + 2.0, row.h), &value_st);
        let nx = icon.right() + 7.0;
        ui.c.text(&a.name, Rect::new(nx, row.y, (row.right() - vw - 8.0 - nx).max(0.0), row.h), &TextStyle::footnote());
    }
}

fn draw_app_icon(ui: &mut Ui, st: &mut AppState, a: &AppUse, r: Rect) {
    // The kernel's own entry has no file: Windows' mark stands for it.
    if !MAC && a.image == "system" {
        ui.c.icon(Icon::Windows, r.inset(1.0, 1.0), palette::BLUE);
        return;
    }
    let img = a.path.as_deref().and_then(|p| match st.app_icons.get(p) {
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

/// Colour-dot items left to right along `r`.
fn legend_row(ui: &mut Ui, r: Rect, items: &[(Option<Color>, String)]) {
    let st = TextStyle::caption().tabular().color(palette::TEXT_DIM);
    let mut x = r.x;
    for (dot, text) in items {
        if let Some(c) = dot {
            ui.c.fill_circle(Point::new(x + 3.5, r.center().y), 3.0, *c);
            x += 10.0;
        }
        let tw = ui.c.measure(text, &st, r.w).0;
        if x + tw > r.right() + 0.5 {
            break;
        }
        ui.c.text(text, Rect::new(x - 1.0, r.y, tw + 2.0, r.h), &st);
        x += tw + 12.0;
    }
}

// ------------------------------------------------------------------ charts

fn chart_box(ui: &mut Ui, r: Rect) {
    ui.c.fill_rrect(r, BOX_R, BOX_C);
    for i in 1..4 {
        let y = (r.y + r.h * i as f32 / 4.0).round();
        ui.c.line(Point::new(r.x + 4.0, y), Point::new(r.right() - 4.0, y), GRID_C, 1.0);
    }
}

/// How a plotted line fills down to its baseline.
#[derive(Clone, Copy, PartialEq)]
enum Fill {
    /// Stats' gradient: strongest at the top of the chart, clear at the bottom.
    Gradient,
    /// One faint wash (the strip's sparklines).
    Faint,
}

/// One series rising from the bottom of `r`, newest at the right edge; gaps
/// (NaN) split the line.
fn plot(ui: &mut Ui, s: &Series, top: f32, color: Color, r: Rect, fill: Fill) {
    plot_from(ui, s, top, color, r, fill, false);
}

/// Like [`plot`]; `down` hangs the series from the top of `r` instead.
fn plot_from(ui: &mut Ui, s: &Series, top: f32, color: Color, r: Rect, fill: Fill, down: bool) {
    if s.is_empty() || top <= 0.0 || r.w < 2.0 || r.h < 2.0 {
        return;
    }
    let step = r.w / (HISTORY_LEN - 1) as f32;
    let x0 = r.right() - (s.len() - 1) as f32 * step;
    let base = if down { r.y } else { r.bottom() };
    let y = |v: f32| {
        let k = (v / top).clamp(0.0, 1.0) * (r.h - 1.0);
        if down {
            base + k
        } else {
            base - k
        }
    };
    let values: Vec<f32> = s.values().collect();
    // Kept to its box: a stroke's cap may otherwise reach past the last point
    // (Wine's renderer draws a few pixels on along the line).
    ui.c.clip(Rect::new(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0));
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
        let mut area = line.clone();
        area.push(Seg::LineTo(xs(run.len() - 1), base));
        area.push(Seg::LineTo(xs(0), base));
        area.push(Seg::Close);
        match fill {
            Fill::Faint => ui.c.path(&area, Some(color.mul_alpha(0.22)), None),
            // Stats' fill: solid enough to read as an area, strongest at the far edge of the chart.
            Fill::Gradient => ui.c.fill_path_fade(&area, color, base, if down { r.bottom() } else { r.y }, 0.3, 0.7),
        }
        ui.c.path(&line, None, Some((color, 1.5)));
    }
    ui.c.unclip();
}

/// Two rate series around the middle of `r`: `up` above it, `down` below,
/// on one scale (the minute's peak, labelled in the corner).
fn mirrored(ui: &mut Ui, up: &Series, up_c: Color, down: &Series, down_c: Color, r: Rect) {
    let inner = r.inset(1.0, 2.0);
    let peak = nice_ceiling(up.max().max(down.max()));
    let mid = (inner.y + inner.h / 2.0).round();
    ui.c.line(Point::new(r.x + 4.0, mid), Point::new(r.right() - 4.0, mid), Color::white(0.14), 1.0);
    plot_from(ui, up, peak, up_c, Rect::new(inner.x, inner.y, inner.w, mid - inner.y), Fill::Gradient, false);
    plot_from(ui, down, peak, down_c, Rect::new(inner.x, mid, inner.w, inner.bottom() - mid), Fill::Gradient, true);
    let label = fmt_rate(f64::from(peak));
    ui.c.text(&label, Rect::new(r.x + 6.0, r.y + 3.0, r.w - 12.0, 12.0), &TextStyle::new(9.5).color(palette::TEXT_FAINT).top());
}

/// Stats' bar chart: one bar per value, rising from the bottom of the box.
/// The bars of each colour go in one shape (a frame draws them once a second).
fn bar_chart(ui: &mut Ui, r: Rect, bars: &[(f32, Color)]) {
    ui.c.fill_rrect(r, BOX_R, BOX_C);
    let n = bars.len();
    if n == 0 {
        return;
    }
    let inner = r.inset(6.0, 5.0);
    let gap = if n > 24 { 1.0 } else { 2.0 };
    let bw = ((inner.w - gap * (n - 1) as f32) / n as f32).max(1.0);
    // Each bar stands in its own grey track, as in Stats.
    let mut shapes: Vec<(Color, Vec<Seg>)> = vec![(Color::white(0.10), Vec::new())];
    for i in 0..n {
        let x = inner.x + i as f32 * (bw + gap);
        shapes[0].1.extend([Seg::MoveTo(x, inner.bottom()), Seg::LineTo(x, inner.y), Seg::LineTo(x + bw, inner.y), Seg::LineTo(x + bw, inner.bottom()), Seg::Close]);
    }
    for (i, &(v, c)) in bars.iter().enumerate() {
        let h = (v.clamp(0.0, 1.0) * inner.h).max(1.5);
        let (x, top, bottom) = (inner.x + i as f32 * (bw + gap), inner.bottom() - h, inner.bottom());
        let segs = match shapes.iter_mut().find(|(k, _)| *k == c) {
            Some((_, segs)) => segs,
            None => {
                shapes.push((c, Vec::new()));
                &mut shapes.last_mut().expect("just pushed").1
            }
        };
        segs.extend([Seg::MoveTo(x, bottom), Seg::LineTo(x, top), Seg::LineTo(x + bw, top), Seg::LineTo(x + bw, bottom), Seg::Close]);
    }
    for (c, segs) in &shapes {
        ui.c.path(segs, Some(*c), None);
    }
}

/// A few wide bars with their names under them (the GPU's engines).
fn labelled_bars(ui: &mut Ui, r: Rect, bars: &[(f32, Color, &str)]) {
    ui.c.fill_rrect(r, BOX_R, BOX_C);
    let cells = r.inset(8.0, 4.0).columns(bars.len().max(1), 8.0);
    let lst = TextStyle::new(9.5).color(palette::TEXT_DIM).center();
    for (&(v, c, name), cell) in bars.iter().zip(cells) {
        let (track, label) = cell.split_bottom(13.0);
        let bar = Rect::new(track.center().x - 7.0, track.y + 2.0, 14.0, track.h - 2.0);
        ui.c.fill_rrect(bar, 3.0, palette::TRACK);
        let h = (v.clamp(0.0, 1.0) * bar.h).max(2.0);
        ui.c.fill_rrect(Rect::new(bar.x, bar.bottom() - h, bar.w, h), 3.0, c);
        ui.c.text(name, label, &lst);
    }
}

/// Stats' "used" bar: memory's parts end to end, with the total in use
/// over it and a note under it.
fn stacked_meter(ui: &mut Ui, r: Rect, parts: &[(f32, Color)], left: &str, right: &str, note: &str) {
    ui.c.fill_rrect(r, BOX_R, BOX_C);
    let inner = r.inset(10.0, 6.0);
    let (text, rest) = inner.split_top(16.0);
    let rst = TextStyle::footnote().tabular().right().color(palette::TEXT_DIM);
    let rw = ui.c.measure(right, &rst, text.w).0;
    ui.c.text(right, Rect::new(text.right() - rw - 2.0, text.y, rw + 2.0, text.h), &rst);
    ui.c.text(left, Rect::new(text.x, text.y, (text.w - rw - 8.0).max(0.0), text.h), &TextStyle::footnote().weight(Weight::Semibold).tabular());
    let bar = Rect::new(rest.x, rest.y + 3.0, rest.w, 7.0);
    ui.c.fill_rrect(bar, 3.5, palette::TRACK);
    let mut x = bar.x;
    for &(f, c) in parts {
        let w = bar.w * f.clamp(0.0, 1.0);
        if w >= 1.0 {
            ui.c.fill_rect(Rect::new(x, bar.y, w.min(bar.right() - x), bar.h), c);
            x += w;
        }
    }
    if !note.is_empty() {
        ui.c.text(note, Rect::new(rest.x, bar.bottom() + 2.0, rest.w, (rest.bottom() - bar.bottom() - 2.0).max(0.0)), &TextStyle::caption().color(palette::TEXT_FAINT));
    }
}

/// Bytes short enough for inside a small ring ("340 MB", "1.2 GB").
fn compact_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = b as f64 / MB;
    if mb < 1000.0 {
        format!("{mb:.0} MB")
    } else if mb < 10.0 * 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{:.0} GB", mb / 1024.0)
    }
}

fn heat_color(c: f32) -> Color {
    if c >= 85.0 {
        palette::RED
    } else if c >= 70.0 {
        palette::ORANGE
    } else {
        palette::GREEN
    }
}

fn pressure_color(p: f32) -> Color {
    match pressure_word(p) {
        "Normal" => palette::GREEN,
        "Warning" => palette::YELLOW,
        _ => palette::RED,
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

    #[test]
    fn columns_fill_the_width() {
        for w in [444.0, 584.0, 864.0] {
            let c = Cols::new(Rect::new(10.0, 0.0, w, 200.0));
            assert!((c.top.right() - (10.0 + w)).abs() < 1e-3);
            assert!(c.mid.w > 120.0, "{w}: middle column {}", c.mid.w);
            assert!((c.mid.x - c.dash.right() - COL_GAP).abs() < 1e-3 && (c.top.x - c.mid.right() - COL_GAP).abs() < 1e-3);
        }
    }

    #[test]
    fn rows_stack_as_many_as_fit() {
        assert_eq!(stack(Rect::new(0.0, 0.0, 10.0, 100.0), 20.0).len(), 5);
        assert_eq!(stack(Rect::new(0.0, 0.0, 10.0, 99.0), 20.0).len(), 4);
        assert!(stack(Rect::new(0.0, 0.0, 10.0, 0.0), 20.0).is_empty());
    }

    #[test]
    fn series_add_up_with_gaps() {
        let (mut a, mut b) = (Series::default(), Series::default());
        a.push(Some(1.0));
        b.push(Some(2.0));
        a.push(None);
        b.push(Some(5.0));
        let s: Vec<f32> = sum(&a, &b).values().collect();
        assert_eq!(s[0], 3.0);
        assert!(s[1].is_nan());
    }
}
