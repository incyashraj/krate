#!/usr/bin/env python3
"""Every page under docs/landing opens and closes its block elements in pairs.

A browser never complains about a missing </div>: it nests everything after
it one level deeper. On 2026-10-06 one missing </div> in the homepage's email
scene put the "Open it anywhere" and "Trust it" scenes inside the hidden
"Send it" scene, so both showed as a blank white panel, and every other check
passed. This one reads each page and names the first element that is never
closed, or the closing tag that has nothing to close.

  python3 scripts/check-html-balance.py              check every page
  python3 scripts/check-html-balance.py --self-test  prove it catches both faults
"""
import sys
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "docs" / "landing"
# Containers whose end tag is required. Elements the HTML parser may close on
# its own (p, li, td, option ...) are left out on purpose.
BLOCKS = {"div", "section", "main", "article", "header", "footer", "nav",
          "aside", "details", "figure", "ul", "ol", "form", "picture", "svg"}


class Balance(HTMLParser):
    def __init__(self):
        super().__init__()
        self.stack = []
        self.faults = []

    def handle_starttag(self, tag, attrs):
        if tag in BLOCKS:
            cls = dict(attrs).get("class") or ""
            self.stack.append((tag, cls, self.getpos()[0]))

    def handle_startendtag(self, tag, attrs):
        pass  # <div/> style self-closing tags open nothing

    def handle_endtag(self, tag):
        if tag not in BLOCKS:
            return
        line = self.getpos()[0]
        if not self.stack:
            self.faults.append(f"line {line}: </{tag}> closes nothing")
            return
        top, cls, opened = self.stack[-1]
        if top == tag:
            self.stack.pop()
            return
        # Name the element that was left open, the useful half of the fault.
        self.faults.append(
            f"line {line}: </{tag}> found while <{top} class=\"{cls}\"> "
            f"from line {opened} is still open")
        # Recover: pop to the matching tag if there is one, so one fault is
        # reported once rather than cascading down the page.
        for i in range(len(self.stack) - 1, -1, -1):
            if self.stack[i][0] == tag:
                del self.stack[i:]
                break


def faults_in(text):
    p = Balance()
    p.feed(text)
    p.close()
    out = list(p.faults)
    for tag, cls, opened in p.stack:
        out.append(f"line {opened}: <{tag} class=\"{cls}\"> is never closed")
    return out


def self_test():
    good = '<section><div class="a"><div class="b"><img src=x></div></div></section>'
    missing = '<section><div class="scene"><div class="card"></div></section>'
    extra = '<section><div class="a"></div></div></section>'
    svg = '<div><svg viewBox="0 0 1 1"><path d="M0 0"/></svg></div>'
    assert faults_in(good) == [], faults_in(good)
    assert faults_in(svg) == [], faults_in(svg)
    m = faults_in(missing)
    assert m and 'class="scene"' in m[0], m
    e = faults_in(extra)
    joined = " ".join(e)
    assert e and ("closes nothing" in joined or "still open" in joined), e
    # The real fault, reduced: a scene left open swallows the next scene.
    real = ('<div class="view"><div class="vw send"><div class="stg">'
            '<div class="sc"><div class="mailw"></div><div class="ok"></div>'
            '</div></div><div class="vw openv"></div></div>')
    r = faults_in(real)
    assert r, "the swallowed-scene fault must be caught"
    print("self-test passed")


def main():
    if "--self-test" in sys.argv:
        self_test()
        return 0
    bad = 0
    pages = sorted(ROOT.rglob("*.html"))
    for page in pages:
        f = faults_in(page.read_text(encoding="utf-8"))
        if f:
            bad += 1
            print(page.relative_to(ROOT.parent.parent))
            for x in f[:5]:
                print("  " + x)
    if bad:
        print(f"{bad} page(s) with unbalanced tags")
        return 1
    print(f"{len(pages)} pages balanced")
    return 0


if __name__ == "__main__":
    sys.exit(main())
