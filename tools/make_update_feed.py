#!/usr/bin/env python3
"""Write latest.json, the file every copy of Oyenza Studio reads to learn that
a new version is out (docs/updates.md). The Website workflow publishes it as
https://studio.oyenza.com/latest.json with every deploy.

It describes the latest GitHub release: its version and day, the first bold
line of its notes as a headline, and the Windows installer and Mac DMG with
their sizes and SHA-256. That description (the payload) is signed with ECDSA
P-256 over SHA-256 with the key in UPDATE_FEED_KEY. The app believes none of
it until the signature checks out against a key in
crates/poni-core/data/update-keys.txt, and this script refuses to sign with a
key that isn't listed there.

Usage:
    python3 tools/make_update_feed.py OUT.json [--released VERSION]
    python3 tools/make_update_feed.py --check [--released VERSION]

    --released  a version that counts as released even while GitHub doesn't
                list it as the latest yet (the release run says which)
    --check     build and print the payload only: no key, nothing signed

Env: UPDATE_FEED_KEY (the private key, PEM), GH_TOKEN (optional: raises the
API's rate limit), GITHUB_REPOSITORY (default lucacadalora/oyenza-studio).
Needs python3 (standard library only) and openssl.
"""

import base64
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KEYS = os.path.join(ROOT, "crates", "poni-core", "data", "update-keys.txt")
REPO = os.environ.get("GITHUB_REPOSITORY") or "lucacadalora/oyenza-studio"
API = "https://api.github.com/repos/" + REPO
SCHEMA = 1
# The app shows the headline on the island, one line: keep it short.
HEADLINE_MAX = 90


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def fetch(url, accept="application/vnd.github+json"):
    """GET with a few tries: GitHub's API hiccups now and then."""
    headers = {"Accept": accept, "User-Agent": "oyenza-studio-update-feed"}
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = "Bearer " + token
    for attempt in range(4):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code == 404 or attempt == 3:
                raise
        except (urllib.error.URLError, TimeoutError):
            if attempt == 3:
                raise
        time.sleep(2 + attempt * 3)


def api(path):
    return json.loads(fetch(API + path))


def version_tuple(v):
    return tuple(int(p) for p in v.split("."))


def is_version(v):
    return bool(re.fullmatch(r"[0-9]+(\.[0-9]+){1,3}", v or ""))


def release(released):
    """The release the file describes: GitHub's latest, or RELEASED when it is newer."""
    latest = api("/releases/latest")
    version = latest["tag_name"].removeprefix("v")
    if released:
        if not is_version(released):
            die(f'--released "{released}" is not a version')
        if not is_version(version) or version_tuple(released) > version_tuple(version):
            try:
                return api(f"/releases/tags/v{released}")
            except urllib.error.HTTPError as e:
                die(f"v{released}: GitHub answers {e.code} (not released yet?)")
    return latest


def headline(notes, version):
    """The notes' first bold line ("**Sing along from the closed island.**")."""
    m = re.search(r"\*\*(.+?)\*\*", notes or "", re.S)
    text = " ".join(m.group(1).split()).rstrip(".") if m else f"Oyenza Studio {version}"
    if len(text) > HEADLINE_MAX:
        text = text[: HEADLINE_MAX - 1].rsplit(" ", 1)[0].rstrip(",;:") + "…"
    return text


def asset(rel, name, version):
    """One file of the release: its address, size and SHA-256, or None."""
    a = next((a for a in rel.get("assets", []) if a["name"] == name), None)
    if a is None:
        return None
    url = a["browser_download_url"]
    prefix = f"https://github.com/{REPO}/releases/download/v{version}/"
    if not url.startswith(prefix):
        die(f"{name}: unexpected address {url}")
    digest = a.get("digest") or ""
    if digest.startswith("sha256:") and re.fullmatch(r"[0-9a-f]{64}", digest[7:]):
        sha = digest[7:]
    else:
        body = fetch(url, accept="application/octet-stream")
        if len(body) != a["size"]:
            die(f"{name}: downloaded {len(body)} bytes, GitHub lists {a['size']}")
        sha = hashlib.sha256(body).hexdigest()
    return {"url": url, "size": a["size"], "sha256": sha}


