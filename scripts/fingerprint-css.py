#!/usr/bin/env python3
"""Give the stylesheet a filename that changes when its contents change.

Every page linked `krate.css` by a name that never changed, and GitHub Pages
serves it with `cache-control: max-age=600`. So a deploy that changed the CSS
shipped new HTML against whatever stylesheet the visitor already had: the page
rendered with no card, no panel, and a headline number set in body text. It
looked like the deploy had failed, and there was nothing on the server to fix
because the server was right.

Naming the file after a hash of its own contents removes the question. A changed
stylesheet is a new URL, so no browser can serve a stale one; an unchanged
stylesheet keeps its URL and stays cached, which is the behaviour worth having.

Run against the assembled site directory, after everything is copied in.
"""

import hashlib
import pathlib
import re
import sys


def fingerprint(site: pathlib.Path, rel: str, required: bool) -> int:
    """Rename site/rel to a name carrying its content hash and repoint every
    page at it. Returns the number of references rewritten, or -1 when the
    file is missing and required."""
    path = site / rel
    if not path.is_file():
        if required:
            # Loud rather than silent: a site with no stylesheet is a broken
            # site, and finding out from a screenshot is how this bug was found.
            sys.stderr.write(f"no stylesheet at {path}; the site would ship unstyled\n")
            return -1
        return 0

    stem, ext = path.stem, path.suffix
    digest = hashlib.sha256(path.read_bytes()).hexdigest()[:12]
    fingerprinted = f"{stem}.{digest}{ext}"
    path.rename(path.with_name(fingerprinted))

    # Every reference, whatever path shape the page used to write it. The
    # directory part of rel must match too, so /kit/site.css is never
    # confused with the old /site.css beside the landing page.
    folder = re.escape(str(pathlib.PurePosixPath(rel).parent).strip("."))
    prefix = rf"(?:[^\"]*?/)?{folder}/" if folder else r"[^\"]*?"
    pattern = re.compile(rf'((?:href|src)="{prefix})' + re.escape(f"{stem}{ext}") + r'(")')
    rewritten = 0
    for page in site.rglob("*.html"):
        text = page.read_text(encoding="utf-8")
        new_text, count = pattern.subn(rf"\g<1>{fingerprinted}\2", text)
        if count:
            page.write_text(new_text, encoding="utf-8")
            rewritten += count
    print(f"{rel} -> {fingerprinted} ({rewritten} reference(s) rewritten)")
    return rewritten


def self_test() -> int:
    import tempfile
    with tempfile.TemporaryDirectory() as d:
        site = pathlib.Path(d)
        (site / "kit").mkdir()
        (site / "site.css").write_text("old{}")
        (site / "kit/site.css").write_text("kit{}")
        (site / "kit/site.js").write_text("void 0")
        (site / "old.html").write_text('<link href="/site.css">')
        (site / "new.html").write_text('<link rel="stylesheet" href="/kit/site.css"><script src="/kit/site.js"></script>')
        sys.argv = ["x", str(site)]
        assert main() == 0, "the build must pass"
        old, new = (site / "old.html").read_text(), (site / "new.html").read_text()
        assert 'href="/site.css"' in old, "a /site.css beside the landing page is not the kit"
        assert re.search(r'href="/kit/site\.[0-9a-f]{12}\.css"', new), "the kit stylesheet is fingerprinted"
        assert re.search(r'src="/kit/site\.[0-9a-f]{12}\.js"', new), "the kit script is fingerprinted"
        assert not (site / "kit/site.css").exists() and (site / "site.css").exists(), "only the kit sheet moved"
    with tempfile.TemporaryDirectory() as d:
        site = pathlib.Path(d)
        (site / "kit").mkdir()
        (site / "kit/site.css").write_text("kit{}")
        (site / "kit/site.js").write_text("void 0")
        (site / "a.html").write_text('<script src="/kit/site.js"></script>')
        sys.argv = ["x", str(site)]
        assert main() == 1, "a kit sheet nothing references must fail the build"
    with tempfile.TemporaryDirectory() as d:
        site = pathlib.Path(d)
        (site / "a.html").write_text("<p>x</p>")
        sys.argv = ["x", str(site)]
        assert main() == 1, "a site without the kit must fail the build"
    with tempfile.TemporaryDirectory() as d:
        site = pathlib.Path(d)
        (site / "kit").mkdir()
        (site / "kit/site.css").write_text("kit{}")
        (site / "kit/site.js").write_text("void 0")
        (site / "krate.css").write_text("a{}")
        (site / "a.html").write_text('<link href="/kit/site.css"><script src="/kit/site.js"></script><link href="/krate.css">')
        sys.argv = ["x", str(site)]
        assert main() == 0
        assert re.search(r'href="/krate\.[0-9a-f]{12}\.css"', (site / "a.html").read_text()), "a leftover krate.css is still hashed"
    print("ok  fingerprint-css self-test")
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    if len(sys.argv) != 2:
        sys.stderr.write("usage: fingerprint-css.py <site-dir>\n")
        return 2

    site = pathlib.Path(sys.argv[1])
    # Since the 2026-10 redesign every page loads the design kit's one
    # stylesheet and one script; the old shared krate.css is gone. A site
    # without the kit, or a kit no page points at, would ship unstyled.
    for rel in ("kit/site.css", "kit/site.js"):
        n = fingerprint(site, rel, required=True)
        if n < 0:
            return 1
        if n == 0:
            sys.stderr.write(f"renamed {rel} but no page referenced it; the site would ship unstyled\n")
            return 1
    # krate.css, if a page still carries it, is hashed the same way.
    if (site / "krate.css").is_file() and fingerprint(site, "krate.css", required=False) == 0:
        sys.stderr.write("renamed krate.css but no page referenced it\n")
        return 1
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
