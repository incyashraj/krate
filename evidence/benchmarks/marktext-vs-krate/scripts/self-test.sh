#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

for script in "$ROOT"/scripts/*.sh; do
  /bin/bash -n "$script"
done

PYTHONDONTWRITEBYTECODE=1 python3 -m py_compile \
  "$ROOT/fixtures/generate.py" "$ROOT/scripts/analyze.py" "$ROOT/scripts/audit.py"

tmp=$(/usr/bin/mktemp -d /tmp/krate-benchmark-selftest.XXXXXX)
python3 "$ROOT/fixtures/generate.py" --output-dir "$tmp" >"$tmp/generate.txt"

five_hash=$(sha256_file "$tmp/notes-5000.md")
fifty_hash=$(sha256_file "$tmp/notes-50000.md")
[[ "$five_hash" == 2a4700117e9c5e661916c9b4d660a374ff603509d5984a288e58f114b5eaabef ]] || \
  die "5,000-line generator hash changed: $five_hash"
[[ "$fifty_hash" == e2f69c96593e3d936d5f8f72e87c0775cde0fa432aaa3d3a9a180788c9f1baa2 ]] || \
  die "50,000-line generator hash changed: $fifty_hash"

swift_build "$ROOT/scripts/window_probe.swift" "$tmp/window-probe"
swift_build "$ROOT/scripts/scroll_driver.swift" "$tmp/scroll-driver"
"$tmp/window-probe" --timeout 2 --terminate -- /usr/bin/true >"$tmp/probe.json" 2>/dev/null && \
  die "window probe should reject a command that exits without a window"
python3 - "$tmp/probe.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["timed_out"] is True
assert data["schema"] == "krate.benchmark.window.v1"
PY

printf 'PASS: shell, Python, fixture hashes, and Swift probes validated\n'
