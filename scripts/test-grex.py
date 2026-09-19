#!/usr/bin/env python3
"""Replay grex's argument/file-input permission boundary using a real runtime.

  python3 scripts/test-grex.py --krate /absolute/path/to/krate
  python3 scripts/test-grex.py --krate /absolute/path/to/krate --repack

--repack applies the reviewed manifest to the existing, unchanged Wasm payload
through `krate pack`, verifies both entries, then runs the same regressions.
It is a metadata repack, not a rebuild of upstream grex source.
"""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "evidence/ported/grex.manifest.toml"
EXPECTED = "^a(?:bc?)?$\n"


def resolve_runtime(path, platform=None):
    """Match Git Bash's implicit .exe lookup without changing Unix lookup."""
    path = Path(path).resolve()
    platform = sys.platform if platform is None else platform
    if platform == "win32" and not path.is_file() and not path.suffix:
        executable = path.with_suffix(".exe")
        if executable.is_file():
            return executable
    return path


class RuntimePathTests(unittest.TestCase):
    def test_explicit_windows_executable(self):
        with tempfile.TemporaryDirectory() as tmp:
            executable = Path(tmp) / "krate.exe"
            executable.touch()
            self.assertEqual(resolve_runtime(executable, "win32"), executable.resolve())

    def test_windows_extensionless_path_finds_executable(self):
        with tempfile.TemporaryDirectory() as tmp:
            executable = Path(tmp) / "krate.exe"
            executable.touch()
            self.assertEqual(resolve_runtime(Path(tmp) / "krate", "win32"), executable.resolve())

    def test_unix_does_not_substitute_windows_executable(self):
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "krate.exe").touch()
            bare = Path(tmp) / "krate"
            for platform in ("darwin", "linux"):
                self.assertEqual(resolve_runtime(bare, platform), bare.resolve())
                self.assertFalse(resolve_runtime(bare, platform).is_file())

    def test_existing_extensionless_file_is_not_replaced(self):
        with tempfile.TemporaryDirectory() as tmp:
            bare = Path(tmp) / "krate"
            bare.touch()
            bare.with_suffix(".exe").touch()
            self.assertEqual(resolve_runtime(bare, "win32"), bare.resolve())

    def test_missing_windows_executable_stays_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            bare = Path(tmp) / "krate"
            self.assertEqual(resolve_runtime(bare, "win32"), bare.resolve())
            self.assertFalse(resolve_runtime(bare, "win32").is_file())

    def test_text_capture_normalizes_windows_newlines(self):
        # The real runtime calls also use text=True, so a Windows CRLF is
        # compared to EXPECTED's LF rather than falsely failing exact output.
        result = subprocess.run([sys.executable, "-c",
                                 "import sys; sys.stdout.buffer.write(b'^a(?:bc?)?$\\r\\n')"],
                                capture_output=True, text=True, check=True)
        self.assertEqual(result.stdout, EXPECTED)


def bundle_parts(bundle):
    with zipfile.ZipFile(bundle) as archive:
        names = set(archive.namelist())
        if names not in ({"manifest.toml", "code.wasm"},
                         {"krate-profile", "manifest.toml", "code.wasm"}):
            raise ValueError("Unexpected content in the grex bundle")
        # Current packers add the standard strict-format marker to older
        # bundles. That is not application code or an extra capability.
        if "krate-profile" in names and archive.read("krate-profile") != b"2":
            raise ValueError("Unexpected Krate bundle profile")
        return archive.read("manifest.toml"), archive.read("code.wasm")


def repack(krate, bundle):
    _, original_code = bundle_parts(bundle)
    with tempfile.TemporaryDirectory(prefix="krate-grex-pack-") as tmp:
        work = Path(tmp)
        with zipfile.ZipFile(bundle) as archive:
            archive.extract("code.wasm", work)
        packed = work / "grex.krate"
        subprocess.run([str(krate), "pack", "--manifest", str(MANIFEST),
                        "--output", str(packed), str(work / "code.wasm")],
                       check=True, timeout=60)
        manifest, code = bundle_parts(packed)
        if manifest != MANIFEST.read_bytes() or code != original_code:
            raise ValueError("Repack changed the code or did not preserve the reviewed manifest")
        shutil.copyfile(packed, bundle)
    print("unchanged code.wasm sha256:", hashlib.sha256(original_code).hexdigest())


