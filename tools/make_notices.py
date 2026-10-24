#!/usr/bin/env python3
"""Write the "Rust crates built into poni.exe" part of THIRD-PARTY-NOTICES.txt.

Usage:
    python3 tools/make_notices.py            # rewrite that part
    python3 tools/make_notices.py --check    # fail when it is out of date

Lists every crate poni.exe links on Windows (normal dependencies of `poni`
for x86_64-pc-windows-msvc, as `cargo tree` resolves them for that target:
`cargo metadata` would also count features that only other platforms turn
on; build tools and proc macros leave nothing in the exe) and carries each
one's MIT license text with its copyright line, identical texts once. Every
crate in that list offers MIT (some also Apache-2.0 or the Unlicense), so Oyen
Poni uses each under MIT; a crate that doesn't offer MIT stops the script,
unless it is in BY_HAND and its notice is written by hand above. A crate that
ships no license file gets it from tools/notices/<crate>.txt.

Everything above the part's heading line is kept as it is (the mecab-ipadic
and Unicode notices are written by hand). Needs cargo and the crate sources
(`cargo fetch --locked`). Python 3 standard library only.
"""

import json
import os
import subprocess
import sys
import textwrap

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NOTICES = os.path.join(ROOT, "THIRD-PARTY-NOTICES.txt")
TARGET = "x86_64-pc-windows-msvc"
RULE = "-" * 78
HEADING = "Rust crates built into poni.exe"
# Crates without an MIT option whose notice is written by hand above the
# heading (the script checks the notice is there).
BY_HAND = {"subtle"}
# Where a crate keeps its MIT text, most specific first.
MIT_FILES = ["license-mit", "license-mit.md", "license-mit.txt", "license", "license.md", "license.txt"]


def linked_crates():
    """[(name, version, license, source dir)] of the crates poni.exe links."""
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout)
    packages = {(p["name"], p["version"]): p for p in meta["packages"]}
    # `cargo tree` applies the feature resolver per target, as the build does.
    # Every path is printed (--no-dedupe) with its depth, so a proc macro's
    # whole subtree can be left out: it runs in the compiler, not in the exe.
    tree = subprocess.run(
        ["cargo", "tree", "--locked", "-p", "poni", "--target", TARGET, "-e", "normal", "--no-dedupe", "--prefix", "depth", "--format", "{p}"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    crates = set()
    skip_below = None  # depth of a proc macro whose subtree is being skipped
    for line in tree.splitlines():
        digits = len(line) - len(line.lstrip("0123456789"))
        if digits == 0:
            continue
        depth = int(line[:digits])
        parts = line[digits:].split()
        if skip_below is not None and depth > skip_below:
            continue
        skip_below = None
        if len(parts) < 2 or not parts[1].startswith("v"):
            continue
        p = packages.get((parts[0], parts[1][1:]))
        if p is None:
            continue
        if any("proc-macro" in t["kind"] for t in p["targets"]):
            skip_below = depth
            continue
        if p["source"] is None:  # our own crates
            continue
        crates.add((p["name"], p["version"], p["license"] or "", os.path.dirname(p["manifest_path"])))
    return sorted(crates)


def clean(text):
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").split("\n")]
    return "\n".join(lines).strip("\n") + "\n"


def mit_text(name, license_expr, directory):
    """The crate's MIT license text (stops the script if there is none)."""
    if "MIT" not in license_expr:
        sys.exit(f"{name}: its license ({license_expr or 'none given'}) doesn't offer MIT; add its notice to THIRD-PARTY-NOTICES.txt by hand")
    override = os.path.join(ROOT, "tools", "notices", f"{name}.txt")
    if os.path.exists(override):
        with open(override, encoding="utf-8") as f:
            return clean(f.read())
    files = {f.lower(): f for f in os.listdir(directory)}
    for want in MIT_FILES:
        if want in files:
            with open(os.path.join(directory, files[want]), encoding="utf-8") as f:
                text = f.read()
            if want.startswith("license-mit") or "Permission is hereby granted" in text:
                return clean(text)
    sys.exit(f"{name}: no MIT license file in the crate; add one as tools/notices/{name}.txt")


def section(crates, head):
    groups = {}
    for name, version, license_expr, directory in crates:
        if name in BY_HAND:
            if f"{name} {version}" not in head:
                sys.exit(f"{name} {version}: write its notice by hand above the crate list (BY_HAND)")
            continue
        groups.setdefault(mit_text(name, license_expr, directory), []).append(f"{name} {version}")
    out = [
        RULE,
        HEADING,
        "",
        "Written by tools/make_notices.py from Cargo.lock. Each of these crates offers the",
        "MIT license (some also Apache-2.0 or the Unlicense), and Oyen Poni uses each one",
        "under MIT, except those whose notice is written out above. Crates with the same",
        "license text share one copy.",
        "",
        # A no-break space keeps each name with its version while wrapping.
        textwrap.fill(", ".join(f"{n}\u00a0{v}" for n, v, _, _ in crates) + ".", 78, break_on_hyphens=False).replace("\u00a0", " "),
    ]
    for text, names in sorted(groups.items(), key=lambda g: g[1][0]):
        out += ["", RULE, "MIT license of: " + ", ".join(names), "", text.rstrip("\n")]
    return "\n".join(out) + "\n"


def main():
    args = sys.argv[1:]
    if args not in ([], ["--check"]):
        sys.exit(__doc__)
    with open(NOTICES, encoding="utf-8") as f:
        current = f.read()
    cut = current.find(RULE + "\n" + HEADING + "\n")
    head = current if cut < 0 else current[:cut]
    wanted = head.rstrip("\n") + "\n\n" + section(linked_crates(), head)
    if args == ["--check"]:
        if wanted != current:
            sys.exit("THIRD-PARTY-NOTICES.txt is out of date: run python3 tools/make_notices.py")
        print("THIRD-PARTY-NOTICES.txt is up to date")
        return
    with open(NOTICES, "w", encoding="utf-8", newline="\n") as f:
        f.write(wanted)
    print(f"wrote {os.path.relpath(NOTICES, ROOT)}")


if __name__ == "__main__":
    main()
