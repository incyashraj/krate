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


def main() -> int:
    if len(sys.argv) != 2:
        sys.stderr.write("usage: fingerprint-css.py <site-dir>\n")
        return 2

    site = pathlib.Path(sys.argv[1])
    css = site / "krate.css"
    if not css.is_file():
        # Loud rather than silent: a site with no stylesheet is a broken site,
        # and finding out from a screenshot is how this bug was found.
        sys.stderr.write(f"no stylesheet at {css}; the site would ship unstyled\n")
        return 1

    digest = hashlib.sha256(css.read_bytes()).hexdigest()[:12]
    fingerprinted = f"krate.{digest}.css"
    css.rename(site / fingerprinted)

    # Every reference, whatever path shape the page used to write it.
    pattern = re.compile(r'(href="[^"]*?)krate\.css(")')
    rewritten = 0
    for page in site.rglob("*.html"):
        text = page.read_text(encoding="utf-8")
        new_text, count = pattern.subn(rf"\1{fingerprinted}\2", text)
        if count:
            page.write_text(new_text, encoding="utf-8")
            rewritten += count

    if rewritten == 0:
        # The rename succeeded but nothing points at the new name, so every
        # page is now unstyled. Fail the build rather than publish that.
        sys.stderr.write(
            f"renamed the stylesheet to {fingerprinted} but no page referenced it; "
            "the site would ship unstyled\n"
        )
        return 1

    print(f"stylesheet -> {fingerprinted} ({rewritten} reference(s) rewritten)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
