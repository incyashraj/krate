#!/usr/bin/env python3
"""The CI cross-build must build the way the release cross-build builds.

CI has a step called "Build the release binary the way the release does".
Its whole job is to catch an arm64 Linux break BEFORE a release hits it,
and it can only do that if the claim in its name is true.

It was not. The release build step sets LIBCLANG_PATH so bindgen loads the
clang 9 that Cross.toml installs; the CI step set nothing, so the container's
preinstalled libclang 3.8 won a race that the release had already fixed
(K-717). The lane failed while the release passed -- the exact inversion the
step exists to prevent, and the kind of drift nobody notices because the two
files are read months apart.

So: every environment variable the release sets for its cross build must also
be set for the CI cross build. Not the other way round -- CI may set extra
things, like a cache key, that a release has no use for.

Run with --self-test to check the checker against the real files and against
a case built to fail.
"""

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
RELEASE = ROOT / ".github/workflows/release.yml"
CI = ROOT / ".github/workflows/ci.yml"

# The step in each file whose env must match, named by the text that
# identifies it. Matching on the step NAME rather than a line number, so
# neither file can drift into a false pass by growing a few lines.
RELEASE_STEP = "- name: Build release binary"
CI_STEP = "- name: Build the release binary the way the release does"


def step_body(text: str, marker: str) -> str:
    """One step's text, from its name to the next step at the same indent."""
    at = text.find(marker)
    if at < 0:
        raise SystemExit(f"could not find the step {marker!r} -- it was renamed or removed")
    indent = len(text[:at].split("\n")[-1])
    rest = text[at + len(marker):]
    # The step ends at the next line starting with the same indent and a dash.
    end = re.search(rf"\n {{{indent}}}- ", rest)
    return rest[: end.start()] if end else rest


# The variable is carried in env as CROSS_LIBCLANG_PATH and handed to the
# cross command alone. Set as LIBCLANG_PATH for a whole step it makes bindgen
# look nowhere else, which is right inside the container and fatal on a
# native host: v0.5.1's first release run lost macOS and x86_64 Linux that
# way. So the env must match AND each step must pass it on this exact line.
HANDOFF = 'LIBCLANG_PATH="$CROSS_LIBCLANG_PATH"'


def step_env(text: str, marker: str) -> dict[str, str]:
    """The env: block of one step, as {NAME: value}."""
    body = step_body(text, marker)

    env: dict[str, str] = {}
    env_at = re.search(r"\n\s+env:\n", body)
    if not env_at:
        return env
    after = body[env_at.end():]
    for line in after.split("\n"):
        if not line.strip() or line.strip().startswith("#"):
            continue
        # The env block ends at the first line indented no further than `env:`.
        here = len(line) - len(line.lstrip())
        if here <= env_at.group(0).count(" ") // 2:
            pass
        m = re.match(r"^\s+([A-Z_][A-Z0-9_]*):\s*(.*)$", line)
        if m:
            env[m.group(1)] = m.group(2).strip()
            continue
        if line.strip() and not line.startswith(" " * 8):
            break
    return env


def check(release_text: str, ci_text: str) -> list[str]:
    release_env = step_env(release_text, RELEASE_STEP)
    ci_env = step_env(ci_text, CI_STEP)
    problems = []
    for name, value in release_env.items():
        # The version stamp is a release-only fact: a CI build is not a
        # release and must not claim a tag.
        if name == "KRATE_RELEASE_VERSION":
            continue
        if name not in ci_env:
            problems.append(
                f"the release cross build sets {name}={value} and the CI one does not, "
                f"so CI is not building the way the release does"
            )
        elif ci_env[name] != value:
            problems.append(
                f"{name} differs: release={value!r}, ci={ci_env[name]!r}"
            )
    for label, text, marker in (("release", release_text, RELEASE_STEP), ("CI", ci_text, CI_STEP)):
        body = step_body(text, marker)
        if not re.search(re.escape(HANDOFF) + r"\s*\\?\s*\n?\s*cross build", body):
            problems.append(
                f"the {label} cross build does not hand {HANDOFF} to `cross build`, "
                f"so bindgen inside the container will not find libclang"
            )
    return problems


def self_test() -> int:
    """Exercise the real function, on the real files and on a rigged one."""
    release_text = RELEASE.read_text()
    ci_text = CI.read_text()

    # 1. The real files must agree today.
    real = check(release_text, ci_text)
    if real:
        print("self-test: the real files disagree, which is the bug this checks for:")
        for p in real:
            print(f"  - {p}")
        return 1

    # 2. A CI file with the variable removed must be caught. This is the
    #    exact shape of K-717, kept as a negative fixture so a checker that
    #    stops checking cannot pass quietly.
    # Found by name, not by value. The first version hardcoded llvm-9 and
    # broke the moment the cross build moved to libclang 8 from Ubuntu's own
    # archive -- it reported "the line moved" rather than silently passing,
    # which is the right failure, but a fixture that needs editing whenever
    # the value changes is a fixture that will one day be edited wrong.
    import re as _re
    hit = _re.search(r"^\s*CROSS_LIBCLANG_PATH: .+$\n", ci_text, _re.M)
    if not hit:
        print("self-test: CI sets no CROSS_LIBCLANG_PATH -- the cross build cannot work")
        return 1
    broken = ci_text[: hit.start()] + ci_text[hit.end() :]
    if broken == ci_text:
        print("self-test: could not build the negative fixture -- the line moved")
        return 1
    caught = check(release_text, broken)
    if not any("CROSS_LIBCLANG_PATH" in p and "does not" in p for p in caught):
        print("self-test: removing CROSS_LIBCLANG_PATH from CI was NOT caught")
        return 1

    # 3. A value that drifts must be caught too, not just an absent one.
    drifted = _re.sub(r"(CROSS_LIBCLANG_PATH: ).+", r"\1/usr/lib/llvm-14/lib", ci_text, count=1)
    if not any("differs" in p for p in check(release_text, drifted)):
        print("self-test: a drifted value was NOT caught")
        return 1

    # 4. The env alone is not enough: the value must be handed to the cross
    #    command. A CI step that keeps the env but drops the handoff builds
    #    with the container's libclang 3.8 again, and a release that drops
    #    it does the same on the one target the variable exists for.
    for label, text in (("CI", ci_text), ("release", release_text)):
        if HANDOFF not in text:
            print(f"self-test: the {label} file has no {HANDOFF} handoff to remove")
            return 1
        no_handoff = text.replace(HANDOFF, "", 1)
        args = (release_text, no_handoff) if label == "CI" else (no_handoff, ci_text)
        if not any("hand" in p and label in p for p in check(*args)):
            print(f"self-test: dropping the handoff from the {label} step was NOT caught")
            return 1

    print(
        "self-test: ok -- agreement holds; a missing value, a drifted value and a "
        "dropped handoff are each caught"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="check the checker")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    problems = check(RELEASE.read_text(), CI.read_text())
    if problems:
        print("The CI cross build does not build the way the release does:")
        for p in problems:
            print(f"  - {p}")
        print(
            "\nThe CI step is named 'Build the release binary the way the release "
            "does'. Either make that true, or rename it."
        )
        return 1
    print("ok  the CI cross build matches the release cross build")
    return 0


if __name__ == "__main__":
    sys.exit(main())
