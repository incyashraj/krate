#!/usr/bin/env bash
# Prove the kit headlessly: launches no app, captures no screen, posts no
# input, changes no setting. Mounting the Obsidian DMG read-only and hidden
# is the one thing it does outside a temp folder, and only to read it.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

tmp=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/obsidian-kit-selftest.XXXXXX")
trap '/bin/rm -rf "$tmp"' EXIT
ok() { printf 'ok    %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; exit 1; }

for s in "$ROOT"/scripts/*.sh; do /bin/bash -n "$s"; done
PYTHONPYCACHEPREFIX="$tmp/pycache" python3 -m py_compile "$ROOT"/scripts/*.py
ok "every script parses"

python3 "$REPO/scripts/make-bench-vault.py" --self-test >"$tmp/gen-selftest.txt" || { cat "$tmp/gen-selftest.txt"; fail "vault generator self-test"; }
python3 "$REPO/scripts/make-bench-vault.py" --out "$tmp/vaults" --all >"$tmp/gen.txt"
python3 - "$tmp/vaults" "$ROOT/scripts" <<'PY' || fail "generated vaults differ from the README table"
import json, sys
from pathlib import Path
sys.path.insert(0, sys.argv[2])
from audit import TREES
for name, digest in TREES.items():
    got = json.loads((Path(sys.argv[1]) / f"{name}.manifest.json").read_text())["tree_sha256"]
    assert got == digest, f"{name}: {got} != {digest}"
PY
ok "vaults generated: light 101, medium 1,001, heavy 10,001 files and the three big notes, every tree_sha256 as published"

python3 "$ROOT/scripts/note-phrases.py" --self-test
python3 "$ROOT/scripts/note-phrases.py" --vaults "$tmp/vaults" --out "$tmp/targets" >"$tmp/targets.txt"
for load in light medium heavy big-50000 big-200000; do
  [[ -s "$tmp/targets/$load.open.tsv" && -s "$tmp/targets/$load.json" ]] || fail "no targets for $load"
done
ok "targets and phrases for every load ($(/usr/bin/awk -F'\t' '{printf "%s %s; ", $1, $3}' "$tmp/targets.txt"))"
python3 "$ROOT/scripts/obsidian-profile.py" --self-test
python3 "$ROOT/scripts/analyze.py" --self-test
python3 "$ROOT/scripts/seal.py" --self-test
python3 "$ROOT/scripts/audit.py" --self-test

for tool in start_probe tree_sample interact; do
  /usr/bin/swiftc -O -parse-as-library "$ROOT/scripts/swift/common.swift" "$ROOT/scripts/swift/$tool.swift" \
    -o "$tmp/${tool//_/-}" 2>"$tmp/$tool.build.txt" || { cat "$tmp/$tool.build.txt"; fail "swiftc $tool"; }
done
ok "Swift instruments compile: start-probe, tree-sample, interact"
"$tmp/start-probe" --self-test
"$tmp/tree-sample" --self-test
"$tmp/interact" --self-test
if "$tmp/start-probe" --timeout 2 --terminate --child-log "$tmp/true.log" -- /usr/bin/true >"$tmp/true.json" 2>/dev/null; then
  fail "start-probe accepted a command that never showed a window"
fi
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["timed_out"] is True and "window_ms" not in d' "$tmp/true.json" \
  || fail "start-probe's refusal of a windowless command is not recorded"
ok "start-probe refuses a command that exits without a window"

# A complete synthetic run through record.py, analyze, seal and audit, and
# tampered copies of it that must each fail for their own reason.
run="$tmp/runs/20261123T090000Z"
python3 "$ROOT/scripts/synth-run.py" "$run" >/dev/null
python3 "$ROOT/scripts/analyze.py" "$run" >/dev/null
python3 "$ROOT/scripts/seal.py" "$run" >/dev/null
BENCH_SELF_TEST=1 python3 "$ROOT/scripts/audit.py" "$run" >"$tmp/audit-clean.txt" || { cat "$tmp/audit-clean.txt"; fail "the clean synthetic run must pass"; }
ok "synthetic run: $(/usr/bin/grep -c . "$run/raw/startup.tsv") start rows and every other leg, analysed, sealed ($(/usr/bin/grep -vc '^#' "$run/seal.sha256") files), audit PASS"
python3 "$ROOT/scripts/audit.py" "$run" >"$tmp/audit-nonself.txt" && fail "a synthetic run passed outside the self-test"
/usr/bin/grep -q "synthetic run" "$tmp/audit-nonself.txt" || fail "the synthetic refusal is not named"
ok "the same synthetic run FAILS outside the self-test (never mistaken for a measurement)"

tamper() {
  local name=$1 expect=$2 reseal=$3 script=$4
  local copy="$tmp/tampered/$name/20261123T090000Z"
  /bin/mkdir -p "$(dirname "$copy")"
  /bin/cp -R "$run" "$copy"
  python3 - "$copy" <<<"$script"
  if [[ "$reseal" == reseal ]]; then
    python3 "$ROOT/scripts/analyze.py" "$copy" >/dev/null
    python3 "$ROOT/scripts/seal.py" "$copy" >/dev/null
  fi
  if BENCH_SELF_TEST=1 python3 "$ROOT/scripts/audit.py" "$copy" >"$copy.audit.txt"; then
    cat "$copy.audit.txt"
    fail "tampered ($name) passed the audit"
  fi
  /usr/bin/grep -q -- "$expect" "$copy.audit.txt" || { cat "$copy.audit.txt"; fail "tampered ($name) failed, but not for: $expect"; }
  ok "tampered: $name -> FAIL ($(/usr/bin/grep -m1 -- "$expect" "$copy.audit.txt" | /usr/bin/cut -c1-110))"
}

tamper "a start time edited after the seal" "seal: raw/startup.tsv: changed" keep '
import sys; from pathlib import Path
p = Path(sys.argv[1]) / "raw/startup.tsv"; t = p.read_text().splitlines()
c = t[1].split("\t"); c[6] = "1.0"; t[1] = "\t".join(c); p.write_text("\n".join(t) + "\n")'
tamper "a launch output edited after the seal" "seal: raw/launch/" keep '
import sys, json; from pathlib import Path
p = next((Path(sys.argv[1]) / "raw/launch").glob("startup-warm-light-1-obsidian.json"))
d = json.loads(p.read_text()); d["content_ms"] = 1.0; p.write_text(json.dumps(d))'
tamper "a sample left out" "samples recorded, 10 scheduled" reseal '
import sys; from pathlib import Path
p = Path(sys.argv[1]) / "raw/startup.tsv"; t = p.read_text().splitlines()
p.write_text("\n".join([t[0]] + [l for l in t[1:] if not ("\tobsidian\theavy\twarm\t7\t" in l)]) + "\n")'
tamper "an accepted row whose probe never saw the note" "does not show the note on screen" reseal '
import sys, json; from pathlib import Path
p = Path(sys.argv[1]) / "raw/launch/startup-warm-medium-3-vault.json"
d = json.loads(p.read_text()); d["content_on_screen"] = False; d["lines_read"] = 0; p.write_text(json.dumps(d))'
tamper "over 20 % of one app rejected" "over 20 %" reseal '
import sys; from pathlib import Path
p = Path(sys.argv[1]) / "raw/startup.tsv"; t = p.read_text().splitlines(); n = 0
for i, l in enumerate(t):
    c = l.split("\t")
    if len(c) > 15 and c[1] == "vault" and n < 16:
        c[14], c[15] = "rejected", "note_not_on_screen"; t[i] = "\t".join(c); n += 1
p.write_text("\n".join(t) + "\n")'
tamper "a different OS build at the end" "machine.end.tsv macos_build" reseal '
import sys; from pathlib import Path
p = Path(sys.argv[1]) / "machine.end.tsv"; p.write_text(p.read_text().replace("26A428", "26A999"))'
tamper "an input swapped during the run" "inputs changed during the run" reseal '
import sys; from pathlib import Path
p = Path(sys.argv[1]) / "inputs.end.tsv"; p.write_text(p.read_text().replace("1.15.1 x86_64", "1.15.2 x86_64"))'
tamper "a protocol not committed before the run" "uncommitted changes" reseal '
import sys, json; from pathlib import Path
p = Path(sys.argv[1]) / "protocol.json"; d = json.loads(p.read_text()); d["protocol_dirty"] = True; p.write_text(json.dumps(d))'
tamper "the protocol text changed in the run" "differs from the text committed" reseal '
import sys, json, hashlib; from pathlib import Path
r = Path(sys.argv[1]); p = r / "PROTOCOL.md"; p.write_text(p.read_text().replace("at least 3 distinct phrases", "at least 1 phrase"))
j = r / "protocol.json"; d = json.loads(j.read_text()); d["protocol_sha256"] = hashlib.sha256(p.read_bytes()).hexdigest(); j.write_text(json.dumps(d))'
tamper "the analysis edited by hand" "analysis.md is not what analyze.py produces" keep '
import sys; from pathlib import Path
p = Path(sys.argv[1]) / "analysis.md"; p.write_text(p.read_text().replace("| 10 |", "| 12 |", 1))'
tamper "Obsidian wrote into the person own profile" "own_obsidian_folder_changed" reseal '
import sys, json; from pathlib import Path
p = Path(sys.argv[1]) / "raw/isolation/startup-warm-light-2-obsidian.json"; p.write_text(json.dumps({"problems": ["own_obsidian_folder_changed"]}))'

# The Obsidian DMG, headlessly: sha256, version, architectures, signature.
dmg="$HOME/krate-bench/obsidian/Obsidian-1.15.1.dmg"
if [[ -f "$dmg" ]]; then
  /usr/bin/sed "s|^OBSIDIAN_DMG=.*|OBSIDIAN_DMG=\"$dmg\"|" "$ROOT/config.example.env" >"$tmp/config.env"
  BENCH_CONFIG="$tmp/config.env" "$ROOT/scripts/install-obsidian.sh" --check >"$tmp/dmg.txt" 2>&1 || { cat "$tmp/dmg.txt"; fail "Obsidian DMG check"; }
  ok "Obsidian DMG: $(/usr/bin/tr '\n' ' ' <"$tmp/dmg.txt" | /usr/bin/sed 's/\t/ /g')"
else
  printf 'skip  Obsidian DMG not at %s (scripts/fetch-obsidian.sh downloads and checks it)\n' "$dmg"
fi
printf 'PASS: the kit is proved headlessly -- parsers, vaults, targets, probes, record/analyze/seal/audit, %s tampered runs refused\n' \
  "$(/bin/ls "$tmp/tampered" | /usr/bin/wc -l | /usr/bin/tr -d ' ')"
