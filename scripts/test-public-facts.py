#!/usr/bin/env python3
"""Regression tests for public fact scope, bundle accounting and generation."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import public_facts as facts


class PublicFactsTests(unittest.TestCase):
    def test_llms_is_generated_and_scoped(self):
        rendered = facts.render_llms()
        self.assertEqual(rendered, (facts.ROOT / "docs/landing/llms.txt").read_text())
        for phrase in ("recipient installs", "not a measurement of the latest release",
                       "Studio has separate licensing", "not all distributed", "Source-bearing"):
            self.assertIn(phrase, rendered)
        for phrase in ("15-40 KB", "guarantees", "cannot do anything", "80-200 MB"):
            self.assertNotIn(phrase, rendered)

    def test_three_claims_use_same_audited_run(self):
        claims = facts.benchmark_claims()
        self.assertEqual(len(claims), 3)
        self.assertEqual(len({c["evidence_run"] for c in claims}), 1)
        self.assertEqual({c["measured_on"] for c in claims}, {"2026-08-25"})
        self.assertEqual(claims[1]["us"], "178.5 MiB, one process")
        self.assertEqual(claims[2]["us"], "237.1 ms median")

    def test_missing_raw_samples_are_not_required_for_site_build(self):
        # The generated pages explain that public analysis != all retained raw data.
        for c in facts.benchmark_claims():
            self.assertTrue((facts.ROOT / c["source"]).is_file())

    def test_release_failure_does_not_promote_a_local_tag(self):
        with patch.dict(os.environ, {}, clear=True), patch("public_facts.subprocess.run", side_effect=OSError):
            self.assertIsNone(facts.published_release())

    def test_release_override_is_validated(self):
        with patch.dict(os.environ, {"KRATE_PUBLIC_RELEASE": "v0.5.0"}):
            self.assertEqual(facts.published_release(), "v0.5.0")
        with patch.dict(os.environ, {"KRATE_PUBLIC_RELEASE": '<script>alert(1)</script>'}):
            with self.assertRaises(ValueError):
                facts.published_release()

    def test_bundles_keep_same_named_artifacts_and_distinguish_payload(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for shelf in ("ported", "store"):
                directory = root / "evidence" / shelf
                directory.mkdir(parents=True)
                with zipfile.ZipFile(directory / "example.krate", "w", zipfile.ZIP_DEFLATED) as z:
                    z.writestr("code.wasm", b"a" * 4096)
                    z.writestr("manifest.toml", "test")
                    if shelf == "store":
                        z.writestr("source/src/lib.rs", "test")
            rows = facts.bundle_inventory(root)
            self.assertEqual(len(rows), 2)
            self.assertEqual(len({r["path"] for r in rows}), 2)
            self.assertEqual({r["source"] for r in rows}, {True, False})
            self.assertTrue(all(r["code_bytes"] == 4096 for r in rows))
            self.assertTrue(all(r["bundle_bytes"] < r["code_bytes"] for r in rows))

    def test_generated_pages_and_links(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ("reports", "progress"):
                output = Path(tmp) / f"{name}.html"
                env = dict(os.environ, KRATE_PUBLIC_RELEASE="v0.5.0")
                subprocess.run(["python3", str(facts.ROOT / f"scripts/build-{name}-page.py"),
                                "--output", str(output)], check=True, env=env, capture_output=True)
                html = output.read_text()
                self.assertIn(f'<link rel="canonical" href="https://krate.tech/{name}/"', html)
                self.assertIn("Full bundle bytes", html)
                self.assertIn("Code payload bytes", html)
                self.assertIn("/docs/limits.html", html)
                for stale in ("vs Discord", "vs a Krate game", "The sandbox is free",
                              "780", "All of them.", "no multiplayer", "Open sign-ups."):
                    self.assertNotIn(stale, html)
                for row in facts.bundle_inventory():
                    self.assertIn(row["path"], html)
                if name == "reports":
                    self.assertIn("237.1 ms median", html)
                    self.assertIn("2,299.4 MiB across four processes", html)
                    self.assertIn("does not run a new benchmark", html)
                else:
                    self.assertIn("not a fresh cross-platform test run", html)

    def test_introduction_has_no_retired_product_state(self):
        intro = (facts.ROOT / "docs/book/src/introduction.md").read_text()
        for stale in ("v0.1.0-rc1", "Mobile hosts, bundles", "- A `.krate` bundle format.",
                      "full GUI surface", "first drawn-widget"):
            self.assertNotIn(stale, intro)
        self.assertIn("recipient installs a compatible runtime once", intro)


if __name__ == "__main__":
    unittest.main()
