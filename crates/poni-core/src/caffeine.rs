//! Caffeine (the Caffeine add-on): what the person drank, how much of it is
//! still in them hour by hour, and the last time a drink can go in and be
//! mostly gone by bedtime.
//!
//! The curve is one compartment with first-order absorption: a drink reaches
//! its highest level about 70 minutes after it goes in ([`KA`]), then halves
//! once every half-life (about 5 hours for a healthy adult, 2 to 10 hours is
//! normal; smoking shortens it, pregnancy and the pill lengthen it).
//!
//! Last call follows Gardiner et al. 2023 (Sleep Medicine Reviews 69:101764,
//! 24 studies): to keep total sleep time from dropping, a 107 mg coffee goes
//! in at least 8.8 hours before bed and a 217.5 mg drink at least 13.2 hours
//! before. At a 5-hour half-life both leave about 35 mg at bedtime, so a drink
//! is late when more than [`Prefs::bed_limit_mg`] of it would still be in the
//! person at bedtime. That puts the two at 8.4 and 13.6 hours, within 25
//! minutes of the review's.
//!
//! An espresso drink is its shots: the person picks the drink and how many
//! shots, and the beans give the mg per shot ([`Bean::mg_per_shot`]). Other
//! drinks count servings of a fixed size ([`Kind::mg_each`]).
//!
//! Drinks live in data.json (`store.caffeine`) for [`KEEP_DAYS`] days, on the
//! computer only. Times are local wall-clock minutes ([`LocalMinute`]): a
//! drink keeps the clock time it was had at, so after a change of time zone
//! the day's drinks sit by their old clock.

use crate::datetime::{days_from_civil, LocalDateTime};
use serde::{Deserialize, Serialize};

/// Absorption rate per hour: a drink peaks about 70 minutes after it goes in
/// (`tmax` at a 5-hour half-life), the middle of the 30 to 120 minutes
/// measured for coffee.
pub const KA: f64 = 2.7;
/// Days of drinks kept in data.json.
pub const KEEP_DAYS: i64 = 14;
/// The most drinks kept, whatever their days.
pub const MAX_DRINKS: usize = 400;
/// The page's day runs from 06:00 to 06:00 the next morning, so a drink
/// after midnight counts with the evening before it.
pub const DAY_START: i64 = 6 * 60;
/// Minutes in a day.
pub const DAY: i64 = 24 * 60;
/// The reminder shows this many minutes before last call.
pub const REMIND_BEFORE: i64 = 30;
/// The most of one thing in one go (shots or servings).
pub const MAX_AMOUNT: u8 = 8;

/// Local wall-clock minutes since 1970-01-01 00:00.
pub type LocalMinute = i64;

/// `t` as [`LocalMinute`]s.
pub fn local_minute(t: &LocalDateTime) -> LocalMinute {
    days_from_civil(t.year, t.month, t.day) * DAY + t.minute_of_day() as i64
}

/// Hour and minute of a [`LocalMinute`].
pub fn clock(m: LocalMinute) -> (u8, u8) {
    let d = m.rem_euclid(DAY);
    ((d / 60) as u8, (d % 60) as u8)
}

/// `HH:MM` (24-hour) of a [`LocalMinute`].
pub fn hhmm(m: LocalMinute) -> String {
    let (h, mi) = clock(m);
    format!("{h:02}:{mi:02}")
}

/// The start (06:00) of the page's day that holds `now`.
pub fn day_start(now: LocalMinute) -> LocalMinute {
    (now - DAY_START).div_euclid(DAY) * DAY + DAY_START
}

/// Bedtime in the page's day that starts at `start`: `bedtime` minutes after
/// midnight, the next calendar day when it is before 06:00 (00:30 is after
/// midnight, not that morning).
pub fn bed_of(start: LocalMinute, bedtime: u16) -> LocalMinute {
    let b = bedtime as i64 % DAY;
    let midnight = start - DAY_START;
    if b < DAY_START { midnight + DAY + b } else { midnight + b }
}

/// Elimination rate per hour for a half-life in hours.
pub fn k_of(half_life: f64) -> f64 {
    std::f64::consts::LN_2 / half_life.max(0.5)
}

/// Hours from a drink going in to its highest level.
pub fn tmax(k: f64) -> f64 {
    (KA / k).ln() / (KA - k)
}

/// Milligrams of a drink of `mg` still in the body `hours` after it went in.
pub fn in_body(mg: f64, hours: f64, k: f64) -> f64 {
    if hours <= 0.0 {
        return 0.0;
    }
    mg * KA / (KA - k) * ((-k * hours).exp() - (-KA * hours).exp())
}

