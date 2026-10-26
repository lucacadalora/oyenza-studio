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

The Pill theme has its own section on the page, so only the states it shows
are cut (PILL_KEEP), into site/assets/shots/pill/. Pill floats below the top
edge, so its pictures keep that gap above the island:

    PONI_SELFTEST_DPI=192 PONI_SELFTEST_THEME=pill tools/run-selftest-wine.sh target/.../poni.exe out-pill
    python3 tools/site_shots.py out-pill pill

In the Oyen theme, music notes float up beside Oyen on the beat, and the
scene's single frame may fall between notes. Record the media pill frame by
frame (PONI_SELFTEST_LOOP=1 writes loop-media-NNN.png), pick a frame with
notes in the air and give its name as the third argument; it becomes
media-activity.webp:

    PONI_SELFTEST_DPI=192 PONI_SELFTEST_THEME=oyen PONI_SELFTEST_LOOP=1 tools/run-selftest-wine.sh target/.../poni.exe out-oyen-loop
    python3 tools/site_shots.py out-oyen-loop oyen loop-media-006

The Glass pictures come from Windows: Wine has no DirectComposition, so under
Wine Glass only draws its smoked-glass fallback. Run the Glass self-test at
200 % on Windows (PONI_SELFTEST_THEME=glass, PONI_SELFTEST_DPI=192 and
PONI_SELFTEST_GLASS=light, dark or flip, which picks the page under the
island). Its pictures show the island over that page, which is what the
glass is made of, so they are kept whole, as glass/<run>-<state>.webp:

    python3 tools/site_shots.py selftest-glass-light glass light
    python3 tools/site_shots.py selftest-glass-dark glass dark
    python3 tools/site_shots.py selftest-glass-flip glass flip

The Mac pictures come from the mac workflow's `mac-renders` artifact (the
Mac app's own `--render`, 200 %, Classic and Oyen, with and without a
MacBook's notch). They show the island with the menu bar and the camera
notch around it, so they are kept as screen pictures, as
mac/<theme>/<state>.webp; the resting states are cut to the middle of the
menu bar:

    gh run download <mac run> -R lucacadalora/oyenza-studio -n mac-renders -D mac-renders
    python3 tools/site_shots.py mac-renders mac

Pill's Mac pictures (mac/pill/, MAC_PILL_KEEP) are cut taller: what hangs
below the notch floats there. A theme as a fourth argument cuts only that
theme's pictures:

    python3 tools/site_shots.py mac-renders mac pill

The page shows the small resting states at twice their size (the hero, the
"While it rests" stage, the games readout). Screens with two or more pixels
per point get pictures from a 400 % run there, cut into
site/assets/shots/zoom/<theme>/ (ZOOM_KEEP). A 400 % island needs a bigger
virtual screen than Wine's default:

    PONI_SELFTEST_DPI=384 PONI_SELFTEST_SCREEN=4096x2400x24 tools/run-selftest-wine.sh target/.../poni.exe out-classic-4x
    PONI_SELFTEST_DPI=384 PONI_SELFTEST_SCREEN=4096x2400x24 PONI_SELFTEST_THEME=oyen tools/run-selftest-wine.sh target/.../poni.exe out-oyen-4x
    python3 tools/site_shots.py out-classic-4x classic zoom
    python3 tools/site_shots.py out-oyen-4x oyen zoom

At 400 % the frame-time checks fail under Wine (drawing is slower); the
pictures are still right.

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
    "page-media", "page-focus", "page-health", "page-shelf", "page-tasks", "page-today-day",
    "page-shelf-phone-ask", "oyenza-money", "page-oyenza", "page-oyenza-signin", "page-settings", "page-system",
    "page-settings-stay-on-top", "page-shelf-pairing", "page-addons", "page-prayer", "prayer-countdown",
    "game-stats", "page-system-games", "page-system-power-estimate", "page-script", "page-script-reading", "media-lyric",
}
# The Pill section: the island at rest (clock and status bar looks), playing music (the line being
# sung), a timer, a pop-up and a notice, and open on the Music page.
PILL_KEEP = {"idle-clock", "idle-status", "media-activity", "timer-activity", "volume-hud", "health-notice", "page-media"}
# Glass pictures keep the page under the island: the glass is made of it.
# By run: the open island in light and dark mode, and the resting island
# over a bright page and over a dark one with Windows still in light mode.
GLASS_KEEP = {"light": {"page-media", "media-activity"}, "dark": {"page-media"}, "flip": {"media-activity"}}
# Mac pictures, with the notch: the resting states sit in the menu bar's line,
# cut to its middle (MAC_STRIP, in pixels at 200 %); the open pages hang below
# the notch and are kept whole.
MAC_KEEP = {"idle-clock", "media-activity", "timer-activity", "page-media", "page-today-day", "page-stocks"}
MAC_STRIP = (840, 136)
# Pill on a Mac: what hangs below the notch (the line being sung, pop-ups) floats there, so its
# strips are cut taller to keep the pill and its shadow; the resting island stays in the ears.
MAC_PILL_KEEP = {"idle-clock", "media-activity", "volume-hud", "page-media", "page-today-day"}
MAC_PILL_STRIP = (840, 216)
# Resting states the page shows at twice their size, cut from a 400 % run.
ZOOM_KEEP = {"idle-clock", "media-activity", "media-lyric", "timer-activity", "volume-hud", "bluetooth-hud", "prayer-countdown", "oyenza-money", "game-stats"}


