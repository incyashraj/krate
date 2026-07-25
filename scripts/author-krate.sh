#!/usr/bin/env sh
# The complete AI authoring loop, end to end.
#
# A request goes in; a working, permission-gated `.krate` comes out, and the
# whole run is recorded as evidence. The steps are:
#
#   1. author  — generate a complete Krate guest crate from the request
#   2. build   — compile it to a wasm component (cargo-component)
#   3. pack    — bundle code + manifest into one `.krate`
#   4. verify  — run it WITH the fs.read grant (works, exit 0) and WITHOUT it
#                (refuses before running, exit 5) — the permission wall
#
# It writes a `krate.author.v1` transcript (the request, every step's command
# and result, the generated files, the code.wasm sha256, and a verdict) plus a
# copy of the packaged `.krate`, so the loop leaves cross-platform evidence.
#
# The "author" step is the seam an AI plugs into. By default it runs the
# deterministic in-tree generator (`krate-author`). Pass `--author-cmd "<cmd>"`
# and that command is run instead to produce the app source — that is where a
# real LLM (or Claude Code) writes the code, with the build/pack/verify steps
# downstream unchanged. The generator path is what CI gates on; the LLM path is
# a demo hook and is never required to be green in CI.
#
# Usage:
#   scripts/author-krate.sh [--name <kebab>] [--top-n <N>]
#                           [--out <dir>] [--evidence <dir>]
#                           [--author-cmd "<command>"]

set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

# Put rustup's cargo (and cargo-component) on PATH the way the sample build
# scripts do, so the generated crate finds the pinned wasm target.
if command -v rustup >/dev/null 2>&1; then
  RUSTUP_CARGO="$(rustup which cargo 2>/dev/null || true)"
  if [ -n "$RUSTUP_CARGO" ]; then
    PATH="$(dirname -- "$RUSTUP_CARGO"):$HOME/.cargo/bin:$PATH"
  fi
fi

# ---- arguments -------------------------------------------------------------

NAME="word-count"
TOP_N="5"
READ_GLOB="./input/**"
OUT="$ROOT/target/authoring"
EVIDENCE="$ROOT/evidence/authoring"
AUTHOR_CMD=""
REQUEST_TEXT="Build a small app that reads a text file and reports its most common words."

while [ $# -gt 0 ]; do
  case "$1" in
    --name) NAME="$2"; shift 2 ;;
    --top-n) TOP_N="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --evidence) EVIDENCE="$2"; shift 2 ;;
    --author-cmd) AUTHOR_CMD="$2"; shift 2 ;;
    --request) REQUEST_TEXT="$2"; shift 2 ;;
    *) echo "author-krate.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

SNAKE="$(printf '%s' "$NAME" | tr '-' '_')"
APP_DIR="$OUT/$NAME"
RUN_DIR="$OUT/run"

# Build the host tools this loop needs once, up front, so a first-time compile
# never races inside a pipeline step where its output would be swallowed.
echo "==> Preparing host tools (krate cli, import checker)"
cargo build -q -p krate-cli -p krate-tools --bin krate --bin check-component-imports
KRATE_BIN="$ROOT/target/debug/krate"
[ -x "$KRATE_BIN" ] || KRATE_BIN="$ROOT/target/debug/krate.exe"
IMPORT_CHECK="$ROOT/target/debug/check-component-imports"
[ -x "$IMPORT_CHECK" ] || IMPORT_CHECK="$ROOT/target/debug/check-component-imports.exe"

mkdir -p "$OUT" "$EVIDENCE"
rm -rf "$APP_DIR" "$RUN_DIR"
mkdir -p "$RUN_DIR/input"

# A portable sha256 helper: coreutils on Linux, shasum on macOS.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# JSON string escaper for embedding command output in the transcript.
json_escape() {
  # Escape backslash, double-quote, and control characters; collapse newlines.
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | tr '\n' ' '
}

TRANSCRIPT="$EVIDENCE/transcript.json"
STEPS_FILE="$(mktemp)"
trap 'rm -f "$STEPS_FILE"' EXIT

# Record one step into the running steps array. Args: name, command, exit, note.
record_step() {
  step_name="$1"; step_cmd="$2"; step_exit="$3"; step_note="$4"
  [ -s "$STEPS_FILE" ] && printf ',\n' >>"$STEPS_FILE"
  printf '    {"step": "%s", "command": "%s", "exit": %s, "note": "%s"}' \
    "$step_name" "$(json_escape "$step_cmd")" "$step_exit" "$(json_escape "$step_note")" \
    >>"$STEPS_FILE"
}

echo "==> Request: $REQUEST_TEXT"

# ---- step 1: author --------------------------------------------------------

if [ -n "$AUTHOR_CMD" ]; then
  echo "==> [1/4] author (external agent command)"
  # The external agent is responsible for writing $APP_DIR/{Cargo.toml,src/lib.rs,manifest.toml}.
  # It is handed the target dir and the request through the environment.
  mkdir -p "$APP_DIR/src"
  KRATE_APP_DIR="$APP_DIR" KRATE_APP_NAME="$NAME" KRATE_REQUEST="$REQUEST_TEXT" \
    sh -c "$AUTHOR_CMD"
  record_step "author" "$AUTHOR_CMD" "$?" "external agent generated the app"
