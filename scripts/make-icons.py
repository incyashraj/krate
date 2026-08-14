#!/usr/bin/env python3
"""Build the Krate icon set from the source mark.

The shipped icon was blue with the mark sitting 56px low on a 1024 canvas
(310px of air above it, 197px below), which reads as a downward shift in the
dock. This lifts the mark off its tile by colour, rescales it, and recentres
it on the true canvas centre, so the centring is computed rather than
hand-nudged -- and asserted at the end, so it cannot regress quietly.
"""
import os
import sys
from PIL import Image, ImageDraw

SRC = "/Users/yashrajpardeshi/Desktop/krate-logo/krate-app-icon-1024.png"
W = H = 1024


def lift_mark(path):
    """Separate the near-white mark from the blue tile behind it."""
    src = Image.open(path).convert("RGBA")
    px = src.load()
    out = Image.new("RGBA", src.size, (0, 0, 0, 0))
    op = out.load()
    xs, ys = [], []
    for y in range(src.size[1]):
        for x in range(src.size[0]):
            r, g, b, a = px[x, y]
            if a < 40:
                continue
            # The tile is strongly blue (b >> r); the mark is neutral.
            if b - r < 60 and r > 150:
                op[x, y] = (255, 255, 255, 255)
                xs.append(x)
                ys.append(y)
            elif b - r < 110 and r > 110:
                # Antialiased edge: partial coverage keeps the mark smooth.
                op[x, y] = (255, 255, 255,
                            int(max(0, min(255, (110 - (b - r)) * 255 / 50))))
    return out.crop((min(xs), min(ys), max(xs) + 1, max(ys) + 1))


def tile(grey=True):
    """The rounded-square plate, with a soft top-to-bottom ramp."""
    inset, radius = 88, 210
    ramp = Image.new("RGBA", (1, H))
    rp = ramp.load()
    for y in range(H):
        t = y / (H - 1)
        if grey:
            v = int(round(124 - 42 * t))
            rp[0, y] = (v, v, v, 255)
        else:
            rp[0, y] = (int(28 + 10 * t), int(110 + 20 * t), int(246 - 30 * t), 255)
    ramp = ramp.resize((W, H))
    mask = Image.new("L", (W, H), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [inset, inset, W - inset, H - inset], radius=radius, fill=255)
    plate = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    plate.paste(ramp, (0, 0), mask)
    return plate


def compose(mark, grey=True, share=0.50):
    plate = tile(grey)
    gw, gh = mark.size
    scale = (W * share) / max(gw, gh)
    m = mark.resize((max(1, int(gw * scale)), max(1, int(gh * scale))), Image.LANCZOS)
    gw, gh = m.size
    plate.alpha_composite(m, ((W - gw) // 2, (H - gh) // 2))
    return plate


def centre_of(img):
    p = img.load()
    xs, ys = [], []
    for y in range(H):
        for x in range(W):
            r, g, b, a = p[x, y]
            if a > 128 and r > 200 and g > 200 and b > 200:
                xs.append(x)
                ys.append(y)
    return (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "dist/icon"
    os.makedirs(out, exist_ok=True)
    mark = lift_mark(SRC)
    app = compose(mark, grey=True)

    cx, cy = centre_of(app)
    # The whole point of this script. Assert it rather than trusting it.
    assert abs(cx - W / 2) <= 3 and abs(cy - H / 2) <= 3, \
        f"mark is off centre: {cx},{cy} (want {W/2},{H/2})"

    app.save(os.path.join(out, "krate-app-icon-1024.png"))
    for size in (512, 256, 128, 64, 32):
        app.resize((size, size), Image.LANCZOS).save(
            os.path.join(out, f"krate-app-icon-{size}.png"))

    # The document icon: same mark, so a .krate in Finder is recognisably
    # Krate, on a lighter plate so it reads as a document and not the app.
    doc = compose(mark, grey=True, share=0.44)
    doc.save(os.path.join(out, "krate-document-icon-1024.png"))
    print(f"icons written to {out} (mark centred at {cx:.1f},{cy:.1f})")


if __name__ == "__main__":
    main()
