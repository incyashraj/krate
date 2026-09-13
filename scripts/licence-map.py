#!/usr/bin/env python3
"""The component licence and provenance map (CP1 exit test; IC-176).

Krate is shipped under three different terms, and every binary links
hundreds of crates under fifteen more. Nobody can hold that in their head,
and the CP1 exit test asks that "the licence map cover every public
boundary and dependency" -- so this writes it down from the sources of
truth rather than from memory:

  * our own components, read from the tree: every workspace crate's Cargo
    licence field, and the LICENSE file beside each service and app that
    is not a workspace crate;
  * every dependency, from `cargo deny list -f json`, with every licence it
    offers and the one Krate actually relies on;
  * the crates that offer a copyleft licence beside a permissive one, with
    the reason that is fine written next to them, because a reader who
    sees "GPL-2.0-only" in a list and no explanation will re-derive the
    answer, wrongly, every time.

What "the licence Krate relies on" means
----------------------------------------
A crate that offers `MIT OR Apache-2.0 OR LGPL-2.1` is used under whichever
of those the allow list in deny.toml accepts, and cargo-deny's own rule is
that any allowed licence in an OR satisfies the check. This picks the FIRST
allowed one in the allow list's order, so the choice is stable and stated.
A crate offering no allowed licence at all is a finding, not a row: it would
already have failed `cargo deny check`, and if it appears here something
has drifted between the two tools.

Drift is the point
------------------
`--check` regenerates the map and compares it to the committed copy. A new
dependency, a dependency that changed its licence on upgrade, or a component
that lost its LICENSE file all change the file, and CI refuses until a
person has looked. The map is generated so that it cannot be stale, and
committed so that a change to it is a change someone reviewed.

  python3 scripts/licence-map.py            # write docs/licence-map.md
  python3 scripts/licence-map.py --check    # fail if the committed map drifted
  python3 scripts/licence-map.py --self-test
"""

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "licence-map.md"
DENY = ROOT / "deny.toml"

# Components that are not workspace crates and carry their own terms. Each
# is read from disk, never assumed: a missing file is reported as missing.
SERVICES = [
    ("studio", "Krate Studio (desktop app)"),
    ("cloud/worker", "Krate hub (Cloudflare Worker)"),
    ("cloud/builder", "Build service"),
]

# Licences that carry copyleft obligations if they were the one relied on.
# Listed so a dual-licensed crate offering one of these gets an explicit
# line saying which permissive alternative is used instead.
COPYLEFT = re.compile(r"^(A?GPL|LGPL|MPL|EUPL|CDDL|EPL|OSL)", re.IGNORECASE)


def sh(args, cwd=ROOT):
    out = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if out.returncode != 0:
        return None
    return out.stdout


# ------------------------------------------------------------- the allow list


def allowed_licences(deny_text):
    """The `allow = [...]` list under `[licenses]`, in its own order."""
    section = re.search(r"\[licenses\](.*?)(?:\n\[|\Z)", deny_text, re.S)
    if not section:
        return []
    m = re.search(r"allow\s*=\s*\[(.*?)\]", section.group(1), re.S)
    if not m:
        return []
    return re.findall(r'"([^"]+)"', m.group(1))


# ---------------------------------------------------------- our own components


def workspace_components(root=ROOT):
    """Every workspace crate and the licence its Cargo.toml states."""
    ws = (root / "Cargo.toml").read_text()
    m = re.search(r'^license\s*=\s*"([^"]+)"', ws, re.M)
    workspace_licence = m.group(1) if m else None
    rows = []
    for toml in sorted(root.glob("crates/*/Cargo.toml")):
        text = toml.read_text()
        name_m = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M)
        name = name_m.group(1) if name_m else toml.parent.name
        if re.search(r"^license\.workspace\s*=\s*true", text, re.M):
            licence = workspace_licence
            source = "workspace Cargo.toml"
        else:
            own = re.search(r'^license(?:-file)?\s*=\s*"([^"]+)"', text, re.M)
            licence = own.group(1) if own else None
            source = toml.relative_to(root).as_posix()
        rows.append({"component": name, "path": toml.parent.relative_to(root).as_posix(),
                     "licence": licence, "source": source})
    return rows