/// How fresh a drink is for the dots: 1 while it is still going in, then
/// fading to 0 over the next hour or so.
pub fn freshness(hours: f64, k: f64) -> f64 {
    let tm = tmax(k);
    if hours <= 0.0 {
        0.0
    } else if hours <= tm {
        1.0
    } else {
        (-(hours - tm) / 0.9).exp()
    }
}

/// Hours before bedtime a drink of `mg` must go in to leave at most `limit`
/// mg at bedtime; `None` when it never leaves that much.
pub fn last_call_hours(mg: f64, k: f64, limit: f64) -> Option<f64> {
    let tm = tmax(k);
    if in_body(mg, tm, k) <= limit {
        return None;
    }
    // `in_body` falls from `tm` on: halve the gap until it is a few seconds.
    let (mut lo, mut hi) = (tm, 96.0);
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if in_body(mg, mid, k) > limit { lo = mid } else { hi = mid }
    }
    Some(hi)
}

/// The beans of an espresso shot (about 30 ml).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bean {
    /// Starbucks' own figure for its shots, the middle of 63 to 85 mg
    /// (USDA's 63 mg for a 1 oz shot, Starbucks' 75 and 85 for Blonde).
    #[default]
    Arabica,
    /// A robusta-heavy blend, as many Indonesian chains pull (Kopi
    /// Kenangan's house blend is 70 % robusta): 80 to 133 mg.
    Blend,
    /// All robusta, 1.4 to 1.8 times arabica's caffeine: 88 to 153 mg.
    Robusta,
    /// 3 to 16 mg measured in decaf shots; the top of that, to be safe.
    Decaf,
    /// Beans a newer version wrote: counted as Arabica, so a file from a
    /// newer version still loads.
    #[serde(other)]
    Other,
}

impl Bean {
    pub const ALL: [Bean; 4] = [Bean::Arabica, Bean::Blend, Bean::Robusta, Bean::Decaf];

    pub fn name(self) -> &'static str {
        match self {
            Bean::Arabica => "Arabica",
            Bean::Blend => "Blend",
            Bean::Robusta => "Robusta",
            Bean::Decaf => "Decaf",
            Bean::Other => "Arabica",
        }
    }

    /// Caffeine in one shot, mg.
    pub fn mg_per_shot(self) -> f64 {
        match self {
            Bean::Arabica | Bean::Other => 75.0,
            Bean::Blend => 105.0,
            Bean::Robusta => 120.0,
            Bean::Decaf => 15.0,
        }
    }
}

/// What a serving of a non-espresso drink is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Shot,
    Spoon,
    Sachet,
    Cup,
    Bottle,
    Can,
    /// A drink the person typed the mg of.
    Mg,
}

impl Unit {
    /// "1 shot", "2 shots", "3 spoons" …
    pub fn count(self, n: u8) -> String {
        let (one, many) = match self {
            Unit::Shot => ("shot", "shots"),
            Unit::Spoon => ("spoon", "spoons"),
            Unit::Sachet => ("sachet", "sachets"),
            Unit::Cup => ("cup", "cups"),
            Unit::Bottle => ("bottle", "bottles"),
            Unit::Can => ("can", "cans"),
            Unit::Mg => ("", ""),
        };
        if self == Unit::Mg {
            return String::new();
        }
        format!("{n} {}", if n == 1 { one } else { many })
    }
}

/// A kind of drink. The espresso drinks are their shots; the rest count
/// servings of a fixed size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Espresso,
    Americano,
    Latte,
    Cappuccino,
    FlatWhite,
    /// Es kopi susu: espresso, milk and palm sugar or syrup over ice.
    KopiSusu,
    Mocha,
    /// Filter coffee (pour-over, V60, drip), per 250 ml cup.
    Brewed,
    /// Cold brew, per 250 ml cup.
    ColdBrew,
    /// Kopi tubruk or any ground coffee brewed in the cup, per heaped teaspoon.
    Tubruk,
    /// Black instant coffee, per teaspoon (about 2 g).
    Instant,
    /// A 3-in-1 sachet (coffee, sugar, creamer).
    Sachet,
    /// Black tea, per cup.
    Tea,
    /// Green tea, per cup.
    GreenTea,
    /// An Indonesian energy drink (Kratingdaeng, Extra Joss, Hemaviton):
    /// 50 mg a serving, the limit their labels keep to.
    Energy,
    /// Cola, per 330 ml can.
    Cola,
    /// A drink with the mg the person typed.
    Custom,
    /// A kind a newer version wrote: shown by its mg.
    #[serde(other)]
    Other,
}

