#!/usr/bin/env bash
# Run the CI job's steps here, before pushing, so the answer to "did this break
# the full suite" takes two minutes instead of forty.
#
# On 2026-09-06 seven small defects in the full-test job cost twelve full CI
# runs -- one fix, one forty-minute round trip, repeat. Every one of them was
# reproducible on a laptop in seconds. This is the script that should have
# existed then: it pulls each `run:` step out of the workflow, fills in what
# the runner would have set, and runs it, reporting one line per step.
#
#   scripts/preflight.sh                 # the full-test job, every step
#   scripts/preflight.sh 9 11 14         # only these steps (numbers from the list)
#   scripts/preflight.sh --list          # show the numbered steps and stop
#   JOB=cold-install scripts/preflight.sh
#
# What it cannot do is be Windows. Steps that only mean something on Windows
# are skipped here and are the one thing that still needs a real machine.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

JOB="${JOB:-full-test}"
export RUNNER_OS="$(uname -s | sed 's/Darwin/macOS/;s/Linux/Linux/')"
export RUNNER_TEMP="${RUNNER_TEMP:-$ROOT/target/preflight}"
export GITHUB_WORKSPACE="$ROOT"
mkdir -p "$RUNNER_TEMP"

case "$RUNNER_OS" in
  macOS) MATRIX_OS=macos-latest ;;
  Linux) MATRIX_OS=ubuntu-latest ;;
  *) MATRIX_OS=ubuntu-latest ;;
esac

# The rustup toolchain, so the same cargo CI uses is the one that runs here
# even when a Homebrew cargo shadows it on PATH.
if [ -d "$HOME/.rustup/toolchains" ]; then
  TC="$(ls -d "$HOME"/.rustup/toolchains/1.*-* 2>/dev/null | sort -V | tail -1)"
  [ -n "$TC" ] && export PATH="$TC/bin:$HOME/.cargo/bin:$PATH"
fi

steps_json="$(python3 - "$JOB" "$MATRIX_OS" <<'PY'
import json, re, sys, yaml
job, matrix_os = sys.argv[1], sys.argv[2]
w = yaml.safe_load(open(".github/workflows/ci.yml"))
out = []
for i, s in enumerate(w["jobs"][job]["steps"]):
    name = s.get("name", s.get("uses", "?").split("@")[0])
    if "run" not in s:
        out.append({"i": i, "name": name, "skip": "action"})
        continue
    cond = str(s.get("if", ""))
    if "windows" in cond and "!=" not in cond:
        out.append({"i": i, "name": name, "skip": "windows-only"})
        continue
    if "ubuntu" in cond and matrix_os != "ubuntu-latest":
        out.append({"i": i, "name": name, "skip": "linux-only"})
        continue
    if "macos" in cond and matrix_os != "macos-latest":
        out.append({"i": i, "name": name, "skip": "macos-only"})
        continue
    body = s["run"]
    body = re.sub(r"\$\{\{\s*matrix\.os\s*\}\}", matrix_os, body)
    body = re.sub(r"\$\{\{\s*runner\.temp\s*\}\}", "$RUNNER_TEMP", body)
    body = re.sub(r"\$\{\{[^}]*\}\}", "x", body)
    out.append({"i": i, "name": name, "run": body, "shell": s.get("shell", "bash")})
print(json.dumps(out))
PY
)"

if [ "${1:-}" = "--list" ]; then
  echo "$steps_json" | python3 -c '
import json,sys
for s in json.load(sys.stdin):
    tag = "[" + s["skip"] + "]" if "skip" in s else "[run]"
    print("  %2d %-15s %s" % (s["i"], tag, s["name"]))'
  exit 0
fi

want="${*:-}"
passed=0; failed=0; skipped=0
failed_names=()

while IFS= read -r line; do
  i="$(echo "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["i"])')"
  name="$(echo "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["name"])')"
  if [ -n "$want" ] && ! echo " $want " | grep -q " $i "; then continue; fi
  skip="$(echo "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("skip",""))')"
  if [ -n "$skip" ]; then
    printf '  %2s  skip  %-58s (%s)\n' "$i" "$name" "$skip"
    skipped=$((skipped+1)); continue
  fi
  shell="$(echo "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["shell"])')"
  if [ "$shell" != "bash" ] && [ "$shell" != "sh" ]; then
    printf '  %2s  skip  %-58s (%s shell)\n' "$i" "$name" "$shell"
    skipped=$((skipped+1)); continue
  fi
  script="$RUNNER_TEMP/step-$i.sh"
  echo "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["run"])' > "$script"
  log="$RUNNER_TEMP/step-$i.log"
  printf '  %2s  ....  %s' "$i" "$name"
  if bash -e "$script" > "$log" 2>&1; then
    printf '\r  %2s  ok    %s\n' "$i" "$name"
    passed=$((passed+1))
  else
    code=$?
    printf '\r  %2s  FAIL  %s  (exit %s, log: %s)\n' "$i" "$name" "$code" "$log"
    tail -5 "$log" | sed 's/^/          /'
    failed=$((failed+1)); failed_names+=("$i $name")
  fi
done < <(echo "$steps_json" | python3 -c 'import json,sys; [print(json.dumps(s)) for s in json.load(sys.stdin)]')

echo
echo "  $passed passed, $failed failed, $skipped skipped  ($JOB on $RUNNER_OS)"
[ "$failed" -eq 0 ] || { printf '  failed: %s\n' "${failed_names[@]}"; exit 1; }
