#!/usr/bin/env bash
set -euo pipefail

cargo test -p krate-layout
cargo bench -p krate-layout --bench layout --no-run