def service_components(root=ROOT):
    """Services and apps outside the workspace: what their LICENSE file says."""
    rows = []
    for rel, label in SERVICES:
        directory = root / rel
        licence, source = None, None
        for candidate in ("LICENSE", "LICENSE.md", "LICENCE", "COPYING"):
            path = directory / candidate
            if path.is_file():
                first = path.read_text(errors="replace").strip().splitlines()
                licence = first[0].strip() if first else "(empty file)"
                source = path.relative_to(root).as_posix()
                break
        if licence is None:
            cargo = directory / "Cargo.toml"
            if cargo.is_file():
                m = re.search(r'^license(?:-file)?\s*=\s*"([^"]+)"', cargo.read_text(), re.M)
                if m:
                    licence, source = m.group(1), cargo.relative_to(root).as_posix()
        if licence is None:
            pkg = directory / "package.json"
            if pkg.is_file():
                try:
                    licence = json.loads(pkg.read_text()).get("license")
                    source = pkg.relative_to(root).as_posix() if licence else None
                except json.JSONDecodeError:
                    pass
        rows.append({"component": label, "path": rel, "licence": licence, "source": source})
    return rows


# --------------------------------------------------------------- dependencies


def dependencies(deny_json, allow):
    """One row per crate spec: every licence offered, and the one relied on."""
    offered = {}
    for licence, crates in deny_json.get("licenses", []):
        for spec in crates:
            offered.setdefault(spec, []).append(licence)
    for spec in deny_json.get("unlicensed", []):
        offered.setdefault(spec, [])

    rows = []
    for spec in sorted(offered):
        name, version = spec.split(" ")[0], spec.split(" ")[1] if " " in spec else "?"
        licences = sorted(offered[spec])
        chosen = next((a for a in allow if a in licences), None)
        rows.append({
            "crate": name,
            "version": version,
            "offered": licences,
            "relied_on": chosen,
            "copyleft_offered": [l for l in licences if COPYLEFT.match(l)],
        })
    return rows


def findings(components, deps):
    """What a reader must act on, as sentences. Empty means clean."""
    out = []
    for c in components:
        if not c["licence"]:
            out.append(f"{c['component']} ({c['path']}) states no licence -- no LICENSE file and no licence field")
    for d in deps:
        if d["relied_on"] is None:
            out.append(f"{d['crate']} {d['version']} offers {d['offered'] or 'no licence'}, none of which the allow list accepts")
    return out


# ------------------------------------------------------------------ rendering


def render(components, services, deps, allow, problems):
    lines = [
        "# Component licence and provenance map",
        "",
        "Generated by `scripts/licence-map.py` from the tree and from",
        "`cargo deny list`. Do not edit by hand: regenerate, review the diff,",
        "commit. CI refuses a stale copy.",
        "",
        "## Our components",
        "",
        "| Component | Path | Licence | Stated in |",
        "|---|---|---|---|",
    ]
    for c in components + services:
        lic = c["licence"] or "**NONE STATED**"
        lines.append(f"| {c['component']} | `{c['path']}` | {lic} | {c['source'] or '-'} |")

    lines += [
        "",
        "## Findings",
        "",
    ]
    if problems:
        lines += [f"- {p}" for p in problems]
    else:
        lines.append("- none: every component states a licence and every dependency offers an allowed one")

    copyleft = [d for d in deps if d["copyleft_offered"]]
    lines += [
        "",
        "## Dependencies that offer a copyleft licence",
        "",
        "These appear under GPL, LGPL or MPL in a raw listing. Each also offers a",
        "permissive licence, and that is the one Krate relies on. cargo-deny's rule",
        "is that any allowed licence in an OR satisfies the check; this map states",
        "which one, so the question is not re-derived.",
        "",
        "| Crate | Offers | Relied on |",
        "|---|---|---|",
    ]
    for d in copyleft:
        lines.append(f"| {d['crate']} {d['version']} | {', '.join(d['offered'])} | {d['relied_on'] or '**NONE**'} |")
    if not copyleft:
        lines.append("| (none) | | |")

    by_licence = {}
    for d in deps:
        by_licence.setdefault(d["relied_on"] or "NONE", []).append(d)
    lines += [
        "",
        "## Dependencies by the licence relied on",
        "",
        f"{len(deps)} crate versions. Allow list order (deny.toml): {', '.join(allow)}.",
        "",
    ]
    for licence in sorted(by_licence, key=lambda l: (-len(by_licence[l]), l)):
        group = by_licence[licence]
        lines.append(f"### {licence} ({len(group)})")
        lines.append("")
        lines.append(", ".join(f"{d['crate']} {d['version']}" for d in group))
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def build(root=ROOT):
    deny_text = (root / "deny.toml").read_text()
    allow = allowed_licences(deny_text)
    raw = sh(["cargo", "deny", "list", "-f", "json"], cwd=root)
    if raw is None:
        return None, ["`cargo deny list` failed; is cargo-deny installed?"]
    deny_json = json.loads(raw)
    components = workspace_components(root)
    services = service_components(root)
    deps = dependencies(deny_json, allow)
    problems = findings(components + services, deps)
    return render(components, services, deps, allow, problems), problems


# ------------------------------------------------------------------- commands


