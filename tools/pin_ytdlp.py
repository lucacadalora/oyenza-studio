#!/usr/bin/env python3
"""Pin the latest yt-dlp in the Downloader add-on, or with --quickjs the
latest QuickJS-ng.

The Downloader takes yt-dlp.exe and qjs.exe on Windows, and yt-dlp_macos on
the Mac, from their own GitHub releases, unchanged, and keeps each only if
its size and SHA-256 are the ones built into the app (`DOWNLOADER_WINDOWS`
and `DOWNLOADER_MAC` in crates/poni-core/src/addons.rs). yt-dlp ships
fixes most weeks, as sites (YouTube most of all) keep changing, so the
Downloader pins workflow (.github/workflows/ytdlp-pin.yml) runs this every
week and opens a pull request when the pin moves (docs/downloader-pins.md).

It reads the tool's latest release from the GitHub API and takes the file's
size from there, and its SHA-256 from two places that must agree:

    yt-dlp       yt-dlp.exe and yt-dlp_macos, together: the API's digest and
                 the release's SHA2-256SUMS
    QuickJS-ng   qjs-windows-x86_64.exe: the API's digest and the file itself,
                 downloaded and hashed (its releases carry no checksum file).
                 The Mac's qjs is our own build (mac.yml's downloader-tools
                 job), pinned by hand with the add-on's pre-release.

Then it rewrites that file's size, sha256 and url in addons.rs, and nothing
else (its name is the name it's saved under and stays). A pin newer than
the latest release, set by hand, is left alone.

Usage:
    python3 tools/pin_ytdlp.py [--quickjs]            # pin the latest release
    python3 tools/pin_ytdlp.py [--quickjs] --check    # say whether a newer one is out; change nothing

In GitHub Actions it also writes changed (true when the pin moves, or with
--check would), version (the latest release), previous (the pin it found)
and line to $GITHUB_OUTPUT, and the line to the run's summary.

Env: GH_TOKEN or GITHUB_TOKEN (optional: raises the API's rate limit).
Python 3 standard library only.
"""

import hashlib
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ADDONS = os.path.join(ROOT, "crates", "poni-core", "src", "addons.rs")
API = "https://api.github.com/repos/"

# Each pin: whose releases it follows, the file it takes from them, the add-on
# (`DOWNLOADER_WINDOWS` or `DOWNLOADER_MAC`) and the name it saves that file
# under, and the checksum file the releases carry (None: hash the download
# instead). yt-dlp's two pins move together, to the same release.
TOOLS = {
    "yt-dlp": [
        {"title": "yt-dlp", "repo": "yt-dlp/yt-dlp", "asset": "yt-dlp.exe", "addon": "DOWNLOADER_WINDOWS", "name": "yt-dlp.exe", "sums": "SHA2-256SUMS"},
        {"title": "yt-dlp", "repo": "yt-dlp/yt-dlp", "asset": "yt-dlp_macos", "addon": "DOWNLOADER_MAC", "name": "yt-dlp", "sums": "SHA2-256SUMS"},
    ],
    "quickjs": [
        {"title": "QuickJS-ng", "repo": "quickjs-ng/quickjs", "asset": "qjs-windows-x86_64.exe", "addon": "DOWNLOADER_WINDOWS", "name": "qjs.exe", "sums": None},
    ],
}

# The values rewritten in a file's `AddonFile { .. }`: what comes before the
# value, the value, what comes after it.
FIELDS = {
    "size": re.compile(r"(\bsize:\s*)([0-9][0-9_]*)()"),
    "sha256": re.compile(r'(\bsha256:\s*")([0-9a-f]*)(")'),
    "url": re.compile(r'(\burl:\s*Some\(")([^"]*)("\))'),
}


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def fetch(url, accept="application/vnd.github+json"):
    """GET with a few tries: GitHub's API hiccups now and then. The token goes
    to the API only, not on to the host a download is sent to."""
    req = urllib.request.Request(url, headers={"Accept": accept, "User-Agent": "oyenza-studio-downloader-pins"})
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        req.add_unredirected_header("Authorization", "Bearer " + token)
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code == 404 or attempt == 3:
                raise
        except (urllib.error.URLError, TimeoutError):
            if attempt == 3:
                raise
        time.sleep(2 + attempt * 3)


