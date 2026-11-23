#!/usr/bin/env python3
"""Studio's IDE starters, made from Krate's own reference apps.

The IDE door on Studio's Home offers a few starters that show what Krate can
make: a game, a dashboard, a 3D scene, a focus timer. Each is one of the reference
apps under apps/, carried to the person's machine as plain text inside
Studio's interface (studio/ui/starters/starters.json), because the interface
ships over the air to Studios that cannot learn a new command.

How a starter is made on the person's machine, with only what Studio 0.6.1
already answers:
  1. ide_new(name, kind) makes a working project from one of the engine's
     built-in kinds. That gives a Cargo.toml and manifest.toml pointing at
     this machine's SDK, under the name the person typed.
  2. ide_write puts the reference app's source files over it, and a manifest
     made of the project's own [app] table followed by the reference app's
     capabilities (starterManifest in studio/ui/ide.js; manifest() below is
     the same rule).
  3. The workbench opens and builds it.

So a starter may only use what the built-in kind's Cargo.toml gives it: the
SDK's gui world and wit-bindgen-rt. An app that needs another crate is
refused here.

    python3 scripts/build-ide-starters.py            write starters.json
    python3 scripts/build-ide-starters.py --check    fail if it is stale
    python3 scripts/build-ide-starters.py --prove ENGINE
        build every starter exactly as Studio would, with that krate binary
        (the released engine, e.g. /Applications/Krate.app/Contents/Resources/bin/krate),
        run it headless, and write its pictures (light and dark) next to the
        JSON. A starter that does not build does not ship.
    python3 scripts/build-ide-starters.py --self-test
"""

import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "studio/ui/starters"
JSON_OUT = OUT / "starters.json"

# What a person sees on each card, and which reference app it is made from.
# Each must pass check-app on the RELEASED engine (--prove): krate-board,
# krate-streak and krate-journal build but fail 0.6.1's layout check (a fixed
# layout that shrinks on a phone), so a new project would open red.
# `kind` is the built-in engine starter the project is first made from; every
# one here is a windowed app, so it rides on the checklist (the gui world).
STARTERS = [
    {"id": "game", "app": "krate-bounce", "kind": "checklist", "name": "My game",
     "title": "A game", "blurb": "Bricks, ball, paddle"},
    {"id": "dashboard", "app": "krate-pulse", "kind": "checklist", "name": "My dashboard",
     "title": "A dashboard", "blurb": "A month's spending"},
    {"id": "scene", "app": "krate-cubes", "kind": "checklist", "name": "My scene",
     "title": "A 3D scene", "blurb": "Lit, spinning cubes"},
    {"id": "timer", "app": "krate-focus", "kind": "checklist", "name": "My timer",
     "title": "A focus timer", "blurb": "A ring that counts down"},
]

# What the built-in kind's Cargo.toml already depends on.
ALLOWED_DEPS = {"krate", "wit-bindgen-rt"}


def deps(cargo: str) -> set[str]:
    """Dependency names in a Cargo.toml's [dependencies] table."""
    out, inside = set(), False
    for line in cargo.splitlines():
        s = line.strip()
        if s.startswith("["):
            inside = s == "[dependencies]"
            continue
        if inside and s and not s.startswith("#") and "=" in s:
            out.add(s.split("=", 1)[0].strip().strip('"'))
    return out


def capabilities(manifest: str) -> str:
    """Everything from the first [[capabilities]] on: what the app asks for."""
    at = manifest.find("[[capabilities]]")
    if at < 0:
        raise ValueError("the manifest asks for nothing")
    return manifest[at:]


def manifest(base: str, caps: str) -> str:
    """The project's own manifest up to its capabilities, then the starter's.

    The [app] table keeps the id, entry and name ide_new wrote for this
    project; the capabilities are the reference app's. Same rule as
    starterManifest in studio/ui/ide.js.
    """
    at = base.find("[[capabilities]]")
    head = base if at < 0 else base[:at]
    return head.rstrip() + "\n\n" + caps


def starter(spec: dict) -> dict:
    app = ROOT / "apps" / spec["app"]
    extra = deps((app / "Cargo.toml").read_text()) - ALLOWED_DEPS
    if extra:
        raise SystemExit(f"{spec['app']} needs {', '.join(sorted(extra))}, which a new project does not have")
    files = {}
    for f in sorted((app / "src").rglob("*.rs")):
        if f.name == "bindings.rs":  # every build writes its own
            continue
        files[f.relative_to(app).as_posix()] = f.read_text()
    if "src/lib.rs" not in files:
        raise SystemExit(f"{spec['app']} has no src/lib.rs")
    return {**spec, "caps": capabilities((app / "manifest.toml").read_text()), "files": files}


def build() -> str:
    return json.dumps({"generated_by": "scripts/build-ide-starters.py", "starters": [starter(s) for s in STARTERS]},
                      indent=1, ensure_ascii=False) + "\n"


