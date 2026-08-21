#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"

require_command curl
require_command ditto

url=https://github.com/marktext/marktext/releases/download/v0.17.1/marktext-arm64-mac.zip
expected=703e5411b80514c867b4e9ce26dde5c16c416158ef45c6479256b6818aea5acf
destination=${1:-"$ROOT/build/dependencies/marktext-0.17.1-arm64"}
archive="$destination/marktext-arm64-mac.zip"
app="$destination/MarkText.app"
/bin/mkdir -p "$destination"

if [[ ! -f "$archive" ]]; then
  /usr/bin/curl --fail --location --proto '=https' --tlsv1.2 "$url" --output "$archive.partial"
  actual=$(sha256_file "$archive.partial")
  [[ "$actual" == "$expected" ]] || die "download hash mismatch: expected $expected, got $actual"
  /bin/mv "$archive.partial" "$archive"
fi

actual=$(sha256_file "$archive")
[[ "$actual" == "$expected" ]] || die "archive hash mismatch: expected $expected, got $actual"
if [[ ! -d "$app" ]]; then
  /usr/bin/ditto -x -k "$archive" "$destination"
fi
[[ -x "$app/Contents/MacOS/MarkText" ]] || die "MarkText executable missing after extraction"
printf 'MarkText 0.17.1 ARM64 ready at %s\n' "$app"
printf 'archive_sha256=%s\n' "$actual"
