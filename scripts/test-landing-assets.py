#!/usr/bin/env python3
"""Dependency-free checks for landing-page lossless image alternatives.

Regenerate a derivative without altering its pixels:
    cwebp -quiet -lossless -exact -m 6 input.png -o output.webp

The byte-for-byte decoded RGBA comparison is performed during asset preparation.
These tests guard the deployed markup, reserved dimensions and lossless format.
"""

from html.parser import HTMLParser
from pathlib import Path
import struct
import unittest


ROOT = Path(__file__).resolve().parents[1] / "docs" / "landing"
ASSETS = ("krate-logo", "app-shots/studio-home", "app-shots/studio-session")


def png_size(path):
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    return struct.unpack(">II", data[16:24])


def lossless_webp_size(path):
    data = path.read_bytes()
    assert data[:4] == b"RIFF" and data[8:12] == b"WEBP"
    offset = 12
    while offset + 8 <= len(data):
        kind = data[offset:offset + 4]
        length = struct.unpack("<I", data[offset + 4:offset + 8])[0]
        chunk = data[offset + 8:offset + 8 + length]
        if kind == b"VP8L":
            assert chunk[0] == 0x2F
            bits = struct.unpack("<I", chunk[1:5])[0]
            return (bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1
        offset += 8 + length + (length & 1)
    raise AssertionError(f"No lossless VP8L chunk in {path}")


class Pictures(HTMLParser):
    def __init__(self, text):
        super().__init__()
        self.pictures = []
        self.current = None
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "picture":
            self.current = {}
        elif self.current is not None and tag in ("source", "img"):
            self.current[tag] = attrs

    def handle_endtag(self, tag):
        if tag == "picture":
            self.pictures.append(self.current)
            self.current = None


class LandingAssets(unittest.TestCase):
    def test_current_social_preview_is_sized_and_referenced(self):
        self.assertEqual(png_size(ROOT / "og-v4.png"), (1200, 630))
        for page in ("index.html", "studio/index.html", "privacy/index.html"):
            source = (ROOT / page).read_text()
            self.assertIn('content="https://krate.tech/og-v4.png"', source)
            self.assertNotIn("og-v3.png", source)
        repo = ROOT.parents[1]
        self.assertIn('src="docs/landing/og-v4.png"', (repo / "README.md").read_text())
        for page in (repo / "docs/answers").glob("*.html"):
            self.assertNotIn("og-v3.png", page.read_text(), str(page))
            self.assertIn('content="https://krate.tech/og-v4.png"', page.read_text())

    def test_social_preview_source_keeps_current_positioning(self):
        source = (ROOT.parent / "social/preview.html").read_text()
        for required in ("krate-glyph-blue.png", "One app file for",
                         "Mac, Windows and Linux.", "krate.tech"):
            self.assertIn(required, source)
        for retired in ("12 stars", "No installer", "tens of kilobytes", "cannot touch anything"):
            self.assertNotIn(retired, source)
        self.assertNotIn("<script", source)
        for decoration in ("box-shadow", "class=\"file\""):
            self.assertNotIn(decoration, source)

    def test_lossless_derivatives_keep_dimensions_and_reduce_bytes(self):
        for asset in ASSETS:
            with self.subTest(asset=asset):
                png, webp = ROOT / (asset + ".png"), ROOT / (asset + ".webp")
                self.assertEqual(png_size(png), lossless_webp_size(webp))
                self.assertLess(webp.stat().st_size, png.stat().st_size)

    def test_picture_fallbacks_reserve_original_dimensions(self):
        # /studio/ lost its brand-bar logo picture with the 2026-10 redesign:
        # the kit header uses the small 3D mark, which is 15 KB as a PNG.
        # The 2026-10 homepage shows its three desktops as WebP images (the
        # OS screenshots are 272 KB as WebP against 3.6 MB as PNG) inside a
        # box with a fixed aspect ratio, so it has no <picture> fallbacks.
        for page, expected in (("index.html", 0), ("studio/index.html", 1)):
            pictures = Pictures((ROOT / page).read_text()).pictures
            self.assertEqual(len(pictures), expected)
            for picture in pictures:
                with self.subTest(page=page, picture=picture):
                    source, image = picture["source"], picture["img"]
                    self.assertEqual(source["type"], "image/webp")
                    self.assertEqual(source["srcset"], image["src"].replace(".png", ".webp"))
                    self.assertTrue((ROOT / source["srcset"].lstrip("/")).is_file())
                    original = ROOT / image["src"].lstrip("/")
                    self.assertEqual((int(image["width"]), int(image["height"])), png_size(original))


class HomeScreenshots(unittest.TestCase):
    def test_the_three_desktops_are_served_and_reserve_their_space(self):
        source = (ROOT / "index.html").read_text()
        for name in ("os-mac", "os-mac-dark", "os-win", "os-win-dark", "os-linux", "os-linux-dark"):
            with self.subTest(name=name):
                self.assertIn(f'src="/{name}.webp"', source)
                self.assertTrue((ROOT / f"{name}.webp").is_file())
        # The box the screenshots sit in has a fixed ratio, so loading them
        # cannot move the page.
        self.assertIn("aspect-ratio: 1200 / 760", source)


if __name__ == "__main__":
    unittest.main()
