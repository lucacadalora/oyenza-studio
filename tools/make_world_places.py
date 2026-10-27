#!/usr/bin/env python3
"""Build site/places/1/: the Prayer times picker's cities outside Indonesia,
looked up online at https://studio.oyenza.com/places/1/ (Kemenag's
places.json, in the add-on, covers Indonesia, Makkah, Madinah and Jeddah).

Cities: GeoNames' cities of 15,000 people or more (cities15000), as packed in
the geonamescache Python package, version 3.0.2:

    pip download --no-deps geonamescache==3.0.2
    unzip geonamescache-3.0.2-py3-none-any.whl 'geonamescache/data/*'

GeoNames data is CC BY 4.0 (https://www.geonames.org); every file says so,
and so does the picker. Time zones: the IANA time zone database through
Python's zoneinfo; each zone's UTC offsets are written out from 2026 to 2045,
summer time included.

    python3 -I tools/make_world_places.py <unzipped>/geonamescache/data

empties site/places/1/ and writes it again, then prints what it holds. Run it
again to pick up a newer tz database or city list; the website deploy puts
it live. The app never holds the whole list: it asks for one file at a time
and keeps only the place picked.

  <ab>.txt          the places with a word of their name starting with "ab"
                    (folded as `fold` below), for typing
  near/<la>_<lo>.txt  the places in the 5-degree square whose south-west
                    corner is la, lo (multiples of 5), for Use my location

Each file stands alone: UTF-8 lines, fields split by tabs, `#` lines are comments.
  Z <n> <IANA name> <minutes east of UTC from 2026> [<UTC minute>=<minutes east> ...]
  C <ISO code> <country> <method> <asr: s = Syafi'i, h = Hanafi>
  A <typed name> <id in places.json>
  B <how many of the first P lines have a million people or more>
  P <name> <search key> <ISO code> <region> <lat> <lng> <zone n>
Z lines are the file's own clocks, numbered from 0 (zones that keep the same
offsets share one); C lines run in code order. The search key is left empty
when it is the name in lowercase. P lines run from the most people to the
fewest. A place's id in settings is "w:" and its lat,lng as written here.
A new format goes in a new folder (places/2/), so copies that read this one
keep working.
"""

import datetime as dt
import json
import math
import shutil
import sys
import unicodedata
from pathlib import Path
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "site/places/1"
# The squares Use my location reads, in degrees.
TILE_DEG = 5
PLACES = ROOT / "addons/sholat/places.json"
FIRST_YEAR, END_YEAR = 2026, 2046

# The method each country's times follow: Adhan's own advice where it names
# countries (Umm al-Qura for Saudi Arabia, Dubai for the UAE, Qatar, Kuwait,
# Singapore for Singapore, Malaysia and Brunei, Diyanet for Turkey, Tehran
# for Iran, the Moonsighting Committee for the UK and North America), the
# PrayTimes regions for the rest (Karachi for Pakistan, Afghanistan,
# Bangladesh and India; Umm al-Qura for the Arabian Peninsula; the Egyptian
# authority for Africa, Syria and Lebanon), and the Muslim World League
# everywhere else.
METHOD_BY_COUNTRY = {
    **dict.fromkeys(["SA", "YE", "OM", "BH"], "ummalqura"),
    "AE": "dubai",
    "QA": "qatar",
    "KW": "kuwait",
    **dict.fromkeys(["SG", "MY", "BN"], "singapore"),
    "TR": "turkey",
    "IR": "tehran",
    **dict.fromkeys(["PK", "AF", "BD", "IN"], "karachi"),
    **dict.fromkeys(["GB", "US", "CA"], "moonsighting"),
    **dict.fromkeys(["EG", "SY", "LB"], "egyptian"),
}
# Ashar by the Hanafi shadow (twice the object) where most mosques keep it.
HANAFI = {"PK", "AF", "BD", "IN", "UZ", "TJ", "KZ", "KG", "TM"}

# Regions shown beside the country where one country has several places of
# the same name: GeoNames' first-level codes for these four.
REGIONS = {
    "GB": {"ENG": "England", "SCT": "Scotland", "WLS": "Wales", "NIR": "Northern Ireland"},
    "CA": {
        "01": "Alberta", "02": "British Columbia", "03": "Manitoba", "04": "New Brunswick",
        "05": "Newfoundland and Labrador", "07": "Nova Scotia", "08": "Ontario",
        "09": "Prince Edward Island", "10": "Quebec", "11": "Saskatchewan", "12": "Yukon",
        "13": "Northwest Territories", "14": "Nunavut",
    },
    "AU": {
        "01": "Australian Capital Territory", "02": "New South Wales", "03": "Northern Territory",
        "04": "Queensland", "05": "South Australia", "06": "Tasmania", "07": "Victoria",
        "08": "Western Australia",
    },
}

