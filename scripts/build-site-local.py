#!/usr/bin/env python3
"""Build krate.tech on this machine exactly the way the deploy does, without deploying.

The deploy (.github/workflows/pages.yml) assembles the site in three `run:`
blocks: "Build docs", "Assemble site" and "Finalize and verify crawl
metadata". This script reads those blocks out of the workflow itself, so the
local site cannot drift from the published one, and runs them in a scratch
copy of the repository: several steps regenerate tracked files (the progress
and reports pages), and a local check must not dirty the working tree.

    python3 scripts/build-site-local.py                 build into /tmp/krate-site
    python3 scripts/build-site-local.py --out DIR       build into DIR
    python3 scripts/build-site-local.py --serve 4174    build, then serve it on 127.0.0.1:4174

KRATE_BIN=/path/to/krate skips the release build of the CLI that the deploy
does for the store shelf; without it the CLI is built in this checkout's
target/ (minutes, and several GB the first time).

Nothing here deploys, pushes or calls a live service. The web Studio served
from the result still talks to the live hub and build service unless the
page is pointed elsewhere; sign in with your own account only.
"""

import argparse
import os
import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/pages.yml"
STEPS = ["Build docs", "Assemble site", "Finalize and verify crawl metadata"]
# What the assemble step reads. Everything else in the repository is left out
# of the scratch copy.
COPY = ["docs", "scripts", "studio/ui", "evidence", ".github", "crates", "wit", "cloud",
        "apps/krate-checklist", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
        "CHANNEL", "README.md", "CHANGELOG.md", "CONTRIBUTING.md", "STATUS.md"]


def run_block(name: str) -> str:
    """The text of one `run: |` block of the deploy's build job."""
    text = WORKFLOW.read_text()
    one = re.search(r"- name: " + re.escape(name) + r"[^\n]*\n\s+run: (?!\|)([^\n]+)\n", text)
    if one:
        return one.group(1).strip() + "\n"
    m = re.search(r"- name: " + re.escape(name) + r"[^\n]*\n(?:\s+\w[^\n]*\n)*?\s+run: \|\n", text)
    if not m:
        raise SystemExit(f"build-site-local: step {name!r} not found in {WORKFLOW}")
    start = m.end()
    lines = text[start:].split("\n")
    indent = len(lines[0]) - len(lines[0].lstrip())
    body = []
    for line in lines:
        if line.strip() and (len(line) - len(line.lstrip())) < indent:
            break
        body.append(line[indent:] if len(line) >= indent else line.strip())
    return "\n".join(body).rstrip() + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="/tmp/krate-site")
    ap.add_argument("--serve", type=int, default=0)
    ap.add_argument("--work", default="/tmp/krate-site-build")
    args = ap.parse_args()

    work = pathlib.Path(args.work)
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    for rel in COPY:
        src = ROOT / rel
        if not src.exists():
            continue
        dst = work / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        if src.is_dir():
            shutil.copytree(src, dst, symlinks=True,
                            ignore=shutil.ignore_patterns("target", "node_modules", "*.bak"))
        else:
            shutil.copy2(src, dst)

    script = "set -e\n"
    for name in STEPS:
        block = run_block(name)
        if name == "Assemble site" and os.environ.get("KRATE_BIN"):
            block = block.replace("cargo build --release -p krate-cli\n",
                                  "echo 'using KRATE_BIN for the store shelf'\n")
            block = block.replace("KRATE_BIN=target/release/krate", f"KRATE_BIN={os.environ['KRATE_BIN']}")
        if name == "Assemble site" and not os.environ.get("KRATE_BIN"):
            # Build in this checkout's target/, not the scratch copy, so the
            # cache survives between local builds.
            block = block.replace("cargo build --release -p krate-cli\n",
                                  f"cargo build --release -p krate-cli --manifest-path {ROOT}/Cargo.toml\n")
            block = block.replace("KRATE_BIN=target/release/krate", f"KRATE_BIN={ROOT}/target/release/krate")
        script += f"echo '== {name}'\n" + block
    (work / "build.sh").write_text(script)
    env = dict(os.environ)
    env.setdefault("KRATE_PUBLIC_RELEASE", "")
    # The progress and reports pages print the commit they were built from;
    # point git at this checkout so they name the real one.
    env["GIT_DIR"] = subprocess.run(["git", "rev-parse", "--absolute-git-dir"], cwd=ROOT,
                                    capture_output=True, text=True, check=True).stdout.strip()
    r = subprocess.run(["bash", "build.sh"], cwd=work, env=env)
    if r.returncode != 0:
        print("build-site-local: the site build failed; the output above says where", file=sys.stderr)
        return r.returncode

    out = pathlib.Path(args.out)
    shutil.rmtree(out, ignore_errors=True)
    shutil.copytree(work / "_site", out)
    print(f"site built into {out}")
    if args.serve:
        print(f"serving on http://127.0.0.1:{args.serve}/  (Ctrl-C to stop)")
        subprocess.run([sys.executable, "-m", "http.server", str(args.serve), "--bind", "127.0.0.1",
                        "--directory", str(out)])
    return 0


if __name__ == "__main__":
    sys.exit(main())
