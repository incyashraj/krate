#!/usr/bin/env python3
"""Every place that names the version must name the same one.

A version bump is not one edit. The number is pinned in the workspace
manifest, the Studio manifest, the builder image, both workspace lockfiles,
and -- the one that is easy to miss -- all 64 app lockfiles under apps/,
because every app pins the SDK by version and CI builds them with --locked.

v0.5.1 found that out the slow way. The bump landed, and then three
separate CI rounds each surfaced one more thing the version had moved:

    Library tests      the synthetic pack digest, because closure.json
                       records "krate-bundle <version>" inside the bundle
    Dependency audit   docs/licence-map.md, generated from the graph
    Fixture build      the lockfile in apps/krate-clock is not current,
                       so a --locked build would fail

Each round cost a full CI cycle to learn one fact. This learns all of them
in about a second, before the push.

It checks agreement, not a particular number: whatever the workspace
manifest says is the version, everything else must say it too. That way it
keeps working for the next bump without being edited, which is the failure
mode of every checker that hardcodes a value.

Run with --self-test to check the checker, including against a tree rigged
to fail.
"""

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

VERSION_LINE = re.compile(r'^version = "([^"]+)"', re.M)
LOCK_KRATE = re.compile(r'^name = "krate"\nversion = "([^"]+)"', re.M)


def workspace_version(root: pathlib.Path) -> str:
    """The number everything else has to match."""
    text = (root / "Cargo.toml").read_text()
    hit = VERSION_LINE.search(text)
    if not hit:
        raise SystemExit("Cargo.toml has no version line -- this checker cannot work")
    return hit.group(1)


def pins(root: pathlib.Path, want: str) -> list[str]:
    """Every disagreement, named with the file that holds it."""
    problems: list[str] = []

    # The other manifests that carry the number by hand.
    studio = root / "studio/Cargo.toml"
    if studio.exists():
        hit = VERSION_LINE.search(studio.read_text())
        if hit and hit.group(1) != want:
            problems.append(f"studio/Cargo.toml says {hit.group(1)}, workspace says {want}")

    # The Tauri bundle's own version, which is what a SHIPPED Studio
    # reports about itself and what its update check compares against.
    #
    # It sat at 0.4.0 while the crate said 0.5.1, so a bundle built from
    # current source told the founder "Version 0.5.1 is ready / You have
    # 0.4.0" and offered him an update he already had. The release
    # workflow patches this file before bundling, so shipped builds were
    # right and only local ones lied -- which is worse, because local is
    # where it gets tested (K-846).
    tauri = root / "studio/tauri.conf.json"
    if tauri.exists():
        hit = re.search(r'"version"\s*:\s*"([^"]+)"', tauri.read_text())
        if hit and hit.group(1) != want:
            problems.append(
                f"studio/tauri.conf.json says {hit.group(1)}, workspace says {want} "
                f"-- a bundle built from here reports the wrong version to its own "
                f"update check"
            )

    # The builder image pins the release it downloads. A stale pin ships an
    # image that names a version predating the work in it -- which is K-715,
    # already paid for once.
    dockerfile = root / "cloud/builder/Dockerfile"
    if dockerfile.exists():
        hit = re.search(r"^ARG KRATE_VERSION=(.+)$", dockerfile.read_text(), re.M)
        if hit and hit.group(1).strip() != want:
            problems.append(
                f"cloud/builder/Dockerfile pins KRATE_VERSION={hit.group(1).strip()}, "
                f"workspace says {want}"
            )

    # Both workspace locks. v0.5.0's bump left studio/Cargo.lock behind,
    # which is why this one is checked by name rather than assumed.
    for lock, crate in (("Cargo.lock", "krate-cli"), ("studio/Cargo.lock", "krate-studio")):
        path = root / lock
        if not path.exists():
            continue
        hit = re.search(rf'^name = "{crate}"\nversion = "([^"]+)"', path.read_text(), re.M)
        if hit and hit.group(1) != want:
            problems.append(f"{lock} has {crate} at {hit.group(1)}, workspace says {want}")

    # Every app's own lock. This is the one that cost a CI round: 64 files,
    # each pinning the SDK, each built with --locked.
    stale = []
    for lock in sorted((root / "apps").glob("*/Cargo.lock")):
        hit = LOCK_KRATE.search(lock.read_text())
        if hit and hit.group(1) != want:
            stale.append(f"{lock.relative_to(root)} ({hit.group(1)})")
    if stale:
        shown = ", ".join(stale[:4])
        more = f" and {len(stale) - 4} more" if len(stale) > 4 else ""
        problems.append(
            f"{len(stale)} app lockfile(s) pin the SDK at an older version, so a "
            f"--locked build fails: {shown}{more}\n"
            f"    fix: for d in apps/*/; do (cd \"$d\" && cargo update -w --offline); done"
        )

    return problems


def self_test() -> int:
    """Exercise the real functions, on the real tree and on rigged ones."""
    want = workspace_version(ROOT)
    if not re.match(r"^\d+\.\d+\.\d+", want):
        print(f"self-test: workspace version {want!r} does not look like a version")
        return 1

    real = pins(ROOT, want)
    if real:
        print("self-test: the real tree disagrees, which is the bug this checks for:")
        for p in real:
            print(f"  - {p}")
        return 1

    # A tree where one app lock is behind must be caught. This is the exact
    # shape that failed CI, kept as a negative fixture.
    import tempfile, shutil
    with tempfile.TemporaryDirectory() as tmp:
        fake = pathlib.Path(tmp)
        (fake / "apps/one").mkdir(parents=True)
        (fake / "Cargo.toml").write_text('[package]\nversion = "9.9.9"\n')
        (fake / "apps/one/Cargo.lock").write_text('[[package]]\nname = "krate"\nversion = "9.9.8"\n')
        caught = pins(fake, "9.9.9")
        if not any("app lockfile" in p for p in caught):
            print("self-test: a stale app lock was NOT caught")
            return 1
        # And a tree where everything agrees must be silent, or the checker
        # is just noise that people learn to ignore.
        (fake / "apps/one/Cargo.lock").write_text('[[package]]\nname = "krate"\nversion = "9.9.9"\n')
        if pins(fake, "9.9.9"):
            print("self-test: an agreeing tree was reported as a problem")
            return 1

    print(f"self-test: ok -- {want} agrees everywhere, a stale app lock is caught, "
          "and an agreeing tree is silent")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="check the checker")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    want = workspace_version(ROOT)
    problems = pins(ROOT, want)
    if problems:
        print(f"The workspace is {want}, and these disagree:")
        for p in problems:
            print(f"  - {p}")
        print(
            "\nA version bump is not one edit. Two derived files also move with "
            "it and are\nnot checked here, because each has its own generator:\n"
            "  crates/bundle/tests/fixtures/repack-digests.json  (closure.json "
            "records the packer)\n"
            "    cargo test -p krate-bundle --lib regenerate_repack_digests -- --ignored\n"
            "  docs/licence-map.md\n"
            "    python3 scripts/licence-map.py"
        )
        return 1
    print(f"ok  every version pin says {want}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
