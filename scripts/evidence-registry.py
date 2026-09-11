#!/usr/bin/env python3
"""The evidence registry and the claim gate (IC-681, IC-682, IC-722).

    scripts/evidence-registry.py validate          # every record and claim is well formed
    scripts/evidence-registry.py state [C-ID ...]  # what the evidence actually supports
    scripts/evidence-registry.py card C-ID         # the plain-language card for one claim
    scripts/evidence-registry.py publication       # refuse an unsupported or expired material claim
    scripts/evidence-registry.py export DIR        # public-safe JSON + Markdown, no secrets, no private paths
    scripts/evidence-registry.py summary           # counts that keep their denominators
    scripts/evidence-registry.py --self-test

An evidence record says what happened: to which exact subject, in which
environment, judged by which oracle, with which outcome. A claim record says
what Krate wants to state. They are kept apart so the same evidence can
support a narrow sentence while refusing a broad one.

The gate never trusts the state a claim declares for itself. It computes the
state from the records the claim links -- including failures, skips, expired
and invalidated records -- and reports the least expansive defensible one:

    PROVED       every required platform has a valid passing record of the
                 right oracle class in an acceptable environment, and nothing
                 unsuperseded contradicts it
    OBSERVED     some of that: proved on the platforms that have it, untested
                 on the rest, and the card says which
    DESIGNED     the claim is declared as designed and no execution backs it
    UNSUPPORTED  a contradiction stands, the evidence expired or was
                 invalidated, or nothing supports it

Records are append-only. A later record supersedes an earlier interpretation
by naming it; nothing is deleted. A passing record does not erase a failure it
does not name, and a pass that needed prerequisites the failure did not have
cannot supersede that failure at all (1344, 1498). A test that returned early
is skipped, not passed (1345); an ignored test is neither, and stays visible
beside every aggregate (1346). Emulated is not native (1497).
"""
import json
import re
import sys
from datetime import date, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "evidence" / "registry"

SCHEMA = "krate.evidence-registry.v1"

RECORD_ID = re.compile(r"^E-[A-Za-z0-9][A-Za-z0-9.-]{3,}$")
CLAIM_ID = re.compile(r"^C-[A-Za-z0-9][A-Za-z0-9.-]{3,}$")
COMMIT = re.compile(r"^[0-9a-f]{7,40}$")
DIGEST = re.compile(r"^[0-9a-f]{64}$")

OUTCOMES = {"pass", "fail", "skipped", "blocked", "flaky", "incomplete", "informational"}
SUBJECT_KINDS = {
    "source tree", "binary", "archive", "installer", ".krate", "service",
    "protocol", "document", "policy",
}
ENVIRONMENT_KINDS = {"native", "hosted", "virtual", "emulated"}
ORACLE_CLASSES = {
    "execution",   # the thing ran and its behaviour was judged
    "launch",      # it started; nothing about what it did after
    "build",       # source compiled under the named conditions
    "typecheck",   # source type-checked for a target; nothing ran
    "manual",      # a person judged it, by a written procedure
    "none",        # nothing judged anything: a no-op
}
INDEPENDENCE = {
    "local reproduction", "first-party API observation",
    "third-party report", "manual declaration",
}
STATES = {"PROVED", "OBSERVED", "DESIGNED", "UNSUPPORTED"}
MATERIAL_SURFACES = {"website", "release", "investor"}
# Scope words that promise every member of a set. A sentence carrying one is
# a claim about all of them, and is PROVED only when all of them are.
ALL_WORDS = re.compile(r"\b(every|all|any|always|never|zero|universal)\b", re.IGNORECASE)
# Things that must never leave the machine in an export.
SECRET_SHAPES = re.compile(
    r"(ghp_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|"
    r"-----BEGIN [A-Z ]*PRIVATE KEY-----|xox[abp]-[A-Za-z0-9-]{10,})"
)
PRIVATE_PATH = re.compile(r"(^|[\s\"'=])(/Users/|/home/|C:\\Users\\|~/)")


# ---- loading ---------------------------------------------------------------

def load_dir(directory, id_pattern, what):
    """Every JSON file in a directory, keyed by id. Problems are returned, not
    raised: a registry with one bad file still has to report the rest."""
    items, problems = {}, []
    if not directory.is_dir():
        return items, problems
    for path in sorted(directory.glob("*.json")):
        try:
            data = json.loads(path.read_text())
        except (OSError, ValueError) as err:
            problems.append(f"{path.name}: unreadable {what}: {err}")
            continue
        ident = data.get("id")
        if not isinstance(ident, str) or not id_pattern.match(ident):
            problems.append(f"{path.name}: {what} has no stable id (got {ident!r})")
            continue
        if ident in items:
            problems.append(f"{path.name}: {what} id {ident} is already used by another file")
            continue
        if path.stem != ident:
            problems.append(f"{path.name}: file must be named after its id ({ident}.json)")
        items[ident] = data
    return items, problems


def load(registry=REGISTRY):
    records, r_problems = load_dir(registry / "records", RECORD_ID, "record")
    claims, c_problems = load_dir(registry / "claims", CLAIM_ID, "claim")
    return records, claims, r_problems + c_problems


# ---- validation ------------------------------------------------------------

