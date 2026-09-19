#!/usr/bin/env python3
"""Finalize and validate crawl metadata on the fully assembled Pages artifact.

No network, dependencies, invented modification dates or changes to source prose.
Canonical signals preserve old inbound URLs; they are NOT HTTP redirects.
"""
import argparse
from collections import defaultdict
import html
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import sys
from urllib.parse import unquote, urljoin, urlsplit, urlunsplit
from urllib.robotparser import RobotFileParser
import xml.etree.ElementTree as ET

ORIGIN = "https://krate.tech"
NS = "http://www.sitemaps.org/schemas/sitemap/0.9"
ALIASES = {"/docs/introduction.html": "/docs/", **{
    f"/{name}.html": f"/{name}/" for name in ("faq", "reports", "progress")}}
# These are interfaces, callbacks or authenticated shells, not help articles.
# /support/ is the token-protected admin console (docs/support/index.html).
UTILITY_ROOTS = {"app", "account", "login", "billing", "publish", "make", "support"}
UTILITY_PATHS = {"/cloud/app/", "/docs/print.html", "/404.html", "/docs/404.html"}
PRIORITY_PATHS = {"/", "/studio/", "/faq/", "/docs/", "/docs/quickstart.html",
                  "/docs/porting.html", "/docs/limits.html", "/reports/", "/progress/",
                  "/open/", "/cloud/", "/portable-desktop-app-format.html",
                  "/run-ai-generated-code-safely.html", "/share-an-app-made-with-ai.html",
                  "/desktop-app-distribution.html"}
TAGS = re.compile(r"<(?:meta|link)\b(?:[^>\"']|\"[^\"]*\"|'[^']*')*>", re.I)
HREF = re.compile(r"(\bhref\s*=\s*)([\"'])(.*?)\2", re.I | re.S)


def compact(text):
    return " ".join(text.split())


class Page(HTMLParser):
    """Read metadata and paragraphs, excluding navigation and script text."""
    def __init__(self, source):
        super().__init__(convert_charrefs=True)
        self.title_parts, self.headings, self.paragraphs = [], [], []
        self.title_count = 0
        self.descriptions, self.canonicals, self.robots, self.jsonld = [], [], [], []
        self.depth = defaultdict(int)
        self.current_heading = self.current_paragraph = self.current_json = None
        self.has_main = bool(re.search(r"<(?:main|article)\b", source, re.I))
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        self.depth[tag] += 1
        if tag == "title":
            self.title_count += 1
        if tag == "meta":
            name = attrs.get("name", "").lower()
            if name == "description":
                self.descriptions.append(attrs.get("content", ""))
            if name in {"robots", "googlebot", "bingbot"}:
                self.robots.append(attrs.get("content", "").lower())
        if tag == "link" and "canonical" in attrs.get("rel", "").lower().split():
            self.canonicals.append(attrs.get("href", ""))
        if tag == "script" and attrs.get("type", "").lower() == "application/ld+json":
            self.current_json = []
        if self.is_content():
            if tag == "h1":
                self.current_heading = []
            if tag == "p":
                self.current_paragraph = []

    def handle_endtag(self, tag):
        if tag == "h1" and self.current_heading is not None:
            self.headings.append(compact("".join(self.current_heading)))
            self.current_heading = None
        if tag == "p" and self.current_paragraph is not None:
            self.paragraphs.append(compact("".join(self.current_paragraph)))
            self.current_paragraph = None
        if tag == "script" and self.current_json is not None:
            self.jsonld.append("".join(self.current_json))
            self.current_json = None
        self.depth[tag] = max(0, self.depth[tag] - 1)

    def handle_data(self, data):
        if self.depth["title"]:
            self.title_parts.append(data)
        if self.current_json is not None:
            self.current_json.append(data)
        if self.is_content():
            if self.current_heading is not None:
                self.current_heading.append(data)
            if self.current_paragraph is not None:
                self.current_paragraph.append(data)

    def is_content(self):
        return (not any(self.depth[t] for t in ("nav", "script", "style", "header", "footer"))
                and (not self.has_main or self.depth["main"] or self.depth["article"]))

    @property
    def title(self):
        return compact("".join(self.title_parts))

    @property
    def noindex(self):
        return any({"noindex", "none"}.intersection(re.split(r"[,\s]+", value))
                   for value in self.robots)


def normalized_path(path):
    if path.endswith("/index.html"):
        path = path[:-10]
    return ALIASES.get(path, path)


def route(file, root):
    return "/" + file.relative_to(root).as_posix()


def is_utility(path):
    return path.lstrip("/").split("/")[0] in UTILITY_ROOTS or path in UTILITY_PATHS


