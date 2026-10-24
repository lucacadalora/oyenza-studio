#!/usr/bin/env python3
"""Fetch the lyrics of Apple Music Japan's most-played songs from LRCLIB into a
folder, for the "Japanese readings compared" CI job.

Usage:
    python3 tools/ja_compare_fetch.py OUT_DIR [--limit 100]

Writes one file per song found (NN.lrc, synced lyrics when LRCLIB has them,
else plain) and chart.txt (rank, song, artist). Song lyrics are fetched at
run time and must never be committed: point OUT_DIR outside the repository.
Python 3 standard library only.
"""

import json
import os
import sys
import time
import urllib.parse
import urllib.request

CHART = "https://rss.applemarketingtools.com/api/v2/jp/music/most-played/{n}/songs.json"
SEARCH = "https://lrclib.net/api/search?"
HEADERS = {"User-Agent": "Oyen Poni CI (https://github.com/lucacadalora/poni)"}


def get_json(url, tries=4, timeout=45):
    """The JSON at `url`, asking again (after 5, 10, 20 s) when it fails."""
    for attempt in range(tries):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=HEADERS), timeout=timeout) as r:
                return json.load(r)
        except Exception:
            if attempt + 1 == tries:
                raise
            time.sleep(5 * 2 ** attempt)


def main():
    args = sys.argv[1:]
    if not args or args[0].startswith("-"):
        sys.exit(__doc__)
    out = args[0]
    limit = int(args[args.index("--limit") + 1]) if "--limit" in args else 100
    os.makedirs(out, exist_ok=True)
    try:
        songs = get_json(CHART.format(n=limit))["feed"]["results"]
    except Exception as e:
        sys.exit(f"Apple Music's chart feed didn't answer ({e}); run the job again later")
    found = 0
    with open(os.path.join(out, "chart.txt"), "w", encoding="utf-8") as chart:
        for rank, song in enumerate(songs, 1):
            name, artist = song["name"], song["artistName"]
            chart.write(f"{rank}\t{name}\t{artist}\n")
            try:
                hits = get_json(SEARCH + urllib.parse.urlencode({"track_name": name, "artist_name": artist}), tries=2, timeout=30)
            except Exception as e:
                print(f"{rank}: LRCLIB lookup failed ({e})", file=sys.stderr)
                continue
            best = next((h for h in hits if h.get("syncedLyrics")), None) or next((h for h in hits if h.get("plainLyrics")), None)
            if best:
                with open(os.path.join(out, f"{rank:03d}.lrc"), "w", encoding="utf-8") as f:
                    f.write(best.get("syncedLyrics") or best.get("plainLyrics"))
                found += 1
            time.sleep(0.3)
    print(f"{found} of {len(songs)} chart songs have lyrics on LRCLIB")
    if found == 0:
        sys.exit("no lyrics fetched")


if __name__ == "__main__":
    main()
