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
    lines = ["# Krate", "", facts["description"], "", facts["prerequisite"], "",
             "The Krate runtime is free and open source (MIT OR Apache-2.0). Studio has separate licensing; see the repository license details. " + facts["studio"], "", facts["bundle_accounting"],
             "", facts["security"], "", "## Scoped benchmark", "",
             "The following is the recorded 2026-08-25 notes comparison, not a measurement of the latest release or all Krate apps.",
             claims[0]["scope"] + ".", ""]
    for claim in claims:
        label = "Historical app bundle versus installed application" if claim["id"] == "app-file-size" else claim["metric"]
        lines.append(f"- {label}: Krate {claim['us']}; MarkText {claim['them']}.")
    lines += ["", f"The historical Krate runtime was installed separately at {runtime_size[1]}. The {claims[0]['us']} app bundle is not the total first-install cost. Warm open is the median of ten runs per app; memory includes the whole process tree.",
              "", "Energy was not measured. Source-bearing bundles can also include SDK interfaces and assets; the historical analysis does not establish the current full-bundle install ratio.",
              "", f"Analysis: {source_url(claims[0]['source'])}",
              "The public benchmark kit contains the protocol, analysis and seal. Retained raw run data is not all distributed in the public checkout.",
              "", "## Main pages", "",
              "- [Website](https://krate.tech/)",
              "- [Developer quickstart](https://krate.tech/docs/quickstart.html)",
              "- [Porting guide](https://krate.tech/docs/porting.html)",
              "- [Current limits](https://krate.tech/docs/limits.html)",
              "- [Studio](https://krate.tech/studio/)",
              "- [Measurements](https://krate.tech/reports/)",
              "- [Repository inventory](https://krate.tech/progress/)",
              "- [FAQ](https://krate.tech/faq/)",
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
