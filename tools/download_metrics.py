#!/usr/bin/env python3
"""Count Oyen Poni's downloads on GitHub and keep the stats page.

GitHub keeps one running download counter per release file and nothing else:
no dates, no history. .github/workflows/metrics.yml runs this every hour and
keeps the history on the `metrics` branch, whose README is the stats page:

    data/downloads.csv     each release file's GitHub counter, how many of those
                           downloads were our own CI, and people's downloads; a
                           row only when they change (the last row carries forward)
    data/ci-downloads.csv  every CI job step that downloaded a release file
    data/repo.csv          stars, forks and watchers, a row when they change
    data/traffic.csv       page views and clones per day (needs METRICS_TOKEN)
    data/referrers.csv     sites visitors came from, GitHub's 14-day totals
    summary.json           the page's numbers, for anything else that wants them
    README.md, downloads.svg, badge.svg, art/

Our CI downloads the 0.1.1 installer on every push to test the upgrade from
it, and GitHub counts that like anyone's download. Steps in
.github/workflows/*.yml that fetch a release file are found by their URL,
their runs are read from the Actions API, and the page leaves them out.
GitHub updates its counters in batches, about every ten minutes, while the
Actions API shows a download at once; so counter minus CI can fall short for
a while but never overshoots, and each file keeps its highest reading.

    GITHUB_TOKEN=... python3 tools/download_metrics.py --out stats

GITHUB_TOKEN reads releases and Actions runs. METRICS_TOKEN, when set, adds
page views and clones, which GitHub keeps for 14 days only (a fine-grained
token for this repository with Administration: Read-only). Standard library
only.
"""
from __future__ import annotations

import argparse
import csv
import io
import json
import math
import os
import re
import shutil
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter
from datetime import date, datetime, timedelta, timezone
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
API = "https://api.github.com"
# The page's picture, copied next to the page so it never links into main.
ART = ROOT / "docs/brand/art/oyen-poni-peek.webp"

# Oyen's look (crates/poni-core/src/look.rs) on a cream card. The bars are a
# step deeper than Oyen's orange so they keep 3:1 contrast on the cream.
CREAM = "#FFF1DE"
INK = "#2B1B10"
SECONDARY = "#6B5442"
MUTED = "#7A6450"
GRID = "#EAD7BC"
AXIS = "#D8BF9C"
BAR = "#D9661C"
ORANGE = "#F0892E"
FONT = "-apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, Helvetica, Arial, sans-serif"

DOWNLOADS = ["date", "release", "file", "github_count", "ci_count", "downloads"]
CI_DOWNLOADS = ["job_id", "step", "run_id", "attempt", "started_at", "branch", "workflow", "release", "file"]
REPO = ["date", "stars", "forks", "watchers"]
TRAFFIC = ["date", "views", "visitors", "clones", "cloners"]
REFERRERS = ["date", "referrer", "views", "visitors"]

# The page's columns, by file name (see kind()).
KINDS = {"button": "Download button", "installer": "Versioned installer", "zip": "Portable zip", "other": "Other files"}
DOWNLOAD_URL = re.compile(r"github\.com/([\w.-]+/[\w.-]+)/releases/download/([^/\s\"']+)/([^\s\"'?#]+)")


def kind(file: str) -> str:
    if file.endswith("-portable.zip"):
        return "zip"
    # The website's and README's Download link: releases/latest/download/Oyen-Poni-Setup.exe.
    if re.fullmatch(r"(Oyen-)?Poni-Setup\.exe", file):
        return "button"
    if file.endswith(".exe"):
        return "installer"
    return "other"


def warn(message: str) -> None:
    print(f"::warning::{message}", flush=True)


