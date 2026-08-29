#!/usr/bin/env python3
"""Generate Krate.icns -- the macOS app icon.

A macOS-style rounded square in neutral grey carrying a white isometric crate.
Deterministic output so the icon can be regenerated from source rather than
checked in as an opaque binary.

Two things this file gets right on purpose, because it got them wrong before:

* **The crate is centred.** It used to run y=310..828 on a 1024 canvas, so its
  centre sat at 569 -- 57px low, with 310px of air above it and 196px below.
  In the dock that reads as a mark sagging in its tile. The geometry below is
  derived from the canvas centre, and `assert_centred` checks the rendered
  pixels rather than trusting the arithmetic.

* **The tile is grey, not blue.** The blue tile competed with the app icons
  people make; a neutral plate lets Krate sit behind their work rather than
  in front of it.

Usage: python3 scripts/make-app-icon.py [output-dir]   (default: dist/icon)
Requires PIL and /usr/bin/iconutil (macOS).
"""

import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 1024
# macOS icon grid: the visible tile is inset from the canvas.
TILE_INSET = 100
CORNER_RADIUS = 185

# Neutral plate, lighter at the top so the tile reads as a surface.
TOP_COLOR = (124, 124, 124)
BOTTOM_COLOR = (82, 82, 82)


def rounded_tile_mask() -> Image.Image:
    mask = Image.new("L", (SIZE, SIZE), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle(
        [TILE_INSET, TILE_INSET, SIZE - TILE_INSET, SIZE - TILE_INSET],
        radius=CORNER_RADIUS,
        fill=255,
    )
    return mask


def gradient_background() -> Image.Image:
    background = Image.new("RGB", (SIZE, SIZE), TOP_COLOR)
    draw = ImageDraw.Draw(background)
    for y in range(SIZE):
        t = y / (SIZE - 1)
        color = tuple(
            round(top + (bottom - top) * t)
            for top, bottom in zip(TOP_COLOR, BOTTOM_COLOR)
        )
        draw.line([(0, y), (SIZE, y)], fill=color)
    return background


def draw_crate(draw: ImageDraw.ImageDraw) -> None:
    """A white isometric crate: top, left and right faces, with slat gaps."""
    cx = SIZE // 2
    half_w = 268
    half_h = 134
    depth = 250
    # The drawn mark spans 2*half_h + depth vertically. Start it so that span
    # is centred on the canvas, rather than at a hand-picked constant.
    top_y = (SIZE - (2 * half_h + depth)) // 2

    north = (cx, top_y)
    east = (cx + half_w, top_y + half_h)
    south = (cx, top_y + 2 * half_h)
    west = (cx - half_w, top_y + half_h)

    east_low = (east[0], east[1] + depth)
    south_low = (south[0], south[1] + depth)
    west_low = (west[0], west[1] + depth)

    white = (255, 255, 255, 255)
    left_shade = (255, 255, 255, 216)
    right_shade = (255, 255, 255, 178)

    draw.polygon([north, east, south, west], fill=white)
    draw.polygon([west, south, south_low, west_low], fill=left_shade)
    draw.polygon([south, east, east_low, south_low], fill=right_shade)

    # Slat gaps: two lines per side face, parallel to the face's top edge, so
    # the box reads as a wooden crate rather than a plain cube.
    gap_width = 14
    for fraction in (0.38, 0.66):
        offset = round(depth * fraction)
        draw.line(
            [
                (west[0], west[1] + offset),
                (south[0], south[1] + offset),
            ],
            fill=(0, 0, 0, 0),
            width=gap_width,
        )
        draw.line(
            [
                (south[0], south[1] + offset),
                (east[0], east[1] + offset),
            ],
            fill=(0, 0, 0, 0),
            width=gap_width,
        )
    # A seam across the top face, hinting at a lid.
    draw.line([west, east], fill=(0, 0, 0, 0), width=gap_width)


def draw_crate_scaled(draw: ImageDraw.ImageDraw, cx: int, cy: int, scale: float,
                      face, left, right, gap_color) -> None:
    """The crate glyph at an arbitrary center and scale, in given colors."""
    half_w = round(268 * scale)
    half_h = round(134 * scale)
    depth = round(250 * scale)
    top_y = cy - half_h - depth // 2

    north = (cx, top_y)
    east = (cx + half_w, top_y + half_h)
    south = (cx, top_y + 2 * half_h)
    west = (cx - half_w, top_y + half_h)
    east_low = (east[0], east[1] + depth)
    south_low = (south[0], south[1] + depth)
    west_low = (west[0], west[1] + depth)

    draw.polygon([north, east, south, west], fill=face)
    draw.polygon([west, south, south_low, west_low], fill=left)
    draw.polygon([south, east, east_low, south_low], fill=right)

    gap_width = max(6, round(14 * scale))
    for fraction in (0.38, 0.66):
        offset = round(depth * fraction)
        draw.line([(west[0], west[1] + offset), (south[0], south[1] + offset)],
                  fill=gap_color, width=gap_width)
        draw.line([(south[0], south[1] + offset), (east[0], east[1] + offset)],
                  fill=gap_color, width=gap_width)
    draw.line([west, east], fill=gap_color, width=gap_width)


def build_document_master() -> Image.Image:
    """The .krate document icon: a white page with a folded corner carrying
    the crate glyph in Krate blue."""
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    page_left, page_top = 232, 96
    page_right, page_bottom = 792, 928
    fold = 150
    page_color = (250, 250, 252, 255)
    edge_color = (188, 195, 208, 255)
    fold_color = (222, 227, 236, 255)

    body = [
        (page_left, page_top),
        (page_right - fold, page_top),
        (page_right, page_top + fold),
        (page_right, page_bottom),
        (page_left, page_bottom),
    ]
    draw.polygon(body, fill=page_color, outline=edge_color, width=6)
    draw.polygon(
        [
            (page_right - fold, page_top),
            (page_right, page_top + fold),
            (page_right - fold, page_top + fold),
        ],
        fill=fold_color,
        outline=edge_color,
        width=6,
    )

    blue_face = (30, 140, 255, 255)
    blue_left = (0, 110, 230, 255)
    blue_right = (0, 88, 196, 255)
    draw_crate_scaled(draw, cx=SIZE // 2, cy=580, scale=0.62,
                      face=blue_face, left=blue_left, right=blue_right,
                      gap_color=page_color)
    return image


def build_master() -> Image.Image:
    background = gradient_background()

    glyph = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    glyph_draw = ImageDraw.Draw(glyph)
    draw_crate(glyph_draw)

    tile = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    tile.paste(background, mask=rounded_tile_mask())
    tile.alpha_composite(glyph)
    return tile


def assert_centred(image: Image.Image, name: str, tolerance: int = 4) -> None:
    """Fail loudly if the mark is not centred on its tile.

    The shipped icon once sat 57px low and nobody caught it until it looked
    wrong in the dock. Centring is arithmetic, so it can be checked: this reads
    the rendered pixels back and refuses to write an icon that sags.
    """
    pixels = image.load()
    xs, ys = [], []
    for y in range(image.height):
        for x in range(image.width):
            r, g, b, a = pixels[x, y]
            if a > 128 and r > 200 and g > 200 and b > 200:
                xs.append(x)
                ys.append(y)
    if not xs:
        raise SystemExit(f"{name}: no mark was drawn")
    cx = (min(xs) + max(xs)) / 2
    cy = (min(ys) + max(ys)) / 2
    want = image.width / 2
    if abs(cx - want) > tolerance or abs(cy - want) > tolerance:
        raise SystemExit(
            f"{name}: the mark is off centre at {cx:.1f},{cy:.1f} "
            f"(want {want:.1f},{want:.1f}); fix the geometry, not the icon"
        )


def main() -> int:
    out_dir = Path(sys.argv[1] if len(sys.argv) > 1 else "dist/icon")
    out_dir.mkdir(parents=True, exist_ok=True)

    for stem, master in (("Krate", build_master()), ("KrateDoc", build_document_master())):
        assert_centred(master, stem)
        master.save(out_dir / f"{stem.lower()}-1024.png")
        iconset = out_dir / f"{stem}.iconset"
        iconset.mkdir(parents=True, exist_ok=True)
        for points in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = points * scale
                name = f"icon_{points}x{points}" + ("@2x" if scale == 2 else "") + ".png"
                master.resize((pixels, pixels), Image.LANCZOS).save(iconset / name)
        # Windows: a .ico carrying every size Explorer picks from. Written
        # first and unconditionally, because iconutil is macOS-only -- doing
        # the .icns first meant the Windows icon could never be generated on
        # the Windows runner, so .krate files got the blank-page icon and
        # nobody noticed the file association was pointing at nothing.
        ico = out_dir / f"{stem}.ico"
        master.save(
            ico,
            format="ICO",
            sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
        )
        print(f"wrote {ico}")

        # macOS: .icns, which needs iconutil and therefore only works here.
        icns = out_dir / f"{stem}.icns"
        if Path("/usr/bin/iconutil").exists():
            subprocess.run(
                ["/usr/bin/iconutil", "-c", "icns", str(iconset), "-o", str(icns)],
                check=True,
            )
            print(f"wrote {icns}")
        else:
            print(f"skipped {icns} (iconutil is macOS-only)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
