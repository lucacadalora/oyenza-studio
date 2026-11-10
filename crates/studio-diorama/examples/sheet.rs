//! Paints the Diorama pill at a list of moments, for looking at while
//! drawing it: `cargo run -p studio-diorama --example sheet -- <out-dir>
//! [scale] [12h] [bare] [charging|low]`. One PPM per moment (`HHMM.ppm`,
//! and `roll-NN.ppm` for the ball's roll at sunset), which any image tool
//! opens. Not part of the app.

use std::io::Write;
use studio_diorama::{render, width, Badge, Clock, Moment, View, H};

fn save(dir: &str, name: &str, m: &Moment, scale: f32, lift: f32) {
    let w = width(m.clock, m.badge);
    let p = render(m, &View { w: w + 2.0 * lift, h: H + 2.0 * lift, rest_w: w, rest_h: H, scale });
    let mut out = format!("P6\n{} {}\n255\n", p.w, p.h).into_bytes();
    for px in p.bgra.chunks(4) {
        out.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    std::fs::File::create(format!("{dir}/{name}.ppm")).and_then(|mut f| f.write_all(&out)).expect("write the picture");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().cloned().unwrap_or_else(|| ".".into());
    let scale: f32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let badge = if has("charging") {
        Some(Badge::Charging)
    } else if has("low") {
        Some(Badge::Low(12))
    } else {
        None
    };
    let day: i32 = args.iter().find_map(|a| a.strip_prefix("day=").and_then(|d| d.parse().ok())).unwrap_or(9817);
    let moment = |minute: f32| {
        let clock = (!has("bare")).then_some(Clock { hour: (minute / 60.0) as u8 % 24, minute: (minute % 60.0) as u8, h24: !has("12h") });
        let mut m = Moment { minute, sunrise: 360.0, sunset: 1080.0, day, lens: 0.0, clock, badge };
        m.lens = m.rest_lens();
        m
    };
    std::fs::create_dir_all(&dir).expect("make the folder");
    for hhmm in [0, 430, 545, 600, 615, 640, 700, 800, 1008, 1200, 1327, 1500, 1650, 1715, 1740, 1759, 1800, 1815, 1840, 2000, 2130, 2359] {
        let m = moment((hhmm / 100 * 60 + hhmm % 100) as f32);
        save(&dir, &format!("{hhmm:04}"), &m, scale, 0.0);
    }
    // The ball's roll at sunset.
    for i in 0..=12 {
        let mut m = moment(1080.0);
        m.lens = i as f32 / 12.0;
        save(&dir, &format!("roll-{i:02}"), &m, scale, 0.0);
    }
    // Under the pointer.
    save(&dir, "lift-1008", &moment(608.0), scale, 3.0);
    save(&dir, "lift-2130", &moment(1290.0), scale, 3.0);
}