def _need(data, field, kind, problems, prefix):
    value = data.get(field)
    if value is None:
        problems.append(f"{prefix}: missing {field}")
        return None
    if kind is not None and not isinstance(value, kind):
        problems.append(f"{prefix}: {field} must be {kind.__name__}, got {type(value).__name__}")
        return None
    return value


def validate_record(rec, records):
    """The rules one record must meet on its own. Returns a list of problems."""
    p = []
    ident = rec.get("id", "?")
    pre = f"record {ident}"
    if not RECORD_ID.match(str(ident)):
        p.append(f"{pre}: id is not stable (must match {RECORD_ID.pattern})")  # 1341

    claims = _need(rec, "claims", list, p, pre)
    if claims is not None and not all(isinstance(c, str) and CLAIM_ID.match(c) for c in claims):
        p.append(f"{pre}: claims must be a list of claim ids")

    subject = _need(rec, "subject", dict, p, pre) or {}
    kind = subject.get("kind")
    if kind not in SUBJECT_KINDS:
        p.append(f"{pre}: subject.kind must be one of {sorted(SUBJECT_KINDS)}, got {kind!r}")
    source = subject.get("source")
    if not isinstance(source, dict):
        p.append(f"{pre}: subject.source must name the commit and the tree separately")  # 1342
    else:
        for part in ("commit", "tree"):
            value = source.get(part)
            if not isinstance(value, str) or not COMMIT.match(value):
                p.append(f"{pre}: subject.source.{part} must be a git object id (1342)")
    if kind in {"binary", "archive", "installer", ".krate"}:
        # 1343: what was run and what it came packed in are two digests.
        for part in ("binary_digest", "package_digest"):
            value = subject.get(part)
            if not isinstance(value, str) or not DIGEST.match(value):
                p.append(f"{pre}: subject.{part} must be a sha256 for a {kind} subject (1343)")
        if subject.get("binary_digest") and subject.get("binary_digest") == subject.get("package_digest"):
            p.append(f"{pre}: binary and package digests are the same value; they identify different things (1343)")
    if not isinstance(subject.get("relationship"), str) or not subject.get("relationship").strip():
        p.append(f"{pre}: subject.relationship must say how the artifact is tied to the source")

    env = _need(rec, "environment", dict, p, pre) or {}
    if env.get("kind") not in ENVIRONMENT_KINDS:
        p.append(f"{pre}: environment.kind must be one of {sorted(ENVIRONMENT_KINDS)} (1497)")
    for part in ("os", "arch"):
        if not isinstance(env.get(part), str) or not env.get(part):
            p.append(f"{pre}: environment.{part} is required")
    if env.get("kind") in {"virtual", "emulated"}:
        for part in ("host_arch", "hypervisor"):
            if not env.get(part):
                p.append(f"{pre}: a {env['kind']} environment must name {part} (1497)")

    command = _need(rec, "command", str, p, pre)
    if command is not None and not command.strip():
        p.append(f"{pre}: command must be the exact invocation or a bounded manual procedure")

    oracle = _need(rec, "oracle", dict, p, pre) or {}
    if oracle.get("class") not in ORACLE_CLASSES:
        p.append(f"{pre}: oracle.class must be one of {sorted(ORACLE_CLASSES)}")
    if not isinstance(oracle.get("description"), str) or not oracle.get("description").strip():
        p.append(f"{pre}: oracle.description must say what decides pass or fail, and why that detects the behaviour")

    outcome = rec.get("outcome")
    if outcome not in OUTCOMES:
        p.append(f"{pre}: outcome must be one of {sorted(OUTCOMES)}, got {outcome!r}")
    raw = rec.get("raw_result")
    if not isinstance(raw, dict):
        p.append(f"{pre}: raw_result must be an object (exit status, counts, where the material is)")
        raw = {}
    # 1345: a test that returned early, or was judged by nothing, did not pass.
    if outcome == "pass" and (raw.get("skipped") is True or oracle.get("class") == "none"):
        p.append(f"{pre}: a no-op is skipped, not passed -- an early return or an oracle of none cannot record pass (1345)")
    if outcome == "pass" and oracle.get("class") == "launch" and raw.get("killed_after_timeout"):
        p.append(f"{pre}: a launch killed at a timeout is not a pass; it is informational until an oracle judged the app")

    scope = _need(rec, "scope", dict, p, pre) or {}
    if not isinstance(scope.get("supports"), str) or not scope.get("supports").strip():
        p.append(f"{pre}: scope.supports must state the narrow claim this record supports")
    if not isinstance(scope.get("exclusions"), list):
        p.append(f"{pre}: scope.exclusions must be a list (empty is allowed, absent is not)")

    if rec.get("independence") not in INDEPENDENCE:
        p.append(f"{pre}: independence must be one of {sorted(INDEPENDENCE)}")

    time = _need(rec, "time", dict, p, pre) or {}
    for part in ("start", "finish"):
        if not _is_iso(time.get(part)):
            p.append(f"{pre}: time.{part} must be an ISO-8601 timestamp")

    if not isinstance(rec.get("operator"), str) or not rec.get("operator"):
        p.append(f"{pre}: operator must name the person or the controlled automation")

    privacy = _need(rec, "privacy", dict, p, pre) or {}
    if privacy.get("audience") not in {"public", "internal"}:
        p.append(f"{pre}: privacy.audience must be public or internal")
    if not isinstance(privacy.get("redact"), list):
        p.append(f"{pre}: privacy.redact must list the fields an export replaces (empty is allowed)")

    expiry = _need(rec, "expiry", dict, p, pre) or {}
    triggers = expiry.get("triggers")
    if not isinstance(triggers, list) or not triggers:
        p.append(f"{pre}: expiry.triggers must name at least one thing that makes this record stale")
    if expiry.get("on") is not None and not _is_iso_date(expiry.get("on")):
        p.append(f"{pre}: expiry.on must be a date (YYYY-MM-DD)")

    for link in ("supersedes", "invalidated_by"):
        target = rec.get(link)
        if target is not None:
            if not isinstance(target, str) or not RECORD_ID.match(target):
                p.append(f"{pre}: {link} must be a record id")
            elif target not in records:
                p.append(f"{pre}: {link} names {target}, which is not in the registry -- nothing may be deleted")
            elif target == ident:
                p.append(f"{pre}: {link} cannot point at itself")
    if rec.get("invalidated_by") and not rec.get("invalidation_reason"):
        p.append(f"{pre}: an invalidated record must keep the reason (broken oracle, wrong subject, corrupted procedure)")
    return p