impl Kind {
    /// The kinds the Add a drink picker offers, in its order.
    pub const PICK: [Kind; 17] = [
        Kind::Americano, Kind::Latte, Kind::KopiSusu, Kind::Cappuccino, Kind::Espresso, Kind::FlatWhite,
        Kind::Mocha, Kind::Brewed, Kind::ColdBrew, Kind::Tubruk, Kind::Instant, Kind::Sachet,
        Kind::Tea, Kind::GreenTea, Kind::Energy, Kind::Cola, Kind::Custom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Espresso => "Espresso",
            Kind::Americano => "Americano",
            Kind::Latte => "Latte",
            Kind::Cappuccino => "Cappuccino",
            Kind::FlatWhite => "Flat white",
            Kind::KopiSusu => "Es kopi susu",
            Kind::Mocha => "Mocha",
            Kind::Brewed => "Filter coffee",
            Kind::ColdBrew => "Cold brew",
            Kind::Tubruk => "Kopi tubruk",
            Kind::Instant => "Instant coffee",
            Kind::Sachet => "Kopi sachet",
            Kind::Tea => "Tea",
            Kind::GreenTea => "Green tea",
            Kind::Energy => "Energy drink",
            Kind::Cola => "Cola",
            Kind::Custom => "Your own",
            Kind::Other => "Drink",
        }
    }

    /// True for the espresso drinks: they count shots, and their beans matter.
    pub fn shots(self) -> bool {
        matches!(self, Kind::Espresso | Kind::Americano | Kind::Latte | Kind::Cappuccino | Kind::FlatWhite | Kind::KopiSusu | Kind::Mocha)
    }

    pub fn unit(self) -> Unit {
        match self {
            k if k.shots() => Unit::Shot,
            Kind::Tubruk | Kind::Instant => Unit::Spoon,
            Kind::Sachet => Unit::Sachet,
            Kind::Brewed | Kind::ColdBrew | Kind::Tea | Kind::GreenTea => Unit::Cup,
            Kind::Energy => Unit::Bottle,
            Kind::Cola => Unit::Can,
            _ => Unit::Mg,
        }
    }

    /// How many the picker starts at: the shots in a Starbucks Tall (one for
    /// an espresso, two for an Americano) or in a regular Indonesian chain
    /// cup (two), one serving for the rest.
    pub fn default_amount(self) -> u8 {
        match self {
            Kind::Espresso => 1,
            k if k.shots() => 2,
            _ => 1,
        }
    }

    /// Caffeine in one serving of a drink that isn't counted in shots, mg.
    pub fn mg_each(self) -> f64 {
        match self {
            // USDA: 40 mg per 100 g of brewed coffee.
            Kind::Brewed => 100.0,
            // Starbucks' cold brew is 155, 205 and 310 mg in 355, 473 and 710 ml.
            Kind::ColdBrew => 109.0,
            // A heaped teaspoon, about 5 g of ground coffee: boiled ground
            // coffees measured 3.3 to 10 mg per gram in Indonesia (robusta at the
            // top), and BPOM allows at most 2 % caffeine (20 mg per gram).
            Kind::Tubruk => 45.0,
            // About 2 g of instant coffee: USDA's 31 mg per gram gives 63,
            // 2 g of Nescafé Classic measured 50 in Indonesia.
            Kind::Instant => 55.0,
            // 13 to 63 mg measured in six Indonesian 3-in-1 sachets (Luwak
            // White Koffie 40, Indocafe 39, Good Day Cappuccino 63), 37 the middle.
            Kind::Sachet => 40.0,
            // USDA: black tea 47 mg, green tea 28 mg a cup.
            Kind::Tea => 47.0,
            Kind::GreenTea => 28.0,
            Kind::Energy => 50.0,
            // Coca-Cola Indonesia's label: 24 mg per 250 ml.
            Kind::Cola => 32.0,
            _ => 0.0,
        }
    }

    /// Caffeine in `amount` of this drink made with `bean`, mg (0 for
    /// [`Kind::Custom`], whose mg the person types).
    pub fn mg(self, amount: u8, bean: Bean) -> f64 {
        let n = amount as f64;
        if self.shots() { n * bean.mg_per_shot() } else { n * self.mg_each() }
    }
}

/// One drink the person had.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drink {
    /// Its id in the log (stays the same while the drink is moved).
    pub id: u32,
    /// When it went in.
    pub at: LocalMinute,
    pub kind: Kind,
    /// Shots for an espresso drink, servings for the rest (1 for Custom).
    pub amount: u8,
    /// The beans of an espresso drink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bean: Option<Bean>,
    /// Caffeine in it, mg, fixed when it was added: a later change to the
    /// mg tables never rewrites what the person had.
    pub mg: f64,
    /// A custom drink's name, as the person typed it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl Drink {
    /// What the list calls it: "Americano", or a custom drink's own name.
    pub fn title(&self) -> &str {
        if self.kind == Kind::Custom && !self.name.trim().is_empty() { self.name.trim() } else { self.kind.name() }
    }

    /// The line under it: "2 shots · Blend", "3 spoons", "" for Custom.
    pub fn detail(&self) -> String {
        let count = self.kind.unit().count(self.amount);
        match (self.kind.shots(), self.bean) {
            (true, Some(b)) => format!("{count} · {}", b.name()),
            _ => count,
        }
    }
}

