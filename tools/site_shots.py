"""Refresh the website's island screenshots from a self-test run.

Run the self-test at 200 % in one theme, then cut its pictures into
site/assets/shots/<theme>/ (island only, real alpha, lossless WebP):

    PONI_SELFTEST_DPI=192 tools/run-selftest-wine.sh target/.../poni.exe out-classic
    PONI_SELFTEST_DPI=192 PONI_SELFTEST_THEME=oyen tools/run-selftest-wine.sh target/.../poni.exe out-oyen
    python3 tools/site_shots.py out-classic classic
    python3 tools/site_shots.py out-oyen oyen

The read-along picture (the Music page with the pronunciation under Korean
lyrics) comes from a run with the sample song in Korean. Give the language as
a third argument and only that page is cut, as page-media-ko.webp:

    PONI_SELFTEST_DPI=192 PONI_SELFTEST_LYRICS=ko tools/run-selftest-wine.sh target/.../poni.exe out-classic-ko
    python3 tools/site_shots.py out-classic-ko classic ko

Shoot both themes in the same minute (run them side by side) so the island's
clock matches when the site switches looks.

The self-test writes NAME.png (the island's canvas on a test gradient, with
margins) and NAME-alpha.png (the canvas's own alpha). This cuts the canvas out,
takes its alpha from NAME-alpha.png and removes the gradient from the
anti-aliased edge pixels. Needs Pillow and NumPy.
"""
import re
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
# States the site shows; the rest of the self-test's states are left out.
KEEP = {
    "idle-clock", "idle-eyes", "media-activity", "timer-activity", "volume-hud", "bluetooth-hud",
    "page-media", "page-focus", "page-health", "page-shelf", "page-tasks", "page-clipboard", "page-today",
    "page-shelf-phone-ask", "oyenza-money", "page-oyenza", "page-oyenza-signin", "page-settings", "page-system",
}


def cut(shot: Path, alpha: Path) -> Image.Image:
    rgba = np.asarray(Image.open(shot).convert("RGBA")).astype(np.float32)
    a = np.asarray(Image.open(alpha).convert("L")).astype(np.float32) / 255.0
    H, W = rgba.shape[:2]
    h, w = a.shape
    x0 = (W - w) // 2
    if x0 < 0 or h > H:
        raise ValueError(f"{shot.name}: canvas {w}x{h} does not fit the shot {W}x{H}")
    canvas = rgba[0:h, x0:x0 + w, :3]
    # The gradient behind each row: the shot's own pixels just outside the canvas.
    left = rgba[0:h, max(0, x0 - 2), :3]
    right = rgba[0:h, min(W - 1, x0 + w + 1), :3]
    bg = ((left + right) / 2.0)[:, None, :]
    color = (canvas - (1.0 - a[:, :, None]) * bg) / np.maximum(a, 1e-3)[:, :, None]
    out = np.dstack([np.clip(color, 0, 255), a * 255.0]).round().astype(np.uint8)
    img = Image.fromarray(out, "RGBA")
    box = img.getchannel("A").point(lambda v: 255 if v > 0 else 0).getbbox()
    return img.crop(box) if box else img


def main() -> None:
    if len(sys.argv) not in (3, 4) or sys.argv[2] not in ("classic", "oyen"):
        sys.exit(__doc__)
    src, theme = Path(sys.argv[1]), sys.argv[2]
    lang = sys.argv[3] if len(sys.argv) == 4 else None
    keep = {"page-media"} if lang else KEEP
    dst = ROOT / "site" / "assets" / "shots" / theme
    dst.mkdir(parents=True, exist_ok=True)
    for shot in sorted(src.glob("[0-9][0-9]-*.png")):
        name = re.sub(r"^\d\d-", "", shot.stem)
        alpha = shot.with_name(shot.stem + "-alpha.png")
        if name.endswith("-alpha") or name not in keep or not alpha.exists():
            continue
        img = cut(shot, alpha)
        out = f"{name}-{lang}" if lang else name
        img.save(dst / f"{out}.webp", lossless=True, method=6)
        print(f"{theme}/{out}.webp  {img.width}x{img.height}")


if __name__ == "__main__":
    main()