def validate_claim(claim, records):
    p = []
    ident = claim.get("id", "?")
    pre = f"claim {ident}"
    sentence = _need(claim, "sentence", str, p, pre)
    if sentence is not None and not sentence.strip():
        p.append(f"{pre}: sentence is empty")
    surfaces = _need(claim, "surfaces", list, p, pre) or []
    if not surfaces:
        p.append(f"{pre}: surfaces must name where this sentence is meant to appear")
    subject = _need(claim, "subject", dict, p, pre) or {}
    if not isinstance(subject.get("commit"), str) or not COMMIT.match(subject.get("commit", "")):
        p.append(f"{pre}: subject.commit must bind the sentence to a commit")

    req = _need(claim, "requires", dict, p, pre) or {}
    classes = req.get("evidence_classes")
    if not isinstance(classes, list) or not classes or not set(classes) <= ORACLE_CLASSES - {"none"}:
        p.append(f"{pre}: requires.evidence_classes must list oracle classes that can support it")
    platforms = req.get("platforms")
    if not isinstance(platforms, list) or not platforms or not all(isinstance(x, str) for x in platforms):
        p.append(f"{pre}: requires.platforms must list every platform the sentence is about")
    if req.get("environment") not in {"native", "any"}:
        p.append(f"{pre}: requires.environment must be native or any (1497)")
    if not isinstance(req.get("same_bytes"), bool):
        p.append(f"{pre}: requires.same_bytes must say whether every platform must have tested identical bytes (1499)")

    linked = _need(claim, "evidence", list, p, pre) or []
    for rid in linked:
        if rid not in records:
            p.append(f"{pre}: links {rid}, which is not in the registry")
    declared = claim.get("declared_state")
    if declared not in STATES:
        p.append(f"{pre}: declared_state must be one of {sorted(STATES)} -- and the gate will check it")

    if not isinstance(claim.get("exclusions"), list):
        p.append(f"{pre}: exclusions must be a list (what the sentence does not say)")
    if not isinstance(claim.get("owner"), str) or not claim.get("owner"):
        p.append(f"{pre}: owner is required")
    expiry = _need(claim, "expiry", dict, p, pre) or {}
    if not isinstance(expiry.get("triggers"), list) or not expiry.get("triggers"):
        p.append(f"{pre}: expiry.triggers must name what forces a recheck")

    # 1420: a correction keeps the earlier wording, evidence and reason.
    revision = claim.get("revision")
    if not isinstance(revision, int) or revision < 1:
        p.append(f"{pre}: revision must be a positive integer")
    else:
        history = claim.get("history")
        if not isinstance(history, list):
            p.append(f"{pre}: history must be a list (empty for revision 1)")
        elif len(history) != revision - 1:
            p.append(f"{pre}: revision {revision} needs {revision - 1} history entries, found {len(history)} -- a correction must keep the earlier wording (1420)")
        else:
            for n, entry in enumerate(history, 1):
                for part in ("wording", "evidence", "reason", "at"):
                    if part not in entry:
                        p.append(f"{pre}: history entry {n} lacks {part} (1420)")
    return p


def validate(records, claims, load_problems):
    problems = list(load_problems)
    for rec in records.values():
        problems.extend(validate_record(rec, records))
    for claim in claims.values():
        problems.extend(validate_claim(claim, records))
    return problems


# ---- the gate ---------------------------------------------------------------

def lifecycle(rec, claim, today):
    """INVALIDATED, EXPIRED or None. A record past either is not evidence for
    anything, and stays in the registry saying so (1347, 1348)."""
    if rec.get("invalidated_by"):
        return "INVALIDATED"
    expiry = rec.get("expiry") or {}
    on = expiry.get("on")
    if on and _is_iso_date(on) and date.fromisoformat(on) < today:
        return "EXPIRED"
    triggers = set(expiry.get("triggers") or [])
    if "source-change" in triggers:
        recorded = ((rec.get("subject") or {}).get("source") or {}).get("commit", "")
        wanted = (claim.get("subject") or {}).get("commit", "")
        if recorded and wanted and not _same_commit(recorded, wanted):
            return "EXPIRED"
    return None


