#!/usr/bin/env python3
"""Build krate.mcpb: Krate as a one-click Claude Desktop extension (IC-1005).

A .mcpb (MCP Bundle) is a zip with a manifest.json and a local MCP server.
Double-clicking it, or dragging it onto Claude Desktop, installs it; Claude
Desktop then starts the server itself. This one carries the `krate` binary
(whose `krate mcp` is the server) and `cargo-component` beside it, for macOS
(one universal binary for Apple silicon and Intel) and Windows x86_64 (which
Windows on Arm runs too), so a person gets Krate and its app builder in one
file: no separate download, no terminal, no config file. Rust itself is not
in the bundle: `krate setup rust` fetches it, pinned and verified, the first
time an app is built (about 126 MB on a Mac).

Inputs are release folders -- an unpacked release archive, each holding
`krate` and `cargo-component` (`.exe` on Windows):

    python3 scripts/build-mcpb.py --release v0.6.1
    python3 scripts/build-mcpb.py \\
        --mac-arm64 target/release --mac-x64 DIR --win-x64 DIR

`--release TAG` downloads the three archives from GitHub (checked against
the release's SHA256SUMS); a folder given for one platform replaces that
platform's download, which is how a local build goes in. Packing and
validation use the official MCPB CLI (@anthropic-ai/mcpb, pinned below),
never a hand-made zip. The manifest's tool list is read from the server
itself, so it cannot drift from what `krate mcp` offers.

Needs macOS for the universal binary (`lipo`), Node's npx, and python3.
Writes dist/krate.mcpb unless --out says otherwise.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MCPB_CLI = "@anthropic-ai/mcpb@2.1.2"
GITHUB_REPO = "incyashraj/krate"
ICON_SOURCE = REPO / "docs" / "landing" / "krate-app-icon.png"

ARCHIVES = {
    "mac-arm64": "krate-{v}-aarch64-apple-darwin.tar.gz",
    "mac-x64": "krate-{v}-x86_64-apple-darwin.tar.gz",
    "win-x64": "krate-{v}-x86_64-pc-windows-msvc.zip",
}


def fail(message: str) -> None:
    print(f"build-mcpb: {message}", file=sys.stderr)
    sys.exit(1)


def workspace_version() -> str:
    for line in (REPO / "Cargo.toml").read_text().splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    fail("no workspace version in Cargo.toml")
    return ""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def download_release(tag: str, wanted: list[str], into: Path) -> dict[str, Path]:
    """The release archives for `wanted` platforms, unpacked, by platform."""
    version = tag.lstrip("v")
    names = {key: ARCHIVES[key].format(v=version) for key in wanted}
    patterns = [arg for name in names.values() for arg in ("-p", name)]
    subprocess.run(
        ["gh", "release", "download", tag, "-R", GITHUB_REPO, "-D", str(into),
         "-p", "SHA256SUMS", *patterns, "--clobber"],
        check=True,
    )
    sums = {}
    for line in (into / "SHA256SUMS").read_text().splitlines():
        parts = line.split()
        if len(parts) == 2:
            sums[parts[1].lstrip("*")] = parts[0]
    found = {}
    for key, name in names.items():
        archive = into / name
        if sums.get(name) != sha256(archive):
            fail(f"{name} does not match the release's SHA256SUMS")
        dest = into / key
        dest.mkdir(exist_ok=True)
        if name.endswith(".zip"):
            with zipfile.ZipFile(archive) as z:
                z.extractall(dest)
        else:
            with tarfile.open(archive) as t:
                t.extractall(dest, filter="data")
        exe = ".exe" if key.startswith("win") else ""
        binaries = list(dest.rglob(f"krate{exe}"))
        if not binaries:
            fail(f"{name} holds no krate{exe}")
        found[key] = binaries[0].parent
    return found


def pick(folder: Path, name: str) -> Path:
    path = folder / name
    if not path.is_file():
        fail(f"{folder} has no {name}")
    return path


def universal(out: Path, arm64: Path, x64: Path) -> None:
    subprocess.run(["lipo", "-create", str(arm64), str(x64), "-output", str(out)], check=True)
    archs = subprocess.run(["lipo", "-archs", str(out)], check=True, capture_output=True, text=True)
    if set(archs.stdout.split()) != {"arm64", "x86_64"}:
        fail(f"{out.name} came out as {archs.stdout.strip()}, not arm64 + x86_64")
    out.chmod(0o755)


def tools_of(server: Path) -> list[dict]:
    """The tools `krate mcp` really offers, asked over stdio."""
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "build-mcpb", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    ]
    with tempfile.TemporaryDirectory() as home:
        run = subprocess.run(
            [str(server), "mcp"],
            input="".join(json.dumps(m) + "\n" for m in messages),
            capture_output=True, text=True, timeout=120,
            env={**os.environ, "HOME": home, "KRATE_MCP_ROOT": home},
        )
    for line in run.stdout.splitlines():
        reply = json.loads(line)
        if reply.get("id") == 2:
            tools = []
            for tool in reply["result"]["tools"]:
                words = " ".join(tool.get("description", "").split())
                first = words.split(". ")[0].rstrip(".") + "."
                tools.append({"name": tool["name"], "description": first})
            return tools
    fail(f"{server} did not answer tools/list:\n{run.stdout}\n{run.stderr}")
    return []


def manifest(version: str, tools: list[dict], platforms: list[str]) -> dict:
    config: dict = {"command": "${__dirname}/server/krate", "args": ["mcp"]}
    if "win32" in platforms:
        config["platform_overrides"] = {
            "win32": {"command": "${__dirname}/server/krate.exe"},
        }
    if "darwin" not in platforms:
        config["command"] = "${__dirname}/server/krate.exe"
        config.pop("platform_overrides", None)
    return {
        "manifest_version": "0.3",
        "name": "krate",
        "display_name": "Krate",
        "version": version,
        "description": "Ask Claude for an app and it makes a real one and opens it on this "
                       "computer, in its own window.",
        "long_description": (
            "Krate lets Claude make real desktop apps -- a habit tracker, a budget, a game -- "
            "and open them right here, each in its own window with its own name and icon. "
            "Every app is one small .krate file you can send to anyone, and it can only do "
            "what its permissions say: the first time it opens, Krate asks you, never Claude.\n\n"
            "Apps are built on this computer. The first build sets up Krate's builder once "
            "(under 150 MB), in Krate's own folder; it never changes any other software."
        ),
        "author": {"name": "Krate", "url": "https://krate.tech"},
        "homepage": "https://krate.tech",
        "support": "https://github.com/incyashraj/krate/issues",
        "repository": {"type": "git", "url": "https://github.com/incyashraj/krate"},
        "icon": "icon.png",
        "server": {
            "type": "binary",
            "entry_point": "server/krate" if "darwin" in platforms else "server/krate.exe",
            "mcp_config": config,
        },
        "tools": tools,
        "keywords": ["apps", "desktop", "make an app", "wasm", "sandbox"],
        "license": "MIT OR Apache-2.0",
        "privacy_policies": ["https://krate.tech/privacy"],
        "compatibility": {"platforms": platforms},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--release", metavar="TAG", help="fetch release archives for TAG")
    for key in ARCHIVES:
        parser.add_argument(f"--{key}", type=Path, metavar="DIR",
                            help=f"folder holding the {key} krate and cargo-component")
    parser.add_argument("--only", choices=["mac", "windows"],
                        help="one platform per bundle instead of both")
    parser.add_argument("--version", help="the bundle's version (default: the workspace's)")
    parser.add_argument("--out", type=Path, default=REPO / "dist" / "krate.mcpb")
    parser.add_argument("--keep", type=Path, help="also keep the unpacked bundle here")
    parser.add_argument("--tools-from", type=Path, metavar="BIN",
                        help="a krate this machine runs, for the manifest's tool list "
                             "(default: the bundle's own Mac binary)")
    parser.add_argument("--single-arch-mac", action="store_true",
                        help="a local test bundle from one Mac folder: that Mac kind only")
    args = parser.parse_args()

    wanted = {"mac": ["mac-arm64", "mac-x64"], "windows": ["win-x64"]}.get(
        args.only, ["mac-arm64", "mac-x64", "win-x64"])
    folders: dict[str, Path] = {k: getattr(args, k.replace("-", "_")) for k in wanted}
    if args.single_arch_mac:
        given = [k for k in ("mac-arm64", "mac-x64") if folders.get(k)]
        if len(given) != 1:
            fail("--single-arch-mac takes exactly one of --mac-arm64 / --mac-x64")
        for key in ("mac-arm64", "mac-x64"):
            if key not in given:
                folders.pop(key, None)
        print(f"build-mcpb: WARNING: {given[0]} only -- the other kind of Mac cannot run "
              "this bundle. Never ship it.", file=sys.stderr)
    work = Path(tempfile.mkdtemp(prefix="krate-mcpb-"))
    try:
        missing = [k for k, v in folders.items() if v is None]
        if missing:
            if not args.release:
                fail(f"no folder for {', '.join(missing)}: pass --{missing[0]} DIR or --release TAG")
            folders.update(download_release(args.release, missing, work / "release"))

        stage = work / "krate"
        server = stage / "server"
        server.mkdir(parents=True)
        platforms = []
        if "mac-arm64" in folders and "mac-x64" in folders:
            for name in ("krate", "cargo-component"):
                universal(server / name, pick(folders["mac-arm64"], name),
                          pick(folders["mac-x64"], name))
            platforms.append("darwin")
        elif "mac-arm64" in folders or "mac-x64" in folders:
            only = folders.get("mac-arm64") or folders["mac-x64"]
            for name in ("krate", "cargo-component"):
                shutil.copy2(pick(only, name), server / name)
                (server / name).chmod(0o755)
            platforms.append("darwin")
        if "win-x64" in folders:
            for name in ("krate.exe", "cargo-component.exe"):
                shutil.copy2(pick(folders["win-x64"], name), server / name)
            platforms.append("win32")

        icon = stage / "icon.png"
        if shutil.which("sips"):
            subprocess.run(["sips", "-Z", "512", str(ICON_SOURCE), "--out", str(icon)],
                           check=True, capture_output=True)
        else:
            shutil.copy2(ICON_SOURCE, icon)
        for license_file in ("LICENSE-MIT", "LICENSE-APACHE"):
            shutil.copy2(REPO / license_file, stage / license_file)

        # The tool list comes from a krate this machine can run: the Mac
        # binary going into the bundle, else --tools-from (a Windows-only
        # bundle built on a Mac), never a bare `krate` on PATH, which is
        # usually an older install.
        host = args.tools_from or server / "krate"
        if not host.is_file():
            fail("no krate this machine can run to read the tool list from: pass --tools-from")
        tools = tools_of(host)
        names = [t["name"] for t in tools]
        for needed in ("krate_check", "krate_package", "krate_open"):
            if needed not in names:
                fail(f"the server offers no {needed}: these binaries predate IC-1005")

        version = args.version or workspace_version()
        (stage / "manifest.json").write_text(
            json.dumps(manifest(version, tools, platforms), indent=2) + "\n")

        npx = ["npx", "--yes", MCPB_CLI]
        subprocess.run([*npx, "validate", str(stage / "manifest.json")], check=True)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        if args.out.exists():
            args.out.unlink()
        subprocess.run([*npx, "pack", str(stage), str(args.out)], check=True)
        subprocess.run([*npx, "info", str(args.out)], check=True)
        if args.keep:
            shutil.rmtree(args.keep, ignore_errors=True)
            shutil.copytree(stage, args.keep, symlinks=True)
        size = args.out.stat().st_size
        print(f"\n{args.out}  {size / 1_000_000:.1f} MB  ({', '.join(platforms)})")
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