def cmd_write():
    text, problems = build()
    if text is None:
        for p in problems:
            print(p, file=sys.stderr)
        return 2
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text)
    print(f"wrote {OUT.relative_to(ROOT)}")
    for p in problems:
        print(f"  finding: {p}")
    return 0


def cmd_check():
    text, problems = build()
    if text is None:
        for p in problems:
            print(p, file=sys.stderr)
        print("the map could not be generated, so it was NOT checked. This is not a pass.",
              file=sys.stderr)
        return 2
    if not OUT.exists():
        print(f"{OUT.relative_to(ROOT)} does not exist; write it first:", file=sys.stderr)
        print("  python3 scripts/licence-map.py", file=sys.stderr)
        return 1
    if OUT.read_text() != text:
        print(f"{OUT.relative_to(ROOT)} is stale: a dependency, a licence, or a component's "
              "terms changed since it was committed.", file=sys.stderr)
        print("Regenerate it, read the diff, and commit the change:", file=sys.stderr)
        print("  python3 scripts/licence-map.py && git diff docs/licence-map.md", file=sys.stderr)
        return 1
    print(f"OK -- {OUT.relative_to(ROOT)} matches the tree and the dependency graph")
    for p in problems:
        print(f"  standing finding: {p}")
    return 0


# ------------------------------------------------------------------ self-test


def self_test():
    failures = []

    def check(name, cond, detail=""):
        if not cond:
            failures.append(f"{name}: {detail}" if detail else name)

    deny = 'x = 1\n[licenses]\nallow = [\n  "MIT",\n  "Apache-2.0",\n]\n[bans]\nallow = ["nothing"]\n'
    allow = allowed_licences(deny)
    check("the allow list is read in order from [licenses] only", allow == ["MIT", "Apache-2.0"], f"{allow}")

    fake = {
        "licenses": [
            ["GPL-2.0-only", ["dual 1.0 reg", "gpl-only 2.0 reg"]],
            ["MIT", ["dual 1.0 reg", "plain 3.0 reg"]],
            ["Apache-2.0", ["plain 3.0 reg"]],
            ["LGPL-2.1-or-later", ["tri 4.0 reg"]],
            ["Apache-2.0", ["tri 4.0 reg"]],
        ],
        "unlicensed": ["mystery 9.9 reg"],
    }
    deps = dependencies(fake, allow)
    by = {d["crate"]: d for d in deps}
    check("a crate offering GPL or MIT relies on MIT", by["dual"]["relied_on"] == "MIT", f"{by['dual']}")
    check("and it is listed as offering copyleft", by["dual"]["copyleft_offered"] == ["GPL-2.0-only"])
    check("a GPL-only crate relies on nothing", by["gpl-only"]["relied_on"] is None)
    check("a tri-licensed crate picks the first ALLOWED in allow order, not in its own order",
          by["tri"]["relied_on"] == "Apache-2.0", f"{by['tri']}")
    check("an unlicensed crate is a row with nothing offered", by["mystery"]["offered"] == [])
    check("a plain crate offering both picks the allow list's first", by["plain"]["relied_on"] == "MIT")

    comps = [{"component": "ok", "path": "a", "licence": "MIT", "source": "x"},
             {"component": "none", "path": "cloud/thing", "licence": None, "source": None}]
    probs = findings(comps, deps)
    check("a component with no licence is a finding", any("cloud/thing" in p and "no licence" in p for p in probs), f"{probs}")
    check("a GPL-only crate is a finding", any("gpl-only" in p for p in probs), f"{probs}")
    check("an unlicensed crate is a finding", any("mystery" in p for p in probs), f"{probs}")
    check("a dual crate is NOT a finding: it has an allowed licence", not any("dual" in p for p in probs), f"{probs}")

    text = render(comps, [], deps, allow, probs)
    check("the map names the component with no licence loudly", "**NONE STATED**" in text)
    check("the map explains the copyleft rows", "dual 1.0" in text and "Relied on" in text)
    check("findings are in the map", "gpl-only" in text)

    clean = render(comps[:1], [], [by["dual"], by["plain"]], allow, [])
    check("a clean map says so", "none: every component states a licence" in clean)

    # The real tree: the generator must run, and every one of our own crates
    # must state a licence -- that is the exit clause itself.
    real = workspace_components()
    check("every workspace crate is found", len(real) >= 10, f"{len(real)}")
    check("every workspace crate states a licence", all(c["licence"] for c in real),
          f"{[c['component'] for c in real if not c['licence']]}")

    if failures:
        print("licence-map self-test FAILED:\n")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("licence-map self-test OK -- the allow list is read in order, an OR resolves to the first "
          "allowed licence, copyleft-only and unlicensed crates and unlicensed components are findings")
    return 0


def main(argv):
    if "--self-test" in argv:
        return self_test()
    if "--check" in argv:
        return cmd_check()
    return cmd_write()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
