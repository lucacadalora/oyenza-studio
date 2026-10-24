"""Draw the Mac app icon's iconset from Oyen Poni's logo (tools/make_brand.py draws the logo itself).

macOS icons sit on Apple's grid: the tile is 824 of the icon's 1024 px, centred, with clear space around it.
Each size is drawn on its own (the logo leaves out the album cover and the bars below 32 px), never scaled
from a bigger one.

Run: python3 tools/make_mac_icon.py <out.iconset>   (needs Pillow), then on a Mac:
     iconutil -c icns <out.iconset> -o OyenPoni.icns
"""
import sys
from pathlib import Path

from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parent))
import make_brand  # noqa: E402

TILE = 824 / 1024  # Apple's icon grid: the tile's share of the icon
# (file name, pixel size) as iconutil expects them.
ICONSET = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]


def icon(px: int) -> Image.Image:
    # draw_logo leaves its own small margin (INSET) around the tile; size it so the tile is TILE of the icon.
    logo_px = round(px * TILE / (1 - 2 * make_brand.INSET))
    logo = make_brand.draw_logo(logo_px, ss=4 if logo_px > 256 else 8)
    out = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    out.paste(logo, ((px - logo_px) // 2, (px - logo_px) // 2), logo)
    return out


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    drawn: dict[int, Image.Image] = {}
    for name, px in ICONSET:
        if px not in drawn:
            drawn[px] = icon(px)
        drawn[px].save(out / name)
        print(f"{out / name}  {px}x{px}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
