#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

require_command python3
require_command swiftc

python3 "$ROOT/fixtures/generate.py" --output-dir "$ROOT/fixtures"
swift_build "$ROOT/scripts/window_probe.swift" "$ROOT/build/window-probe"
swift_build "$ROOT/scripts/scroll_driver.swift" "$ROOT/build/scroll-driver"
"$ROOT/scripts/verify-inputs.sh"

prepared="$ROOT/build/prepared"
/bin/mkdir -p "$prepared"
"$ROOT/scripts/capture-machine.sh" "$prepared/machine.tsv"
"$ROOT/scripts/write-input-manifest.sh" "$prepared/inputs.tsv"
printf 'Prepared benchmark tools and manifests in %s\n' "$prepared"
