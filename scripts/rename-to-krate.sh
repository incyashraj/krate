#!/bin/bash
# Amendment A9 executor: rename Layer36 -> Krate across the repository.
#
#   sh scripts/rename-to-krate.sh            # dry run: reports what would change
#   sh scripts/rename-to-krate.sh --apply    # performs the rename
#
# Ordered per the A9 runbook (Plan/Plan-Amendments-2026-07.md): mechanical
# case-aware replacement plus path renames, EXCLUDING regenerable and
# founder-private material. After --apply, the post-steps below are manual
# because they need toolchains and review:
#
#   POST-STEPS (run in order, verify each):
#     1. For each app in apps/: cargo-component build --release
#        (regenerates src/bindings.rs against the krate:* WIT).
#     2. cargo build && cargo test workspace; clippy -D warnings.
#     3. sh scripts/generate-uapi-reference.sh
#        sh scripts/generate-uapi-freeze-lock.sh   (namespace changed => new hashes)
#        sh scripts/generate-uapi-freeze-evidence.sh
#     4. mdbook build docs/book
#     5. Push; fast CI; dispatch full matrix; iterate to green.
#     6. RUNNER: the blanket rename changes the workflow label
#        `layer36-local` to `krate-local`, but the physical runner keeps its
#        old label until re-registered. Before pushing, re-register it:
#          cd ~/runner/actions-runner && ./svc.sh stop && ./svc.sh uninstall
#          ./config.sh remove --token <remove-token>
#          ./config.sh --unattended --url <repo-url> --token <reg-token> \
#            --name krate-local --labels krate-local --replace
#          ./svc.sh install && ./svc.sh start
#     7. LAST: rename the GitHub repo (Settings) and update Pages URLs;
#        GitHub redirects cover old links; update the runner's configured
#        URL if the slug changes.
#
# The schema id krate.run.v1 replaces layer36.run.v1; keep the old id
# accepted wherever the schema is *parsed* (currently only tests assert it).
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"
APPLY=0
[ "${1:-}" = "--apply" ] && APPLY=1

# Files/dirs never touched: VCS, private folders, regenerable artifacts.
EXCLUDES=(
  -path ./.git -prune -o
  -path ./target -prune -o
  -path ./Invest -prune -o
  -path ./Layer36-Book -prune -o
  -path ./node_modules -prune -o
  -path "./apps/*/target" -prune -o
  -name "Cargo.lock" -prune -o
  -name "bindings.rs" -prune -o
  -name "*.wasm" -prune -o
  -name "*.pdf" -prune -o
)

echo "== A9 rename (apply=$APPLY) =="

# ---- 1. Content replacement: layer36->krate, Layer36->Krate, LAYER36->KRATE,
#         layer6x6 repo slug references left for step 6 (repo rename) except docs URLs.
FILES=$(find . "${EXCLUDES[@]}" -type f \( -name "*.rs" -o -name "*.toml" -o -name "*.wit" -o -name "*.md" -o -name "*.sh" -o -name "*.yml" -o -name "*.yaml" -o -name "*.tex" \) -print)
COUNT=0
for f in $FILES; do
  if grep -qi "layer36" "$f"; then
    COUNT=$((COUNT+1))
    if [ "$APPLY" = "1" ]; then
      sed -i '' -e 's/layer36/krate/g' -e 's/Layer36/Krate/g' -e 's/LAYER36/KRATE/g' "$f" 2>/dev/null \
        || sed -i -e 's/layer36/krate/g' -e 's/Layer36/Krate/g' -e 's/LAYER36/KRATE/g' "$f"
    fi
  fi
done
echo "content: $COUNT files $( [ $APPLY = 1 ] && echo rewritten || echo would change )"

# ---- 2. Path renames (git mv keeps history): apps, scripts, WIT dirs, docs pages.
PATHS=$(git ls-files | grep -i "layer36" | awk -F/ '{ for(i=1;i<=NF;i++){ if (tolower($i) ~ /layer36/) { out=""; for(j=1;j<=i;j++) out=out (j>1?"/":"") $j; print out; break } } }' | sort -u)
echo "paths containing layer36:"
echo "$PATHS" | sed 's/^/  /'
if [ "$APPLY" = "1" ]; then
  # Deepest paths first so parents rename cleanly afterward.
  echo "$PATHS" | awk '{ print gsub(/\//,"/"), $0 }' | sort -rn | cut -d' ' -f2- | while read -r p; do
    [ -e "$p" ] || continue
    np=$(echo "$p" | sed -e 's/layer36/krate/g' -e 's/Layer36/Krate/g')
    [ "$p" = "$np" ] && continue
    git mv "$p" "$np"
    echo "  moved: $p -> $np"
  done
fi

echo "== done. $( [ $APPLY = 1 ] && echo 'Now run the POST-STEPS in the header.' || echo 'Dry run only; re-run with --apply.' ) =="
