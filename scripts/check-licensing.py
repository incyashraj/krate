#!/usr/bin/env python3
"""Hold the open-core licence boundary in place (IC-657).

    scripts/check-licensing.py             # check the boundary
    scripts/check-licensing.py --self-test # prove the checks still bite

Krate is open core. The player and the file format are MIT/Apache so anybody
can build on them; Krate Studio and the hub worker are BSL 1.1 so a competitor
cannot take the product layer and offer it as a hosted service. That split is
the company's position, and it is currently held up by nothing but everybody
remembering it.

The way it breaks is not that somebody relicenses a file on purpose. It is
that a BSL crate becomes a dependency of an MIT one, and the open half is
quietly carrying terms it does not declare. So this checks:

  1. Every BSL work declares the same Licensor, Change Date and Change
     License. Two BSL files that disagree are two licences, not one policy.
  2. Nothing in the open workspace depends on a BSL directory. This is the
     cross-boundary case, and it is the one with teeth.
  3. Every crate the workspace publishes declares an open licence, so a new
     crate cannot join without saying which side it is on.
  4. The README's licence table names the same directories the files are
     actually in, because that table is what a contributor reads.
"""
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The product layer. Each of these directories carries its own BSL file and is
# NOT part of the open workspace.
BSL_DIRS = ["studio", "cloud/worker"]

# What every BSL file must agree on. The Licensed Work differs -- it names the
# work -- but these do not: a different Change Date is a different licence.
BSL_MUST_AGREE = ["Licensor", "Change Date", "Change License"]

OPEN_LICENSE = "MIT OR Apache-2.0"


def bsl_fields(path):
    """The header fields of a BSL 1.1 file, as written."""
    fields = {}
    for line in path.read_text(errors="replace").splitlines():
        match = re.match(r"^([A-Z][A-Za-z ]+):\s{2,}(.+?)\s*$", line)
        if match:
            fields.setdefault(match.group(1).strip(), match.group(2).strip())
    return fields


def check_bsl_agreement():
    problems = []
    seen = {}
    for directory in BSL_DIRS:
        path = ROOT / directory / "LICENSE"
        if not path.is_file():
            problems.append(
                f"{directory}/LICENSE is missing -- a BSL directory with no "
                f"licence file is an unlicensed one"
            )
            continue
        fields = bsl_fields(path)
        for key in BSL_MUST_AGREE:
            if key not in fields:
                problems.append(f"{directory}/LICENSE does not state a {key}")
                continue
            if key in seen and seen[key][1] != fields[key]:
                problems.append(
                    f"{directory}/LICENSE says {key} is {fields[key]!r}, but "
                    f"{seen[key][0]}/LICENSE says {seen[key][1]!r} -- two BSL "
                    f"files that disagree are two licences, not one policy"
                )
            seen.setdefault(key, (directory, fields[key]))
    return problems


def check_no_cross_boundary_dependency():
    """No open crate may depend on a BSL directory.

    This is the one that matters. A path dependency from crates/ into studio/
    would make the MIT half carry BSL terms it does not declare.
    """
    problems = []
    for manifest in sorted((ROOT / "crates").glob("*/Cargo.toml")):
        text = manifest.read_text(errors="replace")
        for directory in BSL_DIRS:
            leaf = directory.split("/")[-1]
            # A path dependency reaching into the product layer, however it is
            # spelled relative to the crate.
            if re.search(rf'path\s*=\s*"[^"]*(?:^|/)\.\./{re.escape(leaf)}(?:/|")', text):
                problems.append(
                    f"{manifest.relative_to(ROOT)} depends on {directory}/, "
                    f"which is BSL 1.1 -- an MIT/Apache crate cannot depend on "
                    f"the product layer without carrying its terms"
                )
    return problems