def platform_of(rec):
    env = rec.get("environment") or {}
    return f"{env.get('os', '?')}-{env.get('arch', '?')}"


def assess(claim, records, today=None):
    """Compute what the linked evidence supports. Returns a dict with the
    state, the platforms proved, the reasons, and the plain-language card."""
    today = today or date.today()
    req = claim.get("requires") or {}
    wanted_classes = set(req.get("evidence_classes") or [])
    wanted_platforms = list(req.get("platforms") or [])
    want_native = req.get("environment") == "native"
    same_bytes = bool(req.get("same_bytes"))

    supporting = {}      # platform -> record id
    reasons = []
    contradictions = []
    superseded = {r.get("supersedes") for r in records.values() if r.get("supersedes")}

    linked = [records[rid] for rid in claim.get("evidence") or [] if rid in records]
    for rec in linked:
        rid = rec["id"]
        state = lifecycle(rec, claim, today)
        if state:
            reasons.append(f"{rid}: {state.lower()} -- not evidence any more" + (
                f" ({rec.get('invalidation_reason')})" if state == "INVALIDATED" else ""))
            continue
        outcome = rec.get("outcome")
        if outcome == "fail":
            if rid in superseded:
                # 1498: only a pass with no extra prerequisites may supersede.
                by = [r for r in records.values() if r.get("supersedes") == rid]
                clean = [r for r in by if not ((r.get("inputs") or {}).get("prerequisites"))
                         and r.get("outcome") == "pass"]
                if clean:
                    reasons.append(f"{rid}: failed, superseded by {clean[0]['id']}")
                    continue
                reasons.append(f"{rid}: failed; the later pass needed prerequisites the failure did not have, so it does not supersede it (1498)")
            contradictions.append(rid)
            continue
        if outcome in {"skipped", "blocked", "incomplete", "informational", "flaky"}:
            reasons.append(f"{rid}: {outcome} -- supports nothing (1345, 1346)")
            continue
        if outcome != "pass":
            continue
        oracle = (rec.get("oracle") or {}).get("class")
        if oracle not in wanted_classes:
            reasons.append(f"{rid}: oracle class {oracle} cannot satisfy a claim that needs {sorted(wanted_classes)} (1350)")
            continue
        env_kind = (rec.get("environment") or {}).get("kind")
        if want_native and env_kind != "native":
            reasons.append(f"{rid}: {env_kind} evidence is not native evidence and is not promoted to it (1497)")
            continue
        platform = platform_of(rec)
        if platform not in wanted_platforms:
            reasons.append(f"{rid}: platform {platform} is not one the claim is about")
            continue
        supporting.setdefault(platform, rid)

    if same_bytes and len(supporting) > 1:
        digests = {(records[rid].get("subject") or {}).get("package_digest") for rid in supporting.values()}
        if len(digests) > 1:
            reasons.append("the platforms were tested with different bytes; a same-file claim needs one package digest across all of them (1499)")
            supporting = {}

    missing = [p for p in wanted_platforms if p not in supporting]
    all_words = bool(ALL_WORDS.search(claim.get("sentence", "")))

    if contradictions:
        state = "UNSUPPORTED"
        why = "a failure stands against it: " + ", ".join(contradictions)
    elif not wanted_platforms:
        state = "UNSUPPORTED"
        why = "the claim names no platform"
    elif not missing:
        state = "PROVED"
        why = "every required platform has valid passing evidence"
    elif supporting:
        state = "OBSERVED"
        why = f"proved on {', '.join(sorted(supporting))}; untested on {', '.join(missing)}"
        if all_words:
            why += " -- and the sentence promises all of them, so it may not be published as written (1349, 1499)"
    elif claim.get("declared_state") == "DESIGNED":
        state = "DESIGNED"
        why = "declared as designed; nothing has run"
    else:
        state = "UNSUPPORTED"
        why = "no valid supporting evidence"

    publishable = state == "PROVED" or (state == "OBSERVED" and not all_words)
    return {
        "id": claim["id"],
        "state": state,
        "why": why,
        "supporting": supporting,
        "missing": missing,
        "contradictions": contradictions,
        "reasons": reasons,
        "publishable": publishable,
    }


def card(claim, verdict):
    """1351: the plain-language card. What is claimed, what it rests on, what
    it leaves out, and what is missing -- in sentences, not field names."""
    lines = [
        f"# {claim['id']}",
        "",
        f"Claim: {claim.get('sentence', '')}",
        f"State: {verdict['state']} -- {verdict['why']}.",
        "",
    ]
    if verdict["supporting"]:
        lines.append("Rests on:")
        for platform, rid in sorted(verdict["supporting"].items()):
            lines.append(f"  - {platform}: {rid}")
    if verdict["missing"]:
        lines.append("Not tested: " + ", ".join(verdict["missing"]) + ".")
    if claim.get("exclusions"):
        lines.append("Says nothing about: " + "; ".join(claim["exclusions"]) + ".")
    if verdict["contradictions"]:
        lines.append("Contradicted by: " + ", ".join(verdict["contradictions"]) + ".")
    if verdict["reasons"]:
        lines.append("")
        lines.append("Set aside:")
        for reason in verdict["reasons"]:
            lines.append(f"  - {reason}")
    lines.append("")
    lines.append("Publishable as written: " + ("yes" if verdict["publishable"] else "no") + ".")
    return "\n".join(lines)