class GitHub:
    def __init__(self, repo: str, token: str | None):
        self.repo = repo
        self.token = token
        self.calls = 0

    def get(self, url: str, params: dict | None = None) -> tuple[object, str | None]:
        """One API call: the JSON, and the next page's URL (or None)."""
        if not url.startswith("https://"):
            url = API + url
        if params:
            url += ("&" if "?" in url else "?") + urllib.parse.urlencode(params)
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "oyen-poni-download-stats"}
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        for attempt in range(5):
            try:
                with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as r:
                    self.calls += 1
                    link = re.search(r'<([^>]+)>;\s*rel="next"', r.headers.get("Link", ""))
                    return json.load(r), link.group(1) if link else None
            except urllib.error.HTTPError as e:
                # GitHub's own hiccups and short rate-limit waits are retried; any other answer is final.
                wait = int(e.headers.get("Retry-After") or 0)
                if attempt == 4 or not (e.code >= 500 or (e.code in (403, 429) and 0 < wait <= 120)):
                    raise
                time.sleep(max(wait, 2 ** attempt))
            except (urllib.error.URLError, TimeoutError):
                if attempt == 4:
                    raise
                time.sleep(2 ** attempt)
        raise AssertionError("unreachable")

    def pages(self, path: str, params: dict | None = None, key: str | None = None) -> list:
        """Every item of a list endpoint, following the Link header."""
        items: list = []
        url: str | None = path
        params = {**(params or {}), "per_page": 100}
        while url:
            data, url = self.get(url, params)
            params = None  # the next link carries them
            items += data[key] if key else data
        return items


def parse_time(text: str) -> datetime:
    return datetime.fromisoformat(text.replace("Z", "+00:00")).astimezone(timezone.utc)


def iso(t: datetime) -> str:
    return t.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def day_text(d: date) -> str:
    return f"{d.day} {d:%b %Y}"


def version_key(tag: str) -> tuple[int, ...]:
    return tuple(int(n) for n in re.findall(r"\d+", tag))


def read_csv(path: Path) -> list[dict]:
    if not path.exists():
        return []
    with path.open(newline="", encoding="utf-8") as f:
        return list(csv.DictReader(f))


def csv_text(fields: list[str], rows: list[dict]) -> str:
    buf = io.StringIO()
    writer = csv.DictWriter(buf, fields, lineterminator="\n")
    writer.writeheader()
    writer.writerows(rows)
    return buf.getvalue()


def write(path: Path, text: str) -> bool:
    """Write only when the text differs, so an unchanged page makes no commit."""
    if path.exists() and path.read_text(encoding="utf-8") == text:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return True


def fetch_steps(repo: str) -> dict[str, dict[str, tuple[str, str]]]:
    """Workflow steps that download one of this repo's release files:
    {workflow file: {step name: (release tag, file name)}}.

    Read straight from the YAML text: a step is a list item and its name is
    the item's own `name:` key. The Actions API reports steps by that name."""
    found: dict[str, dict[str, tuple[str, str]]] = {}
    for workflow in sorted((ROOT / ".github/workflows").glob("*.y*ml")):
        name, key_indent = None, -1
        for line in workflow.read_text(encoding="utf-8").splitlines():
            item = re.match(r"^\s*-\s+([\w-]+):\s*(.*?)\s*$", line)
            if item:
                key_indent = item.start(1)
                name = item.group(2).strip("\"'") if item.group(1) == "name" else None
            else:
                key = re.match(r"^(\s*)name:\s*(.*?)\s*$", line)
                if key and len(key.group(1)) == key_indent:
                    name = key.group(2).strip("\"'")
            for owner_repo, tag, file in DOWNLOAD_URL.findall(line):
                if owner_repo.lower() != repo.lower():
                    continue
                if name:
                    found.setdefault(workflow.name, {})[name] = (tag, urllib.parse.unquote(file))
                else:
                    warn(f"{workflow.name}: a step without a name downloads {tag}/{file}; name it so its downloads are left out")
    return found


def count_ci(gh: GitHub, steps: dict[str, dict[str, tuple[str, str]]], ledger: dict, checked: datetime | None) -> int:
    """Add CI downloads the ledger doesn't have yet; returns how many."""
    added = 0
    for workflow, names in steps.items():
        runs = []
        url: str | None = f"/repos/{gh.repo}/actions/workflows/{workflow}/runs"
        params: dict | None = {"per_page": 100}
        while url:
            data, url = gh.get(url, params)
            params = None
            for run in data["workflow_runs"]:
                # Newest first. Runs created two days before the last check
                # were finished and counted by then (re-runs that late are rare).
                if checked and parse_time(run["created_at"]) < checked - timedelta(days=2):
                    url = None
                    break
                runs.append(run)
        for run in runs:
            # A run that hasn't moved since the last check was counted then.
            if checked and parse_time(run["updated_at"]) < checked - timedelta(minutes=30):
                continue
            for job in gh.pages(f"/repos/{gh.repo}/actions/runs/{run['id']}/jobs", {"filter": "all"}, "jobs"):
                for step in job.get("steps") or []:
                    # A step that started has downloaded: GitHub counts the request, not a finished file.
                    if step["name"] not in names or not step.get("started_at") or step.get("conclusion") == "skipped":
                        continue
                    key = (str(job["id"]), step["name"])
                    if key in ledger:
                        continue
                    tag, file = names[step["name"]]
                    ledger[key] = {
                        "job_id": str(job["id"]), "step": step["name"], "run_id": str(run["id"]), "attempt": str(job.get("run_attempt") or 1),
                        "started_at": step["started_at"], "branch": run.get("head_branch") or "", "workflow": workflow, "release": tag, "file": file,
                    }
                    added += 1
    return added


