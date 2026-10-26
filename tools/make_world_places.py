#!/usr/bin/env python3
"""Build addons/sholat/world.tsv.zst: the Prayer times add-on's places
outside Indonesia (Kemenag's places.json covers Indonesia, Makkah, Madinah
and Jeddah).

Cities: GeoNames' cities of 15,000 people or more (cities15000), as packed in
the geonamescache Python package, version 3.0.2:

    pip download --no-deps geonamescache==3.0.2
    unzip geonamescache-3.0.2-py3-none-any.whl 'geonamescache/data/*'

GeoNames data is CC BY 4.0 (https://www.geonames.org); the add-on's
NOTICE.txt credits it. Time zones: the IANA time zone database through
Python's zoneinfo; each zone's UTC offsets are written out from 2026 to 2045,
summer time included.

    python3 -I tools/make_world_places.py <unzipped>/geonamescache/data

writes addons/sholat/world.tsv.zst (zstd -19) and prints what it
holds. Run it again to pick up a newer tz database or city list; then the
add-on gets a new tag and new numbers in poni_core::addons.

The file: UTF-8 lines, fields split by tabs, `#` lines are comments.
  Z <n> <IANA name> <minutes east of UTC from 2026> [<UTC minute>=<minutes east> ...]
  C <ISO code> <country> <method> <asr: s = Syafi'i, h = Hanafi>
  A <typed name> <id in places.json>
  B <how many of the first P lines have a million people or more>
  P <name> <search key> <ISO code> <region> <lat> <lng> <zone n>
The search key is left empty when it is the name in lowercase. P lines run
from the most people to the fewest. A place's id in settings is "w:" and its
lat,lng as written here (no GeoNames ids: they would make the download a
fifth bigger), and a place whose point moves in a later list is found again
as the nearest one.
"""

import datetime as dt
import json
import math
import subprocess
import sys
import unicodedata
from pathlib import Path
from zoneinfo import ZoneInfo

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "addons/sholat/world.tsv.zst"
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

    zones = sorted({c["timezone"] for c in kept})
    zone_n = {z: i for i, z in enumerate(zones)}
    used = sorted({c["countrycode"] for c in kept})

    lines = [
        "# Oyenza Studio, Prayer times add-on: places outside Indonesia.",
        "# Cities: GeoNames (https://www.geonames.org), CC BY 4.0: cities of 15,000 people or more,",
        "# from geonamescache 3.0.2. Time zones: the IANA time zone database "
        f"({FIRST_YEAR} to {END_YEAR - 1}).",
        "# Built by tools/make_world_places.py; the fields are described there.",
    ]
    lines += [zone_line(i, z) for i, z in enumerate(zones)]
    for cc in used:
        africa = countries[cc]["continentcode"] == "AF"
        method = METHOD_BY_COUNTRY.get(cc, "egyptian" if africa else "mwl")
        lines.append("\t".join(["C", cc, countries[cc]["name"], method, "h" if cc in HANAFI else "s"]))
    lines += ["\t".join(["A", k, v]) for k, v in sorted(ALIASES.items())]
    lines.append("\t".join(["B", str(sum(1 for c in kept if c["population"] >= 1_000_000))]))
    seen = set()
    for c in kept:
        name = " ".join(c["name"].split())
        key = fold(name)
        region = REGIONS.get(c["countrycode"], {}).get(c["admin1code"], "")
        lat, lng = f"{c['latitude']:.3f}", f"{c['longitude']:.3f}"
        # Two places on one point would share an id: the smaller one goes.
        if (lat, lng) in seen:
            continue
        seen.add((lat, lng))
        lines.append("\t".join(["P", name, "" if key == name.lower() else key, c["countrycode"], region, lat, lng, str(zone_n[c["timezone"]])]))
    text = ("\n".join(lines) + "\n").encode()
    # From a file, so the frame records its size and the window stays that
    # small (the app's decoder holds one window in memory).
    plain = OUT.with_suffix("")
    plain.write_bytes(text)
    subprocess.run(["zstd", "-19", "-q", "-f", str(plain), "-o", str(OUT)], check=True)
    plain.unlink()
    packed = OUT.read_bytes()
    dst = sum(1 for l in lines if l.startswith("Z\t") and l.split("\t")[4])
    print(f"{len(seen)} places in {len(used)} countries, {len(zones)} time zones ({dst} with changes)")
    print(f"{OUT.relative_to(ROOT)}: {len(text):,} bytes, packed {len(packed):,}")


if __name__ == "__main__":
    main()