def version_key(tag):
    """2026.10.26 -> (2026, 10, 26); v0.17.0 -> (0, 17, 0)."""
    v = tag.removeprefix("v")
    if not re.fullmatch(r"[0-9]+(\.[0-9]+)*", v):
        die(f'"{tag}" doesn\'t read as a version')
    return tuple(int(p) for p in v.split("."))


def asset(rel, name):
    """One file of the release, fully uploaded."""
    a = next((a for a in rel.get("assets", []) if a.get("name") == name), None)
    if a is None or a.get("state") != "uploaded" or not isinstance(a.get("size"), int) or a["size"] <= 0:
        die(f"release {rel.get('tag_name')} has no {name}, or it isn't fully uploaded")
    return a


def digest(a):
    """The SHA-256 GitHub lists for a release file."""
    d = a.get("digest") or ""
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", d):
        die(f"GitHub lists no SHA-256 for {a['name']} (digest: {d or 'none'})")
    return d.removeprefix("sha256:")


def download(a):
    """A release file's bytes, through the API like everything else here."""
    body = fetch(a["url"], accept="application/octet-stream")
    if len(body) != a["size"]:
        die(f"{a['name']}: downloaded {len(body)} bytes, GitHub lists {a['size']}")
    return body


def from_sums(rel, sums, name):
    """The SHA-256 that the release's checksum file (`<sha256>  <name>`
    lines) gives `name`."""
    s = asset(rel, sums)
    text = download(s)
    if hashlib.sha256(text).hexdigest() != digest(s):
        die(f"{sums} came back different from the one on the release")
    rows = (line.split() for line in text.decode("utf-8", "replace").splitlines())
    found = [r[0].lower() for r in rows if len(r) == 2 and r[1].lstrip("*") == name]
    if len(found) != 1 or not re.fullmatch(r"[0-9a-f]{64}", found[0]):
        die(f"{sums} has {len(found)} lines for {name}, expected one")
    return found[0]


def latest(t):
    """The latest release's tag, and the file's url, size and SHA-256 (the
    SHA-256 checked against a second source)."""
    rel = json.loads(fetch(f"{API}{t['repo']}/releases/latest"))
    tag = rel.get("tag_name") or ""
    version_key(tag)
    if rel.get("draft") or rel.get("prerelease"):
        die(f"{t['title']} {tag} is a draft or a pre-release")
    a = asset(rel, t["asset"])
    url = f"https://github.com/{t['repo']}/releases/download/{tag}/{t['asset']}"
    if a.get("browser_download_url") != url:
        die(f"GitHub gives {t['asset']}'s address as {a.get('browser_download_url')}, not {url}")
    sha = digest(a)
    if t["sums"]:
        other, where = from_sums(rel, t["sums"], t["asset"]), t["sums"]
    else:
        other, where = hashlib.sha256(download(a)).hexdigest(), "the downloaded file"
    if other != sha:
        die(f"{t['asset']} {tag}: the API's digest is {sha} but {where} says {other}; nothing pinned")
    return {"tag": tag, "url": url, "size": a["size"], "sha256": sha, "agree": f"the API's digest and in {where}"}


def block(text, t):
    """Where the `AddonFile { .. }` of the tool's file is, inside its add-on."""
    addon, name = t["addon"], t["name"]
    start = text.find(f"pub const {addon}: Addon = Addon {{")
    end = text.find("\n};", start)
    if start < 0 or end < 0:
        die(f"addons.rs has no `pub const {addon}: Addon = Addon {{ .. }};`")
    hits = [m.start() for m in re.finditer(rf'\bname:\s*"{re.escape(name)}"', text[start:end])]
    if len(hits) != 1:
        die(f"{addon} names {name} {len(hits)} times, expected once")
    at = start + hits[0]
    a = text.rfind("AddonFile {", start, at)
    b = text.find("}", at)
    if a < 0 or not at < b < end:
        die(f"can't find the `AddonFile {{ .. }}` around {name} in {addon}")
    return a, b + 1