# Other spellings of the three Saudi places places.json already holds.
ALIASES = {
    "mecca": "makkah", "makka": "makkah", "mekkah": "makkah", "mekah": "makkah",
    "medina": "madinah", "madina": "madinah", "madinah al munawwarah": "madinah",
    "jiddah": "jeddah", "jedda": "jeddah", "jidda": "jeddah",
}
# GeoNames places this close to one of those three (km) are left out: the
# list already has them, with the times the Haram follows.
SAUDI_NEAR_KM = 15.0

# Letters NFKD doesn't take apart.
SPECIAL = {
    "ß": "ss", "ẞ": "ss", "æ": "ae", "Æ": "ae", "œ": "oe", "Œ": "oe", "ø": "o", "Ø": "o",
    "ł": "l", "Ł": "l", "đ": "d", "Đ": "d", "ð": "d", "Ð": "d", "þ": "th", "Þ": "th",
    "ı": "i", "ħ": "h", "Ħ": "h", "ŀ": "l", "Ŀ": "l", "ŧ": "t", "Ŧ": "t", "ĸ": "k", "ŉ": "n",
}
# Dropped rather than read as a space: "Nar'yan-Mar" -> "naryan mar".
APOSTROPHES = "'’‘ʻʼʿʾ`´"


def fold(s: str) -> str:
    """Lowercase ASCII words, as `poni_core::prayer::world::fold` folds what is typed."""
    out = []
    for ch in s:
        if ch in APOSTROPHES:
            continue
        for c in unicodedata.normalize("NFKD", SPECIAL.get(ch, ch)):
            if unicodedata.combining(c):
                continue
            out.append(c.lower() if c.isascii() and c.isalnum() else " ")
    return " ".join("".join(out).split())


def distance_km(lat1, lng1, lat2, lng2):
    r = math.radians
    a = math.sin(r(lat2 - lat1) / 2) ** 2 + math.cos(r(lat1)) * math.cos(r(lat2)) * math.sin(r(lng2 - lng1) / 2) ** 2
    return 2 * 6371.0088 * math.asin(min(1.0, math.sqrt(a)))


