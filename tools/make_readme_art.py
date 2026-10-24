"""Draw the README's feature pictures from tools/readme_art.html.

    python3 tools/make_readme_art.py            # every picture
    python3 tools/make_readme_art.py hero music  # just these

Each <figure class="card" id="NAME"> in the page becomes
docs/brand/readme/NAME.webp: drawn at 2x (1760 px wide, shown at 880 in the
README), with see-through rounded corners so it sits well on GitHub's light
and dark themes. The islands are the website's self-test screenshots
(site/assets/shots, refreshed by tools/site_shots.py), so rerun this after
retaking them. The Mac card reads site/assets/shots/mac (tools/site_shots.py
<mac-renders> mac).

Needs Playwright for Python with Chromium (pip install playwright, then
playwright install chromium) and Pillow.
"""
import io
import sys
from pathlib import Path

from PIL import Image
from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "tools" / "readme_art.html"
OUT = ROOT / "docs" / "brand" / "readme"


def main(names: list[str]) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 960, "height": 900}, device_scale_factor=2)
        page.goto(SRC.as_uri(), wait_until="networkidle")
        page.wait_for_selector("body[data-ready='1']", state="attached", timeout=30000)
        broken = page.evaluate("[...document.images].filter(i => !i.naturalWidth).map(i => i.src)")
        if broken:
            raise SystemExit(f"images did not load: {broken}")
        cards = page.evaluate("[...document.querySelectorAll('figure.card')].map(f => f.id)")
        for name in names or cards:
            if name not in cards:
                raise SystemExit(f"no card '{name}' (cards: {', '.join(cards)})")
            png = page.locator(f"#{name}").screenshot(type="png", omit_background=True)
            img = Image.open(io.BytesIO(png)).convert("RGBA")
            path = OUT / f"{name}.webp"
            img.save(path, "WEBP", quality=88, alpha_quality=90, method=6)
            print(f"{path.relative_to(ROOT)}  {img.width}x{img.height}  {path.stat().st_size // 1024} KB")
        browser.close()


if __name__ == "__main__":
    main(sys.argv[1:])