def people(row: dict) -> int:
    """A row's downloads by people (rows from before that column: counter minus CI)."""
    if row.get("downloads") not in (None, ""):
        return int(row["downloads"])
    return max(0, int(row["github_count"]) - int(row["ci_count"]))


def carry(rows: list[dict]) -> dict[tuple[str, str], tuple[int, int, int]]:
    """Each release file's last (GitHub count, CI count, people's downloads)."""
    state = {}
    for row in sorted(rows, key=lambda r: r["date"]):
        state[(row["release"], row["file"])] = (int(row["github_count"]), int(row["ci_count"]), people(row))
    return state


def per_day(rows: list[dict], last: date) -> list[tuple[date, int]]:
    """Downloads per day, CI's left out, from the first reading to `last`.
    The first day holds everything before it: GitHub keeps no dates."""
    if not rows:
        return []
    by_date: dict[str, list[dict]] = {}
    for row in rows:
        by_date.setdefault(row["date"], []).append(row)
    files: dict[tuple[str, str], int] = {}
    days, before = [], 0
    d = date.fromisoformat(min(by_date))
    while d <= last:
        for row in by_date.get(d.isoformat(), []):
            files[(row["release"], row["file"])] = people(row)
        total = sum(files.values())
        days.append((d, max(0, total - before)))
        before = total
        d += timedelta(days=1)
    return days


def nice_ticks(top: int) -> list[int]:
    """0 and up to four round steps that reach `top`."""
    top = max(top, 1)
    for step in (m * 10 ** e for e in range(12) for m in (1, 2, 5)):
        if math.ceil(top / step) <= 4:
            return [step * k for k in range(math.ceil(top / step) + 1)]
    raise ValueError(top)


# A generous guess at a label's width, so labels get room whatever font the
# viewer has (the SVG uses the system's own sans).
NARROW = set("fijlrt.,:;'!|() ")


def text_width(text: str, size: float) -> float:
    return size * sum(0.34 if ch in NARROW else 0.92 if ch in "mwMW" else 0.72 if ch.isupper() else 0.64 for ch in text)


def svg_text(x: float, y: float, text: str, size: int, fill: str, weight: int = 400, anchor: str = "start", tabular: bool = False) -> str:
    attrs = f'x="{x:.1f}" y="{y:.1f}" font-size="{size}" fill="{fill}"'
    if weight != 400:
        attrs += f' font-weight="{weight}"'
    if anchor != "start":
        attrs += f' text-anchor="{anchor}"'
    if tabular:
        attrs += ' style="font-variant-numeric: tabular-nums"'
    return f"<text {attrs}>{escape(text)}</text>"


def svg_line(x0: float, y0: float, x1: float, y1: float, stroke: str) -> str:
    return f'<path d="M{x0:.1f},{y0:.1f}L{x1:.1f},{y1:.1f}" stroke="{stroke}" stroke-width="1"/>'


def hbar(x: float, y: float, w: float, h: float, r: float = 4) -> str:
    """A bar growing right from x: square at the baseline, rounded at the end."""
    r = min(r, w, h / 2)
    return (f'<path d="M{x:.1f},{y:.1f}H{x + w - r:.1f}A{r:.1f},{r:.1f} 0 0 1 {x + w:.1f},{y + r:.1f}'
            f'V{y + h - r:.1f}A{r:.1f},{r:.1f} 0 0 1 {x + w - r:.1f},{y + h:.1f}H{x:.1f}Z" fill="{BAR}"/>')