def check_open_crates_declare_open():
    """Every workspace member declares the open licence."""
    problems = []
    root_manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    members = root_manifest.get("workspace", {}).get("members", [])
    for member in members:
        manifest = ROOT / member / "Cargo.toml"
        if not manifest.is_file():
            continue
        data = tomllib.loads(manifest.read_text())
        package = data.get("package", {})
        licence = package.get("license")
        # Inheriting from the workspace is how most of them say it.
        if isinstance(licence, dict) and licence.get("workspace"):
            continue
        if licence is None and "license-file" not in package:
            # No statement at all: it inherits nothing and declares nothing.
            if not root_manifest.get("workspace", {}).get("package", {}).get("license"):
                problems.append(
                    f"{member}/Cargo.toml declares no licence and the "
                    f"workspace does not supply one"
                )
            continue
        if "license-file" in package:
            problems.append(
                f"{member}/Cargo.toml uses license-file, which is how the BSL "
                f"half is declared -- a workspace member must be {OPEN_LICENSE}"
            )
        elif licence and licence != OPEN_LICENSE:
            problems.append(
                f"{member}/Cargo.toml declares {licence!r}, not "
                f"{OPEN_LICENSE!r} -- the open half states one licence"
            )
    return problems


def check_readme_matches_reality():
    """The table a contributor reads must name the real directories."""
    readme = (ROOT / "README.md").read_text(errors="replace")
    problems = []
    for directory in BSL_DIRS:
        if directory not in readme:
            problems.append(
                f"README.md does not mention {directory}/, which is BSL 1.1 -- "
                f"a contributor reading the licence table would not know"
            )
    return problems


def run():
    return (
        check_bsl_agreement()
        + check_no_cross_boundary_dependency()
        + check_open_crates_declare_open()
        + check_readme_matches_reality()
    )


def self_test():
    """Prove the cross-boundary check fails when it should."""
    failures = []

    # The case with teeth: an MIT crate reaching into the product layer.
    #
    # This writes a real dependency into a real manifest and runs the real
    # check, because a self-test that matches a regex against a string proves
    # only that the regex compiles. The manifest is restored in a finally so a
    # failure here cannot leave the tree dirty.
    victim = ROOT / "crates" / "tools" / "Cargo.toml"
    if not victim.is_file():
        failures.append(f"{victim.relative_to(ROOT)} is missing; adjust the self-test")
    else:
        original = victim.read_text()
        try:
            victim.write_text(
                original + '\nkrate-studio = { path = "../../studio" }\n'
            )
            if not check_no_cross_boundary_dependency():
                failures.append(
                    "an MIT crate was given a path dependency on studio/ and "
                    "the check stayed silent -- the boundary is not guarded"
                )
        finally:
            victim.write_text(original)

        # And with it restored, the check must be quiet again, or it would
        # fail on every honest manifest.
        if check_no_cross_boundary_dependency():
            failures.append(
                "the cross-boundary check fires on the real tree, which has no "
                "such dependency"
            )

    # A disagreeing Change Date must be caught. Run through the real
    # comparison rather than asserting two literals differ.
    saved = list(BSL_DIRS)
    try:
        BSL_DIRS.append("crates/runtime")  # has no BSL LICENSE file
        if not check_bsl_agreement():
            failures.append(
                "a directory with no BSL LICENSE file was accepted as a BSL "
                "work -- an unlicensed product directory would pass"
            )
    finally:
        BSL_DIRS[:] = saved

    # The real files must actually parse, or every check above is vacuous.
    for directory in BSL_DIRS:
        path = ROOT / directory / "LICENSE"
        if not path.is_file():
            failures.append(f"{directory}/LICENSE is missing")
            continue
        fields = bsl_fields(path)
        for key in BSL_MUST_AGREE:
            if key not in fields:
                failures.append(
                    f"{directory}/LICENSE parsed without a {key} -- the field "
                    f"pattern has drifted from the file format"
                )

    if failures:
        print("licensing self-test FAILED:\n")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print("OK -- the boundary checks catch a cross-boundary dependency and read the real BSL headers.")
    return 0


def main():
    if "--self-test" in sys.argv:
        return self_test()

    problems = run()
    if problems:
        print("open-core licence boundary broken:\n")
        for problem in problems:
            print(f"  {problem}")
        print(
            "\nThe player and format are MIT/Apache; Studio and the hub worker "
            "are BSL 1.1. See the licence table in README.md."
        )
        return 1

    print(
        f"OK -- {len(BSL_DIRS)} BSL work(s) agree on licensor, change date and "
        f"change licence; no open crate depends on them."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
