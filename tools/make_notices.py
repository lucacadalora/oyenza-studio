#!/usr/bin/env python3
"""Write the "Rust crates built into poni.exe" part of THIRD-PARTY-NOTICES.txt.

Usage:
    python3 tools/make_notices.py            # rewrite that part
    python3 tools/make_notices.py --check    # fail when it is out of date

Lists every crate poni.exe links on Windows (normal dependencies of `poni`
for x86_64-pc-windows-msvc in Cargo.lock; build tools and proc macros leave
nothing in the exe) and carries each one's MIT license text with its
copyright line, identical texts once. Every crate in that list offers MIT
(some also Apache-2.0 or the Unlicense), so Oyen Poni uses each under MIT; a
crate that doesn't offer MIT stops the script, to be handled by hand. A crate
that ships no license file gets it from tools/notices/<crate>.txt.

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
# Where a crate keeps its MIT text, most specific first.
MIT_FILES = ["license-mit", "license-mit.md", "license-mit.txt", "license", "license.md", "license.txt"]


def linked_crates():
    """[(name, version, license, source dir)] of the crates poni.exe links."""
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    meta = json.loads(out)
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == "poni" and p["source"] is None)
    seen, todo = set(), [root]
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            pkg = packages[dep["pkg"]]
            normal = any(k["kind"] is None for k in dep["dep_kinds"])
            macro = any("proc-macro" in t["kind"] for t in pkg["targets"])
            if normal and not macro:
                todo.append(dep["pkg"])
    crates = []
    for pid in seen:
        p = packages[pid]
        if p["source"] is not None:  # skip our own crates
            crates.append((p["name"], p["version"], p["license"] or "", os.path.dirname(p["manifest_path"])))
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


def section(crates):
    groups = {}
    for name, version, license_expr, directory in crates:
        groups.setdefault(mit_text(name, license_expr, directory), []).append(f"{name} {version}")
    out = [
        RULE,
        HEADING,
        "",
        "Written by tools/make_notices.py from Cargo.lock. Each of these crates offers the",
        "MIT license (some also Apache-2.0 or the Unlicense), and Oyen Poni uses each one",
        "under MIT. Crates with the same license text share one copy.",
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
    wanted = head.rstrip("\n") + "\n\n" + section(linked_crates())
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