else
  echo "==> [1/4] author (deterministic generator)"
  # sdk-prefix walks from $APP_DIR back to the repo root: target/authoring/<name>.
  cargo run -q --manifest-path "$ROOT/crates/author/Cargo.toml" -- \
    --out "$APP_DIR" --sdk-prefix "../../.." \
    --name "$NAME" --read-glob "$READ_GLOB" --top-n "$TOP_N" \
    >"$EVIDENCE/author-step.json"
  record_step "author" "krate-author --name $NAME --top-n $TOP_N" "0" \
    "generated $(find "$APP_DIR" -type f | wc -l | tr -d ' ') files"
fi

# ---- step 2: build ---------------------------------------------------------

echo "==> [2/4] build ($NAME -> wasm component)"
( cd "$APP_DIR" && cargo-component build --release )
WASM="$APP_DIR/target/wasm32-wasip1/release/${SNAKE}.wasm"
if [ ! -f "$WASM" ]; then
  echo "author-krate.sh: build did not produce $WASM" >&2
  record_step "build" "cargo-component build --release" "1" "no wasm produced"
  exit 1
fi
record_step "build" "cargo-component build --release" "0" "built ${SNAKE}.wasm"

# The generated component must import only krate:*; a wasi import means the
# generated code broke the fixed-capacity discipline.
if "$IMPORT_CHECK" "$WASM" >/dev/null 2>&1; then
  record_step "check-imports" "check-component-imports" "0" "krate:* imports only"
else
  echo "author-krate.sh: generated component imports non-Krate host APIs" >&2
  record_step "check-imports" "check-component-imports" "1" "non-krate imports present"
  exit 1
fi

# ---- step 3: pack ----------------------------------------------------------

echo "==> [3/4] pack (-> $NAME.krate)"
cp "$WASM" "$RUN_DIR/code.wasm"
# Rewrite the manifest entry to the packed layout (code.wasm beside manifest).
sed "s#entry = \"target/wasm32-wasip1/release/${SNAKE}.wasm\"#entry = \"code.wasm\"#" \
  "$APP_DIR/manifest.toml" >"$RUN_DIR/manifest.toml"
"$KRATE_BIN" pack "$RUN_DIR/code.wasm" --manifest "$RUN_DIR/manifest.toml" \
  -o "$RUN_DIR/$NAME.krate"
CODE_SHA="$(sha256_of "$RUN_DIR/code.wasm")"
record_step "pack" "krate pack -> $NAME.krate" "0" "code.wasm sha256 $CODE_SHA"

# A deterministic input so the report (and thus the evidence) is stable.
printf 'the quick brown fox the lazy dog the fox jumps over the lazy fox\n' \
  >"$RUN_DIR/input/sample.txt"

# ---- step 4: verify the permission wall ------------------------------------

echo "==> [4/4] verify (allow, then deny)"

# 4a: WITH the grant, the app runs and prints its report.
ALLOW_OUT="$(cd "$RUN_DIR" && "$KRATE_BIN" run "$NAME.krate" --auto-grant -- input/sample.txt)"
ALLOW_EXIT=$?
printf '%s\n' "$ALLOW_OUT" >"$EVIDENCE/report.csv"
if [ "$ALLOW_EXIT" -ne 0 ]; then
  echo "author-krate.sh: granted run failed (exit $ALLOW_EXIT)" >&2
  record_step "verify-allow" "krate run --auto-grant" "$ALLOW_EXIT" "granted run failed"
  exit 1
fi
record_step "verify-allow" "krate run --auto-grant" "0" \
  "printed $(printf '%s\n' "$ALLOW_OUT" | wc -l | tr -d ' ') lines"

# 4b: WITHOUT fs.read, the app must refuse before running — exit 5.
set +e
( cd "$RUN_DIR" && "$KRATE_BIN" run "$NAME.krate" \
    --grant io.args --grant io.stdout --grant io.stderr -- input/sample.txt ) \
    >/dev/null 2>&1
DENY_EXIT=$?
set -e
if [ "$DENY_EXIT" -ne 5 ]; then
  echo "author-krate.sh: withholding fs.read should exit 5, got $DENY_EXIT" >&2
  record_step "verify-deny" "krate run (no fs.read)" "$DENY_EXIT" "expected 5"
  exit 1
fi
record_step "verify-deny" "krate run (no fs.read)" "5" "refused before running"

# ---- evidence: the transcript ----------------------------------------------

cp "$RUN_DIR/$NAME.krate" "$EVIDENCE/$NAME.krate"

OS_NAME="$(uname -s 2>/dev/null || echo unknown)"
{
  printf '{\n'
  printf '  "schema": "krate.author.v1",\n'
  printf '  "request": "%s",\n' "$(json_escape "$REQUEST_TEXT")"
  printf '  "app": {"name": "%s", "kind": "word-frequency", "read_glob": "%s", "top_n": %s},\n' \
    "$NAME" "$READ_GLOB" "$TOP_N"
  printf '  "os": "%s",\n' "$OS_NAME"
  printf '  "code_wasm_sha256": "%s",\n' "$CODE_SHA"
  printf '  "krate_bytes": %s,\n' "$(wc -c <"$RUN_DIR/$NAME.krate" | tr -d ' ')"
  printf '  "steps": [\n'
  cat "$STEPS_FILE"
  printf '\n  ],\n'
  printf '  "verdict": "authored a working, permission-gated .krate: runs with fs.read (exit 0), refuses without it (exit 5)"\n'
  printf '}\n'
} >"$TRANSCRIPT"

echo ""
echo "==> Authored $NAME.krate and proved its permission wall."
echo "    transcript: $TRANSCRIPT"
echo "    bundle:     $EVIDENCE/$NAME.krate"
echo "    report:     $EVIDENCE/report.csv"
echo "    code.wasm sha256 ($OS_NAME): $CODE_SHA"
