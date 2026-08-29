#!/usr/bin/env sh
# A real-model --author-cmd for `krate create`: drive the `claude` CLI headless
# to write a Krate guest app into $KRATE_APP_DIR from $KRATE_REQUEST.
#
# krate create hands us:
#   KRATE_APP_DIR   - where to write Cargo.toml, src/lib.rs, manifest.toml
#   KRATE_APP_NAME  - the app's kebab-case name
#   KRATE_REQUEST   - the plain-English request
#
# The model is told the one hard constraint (a Krate component may import only
# krate:*), pointed at the in-repo samples to copy the discipline from, and
# asked to write exactly the three files. krate create then builds, import-
# checks, packs, and verifies -- so a broken app is caught, not shipped.
set -eu

SDK_ROOT="${KRATE_SDK_ROOT:?set KRATE_SDK_ROOT to a Krate checkout for the agent to reference}"

PROMPT="You are writing a Krate guest application in Rust.

Request: ${KRATE_REQUEST}
App name (kebab-case): ${KRATE_APP_NAME}

Write exactly three files into the directory ${KRATE_APP_DIR}:
  - Cargo.toml
  - src/lib.rs
  - manifest.toml

HARD CONSTRAINT: a Krate component may import only krate:* interfaces. Ordinary
std facilities pull wasi:* imports that make the component fail to load -- a
growable Vec's realloc, HashMap, format!, and the args::first / read_to_string
SDK helpers all do this, and LTO cannot strip it. So use fixed-capacity [u8; N]
buffers, .get()/.get_mut() only, args::raw() with manual splitting, and build
strings by hand.

Copy the structure and discipline from the working samples in this repo:
  - ${SDK_ROOT}/apps/krate-checklist  (a GUI checklist that saves locally)
  - ${SDK_ROOT}/apps/krate-notes      (a GUI notes app)
Match a sample's Cargo.toml (the [package.metadata.component] target, the empty
[workspace], and the release profile) but with path dependencies rewritten to
absolute paths under ${SDK_ROOT} (e.g. \"${SDK_ROOT}/wit/krate/phase3\").
The manifest.toml must declare only the capabilities the app uses, with the one
that gates it (fs.write for a saving app) marked required.

Write the files now with the Write tool. Do not explain; just write the files."

# Run the model headless, allowing only file writes into the app dir.
claude -p "$PROMPT" \
  --allowed-tools "Write,Read,Bash" \
  --permission-mode acceptEdits \
  >"${KRATE_APP_DIR}/.agent-transcript.txt" 2>&1 || {
    echo "agent (claude) run failed; see ${KRATE_APP_DIR}/.agent-transcript.txt" >&2
    exit 1
  }
