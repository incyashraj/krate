#!/usr/bin/env sh
# A deterministic stand-in for a real coding model, for proving the --author-cmd
# seam when a live model is unavailable. It writes the three app files into
# $KRATE_APP_DIR by hand, exactly as a model would.
set -eu
SDK_ROOT="${KRATE_SDK_ROOT:?set KRATE_SDK_ROOT}"
mkdir -p "$KRATE_APP_DIR/src"

# src/lib.rs: the maintained checklist sample, verbatim.
cp "$SDK_ROOT/apps/krate-checklist/src/lib.rs" "$KRATE_APP_DIR/src/lib.rs"

# manifest.toml: the sample manifest, name swapped, entry pointed at the artifact.
sed -e "s/^name = .*/name = \"$KRATE_APP_NAME\"/" \
    -e "s#entry = .*#entry = \"target/wasm32-wasip1/release/$(printf '%s' "$KRATE_APP_NAME" | tr '-' '_').wasm\"#" \
    "$SDK_ROOT/apps/krate-checklist/manifest.toml" > "$KRATE_APP_DIR/manifest.toml"

# Cargo.toml: the sample's, with absolute SDK paths and the app name.
snake="$(printf '%s' "$KRATE_APP_NAME" | tr '-' '_')"
sed -e "s/^name = .*/name = \"$KRATE_APP_NAME\"/" \
    -e "s#package = \"krate:checklist\"#package = \"krate:$KRATE_APP_NAME\"#" \
    -e "s#path = \"../../wit/krate/phase3\"#path = \"$SDK_ROOT/wit/krate/phase3\"#" \
    -e "s#path = \"../../wit/krate/phase3/deps/#path = \"$SDK_ROOT/wit/krate/phase3/deps/#g" \
    "$SDK_ROOT/apps/krate-checklist/Cargo.toml" > "$KRATE_APP_DIR/Cargo.toml"
# Add the standalone [workspace] table so it builds outside the workspace.
printf '[workspace]\n\n%s' "$(cat "$KRATE_APP_DIR/Cargo.toml")" > "$KRATE_APP_DIR/Cargo.toml.new"
mv "$KRATE_APP_DIR/Cargo.toml.new" "$KRATE_APP_DIR/Cargo.toml"