def publication_gate(claims, records, today=None):
    """1419: a build for a material surface refuses a claim the evidence does
    not carry -- unsupported, expired, contradicted, or promising every
    platform while some are untested."""
    refusals = []
    for claim in claims.values():
        if not MATERIAL_SURFACES & set(claim.get("surfaces") or []):
            continue
        verdict = assess(claim, records, today)
        if not verdict["publishable"]:
            refusals.append(f"{claim['id']} ({verdict['state']}): {verdict['why']}")
    return refusals


def summary(records):
    """1346: counts that keep their denominators. Skipped and ignored are
    shown beside passes, never folded into them."""
    counts = {k: 0 for k in sorted(OUTCOMES)}
    for rec in records.values():
        counts[rec.get("outcome", "informational")] = counts.get(rec.get("outcome", "informational"), 0) + 1
    ignored = sum(int((r.get("raw_result") or {}).get("ignored", 0) or 0) for r in records.values())
    lines = [f"{len(records)} records:"]
    for outcome, n in counts.items():
        lines.append(f"  {outcome:<14}{n:>5}")
    lines.append(f"  ignored cases inside them: {ignored} (neither passed nor failed; they did not run)")
    return "\n".join(lines)


# ---- export -----------------------------------------------------------------

def public_copy(rec):
    """1352: the record with its private parts replaced, not removed --
    a redaction is recorded, never disguised as complete raw evidence."""
    out = json.loads(json.dumps(rec))
    privacy = out.get("privacy") or {}
    for field in privacy.get("redact") or []:
        node, key = _descend(out, field)
        if node is not None and key in node:
            node[key] = f"[redacted: {field} is not for this audience]"
    out.setdefault("privacy", {})["redacted_fields"] = list(privacy.get("redact") or [])
    return out


def export(records, claims, out_dir, today=None):
    """Public-safe JSON and Markdown. Refuses to write anything that still
    looks like a secret or a private path after redaction."""
    out_dir = Path(out_dir)
    public_records, withheld = {}, []
    for rid, rec in records.items():
        if (rec.get("privacy") or {}).get("audience") != "public":
            withheld.append(rid)
            continue
        public_records[rid] = public_copy(rec)
    problems = []
    for rid, rec in public_records.items():
        text = json.dumps(rec)
        if SECRET_SHAPES.search(text):
            problems.append(f"{rid}: still carries something shaped like a secret after redaction")
        if PRIVATE_PATH.search(text):
            problems.append(f"{rid}: still carries a private path after redaction; list it under privacy.redact")
    if problems:
        return problems
    out_dir.mkdir(parents=True, exist_ok=True)
    verdicts = {cid: assess(claim, records, today) for cid, claim in claims.items()}
    (out_dir / "registry.json").write_text(json.dumps({
        "schema": SCHEMA,
        "records": public_records,
        "withheld_record_ids": withheld,
        "claims": {cid: {**claim, "computed": verdicts[cid]} for cid, claim in claims.items()},
    }, indent=1, sort_keys=True) + "\n")
    md = ["# Evidence registry", "", summary(records), ""]
    if withheld:
        md.append(f"Withheld from this export (internal audience): {', '.join(withheld)}")
        md.append("")
    for cid in sorted(claims):
        md.append(card(claims[cid], verdicts[cid]))
        md.append("")
    (out_dir / "registry.md").write_text("\n".join(md))
    return []


# ---- helpers ----------------------------------------------------------------

def _descend(obj, dotted):
    parts = dotted.split(".")
    node = obj
    for part in parts[:-1]:
        if not isinstance(node, dict) or part not in node:
            return None, None
        node = node[part]
    return (node, parts[-1]) if isinstance(node, dict) else (None, None)


def _same_commit(a, b):
    return a == b or a.startswith(b) or b.startswith(a)


def _is_iso(value):
    if not isinstance(value, str):
        return False
    try:
        datetime.fromisoformat(value.replace("Z", "+00:00"))
        return True
    except ValueError:
        return False


def _is_iso_date(value):
    if not isinstance(value, str):
        return False
    try:
        date.fromisoformat(value)
        return True
    except ValueError:
        return False


# ---- self-test ---------------------------------------------------------------

def _record(**over):
    base = {
        "id": "E-TEST-0001",
        "claims": ["C-TEST-ALPHA"],
        "subject": {
            "kind": "source tree",
            "source": {"commit": "496cc9a15", "tree": "deadbeefcafe"},
            "relationship": "built from this commit in the test",
        },
        "environment": {"kind": "native", "os": "macos", "arch": "arm64"},
        "toolchain": {"rustc": "1.94.1"},
        "command": "cargo test -p krate-bundle",
        "inputs": {},
        "oracle": {"class": "execution", "description": "assertions on behaviour"},
        "raw_result": {"exit": 0},
        "outcome": "pass",
        "scope": {"supports": "the bundle tests pass on this Mac", "exclusions": []},
        "independence": "local reproduction",
        "time": {"start": "2026-09-11T10:00:00Z", "finish": "2026-09-11T10:01:00Z"},
        "operator": "ci",
        "privacy": {"audience": "public", "redact": []},
        "expiry": {"triggers": ["source-change"], "on": None},
        "supersedes": None,
        "invalidated_by": None,
    }
    base.update(over)
    return base