/// What the person usually has: last call and the reminder are worked out for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Usual {
    pub kind: Kind,
    pub amount: u8,
    pub bean: Option<Bean>,
    pub mg: f64,
    /// "a 2-shot Americano", for the reminder's sentence.
    pub phrase: String,
}

/// The drinks in data.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Log {
    /// Oldest first.
    pub drinks: Vec<Drink>,
    /// The id the next drink gets.
    pub next_id: u32,
}

impl Log {
    /// Adds a drink (its `id` is set here) and returns its id.
    pub fn add(&mut self, mut d: Drink) -> u32 {
        self.next_id = self.next_id.max(self.drinks.iter().map(|d| d.id.saturating_add(1)).max().unwrap_or(0));
        if self.next_id >= u32::MAX - 1 {
            // Ids from a hand-edited file near the top: number them again.
            for (i, x) in self.drinks.iter_mut().enumerate() {
                x.id = i as u32;
            }
            self.next_id = self.drinks.len() as u32;
        }
        d.id = self.next_id;
        d.amount = d.amount.clamp(1, MAX_AMOUNT);
        d.mg = d.mg.clamp(0.0, 2000.0);
        self.next_id += 1;
        let at = self.drinks.partition_point(|x| x.at <= d.at);
        self.drinks.insert(at, d);
        self.next_id - 1
    }

    pub fn get(&self, id: u32) -> Option<&Drink> {
        self.drinks.iter().find(|d| d.id == id)
    }

    /// Removes a drink; false when there is none with that id.
    pub fn remove(&mut self, id: u32) -> bool {
        let n = self.drinks.len();
        self.drinks.retain(|d| d.id != id);
        self.drinks.len() != n
    }

    /// Moves a drink to `at`, keeping the list in time order.
    pub fn move_to(&mut self, id: u32, at: LocalMinute) -> bool {
        let Some(i) = self.drinks.iter().position(|d| d.id == id) else { return false };
        let mut d = self.drinks.remove(i);
        d.at = at;
        let j = self.drinks.partition_point(|x| x.at <= at);
        self.drinks.insert(j, d);
        true
    }

    /// Drops drinks older than [`KEEP_DAYS`] (and any past [`MAX_DRINKS`]),
    /// then [`Log::mend`]s the rest; true when a drink went. Call it with
    /// the real time only.
    pub fn tidy(&mut self, now: LocalMinute) -> bool {
        let keep = day_start(now) - KEEP_DAYS * DAY;
        let n = self.drinks.len();
        self.drinks.retain(|d| d.at >= keep);
        self.mend();
        self.drinks.len() != n
    }

    /// What a loaded file needs before use: drops entries that can't be drawn,
    /// sorts by time, keeps [`MAX_DRINKS`] at most and gives every drink its
    /// own id. Drops nothing for age, so it is safe before the clock is known.
    pub fn mend(&mut self) {
        self.drinks.retain(|d| d.mg.is_finite() && d.mg >= 0.0);
        self.drinks.sort_by_key(|d| d.at);
        if self.drinks.len() > MAX_DRINKS {
            let cut = self.drinks.len() - MAX_DRINKS;
            self.drinks.drain(..cut);
        }
        let mut seen = std::collections::BTreeSet::new();
        for d in &mut self.drinks {
            if !seen.insert(d.id) {
                d.id = u32::MAX;
            }
        }
        let mut next = self.drinks.iter().filter(|d| d.id != u32::MAX).map(|d| d.id.saturating_add(1)).max().unwrap_or(0).max(self.next_id).min(u32::MAX - 1 - self.drinks.len() as u32);
        for d in &mut self.drinks {
            if d.id == u32::MAX {
                d.id = next;
                next += 1;
            }
        }
        self.next_id = next;
    }

    /// The drinks of the page's day that starts at `start`.
    pub fn day(&self, start: LocalMinute) -> impl Iterator<Item = &Drink> {
        self.drinks.iter().filter(move |d| d.at >= start && d.at < start + DAY)
    }

    /// Milligrams in the body at `t` from every drink kept (yesterday's
    /// evening coffee still counts in the morning).
    pub fn in_body_at(&self, t: LocalMinute, k: f64) -> f64 {
        self.drinks.iter().fold(0.0, |s, d| s + in_body(d.mg, (t - d.at) as f64 / 60.0, k))
    }