def cut(shot: Path, alpha: Path, floating: bool = False) -> Image.Image:
    rgba = np.asarray(Image.open(shot).convert("RGBA")).astype(np.float32)
    a = np.asarray(Image.open(alpha).convert("L")).astype(np.float32) / 255.0
    if floating:
        # Alpha 1/255 isn't paint: it's the window taking the mouse in the
        # gap above a floating island and around an open one (the hit mask).
        a = np.where(a < 1.5 / 255.0, 0.0, a)
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
    if box and floating:
        # Keep the gap above the island: on the page it floats below the
        # strip's top edge the way it floats below the screen's.
        box = (box[0], 0, box[2], box[3])
    return img.crop(box) if box else img


def glass(src: Path, run: str) -> None:
    dst = ROOT / "site" / "assets" / "shots" / "glass"
    dst.mkdir(parents=True, exist_ok=True)
    for shot in sorted(src.glob("[0-9][0-9]-*.png")):
        name = re.sub(r"^\d\d-", "", shot.stem)
        if name not in GLASS_KEEP[run]:
            continue
        img = Image.open(shot).convert("RGB")
        img.save(dst / f"{run}-{name}.webp", quality=90, method=6)
        print(f"glass/{run}-{name}.webp  {img.width}x{img.height}")


def mac(src: Path, only: str | None = None) -> None:
    for shot in sorted(src.glob("[0-9][0-9]-*-notch.png")):
        name = re.sub(r"^\d\d-", "", shot.stem)[: -len("-notch")]
        theme = "classic"
        for t in ("oyen", "pill"):
            if name.endswith("-" + t):
                name, theme = name[: -len(t) - 1], t
        keep, strip = (MAC_PILL_KEEP, MAC_PILL_STRIP) if theme == "pill" else (MAC_KEEP, MAC_STRIP)
        if name not in keep or (only and theme != only):
            continue
        img = Image.open(shot).convert("RGB")
        if not name.startswith("page-"):
            w, h = strip
            x0 = (img.width - w) // 2
            img = img.crop((x0, 0, x0 + w, h))
        dst = ROOT / "site" / "assets" / "shots" / "mac" / theme
        dst.mkdir(parents=True, exist_ok=True)
        img.save(dst / f"{name}.webp", quality=90, method=6)
        print(f"mac/{theme}/{name}.webp  {img.width}x{img.height}")


def main() -> None:
    if len(sys.argv) == 4 and sys.argv[2] == "glass" and sys.argv[3] in ("light", "dark", "flip"):
        glass(Path(sys.argv[1]), sys.argv[3])
        return
    if len(sys.argv) in (3, 4) and sys.argv[2] == "mac":
        mac(Path(sys.argv[1]), sys.argv[3] if len(sys.argv) == 4 else None)
        return
    if len(sys.argv) not in (3, 4) or sys.argv[2] not in ("classic", "oyen", "pill"):
        sys.exit(__doc__)
    src, theme = Path(sys.argv[1]), sys.argv[2]
    lang = sys.argv[3] if len(sys.argv) == 4 else None
    floating = theme == "pill"
    keep = {"page-media"} if lang else PILL_KEEP if floating else KEEP
    dst = ROOT / "site" / "assets" / "shots" / theme
    if lang == "zoom":
        keep, lang = ZOOM_KEEP, None
        dst = dst.parent / "zoom" / theme
    dst.mkdir(parents=True, exist_ok=True)
    if lang and lang.startswith("loop-media-"):
        img = cut(src / f"{lang}.png", src / f"{lang}-alpha.png", floating)
        img.save(dst / "media-activity.webp", lossless=True, method=6)
        print(f"{theme}/media-activity.webp  {img.width}x{img.height}  (from {lang})")
        return
    for shot in sorted(src.glob("[0-9][0-9]-*.png")):
        name = re.sub(r"^\d\d-", "", shot.stem)
        alpha = shot.with_name(shot.stem + "-alpha.png")
        if name.endswith("-alpha") or name not in keep or not alpha.exists():
            continue
        img = cut(shot, alpha, floating)
        out = f"{name}-{lang}" if lang else name
        img.save(dst / f"{out}.webp", lossless=True, method=6)
        print(f"{theme}/{out}.webp  {img.width}x{img.height}")


if __name__ == "__main__":
    main()
