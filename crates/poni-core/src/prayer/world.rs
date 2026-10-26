//! Places outside Indonesia: GeoNames' cities of 15,000 people or more with
//! their time zones, looked up online one small file at a time
//! (`site/places/1/` in the repo, https://studio.oyenza.com/places/1/ live;
//! tools/make_world_places.py builds them and describes the format).
//!
//! The app never holds the whole list. While the place picker is open it
//! asks for the file of the two letters typed ([`file_for`]); to find the
//! city nearest a location, for the 5-degree squares around it
//! ([`files_near`]); and drops them when done. The place picked or found is
//! kept in settings as its own few lines ([`World::excerpt`]), so its times
//! need nothing online.

use super::{distance_km, Clock, Method, Place};

/// Ids of places from this list: "w:" and the point as the file writes it.
pub const ID_PREFIX: &str = "w:";
/// Where the files are (the website, `site/places/1/`). A new format goes
/// in a new folder, so copies that read this one keep working.
pub const BASE_URL: &str = "https://studio.oyenza.com/places/1/";
/// The squares of the `near/` files, in degrees.
const SQUARE_DEG: f64 = 5.0;

/// The file of the places matching what was typed (folded with [`fold`]):
/// the first two letters of its first word of two or more. `None` until
/// there are two.
pub fn file_for(folded: &str) -> Option<String> {
    let word = folded.split(' ').find(|w| w.len() >= 2)?;
    let head: String = word.chars().take(2).collect();
    (head.len() == 2 && head.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())).then_some(head)
}

/// The files of the 5-degree squares within `km` of a point, its own first.
pub fn files_near(lat: f64, lng: f64, km: f64) -> Vec<String> {
    let dlat = km / 111.0;
    // A degree of longitude shrinks toward the poles (held at 80 degrees).
    let dlng = (km / (111.0 * lat.abs().min(80.0).to_radians().cos())).min(180.0);
    let square = |v: f64| (v / SQUARE_DEG).floor() as i32 * SQUARE_DEG as i32;
    let wrap = |v: i32| (v + 180).rem_euclid(360) - 180;
    let (lat0, lat1) = (square((lat - dlat).max(-90.0)), square((lat + dlat).min(89.999)));
    let (lng0, lng1) = (square(lng - dlng), square(lng + dlng));
    let mut out = vec![format!("near/{}_{}", square(lat), wrap(square(lng)))];
    for la in (lat0..=lat1).step_by(SQUARE_DEG as usize) {
        for lo in (lng0..=lng1).step_by(SQUARE_DEG as usize) {
            let f = format!("near/{la}_{}", wrap(lo));
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// A file's address ("ed" or "near/55_-5").
pub fn url(file: &str) -> String {
    format!("{BASE_URL}{file}.txt")
}

pub struct World {
    text: String,
    zones: Vec<Clock>,
    countries: Vec<Country>,
    /// (typed name, places.json id) for other spellings of the Saudi places.
    aliases: Vec<(String, String)>,
    /// The first `big` rows have a million people or more.
    big: usize,
    /// P lines, most people first: where each line and its search key sit in `text`.
    rows: Vec<Row>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "World({} places)", self.rows.len())
    }
}

struct Country {
    code: [u8; 2],
    name: String,
    method: Method,
    hanafi: bool,
}

#[derive(Clone, Copy)]
struct Row {
    line: (u32, u32),
    /// The search key, or the name when the key is the name in lowercase.
    key: (u32, u32),
    lowercase: bool,
}

/// A match for what was typed: lower `rank` first, then `order`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rank: u8,
    /// Position in the list (most people first).
    pub order: usize,
}

impl World {
    /// No places (a file the website doesn't have: no city there).
    pub fn empty() -> World {
        World { text: String::new(), zones: Vec::new(), countries: Vec::new(), aliases: Vec::new(), big: 0, rows: Vec::new() }
    }

