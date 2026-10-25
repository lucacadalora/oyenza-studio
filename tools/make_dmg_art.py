"""Draw the picture behind the Mac DMG's window from tools/dmg_art.html.

    python3 tools/make_dmg_art.py

Writes installer/mac/dmg/background.png (the notarized DMG) and background-test.png (the ad-hoc test DMG, which
also says how to get past Open Anyway), each at 660x420 and at 2x as <name>@2x.png. dmgbuild joins each pair into
one HiDPI TIFF, so Finder shows the sharp one on a Retina screen (tools/make_mac_dmg.sh). Rerun this after
changing the page, the logo or the Oyen art, and commit the PNGs: the Mac builds read them, not the page.

Needs Playwright for Python with Chromium (pip install playwright, then playwright install chromium) and Pillow.
"""
import io
from pathlib import Path

from PIL import Image
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "tools" / "dmg_art.html"
OUT = ROOT / "installer" / "mac" / "dmg"
W, H = 660, 420  # the window's content, in points (tools/mac_dmg_settings.py)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch()
        for variant, name in (("signed", "background"), ("test", "background-test")):
            for scale in (1, 2):
                page = browser.new_page(viewport={"width": W, "height": H}, device_scale_factor=scale)
                page.goto(f"{SRC.as_uri()}#{variant}", wait_until="networkidle")
                page.evaluate("document.fonts.ready.then(() => true)")
                if not page.evaluate("document.fonts.check('600 27px Outfit')"):
                    raise SystemExit("Outfit did not load (crates/poni/res/fonts)")
                broken = page.evaluate("[...document.images].filter(i => !i.naturalWidth).map(i => i.src)")
                if broken:
                    raise SystemExit(f"images did not load: {broken}")
                img = Image.open(io.BytesIO(page.screenshot(type="png"))).convert("RGB")
                page.close()
                assert img.size == (W * scale, H * scale), img.size
                path = OUT / (f"{name}@2x.png" if scale == 2 else f"{name}.png")
                img.save(path, "PNG", optimize=True)
                print(f"{path.relative_to(ROOT)}  {img.width}x{img.height}  {path.stat().st_size // 1024} KB")
        browser.close()


if __name__ == "__main__":
    main()
