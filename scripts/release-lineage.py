#!/usr/bin/env python3
"""Bind every published release artifact to the thing that made it (IC-612).

A release publishes ten files and a SHA256SUMS beside them. The digests in
that file are computed at publish time, checked once by the verify job, and
then forgotten: nothing retains them, and nothing ties them back to the
commit, the toolchain, the tests or the signer that produced them. The
release decision record says so itself, in its own list of what it does not
assess -- "that published download artifacts byte-match what CI built".

So the facts exist and are thrown away. This writes them down.

What a lineage record binds
---------------------------
One record per published release, holding:

  * the tag, and the commit and tree it resolves to (1129: the release and
    the source are separate immutable identities, and the record names both
    rather than implying one from the other);
  * every published artifact, by name, size, digest and target triple
    (1130, 1132);
  * the toolchain that built it, read from the pinned file AT THAT COMMIT --
    not from this machine, which is a different computer than the one that
    built the release;
  * the CI run whose conclusion the release gate required, by id, so the
    tests are named rather than asserted;
  * the signer, per artifact, as OBSERVED -- see below.

What it refuses to guess
------------------------
The signer is the honest hard case. This script runs on a Mac or in CI long
after the release, and it cannot reach the keychain the release job used.
Two things are therefore kept apart:

  * `signed: "unknown"` -- nobody looked. This is the default, and it is not
    a claim that the artifact is unsigned.
  * `signed: "no"` -- something looked and found no signature.

A record that cannot tell those apart would let "we never checked" read as
"it is fine", which is the same false green the notary rejection of v0.1.54
produced. Only `--inspect` fills this in, and only for artifact kinds this
machine can actually inspect.

Likewise the record carries `intent`, which is a human sentence about why
the release was cut. There is no way to derive it, so it is passed in or it
is absent -- never invented.

Verification, not assertion
---------------------------
`verify` re-reads the published release and checks the record still
describes it: same artifact set, same digests, same sizes. A record that
drifted from the release it describes is worse than no record, because it
reads as provenance while naming bytes nobody can download any more.

  python3 scripts/release-lineage.py record v0.3.0 --intent "..."
  python3 scripts/release-lineage.py verify v0.3.0
  python3 scripts/release-lineage.py inventory v0.3.0
  python3 scripts/release-lineage.py --self-test
"""

import argparse
import datetime
import hashlib
import json
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "evidence" / "lineage"
SCHEMA = "krate.release-lineage.v1"
REPO = "incyashraj/krate"

DIGEST = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
TAG = re.compile(r"^v[0-9][0-9A-Za-z.+-]*$")

# Whether a signature was looked for, and what was found. "unknown" is the
# default and says only that nobody looked -- it must never be read as "no".
SIGNED_STATES = {"yes", "no", "unknown"}

# Target triples appear in artifact names. Recognising one is how an
# artifact gets attributed to a platform (1132); an artifact whose target
# cannot be read is recorded with target None rather than guessed at, and
# the audit says how many those are.
TRIPLES = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
]
# Installers are named for a platform rather than a triple.
LOOSE_TARGETS = [
    ("universal.dmg", "universal-apple-darwin"),
    ("windows-x64-setup.exe", "x86_64-pc-windows-msvc"),
    ("linux-x86_64.AppImage", "x86_64-unknown-linux-gnu"),
]

# What each artifact is FOR. 1129 asks for distinct identities per kind, and
# a flat list of files does not distinguish the thing you run from the thing
# that installs it.
def artifact_role(name):
    if name.endswith(".AppImage") or name.endswith("-setup.exe") or name.endswith(".dmg"):
        return "installer"
    if name.startswith("krate-app-"):
        return "player"
    if name.startswith("krate-studio-"):
        return "studio"
    if name == "SHA256SUMS":
        return "checksums"
    return "cli"


