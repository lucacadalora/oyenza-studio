#!/usr/bin/env python3
"""Build the fonts Oyen Poni ships (crates/poni/res/fonts) from Outfit.

Outfit (SIL Open Font License 1.1, github.com/Outfitio/Outfit-Fonts) is the
Oyen Poni website's typeface; the Oyen theme writes every Latin letter in it.
Google Fonts publishes it as one variable font. The island wants four plain
weights (Regular, Medium, SemiBold, Bold) that every Windows text stack loads,
so this script cuts them out of the variable font, and adds the pinyin tone
letters Outfit lacks (ǐ ǒ ǔ and ü with its four tones) as composites of
Outfit's own letters and marks, so Chinese pronunciation lines don't fall
back to another typeface mid-word.

Usage:  python3 tools/make_fonts.py <Outfit[wght].ttf> [out-dir]
        (out-dir defaults to crates/poni/res/fonts)
Needs:  fontTools (pip install fonttools)
"""

import os
import sys

from fontTools.ttLib import TTFont
from fontTools.ttLib.tables._g_l_y_f import Glyph, GlyphComponent
from fontTools.varLib import instancer

WEIGHTS = [(400, "Regular"), (500, "Medium"), (600, "SemiBold"), (700, "Bold")]

# Pinyin letters to add: (code point, glyph name, base glyph, marks above it).
ADDED = [
    (0x01D0, "uni01D0", "dotlessi", ["uni030C"]),  # ǐ
    (0x01D2, "uni01D2", "o", ["uni030C"]),  # ǒ
    (0x01D4, "uni01D4", "u", ["uni030C"]),  # ǔ
    (0x01D6, "uni01D6", "u", ["uni0308", "uni0304"]),  # ǖ
    (0x01D8, "uni01D8", "u", ["uni0308", "acutecomb"]),  # ǘ
    (0x01DA, "uni01DA", "u", ["uni0308", "uni030C"]),  # ǚ
    (0x01DC, "uni01DC", "u", ["uni0308", "gravecomb"]),  # ǜ
]
# Gap between two stacked marks, in font units.
STACK_GAP = 40


def bounds(font, name):
    g = font["glyf"][name]
    g.recalcBounds(font["glyf"])
    return g.xMin, g.yMin, g.xMax, g.yMax


def add_letters(font):
    cmap = font.getBestCmap()
    glyf = font["glyf"]
    hmtx = font["hmtx"]
    for cp, name, base, marks in ADDED:
        if cp in cmap or name in glyf:
            continue
        if base not in glyf or any(m not in glyf for m in marks):
            print(f"  skip U+{cp:04X}: missing {base} or {marks}")
            continue
        bx0, _, bx1, by1 = bounds(font, base)
        centre = (bx0 + bx1) / 2
        g = Glyph()
        g.numberOfContours = -1
        g.components = []
        c = GlyphComponent()
        c.glyphName, c.x, c.y, c.flags = base, 0, 0, 0x4
        g.components.append(c)
        top = None
        for m in marks:
            mx0, my0, mx1, my1 = bounds(font, m)
            dx = round(centre - (mx0 + mx1) / 2)
            # The first mark sits where Outfit's own accented letters put it;
            # a second one stacks above it.
            dy = 0 if top is None else round(top + STACK_GAP - my0)
            c = GlyphComponent()
            c.glyphName, c.x, c.y, c.flags = m, dx, dy, 0x4
            g.components.append(c)
            top = my1 + dy
        glyf[name] = g  # also appends the name to the glyph order
        g.recalcBounds(glyf)
        # Same advance as the bare letter; the side bearing must match the
        # composite's own left edge (a mark can reach past the base's).
        hmtx[name] = (hmtx[base][0], g.xMin)
        for table in font["cmap"].tables:
            if table.isUnicode():
                table.cmap[cp] = name
    font.setGlyphOrder(list(glyf.glyphOrder))


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    src = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(__file__), "..", "crates", "poni", "res", "fonts")
    os.makedirs(out, exist_ok=True)
    for weight, style in WEIGHTS:
        vf = TTFont(src)
        font = instancer.instantiateVariableFont(vf, {"wght": weight}, updateFontNames=True)
        for t in ["STAT", "MVAR", "HVAR", "gvar", "fvar", "avar", "DSIG"]:
            if t in font:
                del font[t]
        font["OS/2"].usWeightClass = weight
        add_letters(font)
        path = os.path.join(out, f"Outfit-{style}.ttf")
        font.save(path)
        missing = [ch for ch in "āáǎàēéěèīíǐìōóǒòūúǔùǖǘǚǜü" if ord(ch) not in TTFont(path).getBestCmap()]
        print(f"{path}: {os.path.getsize(path)} bytes, weight {weight}, pinyin letters missing: {missing or 'none'}")


if __name__ == "__main__":
    main()
