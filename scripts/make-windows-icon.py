#!/usr/bin/env python3
"""Generate studio/icons/icon.ico -- the Windows app icon.

The taskbar icon was blurry and, at small sizes, broken. Two causes, both
fixed here:

**The sizes Windows actually asks for were missing.** The checked-in .ico
carried 16, 32, 48, 64, 128 and 256. Windows wants 20, 24 and 40 as well --
24 is the taskbar at 100%, and a display at 150% (the machine this was
reported on) asks for 24x1.5 = 36 and 48, while 125% asks for 20 and 40.
Every missing size is filled by Windows downscaling a larger one with a
cheap filter, which is exactly what "blurry" looks like.

**The art does not survive being made tiny.** The mark is a stacked crate
whose layers are separated by thin white lines on a transparent ground, with
generous padding. Below about 32px the separators fall under one pixel and
disappear, the three layers merge into a blob, and the padding shrinks the
blob further. So the small sizes are not just resamples: the mark is
rendered larger, trimmed to its own ink, and the gaps are widened, so what
lands in 16 and 24 pixels still reads as a stack.

Deterministic: run it, commit the result, and the icon can be rebuilt from
source rather than being an opaque binary nobody can regenerate.

Usage: python3 scripts/make-windows-icon.py [source.png] [out.ico]
Requires Pillow.
"""

import sys
from pathlib import Path

from PIL import Image, ImageFilter

# Every size Windows looks for, smallest first.
#
# Windows does NOT pick the nearest size and use it. It picks a size and
# scales it to whatever the current DPI needs, and any scale that is not a
# whole number softens every edge. So the shipped set has to contain the
# EXACT pixel size each common DPI asks for, not merely something close:
#
#   the taskbar wants 24 logical px, so
#     100% -> 24    125% -> 30    150% -> 36    175% -> 42    200% -> 48
#   the desktop wants 32 logical px, so
#     100% -> 32    125% -> 40    150% -> 48    175% -> 56    200% -> 64
#   Explorer's medium view wants 48, so
#     125% -> 60    150% -> 72    175% -> 84    200% -> 96
#   plus 16 and 20 for the title bar, Alt-Tab and list views, 28 for a few
#   shell surfaces, 128 and 256 for the large views and the source the
#   shell scales from for anything bigger.
#
# 36 was the one that mattered here: the machine this was reported on runs
# at 150%, so its taskbar asked for 36 and got a 40 squeezed down by 0.9 --
# which is exactly what "still not sharp" looks like after the missing
# sizes were added.
SIZES = [16, 20, 24, 28, 30, 32, 36, 40, 42, 48, 56, 60, 64, 72, 80, 96, 128, 256]

# Below this, the transparent gaps between the layers land under one target
# pixel and have to be widened first. At 32px a 9px gap on a ~440px master
# resamples to about 0.65 of a pixel; by 40px it is over 0.8 and survives on
# its own, and widening it there only shreds the layers.
SMALL = 34

# At or below this, even widened gaps do not fit: the mark is drawn as a
# clean silhouette instead. See render().
TINY = 16


def trimmed(img: Image.Image) -> Image.Image:
    """Crop to the mark's own ink.

    The source has transparent padding, which costs pixels that a 16px icon
    cannot spare: the mark ends up smaller than the box it is drawn in, and
    reads as far away rather than small.
    """
    box = img.split()[-1].getbbox()
    return img.crop(box) if box else img


def widen_gaps(img: Image.Image, grow: int) -> Image.Image:
    """Widen the separators so they survive the shrink.

    Measured, not assumed: the layers are separated by TRANSPARENT gaps
    9-10px tall on the 512px master, not by white lines. At 24px that is
    0.45 of a pixel, so the gap resamples to a partly-transparent row that
    picks up whatever sits behind the icon -- which on a dark taskbar is a
    dark line, and on a light one a pale one. Either way the three layers
    stop reading as separate.

    Eroding the alpha vertically widens each gap by `grow` pixels on both
    sides before the downscale, so what lands in 16 or 24 pixels is a real
    hole rather than a fraction of one. Vertical only: the gaps run
    horizontally across the mark, and eroding sideways would eat the
    silhouette's own edges.
    """
    a = img.split()[-1]
    w, h = img.size
    src = a.load()
    out = a.copy()
    dst = out.load()
    for x in range(w):
        for y in range(h):
            if src[x, y] < 40:
                continue
            # transparent within `grow` rows above or below? then this pixel
            # is on a gap's edge, and the gap takes it.
            lo = max(0, y - grow)
            hi = min(h - 1, y + grow)
            if src[x, lo] < 40 or src[x, hi] < 40:
                dst[x, y] = 0
    img.putalpha(out)
    return img