def prove(engine: str, only: list[str]) -> int:
    """Build each starter on `engine` exactly as Studio would, and shoot it."""
    data = json.loads(JSON_OUT.read_text())
    bad = 0
    with tempfile.TemporaryDirectory(prefix="ide-starters-") as tmp:
        tmp = pathlib.Path(tmp)
        bases = {}
        for s in data["starters"]:
            if only and s["id"] not in only:
                continue
            kind = s["kind"]
            if kind not in bases:
                # What ide_new runs (studio/src/ide.rs new_project).
                work = tmp / f"base-{kind}"
                ran = subprocess.run([engine, "create", "--kind", kind, "--name", f"base-{kind}", "--work-dir", str(work),
                                      "--output", str(tmp / f"base-{kind}.krate"), "--no-install", "--", "a checklist that saves locally"],
                                     capture_output=True, text=True)
                if ran.returncode != 0:
                    print(f"FAIL the {kind} base did not build:\n{ran.stdout[-2000:]}{ran.stderr[-2000:]}")
                    return 1
                bases[kind] = next(p for p in work.iterdir() if (p / "manifest.toml").exists())
            proj = tmp / s["id"]
            shutil.copytree(bases[kind], proj, ignore=shutil.ignore_patterns("target"))
            # What the interface writes with ide_write.
            (proj / "manifest.toml").write_text(manifest((proj / "manifest.toml").read_text(), s["caps"]))
            for rel, text in s["files"].items():
                (proj / rel).parent.mkdir(parents=True, exist_ok=True)
                (proj / rel).write_text(text)
            # What the workbench runs when it opens (ide_build -> check-app).
            ran = subprocess.run([engine, "check-app", str(proj), "--json", "--shoot", str(tmp / f"{s['id']}-check.png")],
                                 capture_output=True, text=True)
            verdict = next((json.loads(l) for l in reversed(ran.stdout.splitlines()) if l.startswith("{") and '"ok"' in l), {})
            if not verdict.get("ok"):
                print(f"FAIL {s['id']} ({s['app']}): {json.dumps(verdict)[:600] if verdict else ran.stderr[-800:]}")
                bad += 1
                continue
            print(f"ok   {s['id']} ({s['app']}) built and ran: {', '.join(verdict.get('stages', []))}")
            # Its pictures in the person's light and dark: the built component
            # run headless with the look set (check-app picks its own look).
            # An app that draws the same in both gets the desktop window
            # check-app painted, for both.
            entry = re.search(r'^entry\s*=\s*"([^"]+)"', (proj / "manifest.toml").read_text(), re.M).group(1)
            shots = {}
            for look in ("light", "dark"):
                shot = tmp / f"{s['id']}-{look}.png"
                env = {**os.environ, "KRATE_APPEARANCE": look}
                ran = subprocess.run([engine, "run", "--for-screenshot", "--sandbox-root", str(tmp), "--manifest", str(proj / "manifest.toml"),
                                      "--shoot", str(shot), str(proj / entry)], capture_output=True, text=True, env=env)
                if not shot.exists():
                    print(f"FAIL {s['id']} drew no {look} picture: {ran.stderr[-400:]}")
                    bad += 1
                    break
                shots[look] = shot
            if len(shots) != 2:
                continue
            if shots["light"].read_bytes() == shots["dark"].read_bytes():
                desk = tmp / f"{s['id']}-check-desktop.png"
                if desk.exists():
                    shots = {"light": desk, "dark": desk}
            for look, shot in shots.items():
                dest = OUT / f"{s['id']}-{look}.jpg"
                subprocess.run(["sips", "-s", "format", "jpeg", "-s", "formatOptions", "82", "-Z", "720", str(shot), "--out", str(dest)],
                               capture_output=True, check=True)
                print(f"     {dest.relative_to(ROOT)}{' (follows the look)' if shots['light'] != shots['dark'] else ''}")
    return 1 if bad else 0


def self_test() -> int:
    base = '[app]\nid = "dev.krate.x"\nname = "My game"\nentry = "target/x.wasm"\n\n[[capabilities]]\ncap = "store.kv"\n'
    caps = '[[capabilities]]\ncap = "ui.window:create"\n'
    got = manifest(base, caps)
    assert got.startswith('[app]\nid = "dev.krate.x"\nname = "My game"'), got
    assert got.count("[[capabilities]]") == 1 and "store.kv" not in got, got
    assert deps('[dependencies]\nkrate = { path = "x" }\nlibm = "0.2"\n[lib]\nx = 1\n') == {"krate", "libm"}
    try:
        capabilities("[app]\n")
    except ValueError:
        pass
    else:
        raise AssertionError("a manifest with no capabilities was accepted")
    data = json.loads(build())
    assert [s["id"] for s in data["starters"]] == [s["id"] for s in STARTERS]
    assert all("src/lib.rs" in s["files"] and s["caps"].startswith("[[capabilities]]") for s in data["starters"])
    print("self-test ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--prove", metavar="ENGINE")
    ap.add_argument("--only", nargs="*", default=[])
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    text = build()
    if a.check:
        if not JSON_OUT.exists() or JSON_OUT.read_text() != text:
            print("studio/ui/starters/starters.json is stale: run python3 scripts/build-ide-starters.py")
            return 1
        print("starters.json is current")
        return 0
    if a.prove:
        return prove(a.prove, a.only)
    OUT.mkdir(parents=True, exist_ok=True)
    JSON_OUT.write_text(text)
    print(f"wrote {JSON_OUT.relative_to(ROOT)} ({len(text) // 1024} KB, {len(STARTERS)} starters)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