    /// The highest point of the day that starts at `start`, to the minute:
    /// `(when, mg)`, or `None` when nothing is in the body all day.
    pub fn peak(&self, start: LocalMinute, k: f64) -> Option<(LocalMinute, f64)> {
        let mut best = (start, 0.0);
        for t in start..start + DAY {
            let v = self.in_body_at(t, k);
            if v > best.1 {
                best = (t, v);
            }
        }
        (best.1 >= 1.0).then_some(best)
    }

    /// The drink the person has most often in the last [`KEEP_DAYS`] days
    /// (the latest of the tied ones); a 2-shot arabica Americano before the
    /// first drink.
    pub fn usual(&self) -> Usual {
        let mut best: Option<(&Drink, usize)> = None;
        for d in self.drinks.iter().rev() {
            let n = self.drinks.iter().filter(|x| same(x, d)).count();
            if best.is_none_or(|(_, m)| n > m) {
                best = Some((d, n));
            }
        }
        match best {
            Some((d, _)) => {
                let phrase = phrase(d);
                Usual { kind: d.kind, amount: d.amount, bean: d.bean, mg: d.mg, phrase }
            }
            None => {
                let mg = Kind::Americano.mg(2, Bean::Arabica);
                Usual { kind: Kind::Americano, amount: 2, bean: Some(Bean::Arabica), mg, phrase: "a 2-shot Americano".into() }
            }
        }
    }
}

/// The usual drink in a sentence: "a 2-shot Americano", "a 2-shot es kopi
/// susu", "a kopi sachet", "2 cups of tea", "an energy drink".
fn phrase(d: &Drink) -> String {
    let name = match d.kind {
        Kind::Americano => d.kind.name().to_string(),
        k => k.name().to_lowercase(),
    };
    let n = d.amount;
    match d.kind {
        k if k.shots() => format!("{} {n}-shot {name}", if n == 8 { "an" } else { "a" }),
        Kind::Custom | Kind::Other => format!("{} ({:.0} mg)", d.title(), d.mg),
        Kind::Sachet | Kind::Energy if n == 1 => format!("{} {name}", if name.starts_with('e') { "an" } else { "a" }),
        Kind::Sachet | Kind::Energy => format!("{n} {name}s"),
        _ if n == 1 => format!("a {} of {name}", d.kind.unit().count(1).trim_start_matches("1 ")),
        _ => format!("{} of {name}", d.kind.unit().count(n)),
    }
}

fn same(a: &Drink, b: &Drink) -> bool {
    a.kind == b.kind && a.amount == b.amount && a.bean == b.bean && (a.kind != Kind::Custom || a.name == b.name)
}

/// The add-on's settings (`settings.caffeine`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Bedtime, minutes after midnight (before 06:00 = after midnight).
    pub bedtime: u16,
    /// Hours for half the caffeine to leave the body.
    pub half_life: f32,
    /// A drink is late when more than this much of it is left at bedtime, mg.
    pub bed_limit_mg: u16,
    /// The day's total the page warns past, mg.
    pub day_limit_mg: u16,
    /// Show the last call reminder in the island.
    pub remind: bool,
    /// The beans the picker starts with for espresso drinks (the last picked).
    pub bean: Bean,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { bedtime: 23 * 60, half_life: 5.0, bed_limit_mg: 35, day_limit_mg: 400, remind: true, bean: Bean::Arabica }
    }
}

/// Bedtimes the settings offer: 20:00 to 03:00 in quarter hours.
pub const BEDTIMES: std::ops::RangeInclusive<u16> = (20 * 60)..=(27 * 60);
/// Half-lives the settings offer, hours.
pub const HALF_LIVES: [f32; 15] = [2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 7.0, 8.0, 9.0, 10.0, 12.0, 15.0];

impl Prefs {
    /// The settings as they are used: in range, whatever data.json held.
    pub fn sane(&self) -> Prefs {
        let mut p = self.clone();
        p.bedtime %= 24 * 60;
        // 20:00 to 03:00 (`BEDTIMES`), the nearer end for a hand-edited time.
        if (3 * 60 + 1..20 * 60).contains(&p.bedtime) {
            p.bedtime = if p.bedtime < 11 * 60 + 30 { 3 * 60 } else { 20 * 60 };
        }
        p.half_life = if p.half_life.is_finite() { p.half_life.clamp(1.0, 24.0) } else { 5.0 };
        if p.bean == Bean::Other {
            p.bean = Bean::Arabica;
        }
        p.bed_limit_mg = p.bed_limit_mg.clamp(5, 200);
        p.day_limit_mg = p.day_limit_mg.clamp(50, 2000);
        p
    }

    pub fn k(&self) -> f64 {
        k_of(self.sane().half_life as f64)
    }
}