def vbar(x: float, y: float, w: float, h: float, r: float = 4) -> str:
    """A column standing on y + h: square at the baseline, rounded on top."""
    r = min(r, h, w / 2)
    return (f'<path d="M{x:.1f},{y + h:.1f}V{y + r:.1f}A{r:.1f},{r:.1f} 0 0 1 {x + r:.1f},{y:.1f}'
            f'H{x + w - r:.1f}A{r:.1f},{r:.1f} 0 0 1 {x + w:.1f},{y + r:.1f}V{y + h:.1f}Z" fill="{BAR}"/>')


def chart_rows(releases: list[dict]) -> list[tuple[str, int]]:
    """(label, downloads) per release, newest first; past 12 the oldest share a row."""
    rows = [(r["release"].removeprefix("v"), r["downloads"]) for r in releases]
    if len(rows) > 12:
        rows = rows[:11] + [(f"{len(rows) - 11} older", sum(n for _, n in rows[11:]))]
    return rows


def chart_svg(releases: list[dict], days: list[tuple[date, int]]) -> str:
    """Downloads per release, and per day once there are three days."""
    W, PAD = 820, 28
    marks: list[str] = []
    labels: list[str] = []
    y = PAD
    labels.append(svg_text(PAD, y + 16, "Downloads per release", 16, INK, 600))
    y += 40
    rows = chart_rows(releases)
    x0 = round(PAD + max((text_width(label, 14) for label, _ in rows), default=0) + 14)
    top = max([n for _, n in rows] + [1])
    span = W - PAD - x0 - text_width(f"{top:,}", 14) - 10
    first = y
    for label, n in rows:
        w = span * n / top
        labels.append(svg_text(x0 - 10, y + 20, label, 14, SECONDARY, anchor="end", tabular=True))
        if n:
            marks.append(hbar(x0, y + 7, w, 18))
        labels.append(svg_text(x0 + w + 8, y + 20.5, f"{n:,}", 14, INK, 600))
        y += 32
    grid = [svg_line(x0 - 0.5, first, x0 - 0.5, y - 4, AXIS)]

    if len(days) >= 3:
        shown = days[-30:]
        values = [n for _, n in shown]
        y += 28
        labels.append(svg_text(PAD, y + 16, "Downloads per day", 16, INK, 600))
        y += 30 + 24  # the title, then room for a value over the tallest column
        ticks = nice_ticks(max(values))
        gx0 = round(PAD + max(text_width(f"{t:,}", 12) for t in ticks) + 10)
        gx1 = W - PAD
        height = 150
        bottom = y + height
        scale = height / ticks[-1]
        slot = (gx1 - gx0) / len(shown)
        width = min(24.0, slot * 0.6)

        def cx(i: int) -> float:
            return gx0 + slot * (i + 0.5)

        # Values on the highest day and the latest day; the table has the rest.
        boxes: list[tuple[float, float, float, float]] = []
        for i in sorted({max(range(len(values)), key=lambda i: (values[i], i)), len(values) - 1}, reverse=True):
            text = f"{values[i]:,}"
            half = text_width(text, 13) / 2
            ty = bottom - values[i] * scale - 7
            box = (cx(i) - half - 3, ty - 12, cx(i) + half + 3, ty + 3)
            if any(box[0] < b[2] and b[0] < box[2] and box[1] < b[3] and b[1] < box[3] for b in boxes):
                continue
            boxes.append(box)
            labels.append(svg_text(cx(i), ty, text, 13, INK, 600, anchor="middle"))
        # Gridlines at the ticks stop short of any value label they would cross.
        for t in ticks:
            gy = round(bottom - t * scale) + 0.5
            labels.append(svg_text(gx0 - 10, gy + 4, f"{t:,}", 12, MUTED, anchor="end", tabular=True))
            if t == 0:
                continue
            spans = [(float(gx0), float(gx1))]
            for bx0, by0, bx1, by1 in boxes:
                if by0 - 2 <= gy <= by1 + 2:
                    spans = [part for a, b in spans for part in ((a, min(b, bx0 - 4)), (max(a, bx1 + 4), b)) if part[1] - part[0] > 1]
            grid += [svg_line(a, gy, b, gy, GRID) for a, b in spans]
        for i, n in enumerate(values):
            if n:
                marks.append(vbar(cx(i) - width / 2, bottom - n * scale, width, n * scale))
        grid.append(svg_line(gx0, bottom + 0.5, gx1, bottom + 0.5, AXIS))
        # Dates under the latest day and every 7th before it, while they fit.
        left_edge = math.inf
        for i in range(len(shown) - 1, -1, -7):
            text = f"{shown[i][0].day} {shown[i][0]:%b}"
            half = text_width(text, 12) / 2
            x = min(max(cx(i), gx0 + half), W - 12 - half)
            if x + half + 8 > left_edge:
                continue
            labels.append(svg_text(x, bottom + 20, text, 12, MUTED, anchor="middle"))
            left_edge = x - half
        y = bottom + 26

    H = round(y + PAD - 8)
    body = "\n".join(grid + marks + labels)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" role="img" aria-labelledby="title">\n'
            f'<title id="title">Oyen Poni downloads per release{" and per day" if len(days) >= 3 else ""}</title>\n'
            f'<rect width="{W}" height="{H}" rx="16" fill="{CREAM}"/>\n'
            f'<g font-family="{FONT}">\n{body}\n</g>\n</svg>\n')


