#!/usr/bin/env python3
"""Every starter an organisation is offered is the app in apps/, as it is now.

docs/landing/orgs/starters.json lists the starters; each one's .krate under
docs/landing/orgs/starters/ carries the source it was built from. A fix made
in apps/krate-<id>/ and never rebuilt into that file ships the old app to
every organisation that picks it, with nothing to say so. This compares the
source each file carries with apps/: src/lib.rs, manifest.toml and
krate-check.toml byte for byte. Cargo.toml is left out: the bundle's points
at the SDK it carries ({KRATE_SDK}), not at this checkout.

    python3 scripts/check-starters-fresh.py              check the shipped starters
    python3 scripts/check-starters-fresh.py --self-test  prove the check bites
"""
import json
import pathlib
import sys
import tempfile
import zipfile

COMPARED = ("src/lib.rs", "manifest.toml", "krate-check.toml")


def check(root):
    """Problems, as lines; empty when every starter matches its source."""
    listing = root / "docs/landing/orgs/starters.json"
    problems = []
    starters = json.loads(listing.read_text())["starters"]
    if not starters:
        problems.append(f"{listing}: no starters listed")
    for s in starters:
        if s.get("placeholder"):
            continue
        bundle = root / "docs/landing" / s["bundle"].lstrip("/")
        app = root / "apps" / ("krate-" + s["id"])
        if not bundle.exists():
            problems.append(f"{s['id']}: {bundle.relative_to(root)} is listed but missing")
            continue
        if not app.is_dir():
            problems.append(f"{s['id']}: no source at {app.relative_to(root)}")
            continue
        with zipfile.ZipFile(bundle) as z:
            names = set(z.namelist())
            for rel in COMPARED:
                here = app / rel
                inside = "source/" + rel
                if inside not in names and not here.exists():
                    continue
                if inside not in names:
                    problems.append(f"{s['id']}: the shipped file has no {rel}; rebuild it")
                elif not here.exists():
                    problems.append(f"{s['id']}: the shipped file has {rel} and apps/ does not")
                elif z.read(inside) != here.read_bytes():
                    problems.append(
                        f"{s['id']}: {rel} in {bundle.relative_to(root)} is not apps/krate-{s['id']}/{rel}: "
                        "rebuild and repack the starter"
                    )
    return problems


def self_test():
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        (root / "docs/landing/orgs/starters").mkdir(parents=True)
        (root / "apps/krate-notes/src").mkdir(parents=True)
        (root / "docs/landing/orgs/starters.json").write_text(
            json.dumps({"starters": [{"id": "notes", "bundle": "/orgs/starters/notes.krate"}]})
        )
        files = {"src/lib.rs": b"fn app() {}\n", "manifest.toml": b"[app]\n"}
        for rel, body in files.items():
            (root / "apps/krate-notes" / rel).write_bytes(body)

        def pack(entries):
            with zipfile.ZipFile(root / "docs/landing/orgs/starters/notes.krate", "w") as z:
                for rel, body in entries.items():
                    z.writestr("source/" + rel, body)
                z.writestr("source/Cargo.toml", "path = \"{KRATE_SDK}\"\n")

        pack(files)
        assert not check(root), check(root)
        # The app fixed in apps/ and never repacked: named, with the file.
        (root / "apps/krate-notes/src/lib.rs").write_bytes(b"fn app() { fixed(); }\n")
        problems = check(root)
        assert len(problems) == 1 and "notes" in problems[0] and "src/lib.rs" in problems[0], problems
        # A file the source gained that the bundle never carried.
        pack(dict(files, **{"src/lib.rs": b"fn app() { fixed(); }\n"}))
        assert not check(root)
        (root / "apps/krate-notes/krate-check.toml").write_text("[[task]]\n")
        problems = check(root)
        assert len(problems) == 1 and "krate-check.toml" in problems[0], problems
        # A listed starter whose file is gone.
        (root / "apps/krate-notes/krate-check.toml").unlink()
        (root / "docs/landing/orgs/starters/notes.krate").unlink()
        problems = check(root)
        assert len(problems) == 1 and "missing" in problems[0], problems
    print("self-test passed")


def main():
    if "--self-test" in sys.argv:
        self_test()
        return 0
    root = pathlib.Path(__file__).resolve().parent.parent
    problems = check(root)
    for p in problems:
        print(p)
    if problems:
        return 1
    count = len(json.loads((root / "docs/landing/orgs/starters.json").read_text())["starters"])
    print(f"{count} starters match their source in apps/")
    return 0


if __name__ == "__main__":
    sys.exit(main())