    pub fn parse(text: String) -> Result<World, String> {
        let bad = |l: &str| format!("world places: bad line {:?}", l.chars().take(60).collect::<String>());
        let (mut zones, mut countries, mut aliases, mut big, mut rows) = (Vec::new(), Vec::new(), Vec::new(), 0, Vec::new());
        let mut at = 0usize;
        for line in text.split_inclusive('\n') {
            let start = at;
            at += line.len();
            let line = line.trim_end_matches(['\n', '\r']);
            let mut f = line.split('\t');
            match f.next() {
                Some("Z") => {
                    let (Some(n), Some(_name), Some(first), Some(changes)) = (f.next(), f.next(), f.next(), f.next()) else { return Err(bad(line)) };
                    if n.parse::<usize>().ok() != Some(zones.len()) {
                        return Err(bad(line));
                    }
                    zones.push(Clock::parse(first, changes).ok_or_else(|| bad(line))?);
                }
                Some("C") => {
                    let (Some(code), Some(name), Some(method), Some(asr)) = (f.next(), f.next(), f.next(), f.next()) else { return Err(bad(line)) };
                    let code: [u8; 2] = code.as_bytes().try_into().map_err(|_| bad(line))?;
                    // A method this app doesn't know yet: the Muslim World League's.
                    let method = Method::from_key(method).unwrap_or(Method::MuslimWorldLeague);
                    countries.push(Country { code, name: name.to_string(), method, hanafi: asr == "h" });
                }
                Some("A") => {
                    let (Some(name), Some(id)) = (f.next(), f.next()) else { return Err(bad(line)) };
                    aliases.push((name.to_string(), id.to_string()));
                }
                Some("B") => big = f.next().and_then(|n| n.parse().ok()).ok_or_else(|| bad(line))?,
                Some("P") => {
                    let name_at = start + 2;
                    let (Some(name), Some(key)) = (f.next(), f.next()) else { return Err(bad(line)) };
                    let (key_at, key_len, lowercase) = if key.is_empty() { (name_at, name.len(), true) } else { (name_at + name.len() + 1, key.len(), false) };
                    rows.push(Row { line: (start as u32, (start + line.len()) as u32), key: (key_at as u32, (key_at + key_len) as u32), lowercase });
                }
                _ => {}
            }
        }
        // Looked up by code: the file lists them in order.
        if let Some(c) = countries.windows(2).find(|c| c[0].code >= c[1].code) {
            return Err(format!("world places: country {} out of order", String::from_utf8_lossy(&c[1].code)));
        }
        let world = World { text, zones, countries, aliases, big, rows };
        // Every place must resolve: its fields, its country, its zone, its point.
        let resolves = |f: [&str; 8]| {
            world.country(f[3]).is_some()
                && f[7].parse::<usize>().is_ok_and(|z| z < world.zones.len())
                && f[5].parse::<f64>().is_ok()
                && f[6].parse::<f64>().is_ok()
        };
        if let Some(i) = (0..world.rows.len()).find(|&i| !fields(world.line(i)).is_some_and(resolves)) {
            return Err(bad(world.line(i)));
        }
        Ok(world)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Places of a million people or more come before Kemenag's places in
    /// mixed results; smaller ones after.
    pub fn is_big(&self, order: usize) -> bool {
        order < self.big
    }

    fn line(&self, i: usize) -> &str {
        let (a, b) = self.rows[i].line;
        &self.text[a as usize..b as usize]
    }

    /// Row `i` as a place.
    pub fn place(&self, i: usize) -> Option<Place> {
        let [_, name, _key, cc, region, lat_s, lng_s, zone] = fields(self.line(i))?;
        let country = self.country(cc)?;
        let clock = self.zones.get(zone.parse::<usize>().ok()?)?.clone();
        let (lat, lng) = (lat_s.parse().ok()?, lng_s.parse().ok()?);
        Some(Place {
            id: format!("{ID_PREFIX}{lat_s},{lng_s}"),
            label: name.to_string(),
            kind: "world".into(),
            prov: if region.is_empty() { country.name.clone() } else { format!("{region}, {}", country.name) },
            lat,
            lng,
            tz: String::new(),
            h0: super::default_h0(),
            subuh_angle: super::KEMENAG_SUBUH_ANGLE,
            isya_angle: super::KEMENAG_ISYA_ANGLE,
            method: country.method,
            hanafi: country.hanafi,
            clock: Some(clock),
            world_lines: Some(self.excerpt(i)?),
        })
    }

    /// Row `i` alone, as a file of its own: its clock, its country and its
    /// P line, what settings keep of a place picked or found.
    pub fn excerpt(&self, i: usize) -> Option<String> {
        let [_, name, key, cc, region, lat, lng, zone] = fields(self.line(i))?;
        let zone = self.text.lines().find(|l| l.starts_with(&format!("Z\t{zone}\t")))?;
        let zone = zone.splitn(3, '\t').nth(2)?;
        let country = self.text.lines().find(|l| l.starts_with(&format!("C\t{cc}\t")))?;
        let big = u8::from(self.is_big(i));
        Some(format!("Z\t0\t{zone}\n{country}\nB\t{big}\nP\t{name}\t{key}\t{cc}\t{region}\t{lat}\t{lng}\t0\n"))
    }

    fn country(&self, cc: &str) -> Option<&Country> {
        let code: [u8; 2] = cc.as_bytes().try_into().ok()?;
        self.countries.binary_search_by_key(&code, |c| c.code).ok().map(|i| &self.countries[i])
    }

    /// The place whose id is `id` ("w:lat,lng"), if it's in this file.
    pub fn find(&self, id: &str) -> Option<Place> {
        let point = id.strip_prefix(ID_PREFIX)?;
        let wanted = format!("\t{}\t", point.replace(',', "\t"));
        (0..self.rows.len()).find(|&i| self.line(i).contains(&wanted)).and_then(|i| self.place(i))
    }

    /// The place nearest a point, and how far it is (km).
    pub fn nearest(&self, lat: f64, lng: f64) -> Option<(Place, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for i in 0..self.rows.len() {
            let mut f = self.line(i).split('\t').skip(5);
            let (Some(Ok(plat)), Some(Ok(plng))) = (f.next().map(str::parse::<f64>), f.next().map(str::parse::<f64>)) else { continue };
            // Cheap reject before the trigonometry: a degree of latitude is 111 km.
            if best.is_some_and(|(_, d)| (plat - lat).abs() * 111.0 > d) {
                continue;
            }
            let d = distance_km(lat, lng, plat, plng);
            if best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.and_then(|(i, d)| Some((self.place(i)?, d)))
    }

    /// Places matching what was typed (folded with [`fold`]), best first, at
    /// most `limit`: the whole name, then a word of it starting with the
    /// query, then the query anywhere in it; the bigger place first.
    pub fn search(&self, folded: &str, limit: usize) -> Vec<Hit> {
        if folded.is_empty() || limit == 0 {
            return Vec::new();
        }
        // The rows run from the most people down, so each rank's first
        // `limit` matches are its best: no sorting.
        let mut by_rank: [Vec<usize>; 3] = Default::default();
        let mut lowered = String::new();
        for (order, row) in self.rows.iter().enumerate() {
            let (a, b) = row.key;
            let mut key = &self.text[a as usize..b as usize];
            if row.lowercase && !key.bytes().all(|c| c.is_ascii_lowercase() || c == b' ') {
                lowered.clear();
                lowered.push_str(key);
                lowered.make_ascii_lowercase();
                key = &lowered;
            }
            let Some(rank) = match_rank(key, folded) else { continue };
            let found = &mut by_rank[rank as usize];
            if found.len() < limit {
                found.push(order);
            }
            if by_rank[0].len() == limit {
                break;
            }
        }
        let hits = by_rank.iter().zip(0u8..).flat_map(|(found, rank)| found.iter().map(move |&order| Hit { rank, order }));
        hits.take(limit).collect()
    }

    /// The places.json ids whose other spellings match what was typed, with
    /// their rank ("mecca" finds Makkah).
    pub fn aliases(&self, folded: &str) -> Vec<(u8, &str)> {
        let mut out: Vec<(u8, &str)> = Vec::new();
        for (name, id) in &self.aliases {
            if let Some(rank) = match_rank(name, folded) {
                match out.iter_mut().find(|(_, i)| *i == id.as_str()) {
                    Some(hit) => hit.0 = hit.0.min(rank),
                    None => out.push((rank, id)),
                }
            }
        }
        out
    }
}

/// A few lines of the "ed" file (its zones' changes up to 2030) for the
/// apps' self-tests and pictures: the places "edin" finds.
pub const SAMPLE: &str = "\
Z\t0\tEurope/London\t0\t29579100=60 29881500=0 30103260=60 30415740=0 30627420=60 30939900=0 31151580=60 31464060=0 31685820=60 31988220=0
Z\t1\tAmerica/Chicago\t-360\t29549280=-300 29891940=-360 30083520=-300 30426180=-360 30607680=-300 30950340=-360 31131840=-300 31474500=-360 31656000=-300 31998660=-360
Z\t2\tEurope/Chisinau\t120\t29579100=180 29881500=120 30103260=180 30415740=120 30627420=180 30939900=120 31151580=180 31464060=120 31685820=180 31988220=120
C\tGB\tUnited Kingdom\tmoonsighting\ts
C\tMD\tMoldova\tmwl\ts
C\tUS\tUnited States\tmoonsighting\ts
B\t0
P\tEdinburgh\t\tGB\tScotland\t55.952\t-3.196\t0
P\tEdinburg\t\tUS\tTexas\t26.302\t-98.163\t1
P\tEdina\t\tUS\tMinnesota\t44.890\t-93.350\t1
P\tEdine\u{163}\tedinet\tMD\t\t48.173\t27.301\t2
";

/// A P line's eight fields.
fn fields(line: &str) -> Option<[&str; 8]> {
    let mut f = line.split('\t');
    let out = [(); 8].map(|_| f.next());
    if f.next().is_some() {
        return None;
    }
    let [a, b, c, d, e, g, h, i] = out;
    Some([a?, b?, c?, d?, e?, g?, h?, i?])
}

/// 0: the whole name; 1: a word of the name starts with the query; 2: the
/// query is anywhere in it.
pub fn match_rank(name: &str, q: &str) -> Option<u8> {
    if name == q {
        Some(0)
    } else if name.starts_with(q) || name.contains(&format!(" {q}")) {
        Some(1)
    } else if name.contains(q) {
        Some(2)
    } else {
        None
    }
}

/// What was typed, as the list's search keys hold names: lowercase ASCII
/// words, accents taken off ("Düsseldorf" -> "dusseldorf"), apostrophes
/// dropped and other marks read as spaces, like letters without a plain
/// Latin form ("ə", other scripts). Mirrors `fold` in
/// tools/make_world_places.py.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        // Apostrophes, and marks typed apart from their letter ("z\u{327}").
        if matches!(c, '\'' | '\u{2019}' | '\u{2018}' | '\u{2bb}' | '\u{2bc}' | '\u{2bf}' | '\u{2be}' | '`' | '\u{b4}' | '\u{300}'..='\u{36f}') {
            continue;
        }
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii() {
            out.push(' ');
        } else {
            match latin_plain(c) {
                Some(Latin::Letter(b)) => out.push(b as char),
                Some(Latin::Pair(p)) => out.push_str(p),
                Some(Latin::Mark) | None => out.push(' '),
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

enum Latin {
    Letter(u8),
    Pair(&'static str),
    /// Not a letter, or one without a plain form (read as a space).
    Mark,
}

/// A Latin letter with marks, plainly: one letter, or two for the ligatures.
fn latin_plain(c: char) -> Option<Latin> {
    let cp = c as u32;
    let b = match cp {
        0xC0..=0x24F => LATIN[(cp - 0xC0) as usize],
        0x1E00..=0x1EFF => LATIN_ADDITIONAL[(cp - 0x1E00) as usize],
        _ => return None,
    };
    Some(match b {
        b'_' => Latin::Mark,
        b'*' => Latin::Pair(match cp {
            0xC6 | 0xE6 => "ae",
            0xDE | 0xFE => "th",
            0xDF | 0x1E9E => "ss",
            0x132 | 0x133 => "ij",
            0x152 | 0x153 => "oe",
            0x1C4..=0x1C6 | 0x1F1..=0x1F3 => "dz",
            0x1C7..=0x1C9 => "lj",
            _ => "nj",
        }),
        b => Latin::Letter(b),
    })
}

/// U+00C0 to U+024F as tools/make_world_places.py folds them: a letter, `_`
/// for none, `*` for two (see [`latin_plain`]).
const LATIN: &[u8; 400] = b"\
aaaaaa*ceeeeiiiidnooooo_ouuuuy**aaaaaa*ceeeeiiiidnooooo_ouuuuy*y\
aaaaaaccccccccddddeeeeeeeeeegggggggghhhhiiiiiiiiii**jjkkklllllll\
lllnnnnnnn__oooooo**rrrrrrssssssssttttttuuuuuuuuuuuuwwyyyzzzzzzs\
________________________________oo_____________uu_______________\
____*********aaiioouuuuuuuuuu_aaaa____ggkkoooo__j***gg__nnaa____\
aaaaeeeeiiiioooorrrruuuusstt__hh______aaeeooooooooyy____________\
________________";

/// U+1E00 to U+1EFF (Vietnamese and other Latin letters with marks).
const LATIN_ADDITIONAL: &[u8; 256] = b"\
aabbbbbbccddddddddddeeeeeeeeeeffgghhhhhhhhhhiiiikkkkkkllllllllmm\
mmmmnnnnnnnnoooooooopppprrrrrrrrssssssssssttttttttuuuuuuuuuuvvvv\
wwwwwwwwwwxxxxyyzzzzzzhtwyas__*_aaaaaaaaaaaaaaaaaaaaaaaaeeeeeeee\
eeeeeeeeiiiioooooooooooooooooooooooouuuuuuuuuuuuuuyyyyyyyy______";