def local_file(root, path):
    decoded = unquote(path)
    if ".." in Path(decoded).parts:
        return None
    result = root / decoded.lstrip("/")
    if path.endswith("/"):
        result /= "index.html"
    return result if result.is_file() else None


def parse_tag(tag):
    class Tag(HTMLParser):
        def handle_starttag(self, name, attrs):
            self.name, self.attrs = name, dict(attrs)
    parser = Tag()
    parser.feed(tag)
    return parser.name, parser.attrs


def update_head(source, canonical, description, noindex):
    match = re.search(r"<head\b[^>]*>(.*?)</head\s*>", source, re.I | re.S)
    if not match:
        raise ValueError("missing complete HTML head")

    robots_directives = set()

    def remove_managed(match):
        tag, attrs = parse_tag(match.group())
        if tag == "link" and "canonical" in attrs.get("rel", "").lower().split():
            return ""
        if tag == "meta" and attrs.get("name", "").lower() == "description":
            return ""
        # Preserve any existing more-specific crawler directives. noindex on
        # robots applies to all; we don't remove snippet/privacy controls.
        if tag == "meta" and attrs.get("name", "").lower() == "robots" and noindex:
            robots_directives.update(part.strip().lower() for part in attrs.get("content", "").split(",") if part.strip())
            return ""
        if tag == "meta" and attrs.get("property", "").lower() == "og:url":
            return f'<meta property="og:url" content="{html.escape(canonical, quote=True)}">'
        return match.group()

    head = TAGS.sub(remove_managed, match.group(1)).rstrip()
    # Strip the blank lines left by our previous run to stay byte-idempotent.
    head = re.sub(r"\n[ \t]*\n", "\n", head)
    additions = [f'<link rel="canonical" href="{html.escape(canonical, quote=True)}">',
                 f'<meta name="description" content="{html.escape(description, quote=True)}">']
    if noindex:
        robots_directives.discard("index")
        robots_directives.discard("all")
        robots_directives.add("noindex")
        if "nofollow" not in robots_directives and "none" not in robots_directives:
            robots_directives.add("follow")
        else:
            robots_directives.discard("follow")
        additions.append('<meta name="robots" content="' + html.escape(", ".join(sorted(robots_directives)), quote=True) + '">')
    head += "\n" + "\n".join(additions) + "\n"
    return source[:match.start(1)] + head + source[match.end(1):]


def description_for(page, path):
    # mdBook duplicates the book-wide description on every chapter. Replace
    # only that known generic value; editorial descriptions remain untouched.
    existing = page.descriptions[0].strip() if page.descriptions else ""
    generic_book = existing.startswith("Build a desktop app in Rust, compile it to a WebAssembly component")
    if existing and not generic_book:
        return existing
    heading = next((h for h in page.headings if h), page.title)
    paragraph = next((p for p in page.paragraphs if len(p) > 35), "")
    candidate = compact(f"{heading.rstrip('.')}. {paragraph}")
    if len(candidate) > 165:
        candidate = candidate[:162].rsplit(" ", 1)[0].rstrip(".,:;") + "..."
    return candidate or f"Krate documentation: {path}"


def normalize_links(source, source_path, root):
    class Anchors(HTMLParser):
        def __init__(self):
            super().__init__(convert_charrefs=False)
            self.locations = []

        def handle_starttag(self, tag, attrs):
            if tag == "a":
                self.locations.append((self.getpos(), self.get_starttag_text()))

    parser = Anchors()
    parser.feed(source)
    offsets = [0]
    for line in source.splitlines(keepends=True):
        offsets.append(offsets[-1] + len(line))

    def href(attr):
        original = html.unescape(attr.group(3))
        target = urlsplit(urljoin(ORIGIN + source_path, original))
        if target.scheme != "https" or target.netloc != "krate.tech":
            return attr.group()
        preferred = normalized_path(target.path)
        if preferred == target.path or local_file(root, preferred) is None:
            return attr.group()
        value = urlunsplit(("", "", preferred, target.query, target.fragment))
        return attr.group(1) + attr.group(2) + html.escape(value, quote=True) + attr.group(2)

    # HTMLParser does not report tags inside scripts/comments. Replacing in
    # reverse order preserves offsets and never modifies JavaScript strings.
    for (line, column), tag in reversed(parser.locations):
        start = offsets[line - 1] + column
        source = source[:start] + HREF.sub(href, tag) + source[start + len(tag):]
    return source


def documents(root):
    return sorted(root.rglob("*.html"))


def canonical_urls(root):
    urls = set()
    for file in documents(root):
        page = Page(file.read_text())
        if not page.noindex and len(page.canonicals) == 1:
            urls.add(page.canonicals[0])
    return sorted(urls)