def offset_min(zone: ZoneInfo, t: dt.datetime) -> int:
    off = t.astimezone(zone).utcoffset()
    return int(off.total_seconds() // 60)


def zone_line(n: int, name: str) -> str:
    """The zone's offset at the start of FIRST_YEAR and every change until END_YEAR, to the minute."""
    zone = ZoneInfo(name)
    start = dt.datetime(FIRST_YEAR, 1, 1, tzinfo=dt.timezone.utc)
    end = dt.datetime(END_YEAR, 1, 1, tzinfo=dt.timezone.utc)
    first = offset_min(zone, start)
    changes = []
    day, last = start, first
    while day < end:
        nxt = day + dt.timedelta(hours=6)
        off = offset_min(zone, nxt)
        if off != last:
            lo, hi = day, nxt  # the change falls in (lo, hi]: find its minute
            while hi - lo > dt.timedelta(minutes=1):
                mid = lo + (hi - lo) / 2
                mid = mid.replace(second=0, microsecond=0)
                if offset_min(zone, mid) == last:
                    lo = mid
                else:
                    hi = mid
            changes.append(f"{int(hi.timestamp()) // 60}={off}")
            last = off
        day = nxt
    return "\t".join(["Z", str(n), name, str(first), " ".join(changes)])


def tile(lat: float, lng: float) -> str:
    return f"near/{math.floor(lat / TILE_DEG) * TILE_DEG}_{math.floor(lng / TILE_DEG) * TILE_DEG}"


HEADER = (
    "# Oyenza Studio, Prayer times. Cities: GeoNames (https://www.geonames.org), CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). "
    f"Clocks: the IANA time zone database, {FIRST_YEAR} to {END_YEAR - 1}. Built by tools/make_world_places.py."
)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    data = Path(sys.argv[1])
    cities = list(json.loads((data / "cities15000.json").read_text()).values())
    countries = json.loads((data / "countries.json").read_text())
    us_states = {k: v["name"] for k, v in json.loads((data / "us_states.json").read_text()).items()}
    REGIONS["US"] = us_states
    saudi = [(p["lat"], p["lng"]) for p in json.loads(PLACES.read_text())["places"] if p.get("method") == "ummalqura"]

    kept = []
    for c in cities:
        cc = c["countrycode"]
        if cc == "ID":
            continue
        if cc == "SA" and any(distance_km(c["latitude"], c["longitude"], lat, lng) < SAUDI_NEAR_KM for lat, lng in saudi):
            continue
        if cc not in countries:
            continue
        kept.append(c)
    kept.sort(key=lambda c: (-c["population"], c["geonameid"]))

    # Zones that keep the same offsets share a clock; a file names each of
    # its clocks after the zone of its biggest place on it.
    clock_of = {}
    for z in sorted({c["timezone"] for c in kept}):
        _, _, _, first, changes = zone_line(0, z).split("\t")
        clock_of[z] = (first, changes)

    rows, seen = [], set()
    for c in kept:
        name = " ".join(c["name"].split())
        key = fold(name)
        region = REGIONS.get(c["countrycode"], {}).get(c["admin1code"], "")
        lat, lng = f"{c['latitude']:.3f}", f"{c['longitude']:.3f}"
        # Two places on one point would share an id: the smaller one goes.
        if (lat, lng) in seen:
            continue
        seen.add((lat, lng))
        rows.append({"name": name, "key": key, "cc": c["countrycode"], "region": region, "lat": lat, "lng": lng,
                     "zone": c["timezone"], "clock": clock_of[c["timezone"]], "big": c["population"] >= 1_000_000})

    files: dict[str, list] = {}
    unindexed = 0
    for r in rows:
        heads = {w[:2] for w in r["key"].split() if len(w) >= 2}
        good = {h for h in heads if h.isascii() and h.isalnum()}
        unindexed += not good
        for h in good:
            files.setdefault(h, []).append(r)
        files.setdefault(tile(float(r["lat"]), float(r["lng"])), []).append(r)
    aliases: dict[str, list] = {}
    for typed, id_ in sorted(ALIASES.items()):
        for h in {w[:2] for w in typed.split() if len(w) >= 2}:
            aliases.setdefault(h, []).append((typed, id_))
            files.setdefault(h, [])

    if OUT.exists():
        shutil.rmtree(OUT)
    (OUT / "near").mkdir(parents=True)
    total = 0
    for name, members in sorted(files.items()):
        named = {}
        for r in members:
            named.setdefault(r["clock"], r["zone"])
        own = sorted(named, key=lambda k: named[k])
        n = {k: i for i, k in enumerate(own)}
        lines = [HEADER]
        lines += ["\t".join(["Z", str(i), named[k], k[0], k[1]]) for i, k in enumerate(own)]
        for cc in sorted({r["cc"] for r in members}):
            africa = countries[cc]["continentcode"] == "AF"
            method = METHOD_BY_COUNTRY.get(cc, "egyptian" if africa else "mwl")
            lines.append("\t".join(["C", cc, countries[cc]["name"], method, "h" if cc in HANAFI else "s"]))
        lines += ["\t".join(["A", typed, id_]) for typed, id_ in aliases.get(name, [])]
        lines.append("\t".join(["B", str(sum(1 for r in members if r["big"]))]))
        for r in members:
            key = "" if r["key"] == r["name"].lower() else r["key"]
            lines.append("\t".join(["P", r["name"], key, r["cc"], r["region"], r["lat"], r["lng"], str(n[r["clock"]])]))
        text = "\n".join(lines) + "\n"
        (OUT / f"{name}.txt").write_text(text, encoding="utf-8")
        total += len(text.encode())
    names = [k for k in files if not k.startswith("near/")]
    biggest = max(files, key=lambda k: (OUT / f"{k}.txt").stat().st_size)
    print(f"{len(rows)} places in {len({r['cc'] for r in rows})} countries, {len(set(clock_of.values()))} clocks")
    print(f"{OUT.relative_to(ROOT)}: {len(names)} name files, {len(files) - len(names)} squares, {total:,} bytes;"
          f" biggest {biggest}.txt {(OUT / f'{biggest}.txt').stat().st_size:,}; {unindexed} places without a name file")


if __name__ == "__main__":
    main()
