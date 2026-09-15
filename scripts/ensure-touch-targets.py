#!/usr/bin/env python3
"""Give every assembled page the phone rules -- touch targets and a type
floor -- and prove it.

Each page on krate.tech inlines its own CSS -- there is no shared
stylesheet to put a rule in once. So the `@media (pointer: coarse)` block
that makes links and buttons 44px was copy-pasted into seven pages and
missing from three (faq, privacy, login), whose links measured 20px on a
phone. Nothing reported that, because nothing could: the rule's absence
looks exactly like a page that does not need it.

This runs last in the deploy, over the assembled _site, so a page cannot
ship without the rules and a NEW page gets them the day it is added
rather than the day someone notices.

The block is deliberately conservative. It sets a minimum height on
things a thumb presses and leaves everything else alone -- no colours, no
spacing, no layout. A page that already has its own coarse rules keeps
them: this one is appended, so on a tie the page's own rule wins on
source order, and where they disagree the larger minimum applies anyway.

  --self-test  exercises the insertion and the idempotence check on
               fixtures, including a page that already has the rules and
               a page with no <head> at all.
"""

import sys
import re
from pathlib import Path

MARK = "krate-touch-targets"

BLOCK = """
<style id="%s">
/* Added at deploy time by scripts/ensure-touch-targets.py.
   A thumb needs 44px. Every page inlines its own CSS, so this rule used to
   be copy-pasted per page and was missing from three of them. */
@media (pointer: coarse) {
  a[href], button, input[type="submit"], input[type="button"], [role="button"] {
    min-height: 44px;
  }
  /* A link inside a sentence is text, not a control: making it 44px tall
     would stripe the paragraph with gaps. Only standalone links grow. */
  p a[href], li a[href], td a[href], h1 a[href], h2 a[href], h3 a[href] {
    min-height: 0;
  }
  /* Links are inline by default and ignore a height; these do not. */
  nav a[href], footer a[href], .foot a[href],
  a.btn, a.pill, a.cta, a.brand, a.name, .brandbar .name {
    display: inline-flex; align-items: center;
  }
}
/* A floor under the type, on phones only. 10px is a label on a monitor you
   are sitting two feet from and a smudge on a phone held at arm's length.
   The pages set these per-page too, and drifted the same way the touch
   rules did -- the star pill is 11.5px on the homepage and 10px on
   /studio, from the same design. Scoped to small chips and captions, so
   body copy and headings keep whatever the page chose.

   Written `:where(html) .x` so it reads as 0-1-0 in the cascade but still
   outranks a page's own single-class rule by source order -- and, more to
   the point, so a DESCENDANT rule like the gallery's `.appc .kb` (0-2-0)
   does not silently beat it. That one kept the size at 11px through a
   whole deploy while this block was present and correct further down the
   page. Doubling the class is the smallest thing that wins without
   reaching for !important, which would also override a page that has a
   good reason to be smaller. */
@media (max-width: 860px) {
  .starpill.starpill, .badge.badge, .tag.tag, .kicker.kicker,
  .ta-kicker.ta-kicker, .num.num, .dlnote.dlnote, .note.note, .kb.kb {
    font-size: 11.5px;
  }
}
</style>
""".strip() % MARK


def already_has(html: str) -> bool:
    """True when the page carries the injected block itself.

    Matched on the style tag's id attribute, not on the bare word: a page
    that merely mentions the marker in its prose (this repo's own docs do)
    would otherwise be read as already done and silently skipped."""
    return re.search(r'<style[^>]*\bid=["\']%s["\']' % re.escape(MARK), html, re.I) is not None


def ensure(html: str) -> tuple[str, bool]:
    """Return (html, changed). Idempotent: a page already carrying the
    block is returned untouched."""
    if already_has(html):
        return html, False
    # Last thing in the head, so a page's own rules of equal specificity
    # still win on source order.
    at = html.lower().rfind("</head>")
    if at == -1:
        # No head (a fragment, or hand-written HTML that omits it). Put it
        # at the top: a <style> is valid in the body and still applies.
        return BLOCK + "\n" + html, True
    return html[:at] + BLOCK + "\n" + html[at:], True


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    if len(argv) < 2:
        print("usage: ensure-touch-targets.py <site-dir> [--self-test]", file=sys.stderr)
        return 2
    root = Path(argv[1])
    if not root.is_dir():
        print(f"not a directory: {root}", file=sys.stderr)
        return 2

    changed = skipped = 0
    for page in sorted(root.rglob("*.html")):
        text = page.read_text(encoding="utf-8", errors="replace")
        out, did = ensure(text)
        if did:
            page.write_text(out, encoding="utf-8")
            changed += 1
        else:
            skipped += 1
    print(f"touch targets: {changed} pages given the rules, {skipped} already had them")
    return 0


def self_test() -> int:
    # 1. A plain page gets the block, inside the head, once.
    page = "<html><head><title>x</title></head><body><a href='/'>Home</a></body></html>"
    out, did = ensure(page)
    assert did, "a page without the rules must be changed"
    assert MARK in out, "the marker must land in the output"
    assert out.count(MARK) == 1, "exactly one copy"
    assert out.index(MARK) < out.lower().index("</head>"), "it belongs in the head"

    # 2. Running twice changes nothing: the deploy may re-run over a tree.
    again, did2 = ensure(out)
    assert not did2, "a page that already has the rules must be left alone"
    assert again == out, "idempotent"

    # 3. A page with no head still gets them.
    frag = "<div><a href='/'>Home</a></div>"
    out3, did3 = ensure(frag)
    assert did3 and MARK in out3, "a headless fragment still gets the rules"

    # 4. The rule really is scoped to coarse pointers -- a desktop must not
    #    get 44px links out of this.
    assert "@media (pointer: coarse)" in BLOCK, "scoped to touch"
    # 5. ...and prose links are exempted, or paragraphs would grow gaps.
    assert "p a[href]" in BLOCK and "min-height: 0" in BLOCK, "prose links exempt"
    # 5b. The type floor is scoped by WIDTH, not by pointer: a desktop with
    #     a touchscreen should keep the finer setting it was designed with.
    assert "@media (max-width: 860px)" in BLOCK, "the type floor is a phone rule"
    assert "font-size: 11.5px" in BLOCK, "the type floor has a size"
    # 5c. And it must not touch body copy: raising every p would rewrite
    #     the reading size of the whole site.
    floor = BLOCK[BLOCK.index("@media (max-width: 860px)"):]
    for tag in ("p ", "p,", "body", "h1", "h2"):
        assert tag not in floor.split("{")[1], f"the type floor must not name {tag!r}"

    # 6. A negative fixture: a page that merely MENTIONS the marker in its
    #    prose must still get the rules. Matching the bare word would skip
    #    it and the miss would be invisible -- which is the exact shape of
    #    the bug this script exists to prevent.
    prose = "<html><head></head><body>we use krate-touch-targets here</body></html>"
    out6, did6 = ensure(prose)
    assert did6, "a page that only mentions the marker must still be treated"
    assert already_has(out6), "and afterwards it really does have the block"

    # 7. And the reverse: the real block is recognised, however its
    #    attributes are quoted.
    assert already_has('<style id="%s">x</style>' % MARK)
    assert already_has("<style id='%s'>x</style>" % MARK)
    assert not already_has("<p>%s</p>" % MARK), "prose alone is not the block"

    print("ok  ensure-touch-targets self-test")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
