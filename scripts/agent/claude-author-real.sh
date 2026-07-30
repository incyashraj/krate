#!/usr/bin/env sh
# The honest `--author-cmd`: Claude writes the Krate app from the request. It is
# given the working starter to learn the discipline from and the hard rules,
# then authors the app itself. This is the real "an AI authors the app" path.
#
# krate create provides KRATE_APP_DIR (with a compiling starter + CONTRACT.md
# already in it), KRATE_APP_NAME, KRATE_REQUEST, and KRATE_SDK_DIR.
set -eu

# Build the prompt via a quoted heredoc so nothing in it is shell-expanded
# (the rules mention `::*`, `{...}`, `format!`, etc. which the shell would
# otherwise try to interpret). Only the env vars we explicitly want are
# substituted, by appending them.
PROMPT="$(cat <<'RULES'
You are writing a Krate desktop app in Rust, from the user's request.

A COMPILING, WORKING starter is already in the app directory (Cargo.toml,
src/lib.rs, manifest.toml): a checklist GUI that opens a window, shows checkbox
rows, lets the user add and toggle items, and saves them to its granted folder.
Read it first. It already follows every rule below and renders correctly on
macOS, so it is the safest base to build from.

Your job: make the app match the request. If the request is checklist-like,
adapt the starter (title, seed items, wording). If it is genuinely different,
rewrite src/lib.rs following the same structure and rules.

HARD RULES (the starter obeys all of these; do not break them):
- The app is no_std + alloc. Do not add any std usage.
- Import only from the same bindings modules the starter uses (its ui, io, and
  fs imports). Never import wasi interfaces or std io.
- Build strings with the starter's pure_string / number_string helpers, never
  with the format macro.
- Keep the same window / tree / event / save structure so it renders and saves
  the same way.
- A GUI app must still exit promptly when its first argument is the literal word
  quick. The starter already does this; keep it.

After editing, the app must still open a window, show its rows, add and toggle
items, save to the granted folder, and exit on quick. Use Read and Edit or
Write. Do not explain; just make the app.
RULES
)"

PROMPT="${PROMPT}

Request: ${KRATE_REQUEST}
App name: ${KRATE_APP_NAME}
App directory: ${KRATE_APP_DIR}"

claude -p "$PROMPT" \
  --allowed-tools "Read,Edit,Write" \
  --permission-mode acceptEdits \
  >"${KRATE_APP_DIR}/.agent-transcript.txt" 2>&1 || {
    echo "agent (claude) run failed; see ${KRATE_APP_DIR}/.agent-transcript.txt" >&2
    exit 1
  }