# Verdana at 11 px, the badge's font, wide enough for any of its stand-ins.
VERDANA = {**dict.fromkeys("0123456789", 7.0), **dict(zip("abcdefghijklmnopqrstuvwxyz",
           (6.6, 6.9, 5.7, 6.9, 6.6, 3.9, 6.9, 7.0, 3.0, 3.8, 6.5, 3.0, 10.7, 7.0, 6.7, 6.9, 6.9, 4.7, 5.7, 4.3, 7.0, 6.5, 9.0, 6.5, 6.5, 5.8))), ",": 4.0, " ": 3.9}


def badge_svg(downloads: int) -> str:
    left, right = "downloads", f"{downloads:,}"
    lw = round(sum(VERDANA.get(ch, 7.0) for ch in left)) + 12
    rw = round(sum(VERDANA.get(ch, 7.0) * 1.1 for ch in right)) + 12  # bold
    w = lw + rw
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="20" role="img" aria-label="{left}: {right}">\n'
            f'<title>{left}: {right}</title>\n'
            f'<clipPath id="r"><rect width="{w}" height="20" rx="4"/></clipPath>\n'
            f'<g clip-path="url(#r)"><rect width="{lw}" height="20" fill="{INK}"/><rect x="{lw}" width="{rw}" height="20" fill="{ORANGE}"/></g>\n'
            f'<g font-family="Verdana, Geneva, \'DejaVu Sans\', sans-serif" font-size="11" text-anchor="middle">\n'
            f'<text x="{lw / 2:.1f}" y="14" fill="{CREAM}" textLength="{lw - 12}">{left}</text>\n'
            f'<text x="{lw + rw / 2:.1f}" y="14" fill="{INK}" font-weight="bold" textLength="{rw - 12}">{right}</text>\n'
            f'</g>\n</svg>\n')


def summarize(repo: str, rows: list[dict], api_releases: list[dict], repo_info: dict, days: list[tuple[date, int]], stamp: datetime) -> dict:
    by_tag = {r["tag_name"]: r for r in api_releases}
    releases: dict[str, dict] = {}
    for (tag, file), (count, ci, downloads) in carry(rows).items():
        r = releases.setdefault(tag, {"release": tag, "published": None, "downloads": 0, "github_count": 0, "ci_count": 0, "files": {}})
        r["files"][file] = downloads
        r["downloads"] += downloads
        r["github_count"] += count
        r["ci_count"] += ci
    for tag, r in releases.items():
        if tag in by_tag:
            r["published"] = by_tag[tag]["published_at"]
    ordered = sorted(releases.values(), key=lambda r: version_key(r["release"]), reverse=True)
    latest = max((r for r in api_releases if not r["prerelease"]), key=lambda r: r["published_at"] or "", default=None)
    published = [r["published_at"] for r in api_releases if r["published_at"]]
    return {
        "repo": repo,
        "checked_at": iso(stamp),
        "since": parse_time(min(published)).date().isoformat() if published else (rows[0]["date"] if rows else None),
        "downloads": sum(r["downloads"] for r in ordered),
        "github_count": sum(r["github_count"] for r in ordered),
        "ci_count": sum(r["ci_count"] for r in ordered),
        "last_7_days": sum(n for _, n in days[-7:]),
        "latest": {"release": latest["tag_name"], "downloads": releases.get(latest["tag_name"], {}).get("downloads", 0)} if latest else None,
        "stars": repo_info["stargazers_count"],
        "forks": repo_info["forks_count"],
        "watchers": repo_info["subscribers_count"],
        "releases": ordered,
        "days": [{"date": d.isoformat(), "downloads": n} for d, n in days],
    }


