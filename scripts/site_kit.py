#!/usr/bin/env python3
"""The parts every page on the 2026-10 design kit shares, kept in one place.

The site has no server-side templating: each page is a whole HTML file, so
the header, the phone menu and the footer are copied into every page. Copies
drift -- this site once had the 44px touch rule in seven pages and missing
from three. So the copies are written from here and checked against here.

    python3 scripts/site_kit.py --check        every kit page carries the exact parts
    python3 scripts/site_kit.py --apply FILE   rewrite FILE's header, menu and footer from here
    python3 scripts/site_kit.py --self-test

A page is a kit page when it links /kit/site.css. The generators
(build-answer-pages.py, build-reports-page.py, build-progress-page.py) import
HEADER, MNAV, FOOTER, HEAD_THEME and KIT_LINKS from this module.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MARK = "/krate-mark-3d-96.png"

# Set before first paint so a dark-mode reader never sees a light flash. The
# choice is the reader's own (the header's sun/moon button), else the system's.
# It also marks the page `.js`: anything the kit hides until a script reveals
# it is hidden only under .js, so a reader without scripts sees all of it.
HEAD_THEME = """<script>(() => { const d = document.documentElement; d.classList.add('js'); let t = null; try { t = localStorage.getItem('krate-theme'); } catch (e) {} d.dataset.theme = t || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'); })();</script>"""

KIT_LINKS = """<link rel="preload" href="/fonts/geist-var-latin.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="/kit/site.css">"""

KIT_SCRIPT = """<script src="/kit/site.js"></script>"""

ARROW = """<svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4.5 2.5L8 6l-3.5 3.5"/></svg>"""

HEADER = f"""<header class="hd">
  <a class="brand" href="/"><img src="{MARK}" alt="" width="26" height="26">Krate</a>
  <nav class="menu" id="menu" aria-label="Main">
    <a class="mi" href="/" data-set="product">Product</a>
    <a class="mi" href="/studio/" data-set="studio">Studio</a>
    <a class="mi" href="/cloud/" data-set="apps">Apps</a>
    <a class="mi" href="/docs/" data-set="docs">Docs</a>
    <a class="mi" href="/download/">Download</a>
    <div class="drawer" id="drawer" role="menu">
      <div class="set" data-set="product"><a href="/">Overview</a><a href="/reports/">Measurements</a><a href="/progress/">Progress</a><a href="/faq/">Questions</a><a href="https://github.com/incyashraj/krate/releases" target="_blank" rel="noopener">Changelog <span class="ext">↗</span></a></div>
      <div class="set" data-set="studio"><a href="/studio/">Krate Studio</a><a href="/app/">Make an app in your browser</a><a href="/download/">Download Studio</a></div>
      <div class="set" data-set="apps"><a href="/cloud/">Apps</a><a href="/open/">Received an app?</a><a href="/publish/">Publish an app</a></div>
      <div class="set" data-set="docs"><a href="/docs/">Docs</a><a href="/docs/quickstart.html">Quickstart</a><a href="/docs/pages/make-an-app-with-ai.html">Make an app with AI</a><a href="https://github.com/incyashraj/krate" target="_blank" rel="noopener">GitHub <span class="ext">↗</span></a></div>
    </div>
  </nav>
  <div class="hr">
    <button class="thm" id="thm" aria-label="Switch between light and dark"><svg class="sun" width="17" height="17" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><circle cx="9" cy="9" r="3.4"/><path d="M9 1.8v1.6M9 14.6v1.6M1.8 9h1.6M14.6 9h1.6M3.9 3.9l1.1 1.1M13 13l1.1 1.1M3.9 14.1L5 13M13 5l1.1-1.1"/></svg><svg class="moon" width="16" height="16" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"><path d="M15.2 11.2A6.6 6.6 0 0 1 6.8 2.8a6.6 6.6 0 1 0 8.4 8.4z"/></svg></button>
    <a class="signin" href="/login/">Sign in</a>
    <a class="cta" href="/app/">Try Krate</a>
    <button class="burger" id="burger" aria-label="Menu" aria-expanded="false"><svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M3 6.5h12M3 11.5h12"/></svg></button>
  </div>
</header>"""

MNAV = """<nav class="mnav" id="mnav" aria-label="Menu">
  <h6>Krate</h6><a href="/">Overview</a><a href="/studio/">Studio</a><a href="/app/">Make an app in your browser</a><a href="/cloud/">Apps</a><a href="/download/">Download</a>
  <h6>Help</h6><a href="/docs/">Docs</a><a href="/faq/">Questions</a><a href="/open/">Received an app?</a><a href="/contact/">Contact</a><a href="/login/">Sign in</a>
</nav>"""

FOOTER = f"""<footer class="foot">
  <div class="fm"><img src="{MARK}" alt="Krate" width="30" height="30"></div>
  <div><h5>Krate</h5><a href="/studio/">Studio</a><a href="/app/">Make in your browser</a><a href="/cloud/">Apps</a><a href="/download/">Download</a><a href="/about/">About</a></div>
  <div><h5>Developers</h5><a href="/docs/">Docs</a><a href="/docs/quickstart.html">Quickstart</a><a href="https://github.com/incyashraj/krate" target="_blank" rel="noopener">GitHub</a><a href="https://github.com/incyashraj/krate/releases" target="_blank" rel="noopener">Changelog</a></div>
  <div><h5>Help</h5><a href="/faq/">Questions</a><a href="/open/">Received an app?</a><a href="/contact/">Contact</a><a href="/account/">Account</a></div>
  <div><h5>Terms &amp; policies</h5><a href="/terms/">Terms</a><a href="/privacy/">Privacy</a><a href="/copyright/">Copyright</a><a href="/reports/">Measurements</a></div>
  <div class="copy"><span>Krate © 2026 Krate Labs</span><span>Open source under MIT or Apache-2.0</span></div>
</footer>"""

PARTS = {
    "header": (re.compile(r'<header class="hd">.*?</header>', re.S), HEADER),
    "menu": (re.compile(r'<nav class="mnav".*?</nav>', re.S), MNAV),
    "footer": (re.compile(r'<footer class="foot">.*?</footer>', re.S), FOOTER),
    "theme script": (re.compile(r"<script>\(\(\) => \{ (?:const d = document\.documentElement; d\.classList\.add\('js'\); )?let t = null;.*?</script>", re.S), HEAD_THEME),
}

# Where kit pages live in the repository (generated pages are checked by
# their generators' own tests, and in the assembled site by --check-site).
SOURCES = ["docs/landing", "docs/open", "docs/cloud", "docs/support", "docs/pages", "docs/answers"]


def is_kit(html: str) -> bool:
    return re.search(r'<link[^>]+href="[^"]*/kit/site(?:\.[0-9a-f]{12})?\.css"', html) is not None


def problems(html: str) -> list:
    out = []
    for name, (pat, want) in PARTS.items():
        found = pat.findall(html)
        if len(found) != 1:
            out.append(f"has {len(found)} {name}s, wants 1")
        elif found[0] != want:
            out.append(f"{name} differs from scripts/site_kit.py")
    markup = re.sub(r"<script\b.*?</script>", "", html, flags=re.S)
    if markup.count("<!--") != markup.count("-->"):
        # An unclosed comment silently swallows everything after it, scripts
        # included: a page that renders and does nothing.
        out.append("has an unclosed HTML comment")
    if not re.search(r'<script src="[^"]*/kit/site(?:\.[0-9a-f]{12})?\.js"></script>', html):
        out.append("does not load /kit/site.js")
    return out


def apply(html: str) -> str:
    for name, (pat, want) in PARTS.items():
        if len(pat.findall(html)) != 1:
            raise SystemExit(f"site_kit: cannot apply, the page has {len(pat.findall(html))} {name}s")
        html = pat.sub(lambda m: want, html, count=1)
    return html


def kit_pages(root: pathlib.Path):
    for src in SOURCES:
        for page in sorted((root / src).rglob("*.html")):
            text = page.read_text(encoding="utf-8")
            if is_kit(text):
                yield page, text


# Hidden by design rather than waiting for a reveal: a shut drawer or menu.
HIDDEN_STATES = {".drawer", ".mnav"}


def hidden_ungated(css: str) -> list:
    """Selectors in the kit stylesheet that start something invisible
    (opacity: 0) without being gated on .js or being a closed menu. Those
    stay invisible to a reader with scripts off."""
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)
    out = []
    for sel, body in re.findall(r"([^{}]+)\{([^{}]*)\}", css):
        if not re.search(r"opacity:\s*0(?![.\d])", body):
            continue
        for name in (x.strip() for x in sel.split(",")):
            if name.startswith("@") or name in HIDDEN_STATES or name.startswith(".js ") or name in ("from", "to") or name.endswith("%"):
                continue
            out.append(name)
    return out


def check(root: pathlib.Path) -> int:
    bad = 0
    n = 0
    for name in hidden_ungated((root / "docs/landing/kit/site.css").read_text()):
        print(f"docs/landing/kit/site.css: {name} starts hidden and is not gated on .js")
        bad += 1
    for page, text in kit_pages(root):
        n += 1
        for p in problems(text):
            print(f"{page.relative_to(root)}: {p}")
            bad += 1
    print(f"site_kit: {n} kit page(s), {bad} problem(s)")
    return 1 if bad else 0


def self_test() -> int:
    page = f"<html><head>{HEAD_THEME}{KIT_LINKS}</head><body>{HEADER}{MNAV}<main>x</main>{FOOTER}{KIT_SCRIPT}</body></html>"
    assert is_kit(page) and problems(page) == [], problems(page)
    drift = page.replace('href="/faq/">Questions', 'href="/faq/">FAQ', 1)
    assert problems(drift), "a drifted header must be reported"
    assert apply(drift) == page, "apply restores the canonical parts"
    assert not is_kit('<link rel="stylesheet" href="/site.css">'), "the old /site.css is not the kit"
    assert is_kit('<link rel="stylesheet" href="/kit/site.0123456789ab.css">'), "a hashed kit sheet is the kit"
    assert "has an unclosed HTML comment" in problems(page.replace("<main>", "<main><!-- note </main>"))
    nojs = page.replace(KIT_SCRIPT, "")
    assert "does not load /kit/site.js" in problems(nojs)
    assert hidden_ungated(".rv { opacity: 0; }") == [".rv"], "an ungated reveal is reported"
    assert hidden_ungated(".js .rv { opacity: 0; } .drawer { opacity: 0; } .x { opacity: 0.5; }") == []
    print("ok  site_kit self-test")
    return 0


def main(argv) -> int:
    if "--self-test" in argv:
        return self_test()
    if "--check" in argv:
        return check(ROOT)
    if "--apply" in argv:
        for f in argv[argv.index("--apply") + 1:]:
            p = pathlib.Path(f)
            p.write_text(apply(p.read_text(encoding="utf-8")), encoding="utf-8")
            print(f"applied the kit parts to {f}")
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
