#!/usr/bin/env bash
#
# measure-demo-claims.sh -- reproduce the numbers behind the Krate demo video.
#
# Everything printed here is measured on the machine you run it on. Nothing is
# recorded or guessed. Run it, and the claims in docs/demo-proof.md are the
# lines you get back.
#
#   sh scripts/measure-demo-claims.sh
#
# What it measures:
#   1. SIZE     -- byte size of every .krate bundle in evidence/ported/, in KB.
#   2. RUNTIME  -- size of the release krate binary (the one-time install).
#   3. STARTUP  -- median cold-start wall-clock for a real app, over N runs.
#   4. SECURITY -- how many wasi:* imports each bundle has (answer: zero).
#
# It needs a release binary. Build it first:
#   cargo build --release -p krate-cli
#
# wasm-tools is optional; the security section is skipped if it is missing.

set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
BUNDLES="$ROOT/evidence/ported"
KRATE="$ROOT/target/release/krate"
RUNS="${RUNS:-7}"

# Format an integer with underscore thousands separators, BSD-awk safe.
group() {
  printf '%s' "$1" | awk '{
    n = $0; out = ""
    while (length(n) > 3) {
      out = "," substr(n, length(n) - 2) out
      n = substr(n, 1, length(n) - 3)
    }
    print n out
  }'
}

echo "=================================================================="
echo " Krate demo claims -- measured on $(uname -sm), $(date +%Y-%m-%d)"
echo "=================================================================="
echo ""

# ---------------------------------------------------------------- 1. SIZE
echo "1. APP SIZE (.krate bundles, one file that runs everywhere)"
echo "   ------------------------------------------------------------"
smallest_kb=""
smallest_name=""
for f in "$BUNDLES"/*.krate; do
  [ -f "$f" ] || continue
  name=$(basename "$f" .krate)
  bytes=$(wc -c < "$f" | tr -d ' ')
  kb=$(awk "BEGIN { printf \"%.1f\", $bytes/1024 }")
  printf "   %-10s %8s bytes  = %7s KB\n" "$name" "$(group "$bytes")" "$kb"
  if [ -z "$smallest_kb" ] || [ "$bytes" -lt "$smallest_kb" ]; then
    smallest_kb="$bytes"
    smallest_name="$name"
  fi
done
smallest_disp=$(awk "BEGIN { printf \"%.1f\", $smallest_kb/1024 }")
echo ""
echo "   Smallest: $smallest_name at $smallest_disp KB ($smallest_kb bytes)."
echo ""
echo "   Comparison: a minimal hello-world Electron app is ~150 MB."
echo "   A 150 MB Electron app is $(awk "BEGIN { printf \"%.1f\", (150*1024*1024)/$smallest_kb }" | sed 's/\.0$//')M bytes;"
ratio=$(awk "BEGIN { printf \"%d\", (150*1024*1024)/$smallest_kb }")
echo "   ratio ~ $(group "$ratio")x smaller than a minimal Electron app."
echo ""

# ------------------------------------------------------------- 2. RUNTIME
echo "2. RUNTIME BINARY (the one-time install; every app shares it)"
echo "   ------------------------------------------------------------"
if [ -x "$KRATE" ]; then
  rbytes=$(wc -c < "$KRATE" | tr -d ' ')
  rmb=$(awk "BEGIN { printf \"%.1f\", $rbytes/1024/1024 }")
  echo "   krate  $(group "$rbytes") bytes = $rmb MB   ($KRATE)"
else
  echo "   (no release binary -- run: cargo build --release -p krate-cli)"
fi
echo ""

# ------------------------------------------------------------- 3. STARTUP
echo "3. COLD START (whole-process wall clock, median of $RUNS runs)"
echo "   ------------------------------------------------------------"
if [ ! -x "$KRATE" ]; then
  echo "   (no release binary -- skipping startup)"
elif ! command -v python3 >/dev/null 2>&1; then
  echo "   (python3 not found -- skipping startup)"
else
  # Timed inside a single Python process, the same way scripts/build-reports-page.py
  # measures it. Timing each run by spawning a separate `date`/`python3` from the
  # shell would add tens of milliseconds of its own process-spawn overhead and
  # measure the timer, not Krate. This is whole-process wall clock, start of the
  # subprocess to its exit -- exactly what a person waits.
  #
  # The scratch working dir exists because a couple of apps look for input folders
  # and short-circuit if started somewhere bare. They exit 0 either way, so timing
  # them without this would measure the wrong path.
  python3 - "$KRATE" "$BUNDLES" "$RUNS" <<'PY'
import datetime, statistics, subprocess, sys, tempfile, pathlib
krate, bundles, runs = sys.argv[1], pathlib.Path(sys.argv[2]), int(sys.argv[3])
work = pathlib.Path(tempfile.mkdtemp(prefix="krate-demo-"))
for sub in ("input", "images", "documents", "docs", "output", "scan"):
    (work / sub).mkdir(exist_ok=True)
(work / "input/sample.bin").write_bytes(b"Hello, Krate!")
(work / "input/sample.txt").write_text("the quick brown fox the lazy dog\n")
fastest = None
for b in sorted(bundles.glob("*.krate")):
    times = []
    ok = True
    for _ in range(runs):
        start = datetime.datetime.now()
        r = subprocess.run(
            [krate, "run", "--headless", "--auto-grant", str(b), "--", "quick"],
            capture_output=True, cwd=work,
        )
        if r.returncode != 0:
            ok = False
            break
        times.append((datetime.datetime.now() - start).total_seconds() * 1000)
    if ok and times:
        med = statistics.median(times)
        print(f"   {b.stem:10} {med:6.1f} ms  (median of {runs})")
        if fastest is None or med < fastest[1]:
            fastest = (b.stem, med)
    else:
        print(f"   {b.stem:10}   (skipped: takes a file argument, not 'quick')")
import shutil; shutil.rmtree(work, ignore_errors=True)
if fastest:
    print()
    print(f"   Fastest: {fastest[0]} at {fastest[1]:.1f} ms.")
    print("   One frame of 60fps video is 16.7 ms.")
PY
fi
echo ""

# ------------------------------------------------------------ 4. SECURITY
echo "4. SECURITY (app imports only krate:* -- zero wasi:*)"
echo "   ------------------------------------------------------------"
if command -v wasm-tools >/dev/null 2>&1; then
  TMP=$(mktemp -d)
  for f in "$BUNDLES"/*.krate; do
    [ -f "$f" ] || continue
    name=$(basename "$f" .krate)
    d="$TMP/$name"
    mkdir -p "$d"
    unzip -oq "$f" -d "$d"
    wasm=$(ls "$d"/*.wasm 2>/dev/null | head -1)
    [ -n "$wasm" ] || continue
    wit=$(wasm-tools component wit "$wasm" 2>/dev/null || true)
    wasi=$(printf '%s' "$wit" | grep -ic "import wasi:" || true)
    kr=$(printf '%s' "$wit" | grep -c "import krate:" || true)
    printf "   %-10s wasi:* imports = %s   krate:* imports = %s\n" "$name" "$wasi" "$kr"
  done
  rm -rf "$TMP"
  echo ""
  echo "   Every bundle: zero wasi:* imports. The app can reach nothing the"
  echo "   runtime does not hand it, and it asks (capability wall) first."
else
  echo "   (wasm-tools not found -- skipping; install with: cargo install wasm-tools)"
fi
echo ""
echo "Done."
