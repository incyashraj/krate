#!/usr/bin/env bash
# Krate for Claude Desktop, from a release's own binaries (IC-1005).
#
# Run by .github/workflows/release.yml's claude-desktop job on macOS, and by
# hand to dry-run it:
#
#   bash scripts/release-mcpb.sh --version 0.6.2 --artifacts DIR --out DIR
#
# DIR holds the release's three archives (anywhere below it):
#   krate-<version>-aarch64-apple-darwin.tar.gz
#   krate-<version>-x86_64-apple-darwin.tar.gz
#   krate-<version>-x86_64-pc-windows-msvc.zip
# and --out receives krate-<version>-mac.mcpb and krate-<version>-windows.mcpb.
#
# The Mac bundle carries one universal krate and one universal
# cargo-component (lipo of the two arch builds). The release's krate slices
# are already Developer ID signed by the build job with the JIT
# entitlements; lipo keeps each slice's signature. cargo-component ships
# ad-hoc signed in the archives, so it is signed here with the same
# identity. Then the two universal binaries, exactly as packed, are
# notarized. A bare Mach-O cannot be stapled, so Gatekeeper finds the
# ticket online, by the binary's code hash, the first time a quarantined
# copy runs; offline on that first run it cannot, and says it cannot verify
# the developer.
#
# Secrets, all optional so a fork still builds (each absence is a warning):
#   CERT_P12, CERT_PASSWORD                   Developer ID Application (as the build job)
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD  the notary
#   MCPB_SIGN_CERT, MCPB_SIGN_KEY             PEM certificate and key for `mcpb sign`
#   MCPB_SIGN_INTERMEDIATE                    PEM intermediate certificate(s), if any
set -euo pipefail

version="" artifacts="" out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="${2#v}"; shift 2 ;;
    --artifacts) artifacts="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) echo "release-mcpb: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$version" ] && [ -n "$artifacts" ] && [ -n "$out" ] \
  || { echo "usage: release-mcpb.sh --version V --artifacts DIR --out DIR" >&2; exit 2; }

here="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"; [ -n "${KEYCHAIN:-}" ] && security delete-keychain "$KEYCHAIN" 2>/dev/null || true' EXIT

find_one() {
  local name="$1" hit
  hit="$(find "$artifacts" -type f -name "$name" | head -1)"
  [ -n "$hit" ] || { echo "::error::release-mcpb: no $name under $artifacts" >&2; exit 1; }
  printf '%s\n' "$hit"
}

# The folder inside an unpacked archive that holds krate itself.
unpack() {
  local archive="$1" into="$work/$2" exe="$3"
  mkdir -p "$into"
  case "$archive" in
    *.zip) unzip -q "$archive" -d "$into" ;;
    *) tar -xzf "$archive" -C "$into" ;;
  esac
  dirname "$(find "$into" -type f -name "$exe" | head -1)"
}

mac_arm="$(unpack "$(find_one "krate-${version}-aarch64-apple-darwin.tar.gz")" mac-arm64 krate)"
mac_x64="$(unpack "$(find_one "krate-${version}-x86_64-apple-darwin.tar.gz")" mac-x64 krate)"
win_x64="$(unpack "$(find_one "krate-${version}-x86_64-pc-windows-msvc.zip")" win-x64 krate.exe)"
for d in "$mac_arm" "$mac_x64"; do
  [ -f "$d/cargo-component" ] || { echo "::error::$d has no cargo-component" >&2; exit 1; }
done
[ -f "$win_x64/cargo-component.exe" ] || { echo "::error::$win_x64 has no cargo-component.exe" >&2; exit 1; }