def plural(n: int, word: str) -> str:
    return f"{n:,} {word}{'' if n == 1 else 's'}"


def page(s: dict, days: list[tuple[date, int]], steps: dict, traffic: list[dict], referrers: list[dict], has_art: bool) -> str:
    repo = s["repo"]
    stamp = parse_time(s["checked_at"])
    out = []
    if has_art:
        out += [f'<p align="center"><img src="art/{ART.name}" width="260" alt="Oyen peeking over a playing island"></p>', ""]
    out += [f'<h1 align="center">{plural(s["downloads"], "download")}</h1>', ""]
    facts = []
    if s["latest"]:
        facts.append(f'Latest release, {s["latest"]["release"].removeprefix("v")}: <b>{s["latest"]["downloads"]:,}</b>')
    facts.append(f'Last 7 days: <b>{s["last_7_days"]:,}</b>')
    facts.append(f'<b>{s["stars"]:,}</b> star{"" if s["stars"] == 1 else "s"} on GitHub')
    since = f' since {day_text(date.fromisoformat(s["since"]))}' if s["since"] else ""
    out += [f'<p align="center">of Oyen Poni for Windows, from GitHub{since}<br>{" · ".join(facts)}</p>', ""]
    alt = "Bar chart of downloads per release: " + ", ".join(f"{label} {n:,}" for label, n in chart_rows(s["releases"]))
    if len(days) >= 3:
        alt += f"; and per day, for the last {min(len(days), 30)} days"
    out += [f'<p align="center"><img src="downloads.svg" width="820" alt="{escape(alt)}"></p>', ""]

    out += ["## Per release", ""]
    kinds = [k for k in KINDS if any(kind(f) == k for r in s["releases"] for f in r["files"])]
    out.append("| Release | Published | " + " | ".join(KINDS[k] for k in kinds) + " | Total |")
    out.append("| --- | --- | " + " | ".join("--:" for _ in kinds) + " | --: |")
    totals = Counter()
    for r in s["releases"]:
        cells = []
        for k in kinds:
            files = [n for f, n in r["files"].items() if kind(f) == k]
            cells.append(f"{sum(files):,}" if files else "—")
            totals[k] += sum(files)
        published = day_text(parse_time(r["published"]).date()) if r["published"] else "—"
        version = r["release"].removeprefix("v")
        out.append(f"| [{version}](https://github.com/{repo}/releases/tag/{r['release']}) | {published} | " + " | ".join(cells) + f" | {r['downloads']:,} |")
    out.append("| **All releases** | | " + " | ".join(f"**{totals[k]:,}**" for k in kinds) + f" | **{s['downloads']:,}** |")
    out.append("")

    out += ["## Per day", "", "| Day | Downloads |", "| --- | --: |"]
    for d, n in reversed(days[-30:]):
        out.append(f"| {'Today' if d == stamp.date() else day_text(d)} | {n:,} |")
    if len(days) > 30:
        out.append(f"\nEvery day since {day_text(days[0][0])}: [downloads.csv](data/downloads.csv).")
    out.append("")

    out += ["## On GitHub", "", f"{plural(s['stars'], 'star')} · {plural(s['forks'], 'fork')} · {s['watchers']:,} watching", ""]
    if traffic:
        out += ["| Day | Page views | Visitors | Clones |", "| --- | --: | --: | --: |"]
        for row in sorted(traffic, key=lambda r: r["date"], reverse=True)[:14]:
            d = date.fromisoformat(row["date"])
            out.append(f"| {'Today' if d == stamp.date() else day_text(d)} | {int(row['views']):,} | {int(row['visitors']):,} | {int(row['clones']):,} |")
        out.append("")
        latest = max((r["date"] for r in referrers), default=None)
        top = [r for r in referrers if r["date"] == latest]
        if top:
            out += ["Where visitors came from, last 14 days: " + " · ".join(f"{escape(r['referrer'])} {int(r['views']):,}" for r in top), ""]

    fetched = sorted({f"{file} ({tag.removeprefix('v')})" for names in steps.values() for tag, file in names.values()})
    out += ["<details>", "<summary>How we count</summary>", ""]
    out.append("- GitHub counts every download of a release file. The Download button on the [website](https://lucacadalora.github.io/poni/) "
               "and in the README always fetches `Oyen-Poni-Setup.exe` from the newest release; the versioned installer and the portable zip "
               "are on each release's page.")
    if s["ci_count"] or fetched:
        what = f"downloads {', '.join(fetched)}" if fetched else "downloaded release files"
        out.append(f"- Our CI {what} while it tests each push, and GitHub counts those too: {s['ci_count']:,} so far, left out of every number here "
                   f"([ci-downloads.csv](data/ci-downloads.csv) lists each one). GitHub's own counters add up to {s['github_count']:,}.")
    out.append("- GitHub updates its counters every few minutes, so for a while a counter may not show a CI download that is already "
               "taken off. Each file's number keeps its highest reading, so numbers only go up.")
    out.append("- Still in the numbers: our own check of each new release (one download through the Download button) and any bots. "
               "GitHub can't tell those apart.")
    out.append("- Oyen Poni itself sends nothing: no accounts, no tracking. These are GitHub's download counters, read every hour.")
    if s["since"]:
        out.append(f"- GitHub keeps no dates for downloads, so everything before the first reading counts on {day_text(days[0][0]) if days else 'that day'}.")
    if not traffic:
        out.append("- Page views and clones need a `METRICS_TOKEN` repository secret: a fine-grained token for this repository with "
                   "Administration: Read-only. GitHub keeps them for 14 days only.")
    out += ["", "</details>", ""]
    data = ["[downloads](data/downloads.csv)", "[CI downloads](data/ci-downloads.csv)", "[stars](data/repo.csv)"]
    if traffic:
        data += ["[page views](data/traffic.csv)", "[referrers](data/referrers.csv)"]
    out.append(f"<sub>Updated {day_text(stamp.date())}, {stamp:%H:%M} UTC by [Download stats](https://github.com/{repo}/actions/workflows/metrics.yml), "
               f"every hour · Data: {', '.join(data + ['[summary.json](summary.json)'])}</sub>")
    return "\n".join(out) + "\n"


