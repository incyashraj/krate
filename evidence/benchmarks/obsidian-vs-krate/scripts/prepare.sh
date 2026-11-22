#!/usr/bin/env bash
# Everything a run needs, made without launching any app: the vaults
# (checked against the README's tree digests), the targets and phrases,
# the three Swift instruments, the input check and the prepared machine.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

require_command python3
require_command swiftc

/bin/mkdir -p "$BUILD"
python3 "$REPO/scripts/make-bench-vault.py" --out "$BUILD/vaults" --all >"$BUILD/vaults.txt"
python3 - "$BUILD/vaults" <<'PY'
import json, sys
from pathlib import Path
expected = {
    "light": "de15775a8f33f793618de644d4a9e505784f191b50f144ee60756ffe29dbff57",
    "medium": "89e0119e6f93311fb32e2cc925386bed47e2038e5c02239dce3b0779768ce97b",
    "heavy": "42209ae7fc0cd817d9ffdde36d0d97bf594a57c192ba340b3b32adf3d9402040",
    "big-1000": "656754a544229c5e9693e673cb4a2b4d6713aabb2c0d45df7b9bb933581e4e08",
    "big-50000": "08504f9b4ad1858d962a99574cbc3c7eed222716650475b4f399b906301f1610",
    "big-200000": "1e711196337de5c2c367bcee5d90a7086706281dee27886f5a739d1d0ef0de44",
}
for name, digest in expected.items():
    got = json.loads((Path(sys.argv[1]) / f"{name}.manifest.json").read_text())["tree_sha256"]
    if got != digest:
        raise SystemExit(f"ERROR: vault {name} tree_sha256 {got}, expected {digest} (README table)")
print("vaults\tevery tree_sha256 matches the README table")
PY
python3 "$ROOT/scripts/note-phrases.py" --vaults "$BUILD/vaults" --out "$BUILD/targets"
for tool in start_probe tree_sample interact; do swift_build "$tool"; done
"$BUILD/start-probe" --self-test
"$BUILD/tree-sample" --self-test
"$BUILD/interact" --self-test
"$ROOT/scripts/verify-inputs.sh"
"$ROOT/scripts/capture-machine.sh" "$BUILD/machine.prepared.tsv"
"$ROOT/scripts/write-input-manifest.sh" "$BUILD/inputs.prepared.tsv"
note "prepared $BUILD; next: ./scripts/run-suite.sh"