# ------------------------------------------------------------ signing (Mac)
signed=0
if [ -n "${CERT_P12:-}" ]; then
  KEYCHAIN=mcpb.keychain
  security create-keychain -p actions "$KEYCHAIN"
  security list-keychains -d user -s "$KEYCHAIN" $(security list-keychains -d user | tr -d '"')
  security unlock-keychain -p actions "$KEYCHAIN"
  printf '%s' "$CERT_P12" | base64 --decode > "$work/cert.p12"
  security import "$work/cert.p12" -k "$KEYCHAIN" -P "${CERT_PASSWORD:-}" -T /usr/bin/codesign
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k actions "$KEYCHAIN" >/dev/null
  rm -f "$work/cert.p12"
  IDENTITY="$(security find-identity -v -p codesigning "$KEYCHAIN" \
    | grep "Developer ID Application" | head -1 | sed -E 's/.*"(.*)"/\1/')"
  [ -n "$IDENTITY" ] || { echo "::error::no Developer ID Application identity in CERT_P12" >&2; exit 1; }
  for d in "$mac_arm" "$mac_x64"; do
    # The release's krate must already carry the identity (the build job
    # signs it with the JIT entitlements); re-signing here would drop them.
    codesign -dv --verbose=2 "$d/krate" 2>&1 | grep -q "Authority=Developer ID Application" \
      || { echo "::error::$d/krate is not Developer ID signed; the build job's signing did not run" >&2; exit 1; }
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$d/cargo-component"
    codesign --verify --strict "$d/cargo-component"
  done
  signed=1
else
  echo "::warning::release-mcpb: no CERT_P12, so cargo-component stays ad-hoc signed and nothing is notarized."
fi

# ------------------------------------------------------------- the bundles
mac_out="$out/krate-${version}-mac.mcpb"
win_out="$out/krate-${version}-windows.mcpb"
python3 "$here/scripts/build-mcpb.py" --only mac --version "$version" \
  --mac-arm64 "$mac_arm" --mac-x64 "$mac_x64" --out "$mac_out" --keep "$work/mac-bundle"
python3 "$here/scripts/build-mcpb.py" --only windows --version "$version" \
  --win-x64 "$win_x64" --tools-from "$work/mac-bundle/server/krate" --out "$win_out"

# The universal binaries keep a Developer ID signature on each slice.
if [ "$signed" = 1 ]; then
  for f in krate cargo-component; do
    codesign --verify --strict --verbose=2 "$work/mac-bundle/server/$f"
    codesign -dv --verbose=2 "$work/mac-bundle/server/$f" 2>&1 | grep -q "Authority=Developer ID Application" \
      || { echo "::error::universal $f lost its Developer ID signature" >&2; exit 1; }
  done
fi

# ------------------------------------------------------------ notarization
if [ "$signed" = 1 ] && [ -n "${APPLE_ID:-}" ]; then
  APPLE_ID="$(printf '%s' "$APPLE_ID" | tr -d '[:space:]')"
  APPLE_TEAM_ID="$(printf '%s' "${APPLE_TEAM_ID:-}" | tr -d '[:space:]')"
  APPLE_APP_PASSWORD="$(printf '%s' "${APPLE_APP_PASSWORD:-}" | tr -d '[:space:]')"
  # The same bytes the bundle carries: the notary's ticket is keyed on each
  # slice's code hash, so these are the binaries Gatekeeper will look up.
  (cd "$work/mac-bundle/server" && /usr/bin/ditto -c -k --sequesterRsrc . "$work/notary.zip")
  verdict="$(xcrun notarytool submit "$work/notary.zip" --apple-id "$APPLE_ID" \
    --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait 2>&1)" || true
  printf '%s\n' "$verdict"
  if printf '%s' "$verdict" | grep -q "status: Accepted"; then
    echo "notarized: krate and cargo-component in $(basename "$mac_out")"
  elif printf '%s' "$verdict" | grep -q "status:"; then
    id="$(printf '%s\n' "$verdict" | awk '/^  id: /{print $2; exit}')"
    [ -n "$id" ] && xcrun notarytool log "$id" --apple-id "$APPLE_ID" \
      --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" || true
    echo "::error::the notary rejected the Claude Desktop binaries; not shipping $(basename "$mac_out")" >&2
    rm -f "$mac_out"
    exit 1
  else
    echo "::warning::notarization got no verdict (auth or network); $(basename "$mac_out") ships signed but not notarized."
  fi
else
  echo "::warning::release-mcpb: not notarized (no signing identity or no APPLE_ID)."
fi

# --------------------------------------------------------- mcpb signatures
# scripts/sign-mcpb.sh: MCPB_SIGN_* if set, else the Developer ID p12, and
# a file whose signature does not verify ships unsigned.
bash "$here/scripts/sign-mcpb.sh" "$mac_out" "$win_out"

ls -la "$mac_out" "$win_out"
