#!/usr/bin/env sh
# A reliable `--author-cmd` for a live demo: drive the `claude` CLI headless to
# personalize the compiling checklist starter that `krate create` has already
# dropped into $KRATE_APP_DIR. Editing a known-good, rendering, non-hanging base
# is far more dependable for a one-take recording than writing a Krate app from
# scratch -- while still being genuinely AI-authored.
#
# krate create provides:
#   KRATE_APP_DIR   - the app dir, already holding a working src/lib.rs +
#                     Cargo.toml + manifest.toml + CONTRACT.md
#   KRATE_APP_NAME  - the app's kebab-case name
#   KRATE_REQUEST   - the plain-English request
#   KRATE_SDK_DIR   - the materialized Krate SDK (no repo checkout needed)
set -eu

PROMPT="A working Krate checklist app already exists in this directory:
${KRATE_APP_DIR}

The user asked for: ${KRATE_REQUEST}

Edit src/lib.rs in that directory to match the request. In practice this means
setting the window title and the on-screen heading to the name the user asked
for (look at window::create(...) and the header() function's label). Keep
everything else exactly as it is -- the app already builds, renders its items,
saves, and imports only krate:* interfaces, and it must stay that way.

Do NOT rewrite the file or change how it renders, reads args, or saves. Make the
smallest edit that satisfies the request, then stop. Use the Read and Edit tools.
Do not explain; just make the edit."

# Headless run; only reading and editing files in the app dir.
claude -p "$PROMPT" \
  --allowed-tools "Read,Edit" \
  --permission-mode acceptEdits \
  >"${KRATE_APP_DIR}/.agent-transcript.txt" 2>&1 || {
    echo "agent (claude) run failed; see ${KRATE_APP_DIR}/.agent-transcript.txt" >&2
    exit 1
  }
