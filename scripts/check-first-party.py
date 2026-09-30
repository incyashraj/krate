#!/usr/bin/env python3
"""Opening a krate.tech page must not make the reader's browser talk to anyone
but krate.tech.

A script, stylesheet, font, image or fetch from another host hands that host
the reader's IP address the moment the page opens, before they have agreed to
anything. A Munich court fined a site for exactly that with Google Fonts
(LG Muenchen, 3 O 17493/20). On 2026-09-30 the site had three such loads:
the homepage and download page asked api.github.com for the latest release
and the star count (now mirrored by hub.krate.tech), and the docs pulled the
diagram library from cdn.jsdelivr.net (now served from the docs).

Links a person clicks are not loads and are not checked; only what the page
fetches by itself.

    python3 scripts/check-first-party.py            check the site sources
    python3 scripts/check-first-party.py --self-test prove the check bites
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

LOAD = re.compile(
    r"""(?:<script[^>]*\ssrc|<img[^>]*\ssrc|<source[^>]*\ssrc|<video[^>]*\s(?:src|poster)"""
    r"""|<audio[^>]*\ssrc|<iframe[^>]*\ssrc|<link[^>]*\shref)\s*=\s*["'](https?://[^"']+)"""
    r"""|url\(\s*["']?(https?://[^"')]+)"""
    r"""|fetch\(\s*["'](https?://[^"']+)"""
    r"""|\.src\s*=\s*["'](https?://[^"']+)""",
    re.I,
)
# <link> tags that name a URL without loading it.
NOT_A_LOAD = re.compile(r"""rel\s*=\s*["']?(canonical|alternate|me|author|license|sitemap)""", re.I)


def first_party(host):
    host = host.lower()
    return host == "krate.tech" or host.endswith(".krate.tech")


def offenders(name, text):
    out = []
    for m in LOAD.finditer(text):
        url = next(g for g in m.groups() if g)
        host = re.match(r"https?://([^/:]+)", url).group(1)
        if first_party(host):
            continue
        if text[m.start():m.start() + 5].lower() == "<link":
            tag_end = text.find(">", m.start())
            if NOT_A_LOAD.search(text[m.start():tag_end]):
                continue
        line = text[: m.start()].count("\n") + 1
        out.append(f"{name}:{line}: loads {url}")
    return out


def site_files():
    landing = ROOT / "docs" / "landing"
    for ext in ("*.html", "*.css", "*.js"):
        for p in landing.rglob(ext):
            if "node_modules" not in p.parts:
                yield p
    yield from (ROOT / "docs" / "book").glob("*.js")


def self_test():
    bad = [
        ('<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter">', "fonts.googleapis.com"),
        ('<script src="https://cdn.jsdelivr.net/npm/x.js"></script>', "cdn.jsdelivr.net"),
        ('fetch("https://api.github.com/repos/a/b")', "api.github.com"),
        ('@font-face{src:url(https://fonts.gstatic.com/s/x.woff2)}', "fonts.gstatic.com"),
        ('s.src = "https://www.googletagmanager.com/gtag/js";', "googletagmanager.com"),
        ('<img src="https://example.com/a.png">', "example.com"),
    ]
    for snippet, host in bad:
        found = offenders("t", snippet)
        assert found and host in found[0], f"missed a third-party load: {snippet}"
    good = [
        '<a href="https://github.com/incyashraj/krate">GitHub</a>',
        '<link rel="canonical" href="https://www.example.com/">',
        'fetch("https://hub.krate.tech/release/latest")',
        '<script src="/site.js"></script>',
        '<img src="https://krate.tech/og.png">',
    ]
    for snippet in good:
        assert not offenders("t", snippet), f"flagged a first-party load or a plain link: {snippet}"
    print("OK -- third-party loads are caught; links and krate.tech loads pass.")


def main():
    if "--self-test" in sys.argv:
        self_test()
        return 0
    found = []
    for p in site_files():
        found += offenders(str(p.relative_to(ROOT)), p.read_text(errors="ignore"))
    if found:
        print("These make a reader's browser contact another host just by opening the page:")
        print("\n".join("  " + f for f in found))
        print("Serve it from krate.tech (fonts, scripts) or mirror it through hub.krate.tech (API data).")
        return 1
    print("OK -- every page loads from krate.tech only.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