/// Today as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Today {
    /// 06:00 of the page's day.
    pub start: LocalMinute,
    pub bed: LocalMinute,
    /// The day's drinks, in time order.
    pub count: usize,
    /// Their caffeine together, mg.
    pub total_mg: f64,
    pub over_day_limit: bool,
    /// In the body at bedtime, mg (every drink kept, so yesterday's counts too).
    pub at_bed_mg: f64,
    pub peak: Option<(LocalMinute, f64)>,
    pub usual: Usual,
    /// Last call for the usual drink; `None` when it never leaves the
    /// bedtime limit. Before `start` when no time today is early enough.
    pub last_call: Option<LocalMinute>,
}

/// The day's last call without the rest of [`Today`] (no pass over every
/// minute): `(start, bed, usual, last_call)`.
pub fn last_call_of(log: &Log, prefs: &Prefs, now: LocalMinute) -> (LocalMinute, LocalMinute, Usual, Option<LocalMinute>) {
    let p = prefs.sane();
    let start = day_start(now);
    let bed = bed_of(start, p.bedtime);
    let usual = log.usual();
    let call = call_before(&p, bed, usual.mg);
    (start, bed, usual, call)
}

/// Last call for a drink of `mg` with bed at `bed`; `None` when it never
/// leaves the bedtime limit.
fn call_before(p: &Prefs, bed: LocalMinute, mg: f64) -> Option<LocalMinute> {
    last_call_hours(mg, p.k(), p.bed_limit_mg as f64).map(|h| bed - (h * 60.0).ceil() as i64)
}

/// True when a drink of `mg` at `at` is late for bed at `bed`: after its
/// own last call, or at or after bedtime itself. A drink taken just before
/// bed is late although little of it has gone in by then.
pub fn late_for_bed(prefs: &Prefs, bed: LocalMinute, mg: f64, at: LocalMinute) -> bool {
    let p = prefs.sane();
    if at >= bed {
        return mg > 0.0;
    }
    call_before(&p, bed, mg).is_some_and(|call| at > call)
}

impl Today {
    pub fn of(log: &Log, prefs: &Prefs, now: LocalMinute) -> Today {
        let p = prefs.sane();
        let k = p.k();
        let (start, bed, usual, last_call) = last_call_of(log, prefs, now);
        let (count, total_mg) = log.day(start).fold((0, 0.0), |(n, s), d| (n + 1, s + d.mg));
        Today {
            start,
            bed,
            count,
            total_mg,
            over_day_limit: total_mg > p.day_limit_mg as f64,
            at_bed_mg: log.in_body_at(bed, k),
            peak: log.peak(start, k),
            usual,
            last_call,
        }
    }

    /// True when a drink of `mg` at `at` is late for today's bed ([`late_for_bed`]).
    pub fn late(&self, prefs: &Prefs, mg: f64, at: LocalMinute) -> bool {
        late_for_bed(prefs, self.bed, mg, at)
    }
}

/// When the island shows today's last call reminder: [`REMIND_BEFORE`]
/// minutes before last call. It stays due until last call itself, so a
/// locked screen or a busy island at its minute only delays it; `None` once
/// last call has passed, while the reminder is off, and when the usual
/// drink has no last call.
pub fn reminder_at(log: &Log, prefs: &Prefs, now: LocalMinute) -> Option<LocalMinute> {
    if !prefs.remind {
        return None;
    }
    let (start, _, _, call) = last_call_of(log, prefs, now);
    let call = call?;
    let at = call - REMIND_BEFORE;
    (now < call && at >= start).then_some(at)
}

/// True for the drinks the reminder calls coffee.
fn coffee(kind: Kind) -> bool {
    kind.shots() || matches!(kind, Kind::Brewed | Kind::ColdBrew | Kind::Tubruk | Kind::Instant | Kind::Sachet)
}