def read_traffic(repo: str, token: str, today: date, traffic: list[dict], referrers: list[dict]) -> tuple[list[dict], list[dict]]:
    """Today's view of GitHub's 14-day traffic window, merged into the history."""
    gh = GitHub(repo, token)
    try:
        views, _ = gh.get(f"/repos/{repo}/traffic/views", {"per": "day"})
        clones, _ = gh.get(f"/repos/{repo}/traffic/clones", {"per": "day"})
        top, _ = gh.get(f"/repos/{repo}/traffic/popular/referrers")
    except urllib.error.HTTPError as e:
        warn(f"Page views skipped: METRICS_TOKEN got HTTP {e.code} (expired, or without Administration: Read-only on {repo}?)")
        return traffic, referrers
    window = {today - timedelta(days=i): {"views": 0, "visitors": 0, "clones": 0, "cloners": 0} for i in range(14)}
    for v in views["views"]:
        window.setdefault(parse_time(v["timestamp"]).date(), {"views": 0, "visitors": 0, "clones": 0, "cloners": 0}).update(views=v["count"], visitors=v["uniques"])
    for c in clones["clones"]:
        window.setdefault(parse_time(c["timestamp"]).date(), {"views": 0, "visitors": 0, "clones": 0, "cloners": 0}).update(clones=c["count"], cloners=c["uniques"])
    keep = [r for r in traffic if date.fromisoformat(r["date"]) not in window]
    traffic = sorted(keep + [{"date": d.isoformat(), **{k: str(n) for k, n in v.items()}} for d, v in window.items()], key=lambda r: r["date"])
    referrers = [r for r in referrers if r["date"] != today.isoformat()]
    referrers += [{"date": today.isoformat(), "referrer": r["referrer"], "views": str(r["count"]), "visitors": str(r["uniques"])} for r in top]
    return traffic, referrers


