//! Paints the Diorama pill at a list of moments, for looking at while
//! drawing it: `cargo run -p studio-diorama --example sheet -- <out-dir>
//! [scale] [12h] [bare] [charging|low] [day=N] [film] [slide=HHMM]`. One PPM
//! per moment (`HHMM.ppm`, and `roll-NN.ppm` for the ball's roll at
//! sunset), which any image tool opens; `film` writes a whole day instead, a
//! frame every five minutes; `slide=HHMM` the ball slid by hand across the
//! pane at that time (`slide-NN.ppm`, from where it rests). Not part of the
//! app.

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
        let mut m = Moment { minute, sunrise: 360.0, sunset: 1080.0, day, lens: 0.0, clock, badge, dark: None };
        m.lens = m.rest_lens();
        m
    };
    std::fs::create_dir_all(&dir).expect("make the folder");
    if let Some(hhmm) = args.iter().find_map(|a| a.strip_prefix("slide=").and_then(|t| t.parse::<u32>().ok())) {
        // The ball slid by hand from where it rests at that time to the other
        // end: 13 frames, or 49 for a film.
        let m = moment((hhmm / 100 * 60 + hhmm % 100) as f32);
        let steps = if has("film") { 48 } else { 12 };
        for i in 0..=steps {
            let lens = i as f32 / steps as f32;
            save(&dir, &format!("slide-{i:02}"), &m.slid(if m.night() { 1.0 - lens } else { lens }), scale, 0.0);
        }
        return;
    }
    if has("film") {
        // A whole day, a frame every five minutes, with the ball's roll at
        // sunrise and at sunset in between: `film-NNNN.ppm`.
        let mut n = 0;
        let mut frame = |m: &Moment| {
            save(&dir, &format!("film-{n:04}"), m, scale, 0.0);
            n += 1;
        };
        for step in 0..288 {
            let minute = (step * 5) as f32;
            for (at, from) in [(360.0, 1.0), (1080.0, 0.0)] {
                if minute == at {
                    for i in 0..=24 {
                        let t = i as f32 / 24.0;
                        let eased = t * t * t * (t * (6.0 * t - 15.0) + 10.0);
                        let mut m = moment(minute);
                        m.lens = from + (1.0 - 2.0 * from) * eased;
                        frame(&m);
                    }
                }
            }
            frame(&moment(minute));
        }
        return;
    }
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