/// The reminder's words: a title and the line under it (short enough for
/// the notice's one line, about 290 DIPs at 12).
pub fn reminder_text(log: &Log, prefs: &Prefs, now: LocalMinute) -> Option<(String, String)> {
    let p = prefs.sane();
    let (_, bed, usual, call) = last_call_of(log, prefs, now);
    let call = call?;
    Some((
        format!("Last call for {}: {}", if coffee(usual.kind) { "coffee" } else { "caffeine" }, hhmm(call)),
        format!("After that, more than {} mg is left at {}", p.bed_limit_mg, hhmm(bed)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const D0: LocalMinute = 20_402 * DAY; // 10 Nov 2025, 00:00

    fn at(h: i64, m: i64) -> LocalMinute {
        D0 + h * 60 + m
    }

    fn drink(t: LocalMinute, mg: f64) -> Drink {
        Drink { id: 0, at: t, kind: Kind::Custom, amount: 1, bean: None, mg, name: String::new() }
    }

    #[test]
    fn last_call_matches_the_2023_review() {
        // Gardiner et al.: 107 mg at least 8.8 h before bed, 217.5 mg at least 13.2 h.
        let k = k_of(5.0);
        let a = last_call_hours(107.0, k, 35.0).unwrap();
        let b = last_call_hours(217.5, k, 35.0).unwrap();
        assert!((a - 8.8).abs() < 25.0 / 60.0, "{a}");
        assert!((b - 13.2).abs() < 25.0 / 60.0, "{b}");
        // Green tea never leaves 35 mg: no last call.
        assert_eq!(last_call_hours(28.0, k, 35.0), None);
        // A slower body moves last call earlier.
        assert!(last_call_hours(107.0, k_of(8.0), 35.0).unwrap() > a + 3.0);
    }

    #[test]
    fn the_three_cup_day() {
        // The chart this add-on started from: 3 x 95 mg at 08:00, 11:00, 19:21.
        let mut log = Log::default();
        for t in [at(8, 0), at(11, 0), at(19, 21)] {
            log.add(drink(t, 95.0));
        }
        let k = k_of(5.0);
        let (when, mg) = log.peak(day_start(at(12, 0)), k).unwrap();
        assert!((when - at(11, 58)).abs() <= 2, "{}", hhmm(when));
        assert_eq!(mg.round(), 138.0);
        assert_eq!(log.in_body_at(at(23, 0), k).round(), 92.0);
    }

    #[test]
    fn shots_make_the_mg() {
        assert_eq!(Kind::Americano.mg(2, Bean::Arabica), 150.0); // a Starbucks Tall
        assert_eq!(Kind::Espresso.mg(1, Bean::Arabica), 75.0);
        assert_eq!(Kind::KopiSusu.mg(2, Bean::Blend), 210.0);
        assert_eq!(Kind::Tea.mg(2, Bean::Robusta), 94.0); // beans don't touch tea
        assert!(Kind::PICK.iter().all(|k| *k == Kind::Custom || k.mg(1, Bean::Arabica) > 0.0));
        assert!(Bean::ALL.windows(2).take(2).all(|w| w[0].mg_per_shot() < w[1].mg_per_shot()));
    }

    #[test]
    fn days_run_from_six_to_six() {
        assert_eq!(day_start(at(5, 59)), at(-18, 0));
        assert_eq!(day_start(at(6, 0)), at(6, 0));
        assert_eq!(day_start(at(23, 59)), at(6, 0));
        let s = at(6, 0);
        assert_eq!(bed_of(s, 23 * 60), at(23, 0));
        assert_eq!(bed_of(s, 30), at(24, 30)); // 00:30 is after midnight
        assert_eq!(hhmm(at(24, 30)), "00:30");
        let t = LocalDateTime { year: 2025, month: 11, day: 10, hour: 7, minute: 5, second: 0 };
        assert_eq!(local_minute(&t), at(7, 5));
    }

    #[test]
    fn the_log_keeps_order_ids_and_two_weeks() {
        let mut log = Log::default();
        let a = log.add(drink(at(11, 0), 75.0));
        let b = log.add(drink(at(8, 0), 75.0));
        assert_ne!(a, b);
        assert_eq!(log.drinks.iter().map(|d| d.id).collect::<Vec<_>>(), vec![b, a]);
        assert!(log.move_to(b, at(12, 0)));
        assert_eq!(log.drinks.iter().map(|d| d.id).collect::<Vec<_>>(), vec![a, b]);
        log.add(drink(at(9, 0) - 15 * DAY, 75.0));
        log.drinks.push(Drink { id: a, ..drink(at(13, 0), 30.0) }); // a duplicate id from a hand-edited file
        log.tidy(at(14, 0));
        assert_eq!(log.drinks.len(), 3);
        let ids: std::collections::BTreeSet<_> = log.drinks.iter().map(|d| d.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(log.drinks.windows(2).all(|w| w[0].at <= w[1].at));
        assert!(log.remove(a) && !log.remove(a));
        assert_eq!(log.day(at(6, 0)).count(), 2);
    }

    #[test]
    fn usual_is_the_most_often_had() {
        let mut log = Log::default();
        assert_eq!(log.usual().phrase, "a 2-shot Americano");
        let kopi = |t| Drink { id: 0, at: t, kind: Kind::KopiSusu, amount: 2, bean: Some(Bean::Blend), mg: 210.0, name: String::new() };
        log.add(kopi(at(9, 0) - DAY));
        log.add(kopi(at(9, 0)));
        log.add(Drink { kind: Kind::Tea, amount: 1, bean: None, mg: 47.0, ..kopi(at(16, 0)) });
        let u = log.usual();
        assert_eq!((u.kind, u.amount, u.mg), (Kind::KopiSusu, 2, 210.0));
        assert_eq!(u.phrase, "a 2-shot es kopi susu");
        let one = |kind, amount| phrase(&Drink { kind, amount, bean: None, ..kopi(0) });
        assert_eq!(one(Kind::Sachet, 1), "a kopi sachet");
        assert_eq!(one(Kind::Energy, 1), "an energy drink");
        assert_eq!(one(Kind::Energy, 2), "2 energy drinks");
        assert_eq!(one(Kind::Tea, 2), "2 cups of tea");
        assert_eq!(one(Kind::Cola, 1), "a can of cola");
        assert_eq!(one(Kind::Tubruk, 1), "a spoon of kopi tubruk");
        assert_eq!(one(Kind::Americano, 2), "a 2-shot Americano");
        assert_eq!(one(Kind::Latte, 8), "an 8-shot latte");
        // An empty log at any time is nothing in the body: 0, never -0.
        assert!(Log::default().in_body_at(0, k_of(5.0)).is_sign_positive());
    }

    #[test]
    fn today_and_the_reminder() {
        let mut log = Log::default();
        log.add(Drink { kind: Kind::Americano, amount: 2, bean: Some(Bean::Arabica), mg: 150.0, ..drink(at(8, 30), 0.0) });
        let prefs = Prefs::default();
        let t = Today::of(&log, &prefs, at(9, 0));
        assert_eq!((t.count, t.total_mg, t.over_day_limit), (1, 150.0, false));
        // 150 mg at a 23:00 bed: last call about 12:07.
        let call = t.last_call.unwrap();
        assert!((call - at(12, 7)).abs() <= 2, "{}", hhmm(call));
        assert_eq!(reminder_at(&log, &prefs, at(9, 0)), Some(call - REMIND_BEFORE));
        // Due until last call itself: a busy island only delays it.
        assert_eq!(reminder_at(&log, &prefs, call - 1), Some(call - REMIND_BEFORE));
        assert_eq!(reminder_at(&log, &prefs, call), None);
        assert_eq!(reminder_at(&log, &Prefs { remind: false, ..prefs.clone() }, at(9, 0)), None);
        assert!(t.late(&prefs, 150.0, at(15, 0)) && !t.late(&prefs, 150.0, at(11, 0)));
        // Just before bed little of an espresso has gone in, but it's late all
        // the same; anything at or after bedtime is.
        assert!(t.late(&prefs, 75.0, at(22, 50)) && t.late(&prefs, 28.0, at(23, 30)));
        assert!(!t.late(&prefs, 28.0, at(22, 50))); // green tea never leaves 35 mg
        let (title, line) = reminder_text(&log, &prefs, at(9, 0)).unwrap();
        assert_eq!(title, format!("Last call for coffee: {}", hhmm(call)));
        assert_eq!(line, "After that, more than 35 mg is left at 23:00");
        assert!(line.chars().count() <= 46, "the notice has one line of about 290 DIPs");
    }

    #[test]
    fn data_json_round_trips_and_survives_newer_kinds() {
        let mut log = Log::default();
        log.add(Drink { kind: Kind::Latte, amount: 2, bean: Some(Bean::Robusta), mg: 240.0, ..drink(at(10, 0), 0.0) });
        let s = serde_json::to_string(&log).unwrap();
        assert_eq!(serde_json::from_str::<Log>(&s).unwrap(), log);
        let newer = r#"{"drinks":[{"id":3,"at":1,"kind":"matcha_latte","amount":1,"mg":70.0}],"next_id":4}"#;
        let l: Log = serde_json::from_str(newer).unwrap();
        assert_eq!((l.drinks[0].kind, l.drinks[0].title()), (Kind::Other, "Drink"));
        let p: Prefs = serde_json::from_str(r#"{"bedtime":1410}"#).unwrap();
        assert_eq!((p.bedtime, p.half_life, p.remind), (1410, 5.0, true));
        // Beans from a newer version load, as Arabica.
        let l: Log = serde_json::from_str(r#"{"drinks":[{"id":1,"at":1,"kind":"latte","amount":2,"bean":"liberica","mg":150.0}],"next_id":2}"#).unwrap();
        assert_eq!(l.drinks[0].bean, Some(Bean::Other));
        let p: Prefs = serde_json::from_str(r#"{"bean":"liberica"}"#).unwrap();
        assert_eq!(p.sane().bean, Bean::Arabica);
        // A hand-edited bedtime stays in the evening or just after midnight.
        assert_eq!(Prefs { bedtime: 6 * 60, ..Prefs::default() }.sane().bedtime, 3 * 60);
        assert_eq!(Prefs { bedtime: 18 * 60, ..Prefs::default() }.sane().bedtime, 20 * 60);
        assert_eq!(Prefs { half_life: f32::NAN, bed_limit_mg: 0, ..Prefs::default() }.sane().half_life, 5.0);
    }
}
