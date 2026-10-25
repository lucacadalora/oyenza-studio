#!/usr/bin/env python3
"""Pin the Oyenza Studio Recorder release the Screen recorder add-on installs.

Reads the release's assets from GitHub's API (their size and the SHA-256
GitHub keeps for each) and writes the version, the two installers' names,
sizes and SHA-256 into crates/poni-core/src/recorder.rs. Run it once the
recorder release is published, then run `cargo test -p poni-core recorder`.

    python3 tools/pin_recorder.py v1.0.0
"""

import json
import re
import sys
import urllib.request
from pathlib import Path

REPO = "lucacadalora/oyen-rekam"
SOURCE = Path(__file__).resolve().parent.parent / "crates" / "poni-core" / "src" / "recorder.rs"
WANTED = {"WINDOWS_SETUP": re.compile(r"-setup\.exe$"), "MAC_DMG": re.compile(r"-mac\.dmg$")}


def release(tag: str) -> dict:
    request = urllib.request.Request(f"https://api.github.com/repos/{REPO}/releases/tags/{tag}", headers={"Accept": "application/vnd.github+json", "User-Agent": "oyenza-studio-pin-recorder"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def main() -> None:
    if len(sys.argv) != 2 or not re.fullmatch(r"v\d+\.\d+\.\d+", sys.argv[1]):
        raise SystemExit("usage: pin_recorder.py vX.Y.Z")
    tag = sys.argv[1]
    version = tag[1:]
    rel = release(tag)
    if rel.get("draft") or rel.get("prerelease"):
        raise SystemExit(f"{tag} is a draft or a pre-release; pin a published release")
    text = SOURCE.read_text()
    text = re.sub(r'pub const VERSION: &str = "[^"]*";', f'pub const VERSION: &str = "{version}";', text, count=1)
    for const, pattern in WANTED.items():
        assets = [a for a in rel["assets"] if pattern.search(a["name"]) and version in a["name"]]
        if len(assets) != 1:
            raise SystemExit(f"{tag}: expected one asset matching {pattern.pattern}, found {[a['name'] for a in assets]}")
        asset = assets[0]
        digest = asset.get("digest") or ""
        if not digest.startswith("sha256:"):
            raise SystemExit(f"{asset['name']}: GitHub gave no SHA-256 for it")
        line = f'pub const {const}: AddonFile = AddonFile {{ name: "{asset["name"]}", size: {asset["size"]}, sha256: "{digest[7:]}" }};'
        text, found = re.subn(rf"pub const {const}: AddonFile = AddonFile \{{[^}}]*\}};", line, text, count=1)
        if found != 1:
            raise SystemExit(f"{SOURCE.name} no longer has {const} the way this script expects")
        print(f"{const}: {asset['name']}, {asset['size']} bytes, sha256 {digest[7:19]}...")
    SOURCE.write_text(text)
    print(f"Pinned {tag} in {SOURCE.relative_to(SOURCE.parents[3])}")


if __name__ == "__main__":
    main()
