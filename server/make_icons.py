#!/usr/bin/env python3
"""Draws the phone page icons in server/public from docs/brand/oyen-poni-logo-512.png.

  icon-192.png          the logo as it is (purpose "any")
  icon-512.png          full-bleed orange, the logo's tile at 66 % so the island and
                        Oyen stay inside the 40 %-radius circle every mask keeps (purpose "maskable")
  apple-touch-icon.png  180 px, full-bleed square (iOS rounds the corners itself)

Run: python3 server/make_icons.py   (needs Pillow)
"""
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
LOGO = ROOT / "docs" / "brand" / "oyen-poni-logo-512.png"
OUT = ROOT / "server" / "public"
ORANGE = (0xF0, 0x89, 0x2E, 255)  # the tile colour (#F0892E)


def full_bleed(logo: Image.Image) -> Image.Image:
    """The tile cropped to its edges, transparent corners filled with the tile colour."""
    tile = logo.crop(logo.getchannel("A").getbbox())
    square = Image.new("RGBA", tile.size, ORANGE)
    square.alpha_composite(tile)
    return square


def main() -> None:
    logo = Image.open(LOGO).convert("RGBA")
    logo.resize((192, 192), Image.LANCZOS).save(OUT / "icon-192.png", optimize=True)

    tile = full_bleed(logo)
    tile.convert("RGB").resize((180, 180), Image.LANCZOS).save(OUT / "apple-touch-icon.png", optimize=True)

    size = 512
    inner = round(size * 0.66)
    maskable = Image.new("RGBA", (size, size), ORANGE)
    small = tile.resize((inner, inner), Image.LANCZOS)
    maskable.alpha_composite(small, ((size - inner) // 2, (size - inner) // 2))
    maskable.convert("RGB").save(OUT / "icon-512.png", optimize=True)
    for name in ("icon-192.png", "icon-512.png", "apple-touch-icon.png"):
        print(OUT / name, Image.open(OUT / name).size)


if __name__ == "__main__":
    main()