def sh(args, cwd=ROOT):
    """Run a command and return stdout, or None if it failed."""
    try:
        out = subprocess.run(
            args, cwd=cwd, capture_output=True, text=True, timeout=120
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    return out.stdout.strip()


def target_of(name):
    """The platform an artifact is for, or None when the name does not say."""
    for triple in TRIPLES:
        if triple in name:
            return triple
    for suffix, triple in LOOSE_TARGETS:
        if name.endswith(suffix):
            return triple
    return None


def toolchain_at(commit):
    """The pinned toolchain AT THAT COMMIT, not the one on this machine.

    Reading rust-toolchain.toml from the working tree would record whatever
    this checkout happens to be on, which for an old release is a different
    compiler than the one that built it. `git show` asks the commit.
    """
    blob = sh(["git", "show", f"{commit}:rust-toolchain.toml"])
    if blob is None:
        return {"rust": None, "read_from": "unavailable"}
    m = re.search(r'channel\s*=\s*"([^"]+)"', blob)
    return {
        "rust": m.group(1) if m else None,
        "read_from": f"rust-toolchain.toml at {commit[:9]}",
    }


def parse_sha256sums(text):
    """`<digest>  <name>` lines to a dict. Malformed lines are reported."""
    out, bad = {}, []
    for line in text.splitlines():
        line = line.rstrip()
        if not line.strip():
            continue
        parts = line.split(None, 1)
        if len(parts) != 2 or not DIGEST.match(parts[0]):
            bad.append(line)
            continue
        out[parts[1].strip()] = parts[0]
    return out, bad


def gh_release(tag):
    """The published release: its assets, and the SHA256SUMS beside them."""
    raw = sh([
        "gh", "release", "view", tag, "-R", REPO,
        "--json", "tagName,publishedAt,isDraft,isPrerelease,assets",
    ])
    if raw is None:
        return None
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return None


def fetch_sums(tag, tmp):
    """Download SHA256SUMS for a tag. Returns its text, or None."""
    dest = Path(tmp) / "SHA256SUMS"
    if sh(["gh", "release", "download", tag, "-R", REPO,
           "-p", "SHA256SUMS", "-O", str(dest), "--clobber"]) is None:
        return None
    try:
        return dest.read_text()
    except OSError:
        return None


def ci_run_for(commit):
    """The CI run the release gate required on this exact commit.

    The gate at .github/workflows/release.yml already refuses to build a
    release unless CI concluded success here. That check happens and is
    then forgotten; this names the run so the record points at the tests
    rather than asserting them.
    """
    raw = sh([
        "gh", "api",
        f"repos/{REPO}/actions/runs?head_sha={commit}&per_page=100",
        "--jq", '[.workflow_runs[] | select(.name == "CI")][0]'
        ' | {id, conclusion, status, html_url}',
    ])
    if not raw:
        return None
    try:
        run = json.loads(raw)
    except json.JSONDecodeError:
        return None
    return run if isinstance(run, dict) and run.get("id") else None


def build_record(tag, release, sums, commit, tree, ci, intent, now, signed=None):
    """Assemble the record. Pure: every input is already gathered."""
    signed = signed or {}
    assets = {a["name"]: a for a in release.get("assets", [])}
    artifacts = []
    for name in sorted(sums):
        asset = assets.get(name, {})
        artifacts.append({
            "name": name,
            "digest": sums[name],
            "size": asset.get("size"),
            "target": target_of(name),
            "role": artifact_role(name),
            # Default "unknown": nobody looked. Never "no".
            "signed": signed.get(name, "unknown"),
        })

    # The checksums file describes the others and cannot describe itself, so
    # it is listed as an artifact of the release but carries no digest line
    # of its own. Recording it with its published size keeps the inventory
    # complete (1132) without inventing a self-referential digest.
    if "SHA256SUMS" in assets and "SHA256SUMS" not in sums:
        artifacts.append({
            "name": "SHA256SUMS",
            "digest": None,
            "size": assets["SHA256SUMS"].get("size"),
            "target": None,
            "role": "checksums",
            "signed": signed.get("SHA256SUMS", "unknown"),
            "note": "the file that carries the others' digests; it cannot carry its own",
        })

    record = {
        "schema": SCHEMA,
        "id": f"L-{tag}",
        "release": {
            "tag": tag,
            "published_at": release.get("publishedAt"),
            "prerelease": bool(release.get("isPrerelease")),
            "url": f"https://github.com/{REPO}/releases/tag/{tag}",
        },
        "source": {"commit": commit, "tree": tree},
        "toolchain": toolchain_at(commit),
        "tests": (
            {
                "ci_run": ci["id"],
                "conclusion": ci.get("conclusion"),
                "location": ci.get("html_url"),
                "required_by": "the release gate in .github/workflows/release.yml",
            }
            if ci else
            {"ci_run": None, "conclusion": None,
             "note": "no CI run was found for this commit when the record was written"}
        ),
        "intent": intent,
        "artifacts": artifacts,
        "recorded_at": now,
        "scope": {
            "supports": (
                f"the {len(artifacts)} files published under {tag} are these bytes, "
                f"built from {commit[:9]}"
            ),
            "exclusions": [
                "whether those bytes behave correctly -- that is what the CI run says",
                "whether an artifact is signed, unless 'signed' says yes or no",
                "any artifact removed from the release after this record was written",
            ],
        },
    }
    record["content_sha256"] = content_digest(record)
    return record


def content_digest(record):
    """A digest over the record's own content, excluding the digest field."""
    body = {k: v for k, v in record.items() if k != "content_sha256"}
    canonical = json.dumps(body, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()


def validate(record):
    """The rules one record must meet. Returns a list of problems."""
    p = []
    if record.get("schema") != SCHEMA:
        p.append(f"schema must be {SCHEMA}, got {record.get('schema')!r}")

    rel = record.get("release")
    if not isinstance(rel, dict):
        p.append("release must name the tag it describes")
        rel = {}
    tag = rel.get("tag")
    if not isinstance(tag, str) or not TAG.match(tag):
        p.append(f"release.tag must be a version tag, got {tag!r}")

    src = record.get("source")
    if not isinstance(src, dict):
        p.append("source must name the commit and the tree separately (1129)")
        src = {}
    for part in ("commit", "tree"):
        if not COMMIT.match(str(src.get(part, ""))):
            p.append(f"source.{part} must be a full git object id (1129)")
    if src.get("commit") and src.get("commit") == src.get("tree"):
        p.append(
            "source.commit and source.tree are the same value; a commit and the "
            "tree it points at are different objects (1129)"
        )

    arts = record.get("artifacts")
    if not isinstance(arts, list) or not arts:
        p.append("artifacts must list every published file (1132)")
        arts = []
    seen = set()
    for a in arts:
        if not isinstance(a, dict):
            p.append("every artifact must be an object")
            continue
        name = a.get("name")
        if not isinstance(name, str) or not name:
            p.append("every artifact must be named")
            continue
        if name in seen:
            p.append(f"artifact {name} is listed twice")
        seen.add(name)
        digest = a.get("digest")
        if digest is not None and not DIGEST.match(str(digest)):
            p.append(f"artifact {name}: digest must be a sha256 or null, got {digest!r}")
        if a.get("signed") not in SIGNED_STATES:
            p.append(
                f"artifact {name}: signed must be one of {sorted(SIGNED_STATES)} -- "
                "'unknown' means nobody looked and must not be written as 'no'"
            )
        if a.get("role") == "checksums" and digest is not None:
            p.append(
                f"artifact {name}: the checksums file cannot carry its own digest"
            )

    tests = record.get("tests")
    if not isinstance(tests, dict):
        p.append("tests must name the CI run the gate required, or say it is absent (1130)")

    tc = record.get("toolchain")
    if not isinstance(tc, dict) or "rust" not in tc:
        p.append("toolchain must name the pinned compiler, or say it could not be read (1130)")

    if "intent" not in record:
        p.append("intent must be present, even as null -- an absent field reads as forgotten")

    if record.get("content_sha256") != content_digest(record):
        p.append("content_sha256 does not match the record's own content")
    return p


def audit(record):
    """What the record does and does not cover, in sentences."""
    arts = record.get("artifacts", [])
    lines = []
    targets = {}
    for a in arts:
        targets.setdefault(a.get("target"), []).append(a["name"])
    untargeted = targets.pop(None, [])
    for triple in sorted(targets):
        lines.append(f"{triple}: {len(targets[triple])} artifacts")
    if untargeted:
        lines.append(
            f"no target read from the name: {len(untargeted)} "
            f"({', '.join(sorted(untargeted))})"
        )
    unknown = [a["name"] for a in arts if a.get("signed") == "unknown"]
    if unknown:
        lines.append(
            f"signature not looked at: {len(unknown)} of {len(arts)} -- "
            "this is not a claim that they are unsigned"
        )
    if record.get("intent") is None:
        lines.append("no release intent was recorded")
    if not (record.get("tests") or {}).get("ci_run"):
        lines.append("no CI run is named, so these artifacts point at no tests")
    if (record.get("toolchain") or {}).get("rust") is None:
        lines.append("the toolchain could not be read at that commit")
    return lines


def compare(record, sums, release):
    """Has the release drifted from what the record says? Returns problems."""
    p = []
    recorded = {
        a["name"]: a for a in record.get("artifacts", [])
        if a.get("digest") is not None
    }
    for name, digest in sorted(sums.items()):
        # Looked up rather than indexed after a membership test: the two
        # would have to stay in step forever, and a drift check that raises
        # KeyError has told the reader nothing about the release.
        was = recorded.get(name)
        if was is None:
            p.append(f"{name} is published now and is not in the record")
        elif was["digest"] != digest:
            p.append(
                f"{name}: the record says {was['digest'][:12]}..., "
                f"the release now says {digest[:12]}..."
            )
    for name in sorted(recorded):
        if name not in sums:
            p.append(f"{name} is in the record and is no longer published")

    published = {a["name"]: a for a in release.get("assets", [])}
    for a in record.get("artifacts", []):
        asset = published.get(a["name"])
        if asset is None:
            continue
        if a.get("size") is not None and asset.get("size") != a["size"]:
            p.append(
                f"{a['name']}: the record says {a['size']} bytes, "
                f"the release now says {asset.get('size')}"
            )
    return p


def resolve(tag):
    """The commit and tree a tag points at, locally. None when unknown."""
    commit = sh(["git", "rev-parse", f"{tag}^{{commit}}"])
    if not commit or not COMMIT.match(commit):
        return None, None
    tree = sh(["git", "rev-parse", f"{tag}^{{tree}}"])
    if not tree or not COMMIT.match(tree):
        return commit, None
    return commit, tree


def path_for(tag):
    return OUT_DIR / f"{tag}.json"


def cmd_record(args):
    import tempfile

    tag = args.tag
    if not TAG.match(tag):
        print(f"{tag!r} is not a version tag.", file=sys.stderr)
        return 2

    commit, tree = resolve(tag)
    if commit is None:
        print(
            f"{tag} does not resolve to a commit in this checkout.\n"
            "Fetch the tag first: git fetch --tags",
            file=sys.stderr,
        )
        return 2
    if tree is None:
        print(f"{tag} resolves to {commit[:9]} but its tree could not be read.",
              file=sys.stderr)
        return 2

    release = gh_release(tag)
    if release is None:
        print(
            f"the published release {tag} could not be read.\n"
            "This needs gh, authenticated, and network. A record written without\n"
            "reading the release would describe what we hoped was published.",
            file=sys.stderr,
        )
        return 2

    with tempfile.TemporaryDirectory() as tmp:
        text = fetch_sums(tag, tmp)
    if text is None:
        print(
            f"{tag} publishes no SHA256SUMS, so there is nothing to bind the\n"
            "artifacts to. A release without it cannot have a lineage record.",
            file=sys.stderr,
        )
        return 1
    sums, bad = parse_sha256sums(text)
    if bad:
        print(f"SHA256SUMS has {len(bad)} unreadable lines:", file=sys.stderr)
        for line in bad[:5]:
            print(f"  {line}", file=sys.stderr)
        return 1
    if not sums:
        print(f"{tag} publishes an empty SHA256SUMS.", file=sys.stderr)
        return 1

    ci = ci_run_for(commit)
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    record = build_record(tag, release, sums, commit, tree, ci, args.intent, now)

    problems = validate(record)
    if problems:
        print("the record this would write is not valid:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    path_for(tag).write_text(json.dumps(record, indent=1) + "\n")
    print(f"wrote {path_for(tag).relative_to(ROOT)}")
    print(f"  {len(record['artifacts'])} artifacts from {commit[:9]}")
    for line in audit(record):
        print(f"  {line}")
    return 0


def digest_file(path, chunk=1 << 20):
    """sha256 of a file, read in chunks so a 100 MB AppImage is not held."""
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(chunk), b""):
            h.update(block)
    return h.hexdigest()


def cmd_check(args):
    """Check a file already on disk against the committed record.

    This is the part the installers cannot do for themselves. `install.sh`
    verifies a download against a SHA256SUMS fetched from the same release,
    chosen by the same resolution step -- so whoever controls the resolution
    controls the expectation, and a suppressed sums fetch downgrades the
    install to no verification at all (K-307).

    The record is different: it is committed to this repository, so its
    digest travels by a path the download does not. Checking against it is
    an expectation the release origin did not supply.
    """
    path = Path(args.file)
    if not path.is_file():
        print(f"{path} is not a file.", file=sys.stderr)
        return 2

    rec_path = path_for(args.tag)
    if not rec_path.exists():
        print(f"no lineage record for {args.tag}, so there is nothing independent\n"
              f"to check against. This is not a pass.", file=sys.stderr)
        return 2
    try:
        record = json.loads(rec_path.read_text())
    except json.JSONDecodeError as e:
        print(f"{rec_path.name} is not readable JSON: {e}", file=sys.stderr)
        return 2

    problems = validate(record)
    if problems:
        print(f"the record for {args.tag} is not valid, so it cannot be trusted "
              "as an expectation:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 2

    name = args.name or path.name
    wanted = next(
        (a for a in record["artifacts"] if a["name"] == name), None
    )
    if wanted is None:
        print(f"{args.tag} publishes no artifact named {name}.\n"
              f"Names in the record: "
              f"{', '.join(a['name'] for a in record['artifacts'])}",
              file=sys.stderr)
        return 2
    if wanted.get("digest") is None:
        print(f"{name} carries no digest in the record, so it cannot be checked.",
              file=sys.stderr)
        return 2

    actual = digest_file(path)
    if actual != wanted["digest"]:
        print(
            f"{name} is NOT the published artifact.\n"
            f"  the record says {wanted['digest']}\n"
            f"  this file is    {actual}\n"
            "Do not run it.",
            file=sys.stderr,
        )
        return 1

    size = path.stat().st_size
    if wanted.get("size") is not None and size != wanted["size"]:
        # Cannot happen with a matching digest, and saying so is cheap: if it
        # ever does, the record is internally inconsistent and the digest
        # match means less than it appears to.
        print(
            f"{name}: the digest matches but the size does not "
            f"({size} here, {wanted['size']} recorded). The record disagrees "
            "with itself; do not rely on either number.",
            file=sys.stderr,
        )
        return 1

    print(f"OK -- {name} is the artifact {args.tag} published.")
    print(f"  {actual}")
    print(f"  checked against evidence/lineage/{args.tag}.json, which travels "
          "with the source and not with the download")
    if wanted.get("signed") == "unknown":
        print("  this says nothing about whether it is signed: nobody looked")
    return 0


def cmd_verify(args):
    import tempfile

    tag = args.tag
    path = path_for(tag)
    if not path.exists():
        print(f"no lineage record for {tag}. Write one first:\n"
              f"  python3 scripts/release-lineage.py record {tag}", file=sys.stderr)
        return 2
    try:
        record = json.loads(path.read_text())
    except json.JSONDecodeError as e:
        print(f"{path.name} is not readable JSON: {e}", file=sys.stderr)
        return 2

    problems = validate(record)
    if problems:
        print(f"{tag}: the record is not valid:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    release = gh_release(tag)
    if release is None:
        # Fail closed. A verify that cannot read the release has not verified
        # anything, and saying OK here would be the false green this exists
        # to prevent.
        print(
            f"{tag}: the record is internally valid, but the published release\n"
            "could not be read, so it was NOT checked against it. This is not a pass.",
            file=sys.stderr,
        )
        return 2
    with tempfile.TemporaryDirectory() as tmp:
        text = fetch_sums(tag, tmp)
    if text is None:
        print(f"{tag}: SHA256SUMS could not be downloaded, so the digests were "
              "NOT checked. This is not a pass.", file=sys.stderr)
        return 2
    sums, _ = parse_sha256sums(text)

    drift = compare(record, sums, release)
    if drift:
        print(f"{tag}: the record no longer describes the published release:",
              file=sys.stderr)
        for line in drift:
            print(f"  {line}", file=sys.stderr)
        return 1

    print(f"OK -- {tag}: {len(record['artifacts'])} artifacts still match the "
          f"record, built from {record['source']['commit'][:9]}")
    for line in audit(record):
        print(f"  {line}")
    return 0


def cmd_inventory(args):
    path = path_for(args.tag)
    if not path.exists():
        print(f"no lineage record for {args.tag}.", file=sys.stderr)
        return 2
    record = json.loads(path.read_text())
    total = 0
    print(f"{args.tag}  from {record['source']['commit'][:9]}  "
          f"rust {record['toolchain'].get('rust')}")
    for a in record["artifacts"]:
        size = a.get("size")
        total += size or 0
        print(f"  {a['name']}")
        print(f"    {a['role']:<10} {a.get('target') or 'no target in the name':<26} "
              f"{size if size is not None else '?':>10} bytes  signed:{a['signed']}")
    print(f"  {len(record['artifacts'])} artifacts, {total} bytes")
    return 0


# --------------------------------------------------------------- self-test

def _release(**over):
    base = {
        "tagName": "v9.9.9",
        "publishedAt": "2026-09-13T00:00:00Z",
        "isPrerelease": False,
        "assets": [
            {"name": "krate-9.9.9-x86_64-unknown-linux-gnu.tar.gz", "size": 100},
            {"name": "krate-studio-9.9.9-universal.dmg", "size": 200},
            {"name": "SHA256SUMS", "size": 300},
        ],
    }
    base.update(over)
    return base


_SUMS = {
    "krate-9.9.9-x86_64-unknown-linux-gnu.tar.gz": "a" * 64,
    "krate-studio-9.9.9-universal.dmg": "b" * 64,
}
_COMMIT = "1" * 40
_TREE = "2" * 40


def _good():
    """A valid record, built by the real builder from real-shaped inputs."""
    return build_record(
        "v9.9.9", _release(), dict(_SUMS), _COMMIT, _TREE,
        {"id": 42, "conclusion": "success", "html_url": "https://example/42"},
        "the first stable release", "2026-09-13T00:00:00Z",
    )


def self_test():
    failures = []

    def check(name, condition, detail=""):
        if not condition:
            failures.append(f"{name}: {detail}" if detail else name)

    # The builder produces something the validator accepts. If these two ever
    # disagree, every record this script writes is unreadable by its own rules.
    good = _good()
    check("a built record validates", validate(good) == [],
          f"problems: {validate(good)}")

    # 1132: the inventory is complete -- SHA256SUMS is itself an artifact of
    # the release, and leaving it out understates what was published.
    names = {a["name"] for a in good["artifacts"]}
    check("the checksums file is in the inventory", "SHA256SUMS" in names,
          f"artifacts: {sorted(names)}")
    check("every published asset is inventoried",
          names == {a["name"] for a in _release()["assets"]},
          f"got {sorted(names)}")

    # Indexed defensively: when the inventory rule above breaks, this line
    # should report a failed check, not raise IndexError. A self-test that
    # crashes says less than one that names what is wrong.
    sums_entry = next(
        (a for a in good["artifacts"] if a["name"] == "SHA256SUMS"), None
    )
    check("the checksums file carries no digest of its own",
          sums_entry is not None and sums_entry["digest"] is None,
          "SHA256SUMS is not in the inventory at all" if sums_entry is None else "")

    # 1132: each artifact is attributed to a target, including the installer
    # whose name carries no triple.
    by_name = {a["name"]: a for a in good["artifacts"]}
    check("a triple in the name is read",
          by_name["krate-9.9.9-x86_64-unknown-linux-gnu.tar.gz"]["target"]
          == "x86_64-unknown-linux-gnu")
    check("an installer named for a platform is read",
          by_name["krate-studio-9.9.9-universal.dmg"]["target"]
          == "universal-apple-darwin")
    check("an installer is recorded as one",
          by_name["krate-studio-9.9.9-universal.dmg"]["role"] == "installer")

    # 1129: the release identity and the source identity are separate, and
    # the record must not let one stand in for the other.
    same = _good()
    same["source"]["tree"] = same["source"]["commit"]
    same["content_sha256"] = content_digest(same)
    check("a tree equal to its commit is refused",
          any("different objects" in p for p in validate(same)),
          f"problems: {validate(same)}")

    # The signer distinction, which is the whole point of SIGNED_STATES.
    check("unknown is the default, not no",
          all(a["signed"] == "unknown" for a in good["artifacts"]))
    lying = _good()
    lying["artifacts"][0]["signed"] = "probably"
    lying["content_sha256"] = content_digest(lying)
    check("an invented signed state is refused",
          any("signed must be one of" in p for p in validate(lying)))

    # The content digest binds the words to the bytes. An edited record that
    # keeps its old digest is a forgery, and must not validate.
    tampered = _good()
    tampered["artifacts"][0]["digest"] = "f" * 64
    check("an edited record fails its own digest",
          any("content_sha256" in p for p in validate(tampered)),
          f"problems: {validate(tampered)}")

    # 1130: the toolchain and the tests are named, not asserted.
    check("the CI run is named", good["tests"]["ci_run"] == 42)
    no_ci = build_record("v9.9.9", _release(), dict(_SUMS), _COMMIT, _TREE,
                         None, None, "2026-09-13T00:00:00Z")
    check("a record with no CI run still validates", validate(no_ci) == [],
          f"problems: {validate(no_ci)}")
    check("and its audit says the artifacts point at no tests",
          any("point at no tests" in line for line in audit(no_ci)),
          f"audit: {audit(no_ci)}")
    check("a missing intent is said out loud",
          any("no release intent" in line for line in audit(no_ci)),
          f"audit: {audit(no_ci)}")

    # Drift detection: the whole reason verify exists.
    check("an unchanged release does not drift",
          compare(good, dict(_SUMS), _release()) == [],
          f"drift: {compare(good, dict(_SUMS), _release())}")

    changed = dict(_SUMS)
    changed["krate-9.9.9-x86_64-unknown-linux-gnu.tar.gz"] = "c" * 64
    check("a replaced artifact is caught",
          any("the release now says" in d for d in compare(good, changed, _release())),
          f"drift: {compare(good, changed, _release())}")

    removed = dict(_SUMS)
    removed.pop("krate-studio-9.9.9-universal.dmg")
    check("a removed artifact is caught",
          any("no longer published" in d for d in compare(good, removed, _release())),
          f"drift: {compare(good, removed, _release())}")

    added = dict(_SUMS)
    added["krate-9.9.9-aarch64-apple-darwin.tar.gz"] = "d" * 64
    check("an added artifact is caught",
          any("not in the record" in d for d in compare(good, added, _release())),
          f"drift: {compare(good, added, _release())}")

    resized = _release(assets=[
        {"name": "krate-9.9.9-x86_64-unknown-linux-gnu.tar.gz", "size": 999},
        {"name": "krate-studio-9.9.9-universal.dmg", "size": 200},
        {"name": "SHA256SUMS", "size": 300},
    ])
    check("a resized artifact is caught",
          any("bytes" in d for d in compare(good, dict(_SUMS), resized)),
          f"drift: {compare(good, dict(_SUMS), resized)}")

    # SHA256SUMS parsing, including the shapes that would silently drop a line.
    parsed, bad = parse_sha256sums(
        f"{'a' * 64}  one.tar.gz\n{'b' * 64}  two.zip\n"
    )
    check("two good lines parse", parsed == {"one.tar.gz": "a" * 64,
                                             "two.zip": "b" * 64})
    check("and nothing is reported bad", bad == [])
    _, bad2 = parse_sha256sums("not-a-digest  one.tar.gz\n")
    check("a bad digest is reported, not dropped", len(bad2) == 1,
          f"bad: {bad2}")
    _, bad3 = parse_sha256sums(f"{'a' * 64}\n")
    check("a line with no filename is reported", len(bad3) == 1)
    parsed4, _ = parse_sha256sums(f"\n{'a' * 64}  one.tar.gz\n\n")
    check("blank lines are ignored", parsed4 == {"one.tar.gz": "a" * 64})

    # An empty artifact list is not a release.
    empty = _good()
    empty["artifacts"] = []
    empty["content_sha256"] = content_digest(empty)
    check("a record with no artifacts is refused",
          any("every published file" in p for p in validate(empty)))

    # A duplicated artifact would let one file be counted twice in the
    # inventory (1132).
    dupe = _good()
    dupe["artifacts"].append(dict(dupe["artifacts"][0]))
    dupe["content_sha256"] = content_digest(dupe)
    check("a duplicated artifact is refused",
          any("listed twice" in p for p in validate(dupe)))

    # `check` against a file on disk: the independent expectation. Written
    # with real bytes and a real record so the digest is computed, not
    # asserted.
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        payload = b"the bytes a release would publish"
        real = hashlib.sha256(payload).hexdigest()
        f = tmp / "krate-9.9.9-x86_64-unknown-linux-gnu.tar.gz"
        f.write_bytes(payload)
        check("digest_file hashes the bytes on disk", digest_file(f) == real,
              f"{digest_file(f)} != {real}")

        saved = OUT_DIR / "v9.9.9.json"
        rec = _good()
        by = {a["name"]: a for a in rec["artifacts"]}
        by[f.name]["digest"] = real
        by[f.name]["size"] = len(payload)
        rec["content_sha256"] = content_digest(rec)
        existed = saved.exists()
        backup = saved.read_bytes() if existed else None
        try:
            OUT_DIR.mkdir(parents=True, exist_ok=True)
            saved.write_text(json.dumps(rec, indent=1) + "\n")

            class A:
                tag, file, name = "v9.9.9", str(f), None

            check("a matching file is accepted", cmd_check(A()) == 0)

            f.write_bytes(payload + b"!")
            check("a file that is not the published one is refused",
                  cmd_check(A()) == 1)

            f.write_bytes(payload)

            class B(A):
                name = "not-published.zip"

            check("a name the release never published is refused",
                  cmd_check(B()) == 2)

            class C(A):
                name = "SHA256SUMS"

            check("an artifact with no digest cannot be checked",
                  cmd_check(C()) == 2)

            class D(A):
                tag = "v0.0.0-absent"

            check("no record means no pass", cmd_check(D()) == 2)
        finally:
            if backup is not None:
                saved.write_bytes(backup)
            elif saved.exists():
                saved.unlink()

    if failures:
        print("release-lineage self-test FAILED:\n")
        for f in failures:
            print(f"  - {f}")
        return 1
    print(f"release-lineage self-test OK -- {len(_SUMS) + 1} artifacts modelled, "
          "drift, digest, signer-state and inventory rules all exercised")
    return 0


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--self-test" in argv:
        return self_test()

    ap = argparse.ArgumentParser(
        description="Bind published release artifacts to what made them (IC-612)."
    )
    sub = ap.add_subparsers(dest="cmd")

    p_rec = sub.add_parser("record", help="write the lineage record for a tag")
    p_rec.add_argument("tag")
    p_rec.add_argument(
        "--intent", default=None,
        help="why this release was cut, in one sentence. Absent rather than invented.",
    )
    p_rec.set_defaults(func=cmd_record)

    p_ver = sub.add_parser("verify", help="check the record still describes the release")
    p_ver.add_argument("tag")
    p_ver.set_defaults(func=cmd_verify)

    p_chk = sub.add_parser(
        "check",
        help="check a downloaded file against the committed record",
    )
    p_chk.add_argument("tag")
    p_chk.add_argument("file", help="the downloaded file to check")
    p_chk.add_argument(
        "--name", default=None,
        help="the published name, when the local file was renamed",
    )
    p_chk.set_defaults(func=cmd_check)

    p_inv = sub.add_parser("inventory", help="list every artifact and byte")
    p_inv.add_argument("tag")
    p_inv.set_defaults(func=cmd_inventory)

    args = ap.parse_args(argv)
    if not getattr(args, "func", None):
        ap.print_help()
        return 2
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
