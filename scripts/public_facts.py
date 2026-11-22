#!/usr/bin/env python3
"""Shared public identity and scoped evidence, not a second benchmark registry.

Generate llms.txt with --output; --check verifies its checked-in copy.
Page generation never runs applications or silently turns a git tag into a
published release. Existing performance claims remain the numeric authority.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def load_facts(root=ROOT):
    facts = json.loads((root / "docs/public-facts.json").read_text())
    if facts["schema"] != "krate.public-facts.v1":
        raise ValueError("Unsupported public facts schema")
    return facts


def benchmark_claims(root=ROOT):
    facts = load_facts(root)
    claims = {c["id"]: c for c in json.loads(
        (root / "evidence/claims/performance.json").read_text())["claims"]}
    selected = [claims[name] for name in facts["benchmark_claim_ids"]]
    for claim in selected:
        if not claim.get("seal") or "raw samples not retained" in claim["confidence"]:
            raise ValueError(f"Unpublishable measurement: {claim['id']}")
        for key in ("source", "seal"):
            if not (root / claim[key]).is_file():
                raise ValueError(f"Missing measurement evidence: {claim[key]}")
        analysis = (root / claim["source"]).read_text().replace(",", "")
        for side in ("us", "them"):
            # The public analysis formats its rows differently from prose,
            # but the leading quantity and unit must still be present there.
            match = re.match(r"([\d,.]+)\s+(KiB|MiB|ms)", claim[side])
            if not match or f"{match[1].replace(',', '')} {match[2]}" not in analysis:
                raise ValueError(f"Claim differs from public analysis: {claim['id']} {side}")
        audit = root / claim["evidence_run"] / "audit.txt"
        if "PASS: evidence is complete enough for publication under the stated scope" not in audit.read_text():
            raise ValueError(f"Measurement audit did not pass: {claim['id']}")
    return selected


def released_version(root=ROOT):
    """The newest release the changelog records: (version, date).

    llms.txt said "Current release: Krate 0.5.4" for weeks after 0.6.0
    shipped, because the release lived in a hand-edited facts field. Every
    release already writes `## [x.y.z] - YYYY-MM-DD` into CHANGELOG.md, so
    that line is the one source; Unreleased and pre-release headings do not
    match it.
    """
    text = (root / "CHANGELOG.md").read_text()
    match = re.search(r"^## \[(\d+\.\d+\.\d+)\] - (\d{4}-\d{2}-\d{2})\s*$", text, re.M)
    if not match:
        raise ValueError("CHANGELOG.md records no release as '## [x.y.z] - YYYY-MM-DD'")
    return match[1], match[2]


def source_url(path):
    return f"{load_facts()['repository']}/blob/main/{path}"


def published_release():
    """Only a named publication input or the actual latest release API."""
    override = os.environ.get("KRATE_PUBLIC_RELEASE")
    if override:
        if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:[-.][A-Za-z0-9.-]+)?", override):
            raise ValueError("KRATE_PUBLIC_RELEASE must be a release tag")
        return override
    try:
        result = subprocess.run(
            ["gh", "api", "repos/incyashraj/krate/releases/latest", "--jq", ".tag_name"],
            cwd=ROOT, capture_output=True, text=True, check=True, timeout=15)
        tag = result.stdout.strip()
        return tag if re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:[-.][A-Za-z0-9.-]+)?", tag) else None
    except (OSError, subprocess.SubprocessError):
        return None


def bundle_inventory(root=ROOT):
    """Retain each artifact path; two bundles with the same name may differ."""
    rows = []
    for shelf in ("ported", "store"):
        for path in sorted((root / "evidence" / shelf).glob("*.krate")):
            with zipfile.ZipFile(path) as bundle:
                code_bytes = bundle.getinfo("code.wasm").file_size
                carries_source = any(n.startswith("source/") for n in bundle.namelist())
            rows.append({"name": path.stem, "path": path.relative_to(root).as_posix(),
                         "bundle_bytes": path.stat().st_size, "code_bytes": code_bytes,
                         "source": carries_source})
    return sorted(rows, key=lambda row: (-row["bundle_bytes"], row["path"]))


def render_llms():
    facts = load_facts()
    claims = benchmark_claims()
    runtime_size = re.search(r"installed once at ([\d.]+ MiB)", claims[0]["caveat"])
    if not runtime_size:
        raise ValueError("The payload comparison must account for its shared runtime")
    # The release comes from the changelog, not the network: this file is
    # checked in CI, where the releases API may not answer.
    version, released = released_version()
    lines = ["# Krate", "", "> " + facts["summary"], "",
             facts["description"], "", "## Facts", "",
             f"- Current release: Krate {version} ({released}).",
             "- Runtimes: macOS (Apple silicon, Intel), Windows (x64, ARM64), Linux (x86_64, arm64).",
             "- What the recipient installs: " + facts["prerequisite"],
             "- Sandbox: " + facts["security"],
             "- Licences: the Krate runtime, format and CLI are free and open source (MIT OR Apache-2.0). Studio has separate licensing (Business Source License 1.1); see the repository license details.",
             "- Studio: " + facts["studio"],
             "- Make an app in Claude: " + facts["claude_connector"],
             "- When to choose Krate: " + facts["when_to_choose"],
             "- Bundles: " + facts["bundle_accounting"],
             "- Facts page: https://krate.tech/facts/",
             "", "## How Krate compares", ""]
    # Every measured number in the comparison is a sealed claim's number:
    # prose that drifts from the measurement fails the build, not a reader.
    sealed = " ".join(f"{c['us']} {c['them']}" for c in claims) + f" {runtime_size[1]}"
    for line in facts["compared"]:
        for number in re.findall(r"\d[\d,]*(?:\.\d+)?\s(?:KiB|MiB|ms)\b", line):
            if number not in sealed:
                raise ValueError(f"'{number}' in the comparison is not a sealed measurement")
    lines += ["- " + line for line in facts["compared"]]
    lines += ["", "## Not to be confused with", "", facts["disambiguation"],
             "", "## Scoped benchmark", "",
             "The recorded 2026-08-25 notes comparison.",
             claims[0]["scope"] + ".", ""]
    for claim in claims:
        label = "Historical app bundle versus installed application" if claim["id"] == "app-file-size" else claim["metric"]
        lines.append(f"- {label}: Krate {claim['us']}; MarkText {claim['them']}.")
    lines += ["", f"The historical Krate runtime was installed separately at {runtime_size[1]}. The {claims[0]['us']} app bundle is not the total first-install cost. Warm open is the median of ten runs per app; memory includes the whole process tree.",
              "", "Source-bearing bundles can also include SDK interfaces and assets; the historical analysis does not establish the current full-bundle install ratio.",
              "", f"Analysis: {source_url(claims[0]['source'])}",
              "The public benchmark kit contains the protocol, analysis and seal.",
              "", "## Main pages", "",
              "- [Website](https://krate.tech/)",
              "- [Why Krate, and when to choose it](https://krate.tech/why-krate.html)",
              "- [Developer quickstart](https://krate.tech/docs/quickstart.html)",
              "- [Porting guide](https://krate.tech/docs/porting.html)",
              "- [Current limits](https://krate.tech/docs/limits.html)",
              "- [Studio](https://krate.tech/studio/)",
              "- [Make an app with Claude: the Krate connector](https://krate.tech/docs/pages/make-an-app-with-claude.html)",
              "- [Organisations: private apps for a team](https://krate.tech/docs/organisations.html)",
              "- [Security model](https://krate.tech/security/)",
              "- [Measurements](https://krate.tech/reports/)",
              "- [Repository inventory](https://krate.tech/progress/)",
              "- [FAQ](https://krate.tech/faq/)",
              "- [Krate facts](https://krate.tech/facts/)",
              "- [All answers about making and sending apps](https://krate.tech/answers/)",
              "- [Download Krate](https://krate.tech/download/)",
              "- [How to distribute a desktop app: ship one file](https://krate.tech/how-to-distribute-a-desktop-app.html)",
              "- [Krate vs Electron](https://krate.tech/krate-vs-electron.html)",
              "- [Krate vs Tauri](https://krate.tech/krate-vs-tauri.html)",
              "- [Electron alternatives in 2026](https://krate.tech/electron-alternatives.html)",
              "- [Tauri alternatives in 2026](https://krate.tech/tauri-alternatives.html)",
              "- [Cross-platform desktop app frameworks in 2026](https://krate.tech/cross-platform-desktop-app-frameworks.html)",
              "- [Desktop app or web app? One file that is both](https://krate.tech/desktop-app-or-web-app.html)",
              "- [Code signing and Krate](https://krate.tech/desktop-app-code-signing.html)",
              "- [Shipping a desktop app: questions and answers](https://krate.tech/desktop-app-shipping-faq.html)",
              "- [The .krate format: one app file for every screen](https://krate.tech/portable-desktop-app-format.html)",
              "- [Share an app made with AI as one link](https://krate.tech/share-an-app-made-with-ai.html)",
              "- [Run an AI-built app safely](https://krate.tech/run-ai-generated-code-safely.html)",
              "- [Can Claude make an app that runs on my phone and computer?](https://krate.tech/claude-app-phone-and-computer.html)",
              "- [How do I share an app Claude made?](https://krate.tech/share-an-app-claude-made.html)",
              "- [Krate vs Electron vs Tauri: what do you ship?](https://krate.tech/desktop-app-distribution.html)",
              "- [Full text of these pages](https://krate.tech/llms-full.txt)",
              f"- [Latest published release]({facts['latest_release_url']})",
              f"- [Source]({facts['repository']})", ""]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "docs/landing/llms.txt")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    expected = render_llms()
    if args.check:
        if args.output.read_text() != expected:
            parser.exit(1, f"stale public facts: {args.output}\n")
        print(f"PASS: {args.output} matches shared public facts")
    else:
        args.output.write_text(expected)
        print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
