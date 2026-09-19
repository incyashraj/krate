#!/usr/bin/env python3
"""Offline checks for the public contributor entry points."""
from pathlib import Path
import re
import subprocess
import tomllib
import unittest
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parent.parent
DOCS = (
    "CONTRIBUTING.md",
    ".github/PULL_REQUEST_TEMPLATE.md",
    "docs/book/src/contributing/first-pr.md",
)


class ContributorDocsTests(unittest.TestCase):
    def setUp(self):
        self.texts = {name: (ROOT / name).read_text() for name in DOCS}

    def test_no_private_phase_prerequisites_or_future_site_placeholder(self):
        for name, text in self.texts.items():
            with self.subTest(file=name):
                for stale in ("Plan/", "P{N}", "p{N}", "p{phase}",
                              "P1-RT-02", "Good Phase 0", "under 10 minutes",
                              "When GitHub Pages is live"):
                    self.assertNotIn(stale, text)
        self.assertIn("No internal plan or phase task ID is needed.",
                      self.texts[DOCS[2]])
        self.assertIn("if applicable", self.texts[DOCS[1]])

    def test_repository_links_resolve_to_public_tracked_files(self):
        tracked = set(subprocess.check_output(
            ["git", "ls-files"], cwd=ROOT, text=True).splitlines())
        count = 0
        for name, text in self.texts.items():
            for target in re.findall(r"\[[^\]]+\]\(([^)]+)\)", text):
                parsed = urlsplit(target)
                if parsed.scheme or parsed.netloc:
                    prefix = "/incyashraj/krate/blob/main/"
                    if parsed.netloc != "github.com" or not parsed.path.startswith(prefix):
                        continue
                    path = ROOT / unquote(parsed.path[len(prefix):])
                else:
                    path = ROOT / name
                    path = path.parent / unquote(parsed.path)
                relative = path.resolve().relative_to(ROOT).as_posix()
                with self.subTest(file=name, link=target):
                    self.assertIn(relative, tracked, "link must work outside the private checkout")
                    self.assertTrue(path.is_file())
                count += 1
        self.assertGreaterEqual(count, 10)

    def test_build_instructions_match_repository_tools(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        self.assertIn("workspace", manifest)
        toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
        self.assertIn("rustfmt", toolchain["components"])
        self.assertIn("clippy", toolchain["components"])
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("mdbook-v0.4.40", ci)
        for name in (DOCS[0], DOCS[2]):
            for command in ("cargo build --workspace", "cargo test --workspace",
                            "cargo fmt --all -- --check",
                            "cargo clippy --all-targets --all-features -- -D warnings",
                            "cargo install mdbook --locked --version 0.4.40",
                            "mdbook build docs/book"):
                self.assertIn(command, self.texts[name])

    def test_licensing_scope_links_existing_terms(self):
        text = self.texts[DOCS[0]]
        for path in ("LICENSE-MIT", "LICENSE-APACHE", "studio/LICENSE", "cloud/worker/LICENSE"):
            self.assertIn(f"]({path})", text)
        for path in ("studio/LICENSE", "cloud/worker/LICENSE"):
            license_text = (ROOT / path).read_text()
            self.assertIn("Business Source License 1.1", license_text)
            self.assertIn("2030-08-30", license_text)
        self.assertIn("This guide does not replace\nor change those terms.", text)
        self.assertIn("There is no CLA.", text)

    def test_review_policy_stays_intact(self):
        text = self.texts[DOCS[0]]
        policy = text.split("## Decision-making\n", 1)[1].split("\n---", 1)[0].strip()
        self.assertEqual(policy, """- Small changes: PR author decides, one maintainer approves.
- Large changes: write an ADR, open for discussion, merge with two approvals.
  Founder stage, while `docs/governance/maintainers.json` lists one active
  maintainer: that maintainer records their own review in the PR, and the
  two-approval rule resumes when a second is registered.
- Breaking changes to UAPI interfaces: require an ADR + two weeks open comment period.""")
        self.assertIn("All CI checks must pass. Zero clippy warnings.", text)

    def test_commit_example_stages_only_the_changed_file(self):
        text = self.texts[DOCS[2]]
        self.assertNotIn("git add .", text)
        self.assertIn("git add docs/book/src/contributing/first-pr.md", text)
        self.assertIn("git diff --staged", text)

    def test_fast_ci_watches_and_checks_contributor_files(self):
        workflow = (ROOT / ".github/workflows/site-checks.yml").read_text()
        for path in ("CONTRIBUTING.md", ".github/PULL_REQUEST_TEMPLATE.md",
                     "scripts/test-contributor-docs.py"):
            self.assertEqual(workflow.count(f'- "{path}"'), 2, path)
        self.assertIn('run: python3 scripts/test-contributor-docs.py', workflow)


if __name__ == "__main__":
    unittest.main()