def _claim(**over):
    base = {
        "id": "C-TEST-ALPHA",
        "sentence": "The bundle tests pass on macOS.",
        "audience": "developers",
        "surfaces": ["website"],
        "subject": {"commit": "496cc9a15"},
        "requires": {"evidence_classes": ["execution"], "platforms": ["macos-arm64"],
                     "environment": "any", "same_bytes": False},
        "evidence": ["E-TEST-0001"],
        "declared_state": "PROVED",
        "exclusions": ["other operating systems"],
        "owner": "lead",
        "approval": None,
        "expiry": {"triggers": ["source-change"]},
        "revision": 1,
        "history": [],
    }
    base.update(over)
    return base


def self_test():
    failures = []
    today = date(2026, 9, 11)

    def expect(cond, what):
        if not cond:
            failures.append(what)

    # A well-formed record and claim pass, and the gate says PROVED.
    rec = _record()
    recs = {rec["id"]: rec}
    claim = _claim()
    expect(validate_record(rec, recs) == [], f"a good record must validate: {validate_record(rec, recs)}")
    expect(validate_claim(claim, recs) == [], f"a good claim must validate: {validate_claim(claim, recs)}")
    v = assess(claim, recs, today)
    expect(v["state"] == "PROVED" and v["publishable"], f"good evidence proves the claim: {v}")

    # 1341: no stable id.
    bad = _record(id="note-3")
    expect(any("stable" in p for p in validate_record(bad, {"note-3": bad})), "1341: an unstable id must be rejected")
    # 1342: commit and tree separately.
    bad = _record(subject={"kind": "source tree", "source": {"commit": "496cc9a15"}, "relationship": "x"})
    expect(any("tree" in p for p in validate_record(bad, {bad["id"]: bad})), "1342: a record without a tree id must be rejected")
    # 1343: binary and package digests separately.
    bad = _record(subject={"kind": ".krate", "source": {"commit": "496cc9a15", "tree": "cafe1234"},
                           "relationship": "x", "binary_digest": "a" * 64, "package_digest": "a" * 64})
    expect(any("1343" in p for p in validate_record(bad, {bad["id"]: bad})), "1343: one digest for both must be rejected")
    bad = _record(subject={"kind": ".krate", "source": {"commit": "496cc9a15", "tree": "cafe1234"}, "relationship": "x"})
    expect(sum("1343" in p for p in validate_record(bad, {bad["id"]: bad})) == 2, "1343: both digests are required for a .krate subject")

    # 1344: a failure stands after a later pass that does not name it.
    fail = _record(id="E-TEST-FAIL", outcome="fail", raw_result={"exit": 101})
    later = _record(id="E-TEST-LATER", time={"start": "2026-09-12T10:00:00Z", "finish": "2026-09-12T10:01:00Z"})
    recs2 = {r["id"]: r for r in (fail, later)}
    v = assess(_claim(evidence=["E-TEST-FAIL", "E-TEST-LATER"]), recs2, today)
    expect(v["state"] == "UNSUPPORTED" and "E-TEST-FAIL" in v["contradictions"], f"1344: a later pass must not erase a failure: {v}")
    # ...and a pass that names it, with no extra prerequisites, does supersede.
    later_named = _record(id="E-TEST-LATER", supersedes="E-TEST-FAIL")
    recs3 = {r["id"]: r for r in (fail, later_named)}
    v = assess(_claim(evidence=["E-TEST-FAIL", "E-TEST-LATER"]), recs3, today)
    expect(v["state"] == "PROVED", f"a clean superseding pass resolves the failure: {v}")
    # 1498: a pass that needed prerequisites does not supersede a clean failure.
    helped = _record(id="E-TEST-LATER", supersedes="E-TEST-FAIL", inputs={"prerequisites": ["apt install libasound2"]})
    recs4 = {r["id"]: r for r in (fail, helped)}
    v = assess(_claim(evidence=["E-TEST-FAIL", "E-TEST-LATER"]), recs4, today)
    expect(v["state"] == "UNSUPPORTED" and any("1498" in r for r in v["reasons"]), f"1498: a prerequisite-assisted pass keeps the clean failure: {v}")

    # 1345: a no-op cannot record pass.
    noop = _record(raw_result={"exit": 0, "skipped": True})
    expect(any("1345" in p for p in validate_record(noop, {noop["id"]: noop})), "1345: an early return recorded as pass must be rejected")
    unjudged = _record(oracle={"class": "none", "description": "the command ended"})
    expect(any("1345" in p for p in validate_record(unjudged, {unjudged["id"]: unjudged})), "1345: an oracle of none cannot pass")
    skipped = _record(outcome="skipped", raw_result={"skipped": True})
    expect(validate_record(skipped, {skipped["id"]: skipped}) == [], "a skip recorded as skipped is fine")
    v = assess(_claim(), {skipped["id"]: skipped}, today)
    expect(v["state"] == "UNSUPPORTED", f"1345: a skip supports nothing: {v}")

    # 1346: ignored cases stay visible beside the aggregate.
    with_ignored = _record(raw_result={"exit": 0, "ignored": 17})
    text = summary({with_ignored["id"]: with_ignored})
    expect("ignored cases inside them: 17" in text and "pass" in text, f"1346: the summary must show ignored beside passes: {text}")

    # 1347: expired evidence downgrades the claim; the record stays.
    stale = _record(expiry={"triggers": ["time"], "on": "2026-09-01"})
    v = assess(_claim(), {stale["id"]: stale}, today)
    expect(v["state"] == "UNSUPPORTED" and any("expired" in r for r in v["reasons"]), f"1347: expired evidence must downgrade: {v}")
    moved = _record()  # same record, but the claim now names a later commit
    v = assess(_claim(subject={"commit": "f33217251"}), {moved["id"]: moved}, today)
    expect(v["state"] == "UNSUPPORTED" and any("expired" in r for r in v["reasons"]), f"1347: a source change expires source-bound evidence: {v}")

    # 1348: invalidated by a broken oracle, raw evidence kept.
    audit = _record(id="E-TEST-AUDIT", outcome="informational", oracle={"class": "manual", "description": "reviewed the oracle"})
    broken = _record(invalidated_by="E-TEST-AUDIT", invalidation_reason="the oracle only checked that the process ended")
    recs5 = {r["id"]: r for r in (audit, broken)}
    expect(validate_record(broken, recs5) == [], f"an invalidated record with its reason validates: {validate_record(broken, recs5)}")
    v = assess(_claim(), recs5, today)
    expect(v["state"] == "UNSUPPORTED" and any("invalidated" in r for r in v["reasons"]), f"1348: invalidated evidence supports nothing: {v}")
    expect("E-TEST-0001" in recs5, "1348: the raw record is not deleted")
    no_reason = _record(invalidated_by="E-TEST-AUDIT")
    expect(any("reason" in p for p in validate_record(no_reason, recs5)), "1348: invalidation without a reason must be rejected")

    # 1349 / 1499: one platform cannot satisfy an all-platform claim.
    three = _claim(sentence="Krate's tests pass on every desktop OS.",
                   requires={"evidence_classes": ["execution"], "platforms": ["macos-arm64", "ubuntu-x86_64", "windows-x86_64"],
                             "environment": "any", "same_bytes": False})
    v = assess(three, recs, today)
    expect(v["state"] == "OBSERVED" and not v["publishable"] and v["missing"] == ["ubuntu-x86_64", "windows-x86_64"],
           f"1349: one platform is OBSERVED, not PROVED, and 'every' may not publish: {v}")
    expect("untested on ubuntu-x86_64, windows-x86_64" in v["why"], f"the card must name the untested platforms: {v['why']}")
    refusals = publication_gate({three["id"]: three}, recs, today)
    expect(len(refusals) == 1, f"1419/1499: publication must refuse the every-OS sentence: {refusals}")

    # 1350: a typecheck cannot satisfy an execution claim.
    typed = _record(oracle={"class": "typecheck", "description": "cargo check --target"})
    v = assess(_claim(), {typed["id"]: typed}, today)
    expect(v["state"] == "UNSUPPORTED" and any("1350" in r for r in v["reasons"]), f"1350: a typecheck is not execution: {v}")

    # 1351: the card is sentences.
    text = card(three, assess(three, recs, today))
    expect("Not tested: ubuntu-x86_64, windows-x86_64." in text and "Says nothing about: other operating systems." in text,
           f"1351: the card must render scope and exclusions in words: {text}")

    # 1352: export replaces private material and refuses what slips through.
    import tempfile
    private = _record(raw_result={"exit": 0, "location": "/Users/someone/runs/1.log"},
                      privacy={"audience": "public", "redact": ["raw_result.location"]})
    internal = _record(id="E-TEST-INTERNAL", privacy={"audience": "internal", "redact": []})
    with tempfile.TemporaryDirectory() as tmp:
        probs = export({private["id"]: private, internal["id"]: internal}, {claim["id"]: claim}, tmp, today)
        expect(probs == [], f"1352: a redacted export must succeed: {probs}")
        data = json.loads((Path(tmp) / "registry.json").read_text())
        expect("redacted" in data["records"]["E-TEST-0001"]["raw_result"]["location"], "1352: the private path must be replaced, and say so")
        expect(data["withheld_record_ids"] == ["E-TEST-INTERNAL"], "1352: internal records are withheld and listed")
        expect("E-TEST-INTERNAL" not in data["records"], "1352: an internal record must not be exported")
        leaky = _record(raw_result={"exit": 0, "token": "ghp_" + "a" * 30})
        probs = export({leaky["id"]: leaky}, {}, tmp, today)
        expect(probs and "secret" in probs[0], f"1352: a secret shape must refuse the export: {probs}")
        unlisted = _record(raw_result={"exit": 0, "location": "/home/runner/x.log"})
        probs = export({unlisted["id"]: unlisted}, {}, tmp, today)
        expect(probs and "private path" in probs[0], f"1352: an unredacted private path must refuse the export: {probs}")

    # 1419 / 1420: a corrected claim keeps its history; a revision without it is refused.
    corrected = _claim(revision=2, history=[{"wording": "The tests pass everywhere.", "evidence": ["E-TEST-0001"],
                                             "reason": "only macOS was tested", "at": "2026-09-11"}])
    expect(validate_claim(corrected, recs) == [], f"1420: a correction with history validates: {validate_claim(corrected, recs)}")
    silent = _claim(revision=2, history=[])
    expect(any("1420" in p for p in validate_claim(silent, recs)), "1420: a revision without its history must be rejected")

    # 1497: emulated is not native, and is not promoted.
    emulated = _record(environment={"kind": "emulated", "os": "ubuntu", "arch": "x86_64", "host_arch": "arm64", "hypervisor": "qemu"})
    native_only = _claim(requires={"evidence_classes": ["execution"], "platforms": ["ubuntu-x86_64"], "environment": "native", "same_bytes": False})
    v = assess(native_only, {emulated["id"]: emulated}, today)
    expect(v["state"] == "UNSUPPORTED" and any("1497" in r for r in v["reasons"]), f"1497: emulated evidence must not become native: {v}")
    any_env = _claim(requires={"evidence_classes": ["execution"], "platforms": ["ubuntu-x86_64"], "environment": "any", "same_bytes": False})
    v = assess(any_env, {emulated["id"]: emulated}, today)
    expect(v["state"] == "PROVED", f"emulated evidence proves a claim that accepts it: {v}")
    unlabeled = _record(environment={"kind": "emulated", "os": "ubuntu", "arch": "x86_64"})
    expect(sum("1497" in p for p in validate_record(unlabeled, {unlabeled["id"]: unlabeled})) == 2,
           "1497: an emulated record must name the host architecture and the emulator")

    # 1499: same bytes across platforms.
    mac = _record(id="E-TEST-MAC", subject={"kind": ".krate", "source": {"commit": "496cc9a15", "tree": "cafe1234"},
                                            "relationship": "x", "binary_digest": "a" * 64, "package_digest": "b" * 64})
    win = _record(id="E-TEST-WIN", environment={"kind": "hosted", "os": "windows", "arch": "x86_64"},
                  subject={"kind": ".krate", "source": {"commit": "496cc9a15", "tree": "cafe1234"},
                           "relationship": "x", "binary_digest": "a" * 64, "package_digest": "c" * 64})
    both = _claim(sentence="One file opens on both.", evidence=["E-TEST-MAC", "E-TEST-WIN"],
                  requires={"evidence_classes": ["execution"], "platforms": ["macos-arm64", "windows-x86_64"],
                            "environment": "any", "same_bytes": True})
    v = assess(both, {mac["id"]: mac, win["id"]: win}, today)
    expect(v["state"] == "UNSUPPORTED" and any("1499" in r for r in v["reasons"]), f"1499: different bytes cannot prove a same-file claim: {v}")

    # 1500: the export needs no account and no service -- the script talks to
    # nothing but the filesystem.
    source = Path(__file__).read_text()
    network_imports = re.search(r"^\s*(import|from)\s+(urllib|requests|http|socket|ssl)\b", source, re.MULTILINE)
    expect(network_imports is None, "1500: the registry must not reach for the network")

    if failures:
        print("evidence-registry self-test FAILED:\n")
        for f in failures:
            print(f"  {f}")
        return 1
    print("OK -- records need a stable id, a source commit AND tree, separate binary")
    print("and package digests, a named oracle and environment kind; a no-op cannot")
    print("pass; a failure stands until a clean pass names it; expired, invalidated,")
    print("wrong-oracle, wrong-environment and one-platform evidence cannot prove a")
    print("claim; corrections keep their history; exports redact and refuse secrets.")
    return 0


