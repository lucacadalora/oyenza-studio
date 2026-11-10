# The Caffeine add-on

**Caffeine** is an add-on that's built into the app, so there's nothing to download. It stays off until you switch it on in Add-ons (the "+" at the end of the tab row); then a **Caffeine** tab shows. Add what you drink, and the page shows how much caffeine is still in you, hour by hour, and the day's **last call**: the latest time your usual drink can go in and leave at most 35 mg in you at bedtime. The island reminds you 30 minutes before it.

Every number on the page is an average: for that kind of drink, and for an average body. It isn't a reading of your cup or of you, and it isn't medical advice. This page says how the add-on works, what each number is and where it comes from.

## Adding a drink

- **Pick the drink.** An espresso drink (Americano, Latte, Es kopi susu, Cappuccino, Espresso, Flat white, Mocha) is its shots: say how many shots went in, and the beans. The other drinks count servings of a fixed size: Kopi tubruk and Instant coffee by the spoon, Kopi sachet by the sachet, Filter coffee, Cold brew, Tea and Green tea by the cup, Energy drink by the bottle and Cola by the can. **Your own** takes a name and its mg, for anything else.
- **The beans:** Arabica, Blend, Robusta or Decaf. The picker starts with the beans you picked last.
- **How many:** up to 8 at a time. An espresso drink starts at 2 shots, what a Starbucks Tall Americano has (an Espresso starts at 1); the other drinks start at one serving.
- **When:** the time starts at now. Change it before you add the drink, or later by dragging the drink's dot on the chart.

A drink keeps the mg it was added with, so the numbers of a later version never change what you had.

## The chart

The chart runs from 06:00 to 06:00 the next morning, so a drink after midnight counts with the evening before. It adds up every drink of the last two weeks: last night's coffee is still in the morning's line. Each drink is a dot under the time axis, and the chart marks the day's peak and your bedtime, with how much is still in you then.

## Last call

Last call is the latest time today your usual drink can go in and leave at most 35 mg in you at bedtime. Your usual is the drink you had most often in the last two weeks, with its shots and beans; before your first drink, it's a 2-shot Arabica Americano, 150 mg.

- With bedtime at 23:00, a 2-shot Americano (150 mg) has its last call at about 12:07, and a single shot of arabica (75 mg) at about 17:07.
- The bigger the drink, the earlier its last call.
- Some of a drink leaves while the rest is still going in, so at a 5-hour half-life no more than about 85 % of it is in you at once. A drink of up to about 40 mg never reaches 35 mg, so it has no last call: a cup of Green tea (28 mg), a can of Cola (32 mg), a Kopi sachet (40 mg).
- A drink after its last call is late: more than 35 mg of it will still be in you at bedtime, and the page says so.

## The reminder

Thirty minutes before last call, the island says **Last call for coffee: 12:07**, and under it "After that, more than 35 mg is left at 23:00". It comes once a day, only while last call is still ahead, and not at all when your usual drink has no last call. You can switch it off in the Settings window's Caffeine section, where bedtime, the half-life and both limits are too.

## The day limit

The page warns once the day's drinks pass 400 mg, the most the [FDA](https://www.fda.gov/consumers/consumer-updates/spilling-beans-how-much-caffeine-too-much) and [EFSA](https://www.efsa.europa.eu/en/topics/topic/caffeine) give for a healthy adult in a day. EFSA's figure in pregnancy is 200 mg.

## Settings

The add-on's settings hold:

