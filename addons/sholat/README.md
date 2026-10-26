# Prayer times add-on

Prayer times is off by default. Switching it on in the Add-ons tab (the "+"
at the end of the tab row) downloads these files; switching it off deletes
them. The code that works out the times (`crates/poni-core/src/prayer/`) is
part of poni.exe and stays asleep until the add-on is on.

The files are published, byte for byte, on the pre-release
[`addon-sholat-2`](https://github.com/lucacadalora/oyenza-studio/releases/tag/addon-sholat-2).
Each one's size and SHA-256 are built into the app (`poni_core::addons`),
and `cargo test -p poni-core addons` checks that they match the files here.
Changing a file means a new tag (`addon-sholat-3`), new numbers in
`poni_core::addons`, and an app release. Files that didn't change move over
from the older version's folder on people's computers instead of downloading
again. [`addon-sholat-1`](https://github.com/lucacadalora/oyenza-studio/releases/tag/addon-sholat-1)
(no world list, the older NOTICE.txt) stays up for 0.1.17 to 0.2.8.

- `places.json`: the places people can pick. All 517 locations in Kemenag's
  prayer schedule (every kab/kota, plus five island points such as "Pulau
  Laut, Kab. Natuna"), named as Kemenag writes them. Each has the reference
  point and the Terbit and Maghrib horizon (`h0`) that reproduce Kemenag's
  own 2026 schedule with this engine, so the computed times land on
  Kemenag's minutes. Six Papua highland regencies carry Kemenag's wider
  twilight angles there (Subuh 21°, Isya 19°). Makkah, Madinah and Jeddah
  use Umm al-Qura. Settings keep the chosen place by its `id`, so never
  rename one.
- `world.tsv.zst`: everywhere else. GeoNames' cities of 15,000 people or
  more (33,542 of them in 243 countries; Indonesia left to `places.json`, and
  the places within 15 km of Makkah, Madinah and Jeddah), each with its
  country's prayer-time method, Hanafi Ashar where most mosques keep it, and
  its time zone's UTC offsets from 2026 to 2045, summer time included.
  `tools/make_world_places.py` builds it from the geonamescache package and
  Python's time zone data, and describes the format; 1.4 MB unpacked, 0.5 MB
  as downloaded. The app reads it only while the place picker is open, to
  find the city nearest a location, and once at start for a saved city
  abroad. Settings keep a city by its point (`w:55.952,-3.196`); if a newer
  list moves the point, the nearest city within 25 km takes its place.
- `adzan.m4a`: the Maghrib adzan at Masjidil Haram on 25 Feb 2012 (178 s,
  AAC), by 3omar Faruq, from Wikimedia Commons under CC BY 3.0, so the app
  shows its credit. Checked word by word: the whole adzan, ending with "la
  ilaha illallah", and no Subuh line, so it plays for the other four prayers.
  (The first recording, from 21 Jan 2013, stopped mid-way through the closing
  takbir.)
- `NOTICE.txt`: where each file comes from, its licence and what was changed.

## How the times are checked

`cargo test -p poni-core prayer` replays:

- Kemenag's published 2026 schedule for all 517 locations, 227,367 times:
  98.2% exact, never more than 1 minute off. Every place is at least 95%
  exact, and every miss sits within seconds of a minute boundary.
- Kanwil Kemenag Jawa Tengah's Ramadan 1447 H schedule for Kota Semarang,
  21 times, all exact.
- Oyen Umroh's prayer engine (adhan-js with the same rules) on 18,225
  place-days from 2026 to 2075, all identical to the minute.
- adhan-js 4.4.6 itself on 47 cities of the world list (every method, the
  UK past 55° north, the polar circle, the southern hemisphere), every 9
  days of 2026 and 2027 plus the solstices, equinoxes and summer-time
  changes: 4,278 days identical to the minute. On 46 polar winter days
  adhan-js puts Asr after Maghrib or before Dhuhr (the sun stays too low for
  its shadow); there the engine takes the nearest latitude, 0.5° at a time,
  whose day is in order.

The schedules came from api.myquran.com, which republishes Kemenag's
(bimasislam.kemenag.go.id). They are test data only: the add-on ships
places, never Kemenag's tables.
