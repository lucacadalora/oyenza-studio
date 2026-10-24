#!/usr/bin/env python3
"""End-to-end test of paired phone drop (docs/paired-drop.md).

Starts the meeting server (server/drop-server.mjs) on a free port with short
long polls, opens the PC emulator (emulator.html, the PC half of the contract
with the browser's own WebRTC) in one browser context and the phone page in a
second, phone-sized context, then pairs, sends, saves and checks the error
paths, for an iPhone user agent and an Android one. Screenshots of every view
go to tools/pairdrop-e2e/shots/.

Run (browsers through the VM gate):
    vm-gate run -- python3 tools/pairdrop-e2e/run_e2e.py
Options: --only iphone|android, --webkit (also run the iPhone phone in WebKit,
Safari's engine; the PC emulator stays in Chromium), --headed
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from pathlib import Path

from playwright.sync_api import Page, expect, sync_playwright

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SHOTS = HERE / "shots"
PC = "RTX-5090-Astral"

UAS = {
    "iphone": "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1",
    "android": "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36",
}
# The PC emulator's browser shows its real host addresses, as the native PC app
# does; the phone's browser keeps Chromium's default (host addresses hidden
# behind mDNS names), as a real phone browser does.
PC_BROWSER_ARGS = ["--disable-features=WebRtcHideLocalIpsWithMdns"]

HOLD_MS = 3000  # long polls answer after 3 s instead of 25 s
ONLINE_MS = 5000  # a PC is offline 5 s after its last wait instead of 35 s


def b64u(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class Checks:
    def __init__(self) -> None:
        self.results: list[tuple[bool, str, str]] = []

    def check(self, ok: bool, what: str, detail: str = "") -> bool:
        self.results.append((bool(ok), what, detail))
        print(f"  {'PASS' if ok else 'FAIL'}  {what}{'  -- ' + detail if detail and not ok else ''}", flush=True)
        return bool(ok)

    def expect(self, what: str, fn) -> bool:
        try:
            fn()
            return self.check(True, what)
        except Exception as e:  # noqa: BLE001
            return self.check(False, what, f"{type(e).__name__}: {str(e).splitlines()[0][:300]}")

    @property
    def failed(self) -> list[tuple[bool, str, str]]:
        return [r for r in self.results if not r[0]]


class Server:
    def __init__(self, data_dir: str) -> None:
        cmd = [
            "node", str(ROOT / "server" / "drop-server.mjs"), "--port", "0", "--host", "127.0.0.1",
            "--data", data_dir, "--hold-ms", str(HOLD_MS), "--online-ms", str(ONLINE_MS), "--rate", "100000",
        ]
        self.proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        line = self.proc.stdout.readline()
        m = re.search(r"http://127\.0\.0\.1:(\d+)", line or "")
        if not m:
            self.proc.kill()
            raise RuntimeError(f"server did not start: {line!r} {self.proc.stderr.read()}")
        self.base = f"http://127.0.0.1:{m.group(1)}"
        self.errors: list[str] = []
        threading.Thread(target=self._drain, daemon=True).start()

    def _drain(self) -> None:
        for line in self.proc.stderr:
            self.errors.append(line.rstrip())

    def stop(self) -> None:
        self.proc.terminate()
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def serve_emulator(route) -> None:
    name = route.request.url.split("/__emu/", 1)[1].split("?", 1)[0]
    path = HERE / name
    if "/" in name or not path.is_file():
        route.fulfill(status=404, body="not here")
        return
    types = {".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8"}
    route.fulfill(status=200, body=path.read_bytes(), headers={"Content-Type": types.get(path.suffix, "application/octet-stream"), "Cache-Control": "no-store"})


class Run:
    """One phone (user agent) against one emulated PC."""

    def __init__(self, browser, server: Server, kind: str, c: Checks, phone_browser=None, label: str | None = None) -> None:
        self.browser = browser
        self.phone_browser = phone_browser or browser
        self.server = server
        self.kind = kind
        self.label = label or kind
        self.c = c
        self.base = server.base
        self.emu_ctx = browser.new_context()
        self.emu_ctx.route("**/__emu/**", serve_emulator)
        self.emu = self.emu_ctx.new_page()
        self.emu.on("console", lambda m: m.type == "error" and print(f"    [emu console] {m.text}"))
        self.emu.goto(f"{self.base}/__emu/emulator.html")
        self.emu.wait_for_function("window.emu && window.emu.client")
        self.ctx = self.phone_context()
        self.phone = self.ctx.new_page()
        self.console_errors: list[str] = []
        self.phone.on("console", lambda m: m.type == "error" and self.console_errors.append(m.text))
        self.phone.on("pageerror", lambda e: self.console_errors.append(f"pageerror: {e}"))

    def phone_context(self, init_script: str | None = None):
        opts = dict(viewport={"width": 390, "height": 844}, device_scale_factor=1, has_touch=True, user_agent=UAS[self.kind], accept_downloads=True)
        if self.phone_browser.browser_type.name == "chromium":
            opts["is_mobile"] = True
        ctx = self.phone_browser.new_context(**opts)
        if init_script:
            ctx.add_init_script(init_script)
        return ctx

    def shot(self, page: Page, name: str) -> None:
        page.wait_for_timeout(150)
        path = SHOTS / f"{self.label}-{name}.png"
        before = len(self.console_errors)
        page.screenshot(path=str(path), full_page=True)
        page.wait_for_timeout(50)
        # In WebKit, Playwright's screenshot injects a <style> that the page's CSP
        # (rightly) refuses; that console line is the test's, not the page's.
        self.console_errors[before:] = [e for e in self.console_errors[before:] if not e.startswith("Refused to apply a stylesheet")]
        print(f"    shot {path.relative_to(ROOT)}")

    def emu_eval(self, js: str, arg=None):
        return self.emu.evaluate(js, arg)

    def summary(self) -> dict:
        return self.emu_eval("emu.summary()")

    def events(self, kind: str) -> list[dict]:
        return [e for e in self.summary()["events"] if e["type"] == kind]

    # ------------------------------------------------------------------ steps

    def run(self) -> None:
        c = self.c
        phone = self.phone
        s1 = b64u(secrets.token_bytes(16))
        shelf_pdf = secrets.token_bytes(274_000)
        shelf_jpg = secrets.token_bytes(41_000)
        self.emu_eval(
            "(list) => emu.setShelf(list)",
            [{"name": "notes.pdf", "b64": base64.b64encode(shelf_pdf).decode()}, {"name": "beach.jpg", "b64": base64.b64encode(shelf_jpg).decode()}],
        )
        self.emu_eval("(s) => emu.addPair(s)", s1)
        self.emu_eval("emu.start()")
        self.emu_eval("emu.pairDelayMs = 2500")

        print(f"  [{self.label}] pairing")
        phone.goto(f"{self.base}/p#k={s1}")
        c.expect("pairing view shows while the PC answers", lambda: expect(phone.locator("#pair-title")).to_have_text("Pairing with your PC…", timeout=5000))
        self.shot(phone, "01-pairing")
        c.expect("paired with the PC's name from welcome", lambda: expect(phone.locator("#pair-title")).to_have_text(f"Paired with {PC}", timeout=20000))
        self.emu_eval("emu.pairDelayMs = 0")
        pairs = self.summary()["pairs"]
        want_name = "iPhone" if self.kind == "iphone" else ("Android phone", "Pixel 9")
        got_name = pairs[0]["phoneName"] if pairs else None
        c.check(got_name == want_name if isinstance(want_name, str) else got_name in want_name, "the PC got the phone's name in pair", f"got {got_name!r}")
        c.check(f"k={s1}" in phone.url and "n=RTX-5090-Astral" in phone.url, "the address keeps #k= (and the PC name) after pairing", phone.url)
        manifest_links = phone.locator('link[rel="manifest"]').count()
        if self.kind == "iphone":
            c.check(manifest_links == 0, "no manifest link on iOS")
            c.expect("iOS: Share, then Add to Home Screen", lambda: expect(phone.locator("#keep-ios")).to_contain_text("Tap Share, then Add to Home Screen."))
            c.expect("iOS: Copy pairing link button", lambda: expect(phone.locator("#copy-link")).to_be_visible())
        else:
            c.check(manifest_links == 1, "manifest link added from JS on Android")
            c.expect(
                "Android: install button or Menu › Install app",
                lambda: expect(phone.locator("#install:visible, #keep-menu:visible")).to_have_count(1),
            )
        stored = phone.evaluate("JSON.parse(localStorage.getItem('oyen-drop.v1'))")
        c.check(stored and stored["pairs"] and stored["pairs"][0]["k"] == s1 and stored["pairs"][0]["name"] == PC, "pair stored in oyen-drop.v1", json.dumps(stored))
        self.shot(phone, "02-paired")

        print(f"  [{self.label}] home")
        phone.click("#pair-done")
        c.expect("home lists the PC as online", lambda: expect(phone.locator("#pcs")).to_contain_text(f"{PC}Online", timeout=15000))
        my_name = "Luca’s iPhone" if self.kind == "iphone" else "Luca’s Pixel"
        phone.fill("#phone-name", f"  {my_name}\n")
        phone.press("#phone-name", "Enter")
        c.expect("phone name saved", lambda: expect(phone.locator("#phone-name-note")).to_have_text("Saved. Your PCs show this name."))
        stored = phone.evaluate("JSON.parse(localStorage.getItem('oyen-drop.v1'))")
        c.check(stored.get("phoneName") == my_name, "phone name kept in oyen-drop.v1", json.dumps(stored.get("phoneName")))
        self.shot(phone, "03-home")

        print(f"  [{self.label}] PC view and connect")
        phone.click("#pcs .pc-open")
        c.expect("PC view title", lambda: expect(phone.locator("#pc-title")).to_have_text(f"Send to {PC}"))
        connected = c.expect(
            "connected, with the road shown",
            lambda: expect(phone.locator("#road")).to_have_attribute("data-road", re.compile("wifi|internet|relay|connected"), timeout=40000),
        )
        if not connected:
            print("    emulator events:", json.dumps(self.summary()["events"][-15:]))
            return
        c.expect("road: straight over your Wi-Fi (both on this machine)", lambda: expect(phone.locator("#road-text")).to_have_text("Straight over your Wi-Fi"))
        c.expect("the Shelf from the PC's hello", lambda: expect(phone.locator("#shelf")).to_contain_text("notes.pdf"))
        hello = self.events("hello")
        c.check(bool(hello) and hello[-1].get("v") == 1, "phone said hello v1 first", json.dumps(hello[-1:]))
        offers = self.events("offer")
        c.check(bool(offers) and offers[-1].get("name") == my_name and hello[-1].get("name") == my_name, "offer and hello carry the new phone name", json.dumps(offers[-1:]))
        self.shot(phone, "04-pc-idle")

        print(f"  [{self.label}] send two files")
        big = secrets.token_bytes(3 * 1024 * 1024 + 12345)
        tiny = b"Hello from the phone.\n"
        self.emu_eval("emu.paceMs = 3000")
        phone.set_input_files("#pick", files=[
            {"name": "random-3mb.bin", "mimeType": "application/octet-stream", "buffer": big},
            {"name": "note.txt", "mimeType": "text/plain", "buffer": tiny},
        ])
        c.expect("chosen files listed", lambda: expect(phone.locator("#chosen-count")).to_have_text("2 files"))
        phone.click("#send")
        c.expect("progress bar while sending", lambda: expect(phone.locator("#progress")).to_be_visible(timeout=20000))
        phone.wait_for_timeout(1200)
        self.shot(phone, "05-sending")
        c.expect("sent message names the PC", lambda: expect(phone.locator("#send-status-text")).to_have_text(f"Sent 2 files to {PC}", timeout=40000))
        self.emu_eval("emu.paceMs = 0")
        got = {r["name"]: r for r in self.summary()["received"]}
        c.check(got.get("random-3mb.bin", {}).get("sha256") == sha(big), "PC got the 3 MB file byte for byte", json.dumps(got.get("random-3mb.bin")))
        c.check(got.get("note.txt", {}).get("sha256") == sha(tiny), "PC got the tiny file byte for byte")
        c.expect("the Shelf update shows the new files", lambda: expect(phone.locator("#shelf")).to_contain_text("random-3mb.bin"))
        self.shot(phone, "06-sent")

        print(f"  [{self.label}] save a Shelf file")
        row = phone.locator("#shelf li", has_text="notes.pdf")
        row.locator("button.save").click()
        link = row.locator("a.save.ready")
        if c.expect("a download link once the bytes are in", lambda: expect(link).to_be_visible(timeout=20000)):
            with phone.expect_download(timeout=20000) as dl:
                link.click()
            data = Path(dl.value.path()).read_bytes()
            c.check(dl.value.suggested_filename == "notes.pdf", "download keeps the file name", dl.value.suggested_filename)
            c.check(sha(data) == sha(shelf_pdf) and len(data) == len(shelf_pdf), "saved Shelf file is byte for byte", f"{len(data)} bytes")
        self.shot(phone, "07-saved")
        self.refusals(c)

        print(f"  [{self.label}] busy")
        self.emu_eval("emu.busy = true")
        phone.click("#pc-back")
        expect(phone.locator("#v-home")).to_be_visible()
        c.expect("leaving the PC view says bye", lambda: self.emu.wait_for_function("emu.events.some((e) => e.type === 'bye')", timeout=5000))
        phone.click("#pcs .pc-open")
        c.expect("busy message", lambda: expect(phone.locator("#pc-error-text")).to_have_text(f"{PC} is busy with other phones. Try again in a moment.", timeout=30000))
        self.shot(phone, "08-busy")
        self.emu_eval("emu.busy = false")
        phone.click("#pc-retry")
        c.expect("try again connects once the PC is free", lambda: expect(phone.locator("#road")).to_have_attribute("data-road", re.compile("wifi|internet|relay|connected"), timeout=40000))

        if self.kind == "android":
            self.share_sheet(c)

        if self.kind == "iphone":
            self.ios_home_screen(c, s1)

        print(f"  [{self.label}] offline")
        phone.goto(f"{self.base}/")
        self.emu_eval("emu.stop()")
        phone.wait_for_timeout(ONLINE_MS + 1500)
        phone.reload()
        c.expect("home shows the PC offline with when it was seen", lambda: expect(phone.locator("#pcs")).to_contain_text(f"{PC}Offline · seen just now", timeout=15000))
        self.shot(phone, "10-offline")
        phone.click("#pcs .pc-open")
        c.expect("PC view says the PC is offline", lambda: expect(phone.locator("#pc-error-text")).to_have_text(f"{PC} is offline. Start Oyen Poni on it, then try again.", timeout=30000))
        self.shot(phone, "11-pc-offline")

        print(f"  [{self.label}] removed on the PC")
        status = self.emu_eval("(s) => emu.unpair(s)", s1)
        c.check(status == 200, "PC unpair answered ok")
        # iOS Safari keeps the pairing link in the address after pairing
        phone.goto(f"{self.base}/p#k={s1}&n={PC}" if self.kind == "iphone" else f"{self.base}/")
        c.expect("home shows Removed from this PC", lambda: expect(phone.locator("#pcs")).to_contain_text("Removed from this PC", timeout=15000))
        self.shot(phone, "12-gone")
        phone.click("#pcs button.save:has-text('Forget')")
        c.expect("empty state after Forget", lambda: expect(phone.locator("#empty")).to_contain_text("No PC paired yet"))
        stored = phone.evaluate("JSON.parse(localStorage.getItem('oyen-drop.v1'))")
        c.check(stored["pairs"] == [], "forgotten in storage")
        c.check("k=" not in phone.url, "the address no longer carries the forgotten pairing", phone.url)
        phone.reload()
        c.expect("still forgotten after a reload", lambda: expect(phone.locator("#empty")).to_be_visible())
        self.shot(phone, "13-empty")

        print(f"  [{self.label}] pairing with no PC listening (retries for 20 s)")
        s2 = b64u(secrets.token_bytes(16))
        t0 = time.time()
        phone.goto(f"{self.base}/p#k={s2}")
        c.expect(
            "pairing gives up with the plain message",
            lambda: expect(phone.locator("#pair-status-text")).to_have_text(
                "Your PC didn't answer. Keep Shelf › Phone › Pair a phone open on the PC and scan again.", timeout=40000),
        )
        waited = time.time() - t0
        c.check(18 <= waited <= 30, "it kept asking for about 20 s first", f"{waited:.1f} s")
        self.shot(phone, "14-pair-failed")

        print(f"  [{self.label}] pairing when the PC starts listening a moment late")
        s3 = b64u(secrets.token_bytes(16))
        self.emu_eval("emu.start()")
        phone.goto(f"{self.base}/p#k={s3}")
        phone.wait_for_timeout(4000)
        self.emu_eval("(s) => emu.addPair(s)", s3)
        c.expect("late PC still pairs (retry every 1.5 s)", lambda: expect(phone.locator("#pair-title")).to_have_text(f"Paired with {PC}", timeout=25000))

        print(f"  [{self.label}] first connect right after pairing, while the PC is briefly away")
        phone.click("#pair-done")
        expect(phone.locator("#v-home")).to_be_visible()
        self.emu_eval("emu.stop()")
        phone.wait_for_timeout(ONLINE_MS + 1000)
        offers_before = len(self.events("offer"))
        phone.click("#pcs .pc-open")
        phone.wait_for_timeout(2500)
        self.emu_eval("emu.start()")
        c.expect(
            "the offer is retried for up to 6 s right after pairing",
            lambda: expect(phone.locator("#road")).to_have_attribute("data-road", re.compile("wifi|internet|relay|connected"), timeout=30000),
        )
        c.check(len(self.events("offer")) == offers_before + 1, "one offer reached the PC")
        phone.click("#pc-back")

        c.check(not self.console_errors, "no errors in the phone page console", " | ".join(self.console_errors[:5]))

    def refusals(self, c: Checks) -> None:
        """The PC says no, fails part way, or asks its user first."""
        phone = self.phone
        print(f"  [{self.label}] declined, failed part way, PC asks first")
        self.emu_eval("emu.decline = true")
        phone.set_input_files("#pick", files=[{"name": "a.txt", "mimeType": "text/plain", "buffer": b"a"}])
        phone.click("#send")
        c.expect("declined in a plain sentence", lambda: expect(phone.locator("#send-status-text")).to_have_text(f"Declined on {PC}.", timeout=20000))
        self.emu_eval("emu.decline = false; emu.failAfter = 1000000")
        phone.click("#clear")
        big = secrets.token_bytes(2_500_000)
        phone.set_input_files("#pick", files=[{"name": "big.bin", "mimeType": "application/octet-stream", "buffer": big}])
        phone.click("#send")
        c.expect(
            "a failure part way is a plain sentence",
            lambda: expect(phone.locator("#send-status-text")).to_have_text(f"{PC} is out of disk space. Nothing was kept.", timeout=30000),
        )
        c.expect("the phone hangs up after a failed send", lambda: expect(phone.locator("#road")).to_have_attribute("data-road", "paused", timeout=10000))
        self.emu_eval("emu.failAfter = 0; emu.auto = false; emu.askMs = 2000")
        phone.click("#send")
        c.expect(
            "a PC that asks first: the phone says so",
            lambda: expect(phone.locator("#send-status-text")).to_have_text(f"Waiting for {PC} to accept. Look at the island on the PC.", timeout=30000),
        )
        self.shot(phone, "07b-asking")
        c.expect("then it sends once accepted", lambda: expect(phone.locator("#send-status-text")).to_have_text(f"Sent 1 file to {PC}", timeout=30000))
        got = [r for r in self.summary()["received"] if r["name"] == "big.bin"]
        c.check(len(got) == 1 and got[0]["sha256"] == sha(big), "the resent file arrived byte for byte (the failed one was not kept)")
        self.emu_eval("emu.auto = true")

    def share_sheet(self, c: Checks) -> None:
        """Android share target: POST /share through the service worker, then /?share=<id>."""
        phone = self.phone
        print(f"  [{self.label}] share sheet")
        phone.click("#pc-back")
        phone.evaluate("navigator.serviceWorker.ready.then(() => true)")
        if not phone.evaluate("Boolean(navigator.serviceWorker.controller)"):
            phone.reload()
            phone.wait_for_function("Boolean(navigator.serviceWorker.controller)", timeout=10000)
        photos = [(f"IMG_00{i}.jpg", secrets.token_bytes(150_000 + i * 1000)) for i in range(1, 4)]
        phone.evaluate("""() => {
            const f = document.createElement('form');
            f.id = 'share-form'; f.method = 'post'; f.action = '/share'; f.enctype = 'multipart/form-data';
            const i = document.createElement('input');
            i.type = 'file'; i.name = 'files'; i.multiple = true; i.id = 'share-input';
            f.appendChild(i); document.body.appendChild(f);
        }""")
        phone.set_input_files("#share-input", files=[{"name": n, "mimeType": "image/jpeg", "buffer": b} for n, b in photos])
        with phone.expect_navigation():
            phone.evaluate("document.getElementById('share-form').submit()")
        c.check(re.search(r"/\?share=[0-9a-f]{24}$", phone.url) is not None, "the service worker took the share and opened /?share=<id>", phone.url)
        c.expect("share view: Send 3 photos", lambda: expect(phone.locator("#share-title")).to_have_text("Send 3 photos"))
        button = phone.locator("#share-pcs button", has_text=PC)
        c.expect("a button for the online PC", lambda: expect(button).to_be_enabled(timeout=15000))
        self.shot(phone, "09-share")
        before = len(self.summary()["received"])
        button.click()
        c.expect("shared photos sent", lambda: expect(phone.locator("#share-status-text")).to_have_text(f"Sent 3 photos to {PC}", timeout=40000))
        got = self.summary()["received"][before:]
        c.check([g["sha256"] for g in got] == [sha(b) for _, b in photos], "PC got the 3 shared photos byte for byte", json.dumps(got))
        left = phone.evaluate("""() => new Promise((res) => { const r = indexedDB.open('oyen-drop', 1);
            r.onsuccess = () => { const q = r.result.transaction('shared').objectStore('shared').count(); q.onsuccess = () => res(q.result); }; })""")
        c.check(left == 0, "the shared files are deleted after sending", f"{left} left")
        self.shot(phone, "09b-share-sent")
        phone.click("#share-done")
        expect(phone.locator("#v-home")).to_be_visible()
        phone.goto(f"{self.base}/?shared=lost")
        c.expect("a share that missed the service worker says so", lambda: expect(phone.locator("#home-note")).to_have_text("The shared files didn't come through. Share them again now."))

    def ios_home_screen(self, c: Checks, s1: str) -> None:
        """Home Screen app (standalone, own storage) opened at the saved #k= address."""
        print(f"  [{self.label}] Home Screen app takes the pairing in")
        pairs_before = len(self.events("pair"))
        ctx = self.phone_context("Object.defineProperty(Navigator.prototype, 'standalone', { get: () => true });")
        page = ctx.new_page()
        page.goto(f"{self.base}/p#k={s1}&n={PC}")
        c.expect("standalone app lists the PC from the address", lambda: expect(page.locator("#pcs")).to_contain_text(f"{PC}Online", timeout=15000))
        c.check(page.url == f"{self.base}/", "standalone app tidies the address", page.url)
        c.check(len(self.events("pair")) == pairs_before, "no second pairing message for an import")
        page.goto(f"{self.base}/p#k={s1}&n=Other")
        stored = page.evaluate("JSON.parse(localStorage.getItem('oyen-drop.v1'))")
        c.check(len(stored["pairs"]) == 1 and stored["pairs"][0]["name"] == PC, "importing again changes nothing", json.dumps(stored))
        c.expect("paste field for pairing links in the Home Screen app", lambda: expect(page.locator("#paste-card")).to_be_visible())
        ctx.close()

    def close(self) -> None:
        for ctx in (self.ctx, self.emu_ctx):
            try:
                ctx.close()
            except Exception:  # noqa: BLE001
                pass


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", choices=sorted(UAS))
    ap.add_argument("--headed", action="store_true")
    ap.add_argument("--webkit", action="store_true")
    args = ap.parse_args()
    SHOTS.mkdir(parents=True, exist_ok=True)
    data_dir = tempfile.mkdtemp(prefix="pairdrop-e2e-")
    server = Server(data_dir)
    print(f"meeting server at {server.base}  (hold {HOLD_MS} ms, online window {ONLINE_MS} ms)")
    c = Checks()
    t0 = time.time()
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch(headless=not args.headed, args=PC_BROWSER_ARGS)
            phones = pw.chromium.launch(headless=not args.headed)
            print(f"chromium {browser.version}: PC emulator with {' '.join(PC_BROWSER_ARGS)}, phones with defaults")
            runs = [(kind, phones, kind) for kind in ([args.only] if args.only else ["iphone", "android"])]
            webkit = None
            if args.webkit:
                webkit = pw.webkit.launch(headless=not args.headed)
                print(f"webkit {webkit.version} for the extra iPhone run")
                runs.append(("iphone", webkit, "webkit-iphone"))
            for kind, phone_browser, label in runs:
                print(f"\n== {label} ==")
                run = Run(browser, server, kind, c, phone_browser, label)
                try:
                    run.run()
                except Exception as e:  # noqa: BLE001
                    traceback.print_exc()
                    c.check(False, f"{label} run finished", f"{type(e).__name__}: {e}")
                finally:
                    try:
                        print("    last emulator events:", json.dumps(run.summary()["events"][-6:]))
                    except Exception:  # noqa: BLE001
                        pass
                    run.close()
            if webkit:
                webkit.close()
            phones.close()
            browser.close()
    finally:
        server.stop()
        shutil.rmtree(data_dir, ignore_errors=True)
    c.check(not server.errors, "the meeting server logged no errors", " | ".join(server.errors[:5]))
    passed = sum(1 for r in c.results if r[0])
    print(f"\n{passed} passed, {len(c.failed)} failed, {time.time() - t0:.0f} s")
    for _, what, detail in c.failed:
        print(f"  FAILED: {what}  {detail}")
    return 0 if not c.failed else 1


if __name__ == "__main__":
    sys.exit(main())