def main() -> int:
    ap = argparse.ArgumentParser(description="Count Oyen Poni's downloads on GitHub and keep the stats page.")
    ap.add_argument("--out", type=Path, required=True, help="the metrics branch's checkout")
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY") or "lucacadalora/poni")
    ap.add_argument("--now", help="pretend it's this UTC time (for tests)")
    args = ap.parse_args()
    out: Path = args.out
    now = parse_time(args.now) if args.now else datetime.now(timezone.utc).replace(microsecond=0)
    today = now.date()
    gh = GitHub(args.repo, os.environ.get("GITHUB_TOKEN") or None)

    try:
        checked = parse_time(json.loads((out / "summary.json").read_text(encoding="utf-8"))["checked_at"])
    except (OSError, ValueError, KeyError):
        checked = None

    # CI's downloads first, then the counters. The counters trail downloads by
    # minutes, so every CI download they hold is already in the tally: counter
    # minus CI never overstates people's downloads, it can only fall short
    # until the counter catches up. A file's highest reading so far stands.
    ledger = {(r["job_id"], r["step"]): r for r in read_csv(out / "data/ci-downloads.csv")}
    steps = fetch_steps(args.repo)
    added = count_ci(gh, steps, ledger, checked)
    api_releases = [r for r in gh.pages(f"/repos/{args.repo}/releases") if not r["draft"]]
    repo_info, _ = gh.get(f"/repos/{args.repo}")

    ci = Counter((r["release"], r["file"]) for r in ledger.values())
    existing = read_csv(out / "data/downloads.csv")
    rows = [r for r in existing if r["date"] < today.isoformat()]
    earlier_today = {(r["release"], r["file"]): people(r) for r in existing if r["date"] == today.isoformat()}
    before = carry(rows)
    for release in api_releases:
        for asset in release["assets"]:
            key = (release["tag_name"], asset["name"])
            highest = max(before[key][2] if key in before else 0, earlier_today.get(key, 0))
            counts = (asset["download_count"], ci[key], max(highest, asset["download_count"] - ci[key]))
            if before.get(key) != counts:
                rows.append({"date": today.isoformat(), "release": key[0], "file": key[1], "github_count": counts[0], "ci_count": counts[1], "downloads": counts[2]})
    rows.sort(key=lambda r: (r["date"], version_key(r["release"]), r["file"]))

    repo_rows = [r for r in read_csv(out / "data/repo.csv") if r["date"] < today.isoformat()]
    stars = {"stars": str(repo_info["stargazers_count"]), "forks": str(repo_info["forks_count"]), "watchers": str(repo_info["subscribers_count"])}
    if not repo_rows or {k: repo_rows[-1][k] for k in stars} != stars:
        repo_rows.append({"date": today.isoformat(), **stars})

    traffic, referrers = read_csv(out / "data/traffic.csv"), read_csv(out / "data/referrers.csv")
    if os.environ.get("METRICS_TOKEN"):
        traffic, referrers = read_traffic(args.repo, os.environ["METRICS_TOKEN"], today, traffic, referrers)

    data = {
        "data/downloads.csv": csv_text(DOWNLOADS, rows),
        "data/ci-downloads.csv": csv_text(CI_DOWNLOADS, sorted(ledger.values(), key=lambda r: (r["started_at"], int(r["job_id"])))),
        "data/repo.csv": csv_text(REPO, repo_rows),
    }
    if traffic:
        data["data/traffic.csv"] = csv_text(TRAFFIC, traffic)
        data["data/referrers.csv"] = csv_text(REFERRERS, referrers)
    changed = any(not (out / p).exists() or (out / p).read_text(encoding="utf-8") != text for p, text in data.items())
    # New numbers, or a new day, get a fresh time stamp; otherwise the page is
    # rebuilt with the old one, so it only changes when this script does.
    stamp = now if changed or not checked or checked.date() != today else checked

    art = out / "art" / ART.name
    if ART.exists() and (not art.exists() or art.read_bytes() != ART.read_bytes()):
        art.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ART, art)
    days = per_day(rows, stamp.date())
    summary = summarize(args.repo, rows, api_releases, repo_info, days, stamp)
    files = {
        **data,
        "summary.json": json.dumps(summary, indent=2) + "\n",
        "README.md": page(summary, days, steps, traffic, referrers, art.exists()),
        "downloads.svg": chart_svg(summary["releases"], days),
        "badge.svg": badge_svg(summary["downloads"]),
    }
    written = [p for p, text in files.items() if write(out / p, text)]
    print(f"{summary['downloads']:,} downloads ({summary['github_count']:,} on GitHub's counters, {summary['ci_count']:,} by CI, "
          f"{added} new CI downloads); {gh.calls} API calls; {'wrote ' + ', '.join(written) if written else 'no change'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