def payload(released):
    rel = release(released)
    version = rel["tag_name"].removeprefix("v")
    if not is_version(version):
        die(f'the release "{rel["tag_name"]}" is not a version tag')
    if rel.get("draft") or rel.get("prerelease"):
        die(f"v{version} is a draft or a pre-release")
    windows = asset(rel, f"Oyenza-Studio-Setup-{version}.exe", version)
    if windows is None:
        die(f"v{version} has no Oyenza-Studio-Setup-{version}.exe")
    p = {
        "schema": SCHEMA,
        "version": version,
        "date": (rel.get("published_at") or "")[:10],
        "headline": headline(rel.get("body"), version),
        "notes": rel["html_url"],
        "windows": windows,
    }
    # The mac workflow adds the DMG minutes after the release is up, then
    # starts the Website workflow again: until then the file says nothing to
    # Macs, so they aren't sent to a download that isn't there yet.
    mac = asset(rel, f"Oyenza-Studio-{version}-mac.dmg", version)
    if mac is not None:
        p["mac"] = mac
    return p


def trusted_keys():
    keys = {}
    with open(KEYS, encoding="utf-8") as f:
        for line in f:
            parts = line.split()
            if len(parts) == 2 and not line.startswith("#"):
                keys[parts[0]] = bytes.fromhex(parts[1])
    return keys


def openssl(*args, data=None):
    r = subprocess.run(["openssl", *args], input=data, capture_output=True)
    if r.returncode != 0:
        die(f"openssl {args[0]} failed: {r.stderr.decode(errors='replace').strip()}")
    return r.stdout


def der_to_raw(der):
    """ECDSA-Sig-Value (SEQUENCE of two INTEGERs) to r || s, 32 bytes each."""
    if len(der) < 8 or der[0] != 0x30:
        die("the signature isn't DER")
    i = 3 if der[1] == 0x81 else 2
    out = b""
    for _ in range(2):
        if der[i] != 0x02:
            die("the signature isn't DER")
        n = der[i + 1]
        num = der[i + 2 : i + 2 + n].lstrip(b"\0")
        if len(num) > 32:
            die("the signature's numbers are too long for P-256")
        out += num.rjust(32, b"\0")
        i += 2 + n
    return out


def sign(text):
    pem = os.environ.get("UPDATE_FEED_KEY", "")
    if "PRIVATE KEY" not in pem:
        die("UPDATE_FEED_KEY is not set (the release environment's secret)")
    with tempfile.TemporaryDirectory() as tmp:
        key = os.path.join(tmp, "key.pem")
        fd = os.open(key, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(pem if pem.endswith("\n") else pem + "\n")
        public = openssl("ec", "-in", key, "-pubout", "-outform", "DER")[-65:]
        key_id = hashlib.sha256(public).hexdigest()[:16]
        if trusted_keys().get(key_id) != public:
            die(f"key {key_id} is not in {os.path.relpath(KEYS, ROOT)}, so the app would reject the file")
        data = os.path.join(tmp, "payload")
        with open(data, "wb") as f:
            f.write(text.encode("utf-8"))
        der = openssl("dgst", "-sha256", "-sign", key, data)
        # Check it the way the app will, with the public key alone.
        pub = os.path.join(tmp, "pub.pem")
        with open(pub, "wb") as f:
            f.write(openssl("ec", "-in", key, "-pubout"))
        sig = os.path.join(tmp, "sig.der")
        with open(sig, "wb") as f:
            f.write(der)
        openssl("dgst", "-sha256", "-verify", pub, "-signature", sig, data)
    return key_id, base64.b64encode(der_to_raw(der)).decode()


def main(argv):
    check = "--check" in argv
    released = None
    rest = []
    it = iter(argv)
    for a in it:
        if a == "--released":
            released = next(it, "") or None
        elif a != "--check":
            rest.append(a)
    if check == bool(rest) or len(rest) > 1:
        die("usage: make_update_feed.py OUT.json [--released VERSION], or --check [--released VERSION]")
    p = payload(released)
    text = json.dumps(p, ensure_ascii=False, separators=(",", ":"))
    if check:
        print(json.dumps(p, ensure_ascii=False, indent=2))
        return
    key_id, signature = sign(text)
    feed = {
        "about": "Oyenza Studio's update file. payload is signed (ECDSA P-256, SHA-256) with the key named by key; see docs/updates.md.",
        "payload": text,
        "signature": signature,
        "key": key_id,
    }
    with open(rest[0], "w", encoding="utf-8") as f:
        json.dump(feed, f, ensure_ascii=False, indent=2)
        f.write("\n")
    line = f"latest.json: v{p['version']} ({p['date']}), \"{p['headline']}\", Windows {p['windows']['size']} B" + (
        f", Mac {p['mac']['size']} B" if "mac" in p else ", no Mac DMG yet"
    ) + f", signed with key {key_id}"
    print(line)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(line + "\n")


if __name__ == "__main__":
    main(sys.argv[1:])
