#!/usr/bin/env python3
"""Phone page against the real PC code: the meeting server, the phone page in a
headless browser, and Oyen Poni's own Rust side through the pairdrop_pc example
(crates/poni-core/examples/pairdrop_pc.rs). Pairs, sends two files, saves a
Shelf file, then checks the PC going offline.

Needs the example built first (this script never builds anything):
    cargo build -p poni-core --example pairdrop_pc
Run (browsers through the VM gate):
    vm-gate run -- python3 tools/pairdrop-e2e/run_interop.py [--webkit] [--pc PATH]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import queue
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

from run_e2e import ONLINE_MS, ROOT, SHOTS, UAS, Checks, Server

PC = "RTX-5090-Astral"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class PcProcess:
    """target/debug/examples/pairdrop_pc, reading its LINK and EVENT lines."""

    def __init__(self, binary: Path, server: str, work: Path, shelf: Path) -> None:
        self.inbox = work / "inbox"
        self.inbox.mkdir()
        cmd = [str(binary), "--server", server, "--inbox", str(self.inbox), "--book", str(work / "book.json"),
               "--pair", "--shelf", str(shelf), "--name", PC]
        self.proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.lines: queue.Queue[str] = queue.Queue()
        self.events: list[dict] = []
        self.log: list[str] = []
        threading.Thread(target=self._read_out, daemon=True).start()
        threading.Thread(target=self._read_err, daemon=True).start()
        self.link = self.wait_line("LINK ", 20).split(" ", 1)[1]

    def _read_out(self) -> None:
        for line in self.proc.stdout:
            line = line.rstrip()
            if line.startswith("EVENT "):
                try:
                    self.events.append(json.loads(line[6:]))
                except ValueError:
                    pass
            self.lines.put(line)

    def _read_err(self) -> None:
        for line in self.proc.stderr:
            self.log.append(line.rstrip())

    def wait_line(self, prefix: str, timeout: float) -> str:
        end = time.time() + timeout
        while time.time() < end:
            try:
                line = self.lines.get(timeout=0.2)
            except queue.Empty:
                continue
            if line.startswith(prefix):
                return line
        raise TimeoutError(f"no {prefix!r} line from pairdrop_pc; stderr: {self.log[-5:]}")

    def wait_event(self, kind: str, timeout: float) -> dict | None:
        end = time.time() + timeout
        while time.time() < end:
            for e in self.events:
                if e.get("type") == kind:
                    return e
            time.sleep(0.2)
        return None

    def stop(self) -> None:
        self.proc.terminate()
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def run(browser, server: Server, binary: Path, kind: str, label: str, c: Checks) -> None:
    work = Path(tempfile.mkdtemp(prefix="pairdrop-interop-"))
    shelf_bytes = secrets.token_bytes(300_000)
    shelf = work / "shelf-notes.pdf"
    shelf.write_bytes(shelf_bytes)
    pc = PcProcess(binary, server.base, work, shelf)
    opts = dict(viewport={"width": 390, "height": 844}, device_scale_factor=1, has_touch=True, user_agent=UAS[kind], accept_downloads=True)
    if browser.browser_type.name == "chromium":
        opts["is_mobile"] = True
    ctx = browser.new_context(**opts)
    phone = ctx.new_page()
    errors: list[str] = []
    phone.on("pageerror", lambda e: errors.append(str(e)))
    try:
        m = re.search(r"#k=[A-Za-z0-9_-]{22}", pc.link)
        c.check(bool(m) and pc.link.startswith(server.base), f"[{label}] the PC prints a pairing link on this server", pc.link)
        print(f"  [{label}] pairing with the Rust PC")
        phone.goto(pc.link)
        c.expect(f"[{label}] paired with the real PC", lambda: expect(phone.locator("#pair-title")).to_have_text(f"Paired with {PC}", timeout=30000))
        paired = pc.wait_event("paired", 10)
        c.check(bool(paired), f"[{label}] the PC reports the pairing", json.dumps(pc.events[-3:]))
        phone.click("#pair-done")
        c.expect(f"[{label}] the real PC shows online", lambda: expect(phone.locator("#pcs")).to_contain_text(f"{PC}Online", timeout=20000))
        phone.click("#pcs .pc-open")
        c.expect(
            f"[{label}] connected to the real PC",
            lambda: expect(phone.locator("#road")).to_have_attribute("data-road", re.compile("wifi|internet|relay|connected"), timeout=45000),
        )
        print(f"    road: {phone.locator('#road-text').text_content()}  (selected pair, local>remote: {phone.locator('#road').get_attribute('data-path')})")
        c.expect(f"[{label}] the real PC's Shelf is listed", lambda: expect(phone.locator("#shelf")).to_contain_text("shelf-notes.pdf", timeout=10000))
        phone.screenshot(path=str(SHOTS / f"interop-{label}-pc.png"), full_page=True)

        print(f"  [{label}] sending two files")
        big = secrets.token_bytes(3 * 1024 * 1024 + 777)
        tiny = b"tiny file for the real PC\n"
        phone.set_input_files("#pick", files=[
            {"name": "random-3mb.bin", "mimeType": "application/octet-stream", "buffer": big},
            {"name": "note.txt", "mimeType": "text/plain", "buffer": tiny},
        ])
        phone.click("#send")
        c.expect(f"[{label}] sent to the real PC", lambda: expect(phone.locator("#send-status-text")).to_have_text(f"Sent 2 files to {PC}", timeout=60000))
        found = {p.name: p.read_bytes() for p in pc.inbox.rglob("*") if p.is_file()}
        c.check(sha(found.get("random-3mb.bin", b"")) == sha(big), f"[{label}] the 3 MB file is in the PC's inbox byte for byte", ", ".join(found))
        c.check(found.get("note.txt") == tiny, f"[{label}] the tiny file is in the PC's inbox byte for byte")
        phone.screenshot(path=str(SHOTS / f"interop-{label}-sent.png"), full_page=True)

        print(f"  [{label}] saving a Shelf file from the real PC")
        row = phone.locator("#shelf li", has_text="shelf-notes.pdf").first
        row.locator("button.save").click()
        link = row.locator("a.save.ready")
        if c.expect(f"[{label}] the Shelf file arrives", lambda: expect(link).to_be_visible(timeout=30000)):
            with phone.expect_download(timeout=20000) as dl:
                link.click()
            data = Path(dl.value.path()).read_bytes()
            c.check(sha(data) == sha(shelf_bytes), f"[{label}] the saved Shelf file is byte for byte", f"{len(data)} bytes")

        print(f"  [{label}] the PC stops")
        phone.click("#pc-back")
        pc.stop()
        phone.wait_for_timeout(ONLINE_MS + 3500)
        phone.reload()
        c.expect(f"[{label}] home shows the real PC offline", lambda: expect(phone.locator("#pcs")).to_contain_text(f"{PC}Offline · seen", timeout=20000))
        c.check(not errors, f"[{label}] no page errors", " | ".join(errors[:3]))
    finally:
        pc.stop()
        ctx.close()
        if pc.log:
            print("    pairdrop_pc log (last lines):", " | ".join(pc.log[-6:]))
        shutil.rmtree(work, ignore_errors=True)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pc", default=str(ROOT / "target" / "debug" / "examples" / "pairdrop_pc"))
    ap.add_argument("--webkit", action="store_true", help="also run an iPhone run in WebKit")
    args = ap.parse_args()
    binary = Path(args.pc)
    if not binary.is_file():
        print(f"{binary} is missing: cargo build -p poni-core --example pairdrop_pc")
        return 2
    SHOTS.mkdir(parents=True, exist_ok=True)
    data_dir = tempfile.mkdtemp(prefix="pairdrop-interop-server-")
    server = Server(data_dir)
    print(f"meeting server at {server.base}; PC: {binary}")
    c = Checks()
    try:
        with sync_playwright() as pw:
            chromium = pw.chromium.launch()
            run(chromium, server, binary, "android", "android", c)
            if args.webkit:
                webkit = pw.webkit.launch()
                run(webkit, server, binary, "iphone", "webkit-iphone", c)
                webkit.close()
            chromium.close()
    finally:
        server.stop()
        shutil.rmtree(data_dir, ignore_errors=True)
    c.check(not server.errors, "the meeting server logged no errors", " | ".join(server.errors[:5]))
    passed = sum(1 for r in c.results if r[0])
    print(f"\n{passed} passed, {len(c.failed)} failed")
    for _, what, detail in c.failed:
        print(f"  FAILED: {what}  {detail}")
    return 0 if not c.failed else 1


if __name__ == "__main__":
    sys.exit(main())
