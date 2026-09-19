#!/usr/bin/env python3
"""Offline regression coverage for Pages crawl metadata."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("site_seo", Path(__file__).with_name("site-seo.py"))
seo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(seo)


class SeoTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "robots.txt").write_text("User-agent: *\nAllow: /\nSitemap: https://krate.tech/sitemap.xml\n")
        self.page("index.html", "Krate")

    def page(self, path, title, body="", head=""):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(f'<!doctype html><html><head><title>{title}</title>{head}</head>'
                        f'<body><nav><p>Unrelated navigation text must never become a description.</p></nav>'
                        f'<main><h1>{title}</h1><p>This explains {title} with enough useful detail for a page description.</p>'
                        f'{body}</main></body></html>')
        return file

    def test_canonical_aliases_and_sitemap_discovery(self):
        for path in ("docs/index.html", "docs/introduction.html"):
            self.page(path, "Introduction")
        for path in ("faq.html", "faq/index.html"):
            self.page(path, "Frequently asked questions")
        for path in ("docs/porting.html", "docs/limits.html", "docs/phase2/benchmarks.html",
                     "desktop-app-distribution.html"):
            self.page(path, path)
        seo.finalize(self.root)
        self.assertTrue(seo.check(self.root))
        sitemap = (self.root / "sitemap.xml").read_text()
        self.assertNotIn("introduction.html", sitemap)
        self.assertNotIn("faq.html", sitemap)
        self.assertNotIn("lastmod", sitemap)
        self.assertIn("https://krate.tech/docs/</loc>", sitemap)
        for path in ("docs/porting.html", "docs/limits.html", "docs/phase2/benchmarks.html",
                     "desktop-app-distribution.html"):
            self.assertIn(path, sitemap)

    def test_noindex_shells_but_not_help_contact_or_privacy(self):
        private = ("app/index.html", "login/index.html", "login/done/index.html", "account/index.html",
                   "billing/done/index.html", "publish/index.html", "support/index.html",
                   "cloud/app/index.html", "docs/print.html", "404.html")
        public = ("contact/index.html", "privacy/index.html", "docs/help.html", "studio/index.html")
        for path in private + public:
            self.page(path, path)
        seo.finalize(self.root)
        for path in private:
            self.assertTrue(seo.Page((self.root / path).read_text()).noindex, path)
        for path in public:
            self.assertFalse(seo.Page((self.root / path).read_text()).noindex, path)
        self.assertTrue(seo.check(self.root))

    def test_preserves_editorial_description_and_identity_jsonld(self):
        file = self.page("studio/index.html", "Studio", head='<meta name="description" content="An editorial description.">'
                         '<script type="application/ld+json">{"@type":"SoftwareApplication","name":"Krate"}</script>')
        seo.finalize(self.root)
        page = seo.Page(file.read_text())
        self.assertEqual(page.descriptions, ["An editorial description."])
        self.assertEqual(page.jsonld, ['{"@type":"SoftwareApplication","name":"Krate"}'])

    def test_description_uses_main_content_not_navigation(self):
        file = self.page("docs/porting.html", "Porting", head='<meta name="description" content="Build a desktop app in Rust, compile it to a WebAssembly component.">')
        seo.finalize(self.root)
        description = seo.Page(file.read_text()).descriptions[0]
        self.assertIn("Porting", description)
        self.assertNotIn("navigation", description)
        self.assertNotIn("Build a desktop", description)

    def test_seven_editorial_descriptions_are_unique_and_page_specific(self):
        descriptions = seo.EDITORIAL_DESCRIPTIONS
        self.assertEqual(len(descriptions), 7)
        self.assertEqual(len(set(descriptions.values())), 7)
        for path, description in descriptions.items():
            with self.subTest(path=path):
                self.assertGreaterEqual(len(description), 100)
                self.assertLessEqual(len(description), 170)
                self.assertNotIn("15 kilobytes", description)
                self.assertNotIn("15 to 40 KB", description)
                self.assertNotIn("Status: Active", description)
                self.assertNotIn("safe to open", description)
                self.assertTrue((Path(__file__).resolve().parents[1] / "docs/book/src" /
                                 path.removeprefix("/docs/").replace(".html", ".md")).is_file())

    def test_editorial_summaries_replace_old_snippets_without_changing_body(self):
        body = '<p>Original dated prose: 15 kilobytes. Status: Active.</p>'
        for path, description in seo.EDITORIAL_DESCRIPTIONS.items():
            file = self.page(path.lstrip("/"), path, body=body,
                             head='<meta name="description" content="Old summary: 15 kilobytes.">')
            original_body = file.read_text().split("<body>", 1)[1]
            seo.finalize(self.root)
            self.assertEqual(seo.Page(file.read_text()).descriptions, [description])
            self.assertEqual(file.read_text().split("<body>", 1)[1], original_body)
        before = {p: p.read_bytes() for p in self.root.rglob("*.html")}
        seo.finalize(self.root)
        self.assertEqual(before, {p: p.read_bytes() for p in self.root.rglob("*.html")})

    def test_historical_summaries_do_not_reframe_current_quickstart_as_archive(self):
        descriptions = seo.EDITORIAL_DESCRIPTIONS
        self.assertIn("Historical product framing", descriptions[
            "/docs/blog/0008-make-a-desktop-app-without-being-a-programmer.html"])
        self.assertIn("notes-v0.1.0", descriptions["/docs/try-krate-notes.html"])
        self.assertIn("Phase 2 design record", descriptions["/docs/phases/phase-2.html"])
        self.assertIn("Install Krate", descriptions["/docs/quickstart.html"])
        self.assertNotIn("Historical", descriptions["/docs/quickstart.html"])

    def test_internal_alias_links_keep_query_and_anchor(self):
        self.page("docs/index.html", "Docs")
        self.page("docs/introduction.html", "Docs")
        file = self.page("docs/porting.html", "Porting", body='<a href="introduction.html?x=1&amp;y=2#build">Docs</a>'
                         '<a href="https://example.com/docs/index.html">External</a>')
        seo.finalize(self.root)
        self.assertIn('href="/docs/?x=1&amp;y=2#build"', file.read_text())
        self.assertIn('href="https://example.com/docs/index.html"', file.read_text())

    def test_link_normalization_leaves_javascript_and_comments_untouched(self):
        self.page("docs/index.html", "Docs")
        self.page("docs/introduction.html", "Docs")
        body = '<script>const template = \'<a href="introduction.html">Docs</a>\';</script><!-- <a href="introduction.html">Docs</a> -->'
        file = self.page("docs/porting.html", "Porting", body=body)
        seo.finalize(self.root)
        self.assertIn(body, file.read_text())

    def test_idempotent(self):
        self.page("app/index.html", "App")
        seo.finalize(self.root)
        before = {p.relative_to(self.root): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        seo.finalize(self.root)
        after = {p.relative_to(self.root): p.read_bytes() for p in self.root.rglob("*") if p.is_file()}
        self.assertEqual(before, after)

    def test_fails_missing_canonical_destination(self):
        self.page("reports.html", "Reports")
        with self.assertRaisesRegex(ValueError, "does not exist"):
            seo.finalize(self.root)

    def test_fails_sitemap_drift(self):
        seo.finalize(self.root)
        (self.root / "sitemap.xml").write_text(f'<urlset xmlns="{seo.NS}"></urlset>')
        self.assertFalse(seo.check(self.root))

    def test_fails_malformed_structured_data(self):
        self.page("broken.html", "Broken", head='<script type="application/ld+json">{invalid}</script>')
        seo.finalize(self.root)
        self.assertFalse(seo.check(self.root))

    def test_fails_duplicate_priority_descriptions(self):
        for path, title in (("index.html", "Home"), ("studio/index.html", "Studio")):
            self.page(path, title, head='<meta name="description" content="Same description.">')
        seo.finalize(self.root)
        self.assertFalse(seo.check(self.root))

    def test_preserves_restrictive_robots_controls(self):
        file = self.page("support/index.html", "Admin", head='<meta name="robots" content="noindex, nofollow, nosnippet">')
        seo.finalize(self.root)
        self.assertEqual(seo.Page(file.read_text()).robots, ["nofollow, noindex, nosnippet"])

    def test_fails_search_crawler_block_but_allows_training_block(self):
        seo.finalize(self.root)
        robots = self.root / "robots.txt"
        robots.write_text("User-agent: *\nAllow: /\n\nUser-agent: GPTBot\nDisallow: /\n\nSitemap: https://krate.tech/sitemap.xml\n")
        self.assertTrue(seo.check(self.root))
        robots.write_text("User-agent: *\nDisallow: /\nSitemap: https://krate.tech/sitemap.xml\n")
        self.assertFalse(seo.check(self.root))

    def test_robots_none_is_noindex(self):
        file = self.page("hidden.html", "Hidden", head='<meta name="robots" content="none">')
        seo.finalize(self.root)
        self.assertTrue(seo.Page(file.read_text()).noindex)
        self.assertNotIn("hidden.html", (self.root / "sitemap.xml").read_text())

    def test_required_production_routes_fail_when_missing(self):
        seo.finalize(self.root)
        self.assertFalse(seo.check(self.root, require_priority=True))
        for path in seo.PRIORITY_PATHS - {"/"}:
            self.page(path.lstrip("/") + ("index.html" if path.endswith("/") else ""), path)
        seo.finalize(self.root)
        self.assertTrue(seo.check(self.root, require_priority=True))


if __name__ == "__main__":
    unittest.main()