def pinned(text, t):
    """What addons.rs pins for the tool: tag, url, size and SHA-256."""
    a, b = block(text, t)
    got = {}
    for key, pat in FIELDS.items():
        found = pat.findall(text[a:b])
        if len(found) != 1:
            die(f"{t['name']}'s {key} in {t['addon']} isn't written the way tools/pin_ytdlp.py reads it")
        got[key] = found[0][1]
    m = re.fullmatch(rf"https://github\.com/{re.escape(t['repo'])}/releases/download/([^/]+)/{re.escape(t['asset'])}", got["url"])
    if not m:
        die(f"{t['name']} in {t['addon']} comes from {got['url']}, not from {t['repo']}'s releases: change its pin by hand")
    return {"tag": m.group(1), "url": got["url"], "size": int(got["size"].replace("_", "")), "sha256": got["sha256"]}


def repin(text, t, new):
    """addons.rs with the tool's size, sha256 and url set to `new`'s."""
    a, b = block(text, t)
    part = text[a:b]
    for key, value in (("size", f"{new['size']:_}"), ("sha256", new["sha256"]), ("url", new["url"])):
        part = FIELDS[key].sub(lambda m, v=value: m.group(1) + v + m.group(3), part, count=1)
    return text[:a] + part + text[b:]


def output(**values):
    """For the workflow: its outputs, and the line in the run's summary."""
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as f:
            for k, v in values.items():
                f.write(f"{k}={v}\n")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(values["line"] + "\n")


def main(argv):
    if any(a not in ("--check", "--quickjs") for a in argv):
        sys.exit(__doc__)
    check = "--check" in argv
    pins = TOOLS["quickjs" if "--quickjs" in argv else "yt-dlp"]
    with open(ADDONS, encoding="utf-8", newline="") as f:
        text = f.read()
    lines, changed, first = [], False, None
    for t in pins:
        old = pinned(text, t)
        try:
            new = latest(t)
        except urllib.error.HTTPError as e:
            die(f"GitHub answered {e.code} {e.reason} for {e.url}" + (" (set GH_TOKEN for a higher rate limit)" if e.code in (403, 429) else ""))
        except (urllib.error.URLError, TimeoutError) as e:
            die(f"couldn't reach GitHub: {e}")
        first = first or (old, new)
        size = f"{old['size']:,} -> {new['size']:,} bytes" if old["size"] != new["size"] else f"{new['size']:,} bytes"
        facts = f"{size}, SHA-256 {new['sha256']}, the same in {new['agree']}"
        if version_key(new["tag"]) < version_key(old["tag"]):
            line = f"{t['asset']}: the pin, {old['tag']}, is newer than {t['title']}'s latest release, {new['tag']}; left as it is"
        elif (old["tag"], old["size"], old["sha256"]) == (new["tag"], new["size"], new["sha256"]):
            line = f"{t['asset']}: {old['tag']} is pinned, and it is {t['title']}'s latest release ({facts})"
        else:
            changed = True
            if new["tag"] == old["tag"]:
                what = f"the pin of {t['title']} {new['tag']} doesn't match the release"
            else:
                what = f"{t['title']} {new['tag']} is out, the pin is {old['tag']}"
            if check:
                line = f"{t['asset']}: {what} ({facts}). Run without --check to pin it."
            else:
                text = repin(text, t, new)
                if pinned(text, t) != {k: new[k] for k in ("tag", "url", "size", "sha256")}:
                    die("addons.rs doesn't read back as the new pin; nothing written")
                line = f"{t['asset']}: {what}; pinned {new['tag']} in addons.rs ({facts})"
        print(line)
        lines.append(line)
    # Written once every pin read back right: all of them move, or none.
    if changed and not check:
        with open(ADDONS, "w", encoding="utf-8", newline="") as f:
            f.write(text)
    old, new = first
    output(changed=str(changed).lower(), version=new["tag"], previous=old["tag"], line=" ".join(lines))


if __name__ == "__main__":
    main(sys.argv[1:])
