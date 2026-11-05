#!/usr/bin/env python3
"""Hold Krate's reference apps to the check every author is held to (K-1216).

    scripts/check-reference-apps.py              # check-app every reference app
    scripts/check-reference-apps.py --list       # the apps it would check
    scripts/check-reference-apps.py --self-test  # prove the reading and judging

The authoring pack points authors -- people and AI -- at a short list of apps
as the shapes to start from (`curated_shape` in
crates/cli/src/authoring_context.rs). Whatever those apps do, every app made
from them does. On 2026-11-05 nine of them failed `krate check-app`: four
desktop layouts shrunk onto a phone, text at 7 points, text that ignored a
person who asked for it larger, a screen reader told nothing. Each was copied
into every generated app that learned from it, and nothing ran the check on
the examples themselves.

The list is read from curated_shape, not kept here, so an app added to the
pack is checked the day it is added. The krate binary is
target/release/krate unless KRATE_BIN names another.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACK = ROOT / "crates" / "cli" / "src" / "authoring_context.rs"


def reference_apps(source: str) -> list[str]:
    """The app directories curated_shape names, in its order, once each."""
    start = source.find("fn curated_shape(")
    if start < 0:
        raise SystemExit("error: curated_shape not found in authoring_context.rs")
    end = source.find("\n}\n", start)
    body = source[start:end if end > 0 else len(source)]
    seen: list[str] = []
    for name in re.findall(r'"(krate-[a-z0-9-]+)"', body):
        if name not in seen:
            seen.append(name)
    return seen


def passed(output: str) -> bool:
    """check-app's verdict: its first line is OK, or FAILED at <stage>."""
    first = output.lstrip().splitlines()[0] if output.strip() else ""
    return first.strip() == "OK"


def self_test() -> None:
    sample = '''
fn other() { "krate-not-this" }
fn curated_shape(name: &str) -> Option<&'static str> {
    Some(match name {
        "krate-notes" => "GUI list",
        "krate-notes-clip" | "krate-clip" => "clipboard",
        "krate-gram" => {
            "GUI photo feed"
        }
        "krate-notes" => "again",
        _ => return None,
    })
}
fn after() { "krate-after" }
'''
    got = reference_apps(sample)
    want = ["krate-notes", "krate-notes-clip", "krate-clip", "krate-gram"]
    assert got == want, f"read {got}, wanted {want}"
    real = reference_apps(PACK.read_text())
    assert "krate-notes" in real and len(real) >= 10, f"the real pack reads as {real}"
    assert passed("OK\n  layout passed\n")
    assert not passed("FAILED at usability\n\nthe app is laid out as a fixed 880x560 layout")
    assert not passed("")
    assert not passed("error: no such app\nOK")
    print("check-reference-apps: self-test passed")


def main() -> int:
    if "--self-test" in sys.argv:
        self_test()
        return 0
    apps = reference_apps(PACK.read_text())
    if "--list" in sys.argv:
        print("\n".join(apps))
        return 0
    krate = os.environ.get("KRATE_BIN", str(ROOT / "target" / "release" / "krate"))
    failed: list[tuple[str, str]] = []
    checked = 0
    for app in apps:
        folder = ROOT / "apps" / app
        if not folder.is_dir():
            # A name the pack lists with no app behind it is its own fault:
            # an author is pointed at nothing.
            failed.append((app, "the pack names it, and apps/ has no such app"))
            continue
        run = subprocess.run(
            [krate, "check-app", str(folder)],
            capture_output=True,
            text=True,
            timeout=900,
        )
        out = run.stdout + run.stderr
        checked += 1
        if passed(out) and run.returncode == 0:
            print(f"ok      {app}")
        else:
            reason = " ".join(line.strip() for line in out.strip().splitlines()[:3] if line.strip())
            print(f"FAILED  {app}: {reason}")
            failed.append((app, reason))
    print(f"\n{checked} reference apps checked, {len(failed)} failed")
    for app, reason in failed:
        print(f"  {app}: {reason}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