def check_case(krate, bundle, name, args, grant=False, denied=False,
               help_text=False, invalid_path=False):
    # Every case gets a fresh home/profile and real input files. No persisted
    # permission from a prior run, and no access to the developer's own data.
    with tempfile.TemporaryDirectory(prefix="krate-grex-test-") as tmp:
        work = Path(tmp)
        (work / "input").mkdir()
        (work / "input/examples.txt").write_text("a\nab\nabc\n", encoding="utf-8")
        (work / "outside.txt").write_text("private-grex-fixture\n", encoding="utf-8")
        home = work / "home"
        home.mkdir()
        env = dict(os.environ, HOME=str(home), USERPROFILE=str(home),
                   XDG_CONFIG_HOME=str(home / "config"),
                   XDG_DATA_HOME=str(home / "data"),
                   XDG_CACHE_HOME=str(home / "cache"),
                   APPDATA=str(home / "appdata"), LOCALAPPDATA=str(home / "local"),
                   DO_NOT_TRACK="1")
        command = [str(krate), "run", str(bundle), "--headless",
                   "--profile", "grex-regression"]
        if grant:
            command += ["--grant", "fs.read:./input/**"]
        result = subprocess.run(command + ["--", *args], cwd=work, env=env,
                                capture_output=True, text=True, timeout=30)
        if invalid_path:
            correct = (result.returncode == 1 and not result.stdout
                       and "could not be read" in result.stderr)
        elif denied:
            correct = (result.returncode == 5 and not result.stdout
                       and "permission denied" in result.stderr.lower())
        elif help_text:
            correct = result.returncode == 0 and "Usage: grex" in result.stdout
        else:
            correct = result.returncode == 0 and result.stdout == EXPECTED
        if not correct:
            raise AssertionError(f"{name}: exit={result.returncode}, "
                                 f"stdout={result.stdout!r}, stderr={result.stderr!r}")
        print(f"grex {name}: PASS (exit {result.returncode})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--krate", type=Path)
    parser.add_argument("--bundle", type=Path,
                        default=ROOT / "evidence/ported/grex.krate")
    parser.add_argument("--repack", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        unittest.main(argv=[__file__])
        return
    if args.krate is None:
        parser.error("--krate is required unless --self-test is used")
    krate, bundle = resolve_runtime(args.krate), args.bundle.resolve()
    if not krate.is_file():
        parser.error(f"runtime does not exist: {krate}")
    print(subprocess.check_output([str(krate), "--version"], text=True).strip())
    if args.repack:
        repack(krate, bundle)
    manifest, _ = bundle_parts(bundle)
    if manifest != MANIFEST.read_bytes():
        parser.error("bundle manifest differs from grex.manifest.toml; use --repack")
    check_case(krate, bundle, "direct arguments, no grants", ["a", "ab", "abc"])
    check_case(krate, bundle, "quick, no grants", ["quick"])
    check_case(krate, bundle, "help, no grants", ["--help"], help_text=True)
    check_case(krate, bundle, "file denied without grant", ["--file", "input/examples.txt"], denied=True)
    check_case(krate, bundle, "file allowed with scoped grant", ["--file", "input/examples.txt"], grant=True)
    check_case(krate, bundle, "short file flag allowed", ["-f", "input/examples.txt"], grant=True)
    check_case(krate, bundle, "outside file denied", ["--file", "outside.txt"], grant=True, denied=True)
    check_case(krate, bundle, "parent traversal rejected", ["--file", "input/../outside.txt"], grant=True, invalid_path=True)
    print(f"grex: 8 input/permission checks passed ({EXPECTED.strip()})")


if __name__ == "__main__":
    main()