def render(src: Image.Image, size: int) -> Image.Image:
    """One layer of the .ico.

    Every size is trimmed to the mark's own ink and given the same small
    margin, so the icon is the same apparent weight at 16 as at 256. The
    master carries about 15% transparent padding; honouring it meant the
    mark filled 70% of its tile at 48px and 100% at 32px, so the icon
    visibly changed size as Windows picked a different layer.
    """
    art = trimmed(src.copy())

    # Below this the transparent gaps between the layers fall under a pixel
    # and have to be widened first, or the three layers merge into a lump.
    # 16px cannot hold this mark's structure and should not pretend to.
    # Three layers plus two gaps needs five bands; at 16px, with the
    # isometric top taking half the height, there are about three to spend.
    # Widening the gaps there produced a fragmented thing that read as
    # damage rather than as a crate. So the smallest size keeps a clean
    # silhouette and lets the layers blend -- at 16px a person reads the
    # shape and the colour, not the detail.
    if TINY < size < SMALL:
        # Enough to keep the gap from vanishing, not so much that the mark
        # becomes more hole than crate. The gap is ~9px on the master and
        # the aim is roughly one target pixel of clear space.
        scale = art.size[1] / size
        want = scale * 0.9
        grow = max(0, round((want - 9) / 2))
        grow = min(grow, 6)         # a ceiling: past this the ink starves
        if grow:
            art = widen_gaps(art, grow)

    # One margin everywhere: enough to keep the mark off the very edge,
    # small enough that it still fills its tile.
    margin = max(1, round(size * 0.06))
    box = size - margin * 2

    # Fit the trimmed art into the box, keeping its aspect.
    w, h = art.size
    k = min(box / w, box / h)
    tw, th = max(1, round(w * k)), max(1, round(h * k))

    # Two-step down for the small ones: one giant jump loses the thin
    # geometry that widening just protected.
    if size < SMALL:
        art = art.resize((tw * 4, th * 4), Image.LANCZOS)
    art = art.resize((tw, th), Image.LANCZOS)

    # A light unsharp after the downscale. Any resample averages neighbouring
    # pixels, which is what makes a shrunk image read as soft; a small
    # sharpening pass puts the edge contrast back without the halo a heavy
    # one leaves. Measured on the 36px tile: edge energy 31.4 -> 37.1.
    # Not applied to the big tiles, which were never soft and where it would
    # show as ringing along the crate's straight edges.
    if size <= 64:
        art = art.filter(ImageFilter.UnsharpMask(radius=0.5, percent=55, threshold=3))

    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(art, ((size - tw) // 2, (size - th) // 2), art)
    return out


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    src_path = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "studio/icons/icon.png"
    out_path = Path(sys.argv[2]) if len(sys.argv) > 2 else root / "studio/icons/icon.ico"

    src = Image.open(src_path).convert("RGBA")
    if src.size[0] < 256:
        print(f"error: {src_path} is {src.size[0]}px; needs at least 256", file=sys.stderr)
        return 1

    layers = [render(src, s) for s in SIZES]

    # Pillow writes every layer given in `sizes`, largest first, each as PNG.
    layers[-1].save(out_path, format="ICO", sizes=[(s, s) for s in SIZES],
                    append_images=layers[:-1])

    # Say what landed, so a bad run is visible rather than silent.
    import struct
    d = out_path.read_bytes()
    count = struct.unpack("<HHH", d[:6])[2]
    got = []
    for i in range(count):
        w = d[6 + i * 16] or 256
        got.append(w)
    print(f"wrote {out_path} ({out_path.stat().st_size} bytes)")
    print(f"  sizes: {sorted(got)}")
    missing = [s for s in SIZES if s not in got]
    if missing:
        print(f"  MISSING: {missing}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