- **Bedtime:** 23:00, unless you pick another time from 20:00 to 03:00, in quarter hours. A bedtime after midnight is that night's.
- **Half-life:** how long your body takes to clear half the caffeine. About 5 hours on average, and it varies a lot from person to person, mostly with the liver enzyme CYP1A2: smoking shortens it, and pregnancy and the pill lengthen it ([Nehlig 2018](https://doi.org/10.1124/pr.117.014407), Pharmacological Reviews 70:384–411). It's 5 hours unless you pick another, from 2 to 15. A longer half-life moves last call earlier: at 8 hours, a 107 mg coffee's last call moves from 8.4 to 13.3 hours before bed.
- **The reminder:** on, unless you switch it off.
- **The limits:** 35 mg left at bedtime, and 400 mg a day.

## How the numbers work

### The curve

Each drink is worked out on its own with a simple model, then they're added up. Its caffeine goes in over the first hour or so and peaks about 70 minutes after the drink, then halves once every half-life. At the default 5 hours, three coffees of 95 mg at 08:00, 11:00 and 19:21 peak at 138 mg at about 11:58, and leave 92 mg in you at 23:00.

For each drink of `mg` milligrams, `t` hours after it went in:

```
in you = mg × ka / (ka − k) × (e^(−k·t) − e^(−ka·t))
```

`ka` is 2.7 an hour (the peak at about 70 minutes), and `k` is ln 2 divided by the half-life.

### Espresso: shots and beans

An espresso drink is its shots times the beans' mg per shot. A shot is about 30 ml.

| Beans | A shot | Range | Where it comes from |
|---|---|---|---|
| Arabica | 75 mg | 63 to 85 | Starbucks' own menu: its Signature espresso shot is [75 mg](https://www.starbucks.com/menu/product/410/hot), its Blonde shot [85](https://www.starbucks.com/menu/product/2122164/hot) and a ristretto shot [65](https://www.starbucks.com/menu/product/565/hot). USDA's restaurant espresso is [212 mg per 100 g](https://fdc.nal.usda.gov/food-details/171891/nutrients), 63 mg in a 1 fl oz shot. |
| Blend | 105 mg | 80 to 133 | A robusta-heavy blend, 70 % robusta and 30 % arabica, as Kopi Kenangan's house blend, [No. 7](https://kenanganglobal.com/id/), is. The range is that mix of the arabica and robusta ranges. |
| Robusta | 120 mg | 88 to 153 | Robusta has [1.4 to 1.8 times](https://doi.org/10.3390/foods10061208) arabica's caffeine: arabica's 63 to 85 mg, times that. |
| Decaf | 15 mg | 3 to 16 | Starbucks' decaf shots measured [3.0 to 15.8 mg](https://doi.org/10.1093/jat/30.8.611). The add-on takes the top, to be safe. |

**75 mg a shot, not 15.** A figure of 15 mg for a Starbucks shot goes around, and it's about five times too low: Starbucks publishes 75 mg for its Signature shot. 15 mg is Starbucks' [Short decaf brewed coffee](https://www.starbucks.com/menu/product/481/hot), about the most a decaf shot holds. It's also about half of the 29 mg an Indonesian lab study measured in a Starbucks Americano (Sukrasno 2018, below), where Starbucks itself says 150.

**How many shots?** At Starbucks, an Americano is 1, 2, 3 or 4 shots in a Short, Tall, Grande or Venti, [75 to 300 mg](https://www.starbucks.com/menu/product/406/hot). A [Latte](https://www.starbucks.com/menu/product/407/hot) or a [Cappuccino](https://www.starbucks.com/menu/product/409/hot) is 1, 1, 2 or 2 shots, and an iced Venti latte 3. Coffee chains in Indonesia don't publish how many shots go into their drinks, or their caffeine, so the shot count is yours to enter: ask at the counter.

### Other drinks

| Drink | One serving | mg | Where it comes from |
|---|---|---|---|
| Filter coffee | a 250 ml cup | 100 | USDA: [40 mg per 100 g](https://fdc.nal.usda.gov/food-details/171890/nutrients) of brewed coffee, 95 mg in 237 ml. |
| Cold brew | a 250 ml cup | 109 | Starbucks: [155, 205 and 310 mg](https://www.starbucks.com/menu/product/2121255/iced) in 355, 473 and 710 ml. |
| Kopi tubruk | a heaped teaspoon, about 5 g of ground coffee | 45 | Indonesian ground coffees measured 3.3 to 10 mg per gram, robusta at the top ([Sukrasno 2018](https://pubs.sciepub.com/jfnr/6/8/5/index.html)), and [BPOM](https://www.pom.go.id/penjelasan-publik/penjelasan-badan-pom-terkait-bahaya-kopi-instan) allows ground coffee at most 2 % caffeine. No study of kopi tubruk itself was found, so this one is an estimate. |
| Instant coffee | a teaspoon, about 2 g | 55 | USDA: [3,142 mg per 100 g](https://fdc.nal.usda.gov/food-details/171893/nutrients), 63 mg in 2 g. 2 g of Nescafé Classic measured 50 ([Sukrasno 2018](https://pubs.sciepub.com/jfnr/6/8/5/index.html)). |
| Kopi sachet | a 3-in-1 sachet | 40 | Six Indonesian sachets measured 13 to 63 mg, 37 the middle: Luwak White Koffie 40, Indocafe 39, Good Day Cappuccino 63 ([Sukrasno 2018](https://pubs.sciepub.com/jfnr/6/8/5/index.html)). |
| Tea | a cup | 47 | USDA's [black tea](https://fdc.nal.usda.gov/food-details/173227/nutrients). |
| Green tea | a cup | 28 | USDA's [green tea](https://fdc.nal.usda.gov/food-details/171917/nutrients). |
| Energy drink | a bottle, or a sachet | 50 | The labels of Indonesian energy drinks: Kratingdaeng (150 ml), [Extra Joss](https://dsld.od.nih.gov/label/34042) (a sachet) and Hemaviton (250 ml). Indonesia's standard for them, SNI 01-7152-2006, caps a serving at 50 mg. A can of Red Bull is [80 mg](https://www.redbull.com/id-id/energydrink/red-bull-energy-drink): add it as Your own. |
| Cola | a 330 ml can | 32 | Coca-Cola Indonesia's label: [24 mg per 250 ml](https://www.coca-cola.com/id/id/about-us/faq/mengapa-beberapa-minuman-coca-cola-mengandung-kafein). |
| Your own | one drink | the mg you type | Anything else, by a name and its mg. |

[Sukrasno 2018](https://pubs.sciepub.com/jfnr/6/8/5/index.html) is an Indonesian lab study that used a less precise method (thin-layer chromatography), and its numbers run low: it measured a Starbucks Americano at 29 mg, where Starbucks publishes 150. Read its figures as the least a drink holds.

### Why 35 mg at bedtime

[Gardiner et al. 2023](https://pubmed.ncbi.nlm.nih.gov/36870101/) (Sleep Medicine Reviews 69:101764) brought together 24 studies of caffeine and sleep. Its advice, to keep total sleep time from dropping: a 107 mg coffee at least 8.8 hours before bed, and a 217.5 mg drink at least 13.2 hours before. At a 5-hour half-life, both leave about 35 mg in the body at bedtime, so that's where the add-on draws its line: a drink is late when more than 35 mg of it would still be in you at bedtime. With the add-on's curve, the review's two drinks get their last calls 8.4 and 13.6 hours before bed, within 25 minutes of the review's own.

35 mg is a line drawn for an average body from those studies. It isn't a promise about your sleep.

## What is an estimate, and what isn't

**Published or measured**, each with its link above:

- Starbucks' shots, its Americano sizes and its cold brew, from Starbucks' own menu.
- USDA's espresso, filter coffee, instant coffee and teas.
- The labels: Indonesian energy drinks, Red Bull and Coca-Cola Indonesia.
- Indonesian sachets and Nescafé Classic, measured in a lab, with a method that reads low.
- The 35 mg line at bedtime, from the 2023 review, and the 400 mg day, from the FDA and EFSA.

**Estimates:**

- Every cup is an average for its kind, and real ones vary a lot: 104 espressos bought in cafés measured [48 to 317 mg](https://doi.org/10.1039/c4fo00290c) a serving.
- Blend and Robusta are worked out from arabica's caffeine and robusta's ratio to it, not measured in a cup.
- Kopi tubruk's 45 mg a spoon is worked out from ground coffee's caffeine.
- Your body. The curve uses the half-life in the settings, 5 hours unless you change it, and two people can clear the same coffee hours apart.

**Not published, as far as we could find:**

- Coffee chains in Indonesia (Kopi Kenangan, Fore, Tomoro, Janji Jiwa, Kopi Nako, Flash Coffee) don't publish how many shots go into a drink, their cup sizes or their caffeine, and we found no lab test of their drinks by name. So the add-on has no chain drinks: you pick the kind of drink and enter its shots.
- Kapal Api Special Mix, Teh Botol Sosro and Teh Pucuk don't give their caffeine on the label.
- One small study of es kopi susu bought in Bekasi, from shops it doesn't name, measured less caffeine than their shots would give. Why is still open, so the add-on goes by the shots.

## Your drinks stay on your computer

Your drinks and the add-on's settings stay on your computer, and nothing about them is sent anywhere: the add-on makes no request at all.

- **Drinks:** `data.json`, under `caffeine`, in `%APPDATA%\Poni\` on Windows and `~/Library/Application Support/Poni/` on a Mac. For each drink: its kind, its shots or servings, the beans, the time, its mg and, for Your own, the name you typed. It keeps the last 14 days, at most 400 drinks; older ones are dropped as new ones come in.
- **Settings:** `settings.json` in the same folder, under `caffeine`: `bedtime` (minutes after midnight), `half_life` (hours), `bed_limit_mg`, `day_limit_mg`, `remind`, and `bean`, the beans the picker starts with.
- **Switched off**, there's no Caffeine tab and no reminder, and nothing of the add-on runs. The drinks stay in `data.json`.

## For contributors

- **The numbers and rules** are in `crates/poni-core/src/caffeine.rs`: the mg per shot (`Bean::mg_per_shot`) and per serving (`Kind::mg_each`), the curve (`in_body`, `KA`), last call (`last_call_hours`, `Today`), the reminder's time and words (`reminder_at`, `reminder_text`), the drinks (`Log`, kept for `KEEP_DAYS`) and the settings (`Prefs`, kept in range by `Prefs::sane`). `cargo test -p poni-core caffeine` runs its tests; `last_call_matches_the_2023_review` holds the 35 mg line to the review's two drinks.
- **The page** is `crates/poni/src/pages/caffeine.rs`, a tab only while the add-on is on (`AppState::caffeine_on`).
- **The reminder** is a notice in `tick` at `caffeine::reminder_at`, nothing running in between.
- **Times** are local wall-clock minutes (`LocalMinute`), so nothing depends on the time zone. The page's day starts at 06:00 (`DAY_START`).
- **A drink keeps its mg** (`Drink::mg`), fixed when it was added: changing a table never rewrites what someone had. A kind that a newer version wrote reads as "Drink", with its mg (`Kind::Other`).
- **A number changes only with a source:** put it in the tables above with its link, and in the comment beside it in `caffeine.rs`.