def finalize(root):
    if not (root / "index.html").is_file():
        raise ValueError("site root must contain index.html")
    for file in documents(root):
        source = file.read_text()
        path = route(file, root)
        canonical_path = normalized_path(path)
        if local_file(root, canonical_path) is None:
            raise ValueError(f"{path}: canonical target {canonical_path} does not exist")
        page = Page(source)
        if not page.title:
            raise ValueError(f"{path}: missing title; fix its source rather than inventing one")
        source = update_head(source, ORIGIN + canonical_path, description_for(page, path),
                             page.noindex or is_utility(canonical_path))
        source = normalize_links(source, path, root)
        file.write_text(source)
    entries = "\n".join(f"  <url><loc>{html.escape(url)}</loc></url>" for url in canonical_urls(root))
    sitemap = f'<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="{NS}">\n{entries}\n</urlset>\n'
    (root / "sitemap.xml").write_text(sitemap)
    # mdBook copies its source sitemap. Keep that legacy secondary URL from
    # serving an outdated list of aliases; the root remains the advertised one.
    if (root / "docs/sitemap.xml").exists():
        (root / "docs/sitemap.xml").write_text(sitemap)
    print(f"Finalized {len(documents(root))} HTML files; {len(canonical_urls(root))} canonical sitemap URLs")


def check(root, require_priority=False):
    errors, seen = [], defaultdict(list)
    for file in documents(root):
        path = route(file, root)
        page = Page(file.read_text())
        expected = ORIGIN + normalized_path(path)
        if page.canonicals != [expected]:
            errors.append(f"{path}: expected one canonical {expected}; got {page.canonicals}")
        if local_file(root, normalized_path(path)) is None:
            errors.append(f"{path}: canonical target missing")
        if not page.title or page.title_count != 1:
            errors.append(f"{path}: expected one nonempty title")
        if len(page.descriptions) != 1 or not page.descriptions[0].strip():
            errors.append(f"{path}: expected one nonempty description")
        if is_utility(normalized_path(path)) and not page.noindex:
            errors.append(f"{path}: utility shell must be noindex")
        for data in page.jsonld:
            try:
                json.loads(data)
            except ValueError as exc:
                errors.append(f"{path}: invalid JSON-LD: {exc}")
        if not page.noindex:
            seen[expected].append(page)
    for url, pages in seen.items():
        target = local_file(root, urlsplit(url).path)
        if target and Page(target.read_text()).noindex:
            errors.append(f"{url}: indexable alias points to a noindex target")
    for field in ("title", "descriptions"):
        values = defaultdict(set)
        for url, pages in seen.items():
            if urlsplit(url).path in PRIORITY_PATHS:
                values[str(getattr(pages[0], field))].add(url)
        for value, urls in values.items():
            if len(urls) > 1:
                errors.append(f"duplicate priority {field}: {sorted(urls)}")
    if require_priority:
        for path in sorted(PRIORITY_PATHS):
            if ORIGIN + path not in seen:
                errors.append(f"priority page missing or noindex: {path}")
    try:
        tree = ET.parse(root / "sitemap.xml")
        entries = [element.text for element in tree.findall(f"{{{NS}}}url/{{{NS}}}loc")]
        expected = canonical_urls(root)
        if sorted(entries) != expected:
            errors.append("sitemap does not exactly match unique indexable canonical HTML URLs")
        if any(urlsplit(url).netloc != "krate.tech" or not url.startswith(ORIGIN + "/") for url in entries):
            errors.append("sitemap contains a noncanonical origin")
    except (ET.ParseError, OSError) as exc:
        errors.append(f"sitemap unreadable: {exc}")
    robots = root / "robots.txt"
    if not robots.is_file() or f"Sitemap: {ORIGIN}/sitemap.xml" not in robots.read_text():
        errors.append("robots.txt must advertise the root sitemap")
    elif robots.is_file():
        parser = RobotFileParser()
        parser.parse(robots.read_text().splitlines())
        for bot in ("Googlebot", "bingbot", "OAI-SearchBot", "PerplexityBot"):
            for url in seen:
                if not parser.can_fetch(bot, url):
                    errors.append(f"robots.txt blocks {bot} from indexable URL {url}")
    if errors:
        print("SEO checks failed:\n" + "\n".join(errors), file=sys.stderr)
        return False
    print(f"SEO checks passed: {len(documents(root))} HTML files, {len(seen)} canonical indexable URLs")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("finalize", "check"))
    parser.add_argument("site", type=Path)
    parser.add_argument("--require-priority", action="store_true",
                        help="also require the production discovery routes to be indexable")
    args = parser.parse_args()
    try:
        if args.command == "finalize":
            finalize(args.site)
        elif not check(args.site, args.require_priority):
            return 1
    except (ValueError, OSError) as exc:
        print(f"SEO error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