# ---- main --------------------------------------------------------------------

def main(argv):
    if "--self-test" in argv:
        return self_test()
    if not argv:
        print(__doc__.strip().splitlines()[0])
        print("usage: evidence-registry.py validate|state|card|publication|export|summary|--self-test")
        return 2
    command, args = argv[0], argv[1:]
    records, claims, load_problems = load()

    if command == "validate":
        problems = validate(records, claims, load_problems)
        if problems:
            print("evidence registry: NOT VALID")
            for p in problems:
                print(f"  - {p}")
            return 1
        print(f"evidence registry: {len(records)} record(s), {len(claims)} claim(s), all well formed")
        return 0

    problems = validate(records, claims, load_problems)
    if problems:
        print("evidence registry: NOT VALID -- run `validate` first", file=sys.stderr)
        return 1

    if command == "state":
        wanted = args or sorted(claims)
        for cid in wanted:
            if cid not in claims:
                print(f"{cid}: no such claim")
                return 1
            v = assess(claims[cid], records)
            print(f"{cid}: {v['state']} -- {v['why']}")
        return 0
    if command == "card":
        if len(args) != 1 or args[0] not in claims:
            print("card needs one claim id")
            return 2
        print(card(claims[args[0]], assess(claims[args[0]], records)))
        return 0
    if command == "publication":
        refusals = publication_gate(claims, records)
        if refusals:
            print("publication: REFUSED -- a material surface would carry a claim the evidence does not:")
            for r in refusals:
                print(f"  - {r}")
            return 1
        print("publication: every material claim is carried by its evidence")
        return 0
    if command == "export":
        if len(args) != 1:
            print("export needs an output directory")
            return 2
        problems = export(records, claims, args[0])
        if problems:
            print("export: REFUSED")
            for p in problems:
                print(f"  - {p}")
            return 1
        print(f"exported {len(records)} record(s) and {len(claims)} claim(s) to {args[0]}")
        return 0
    if command == "summary":
        print(summary(records))
        return 0
    print(f"unknown command {command}")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
