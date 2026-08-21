#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

require_command python3
require_command swiftc
require_command footprint

[[ -d "$MARKTEXT_APP" ]] || die "MarkText app not found: $MARKTEXT_APP"
[[ -f "$KRATE_BUNDLE" ]] || die "Krate bundle not found: $KRATE_BUNDLE"
[[ -x "$KRATE_BIN" ]] || die "Krate binary is not executable: $KRATE_BIN"

MARKTEXT_BIN=$(marktext_binary)
[[ -x "$MARKTEXT_BIN" ]] || die "MarkText executable not found: $MARKTEXT_BIN"

host_arch=$(/usr/bin/uname -m)
marktext_file=$(/usr/bin/file "$MARKTEXT_BIN")
krate_file=$(/usr/bin/file "$KRATE_BIN")

if [[ "$host_arch" == arm64 && "$marktext_file" != *arm64* ]]; then
  die "MarkText is not ARM64 on an ARM64 Mac. Refusing a Rosetta-distorted comparison: $marktext_file"
fi
if [[ "$host_arch" == arm64 && "$krate_file" != *arm64* && "$krate_file" != *universal* ]]; then
  die "Krate is not ARM64/universal on an ARM64 Mac: $krate_file"
fi

expected_bundle_sha=f2f8027ed356e8217e2e1d2764a8befe2d02be58fef5f4e1ce4bd932f074be44
actual_bundle_sha=$(sha256_file "$KRATE_BUNDLE")
if [[ "${ALLOW_DIFFERENT_KRATE_BUNDLE:-0}" != 1 && "$actual_bundle_sha" != "$expected_bundle_sha" ]]; then
  die "Krate bundle hash differs from the historical 37 KB artifact. Set ALLOW_DIFFERENT_KRATE_BUNDLE=1 only for a separately labelled new comparison. Expected $expected_bundle_sha, got $actual_bundle_sha"
fi

version=$(/usr/bin/defaults read "$MARKTEXT_APP/Contents/Info.plist" CFBundleShortVersionString)
[[ "$version" == 0.17.1 ]] || die "expected MarkText 0.17.1, found $version"
krate_version=$($KRATE_BIN version 2>/dev/null || $KRATE_BIN --version)
krate_commit=$(printf '%s\n' "$krate_version" | /usr/bin/awk '$1 == "commit" {print $2; exit}')
[[ -n "$krate_commit" ]] || die "krate version did not report its source commit"
if [[ "$krate_commit" != "$BENCH_EXPECTED_KRATE_COMMIT"* && "$BENCH_EXPECTED_KRATE_COMMIT" != "$krate_commit"* ]]; then
  die "Krate binary commit $krate_commit does not match BENCH_EXPECTED_KRATE_COMMIT=$BENCH_EXPECTED_KRATE_COMMIT"
fi

for fixture in "$ROOT/fixtures/notes-5000.md" "$ROOT/fixtures/notes-50000.md"; do
  [[ -f "$fixture" ]] || die "missing fixture: $fixture (run prepare.sh)"
done

printf 'PASS\tinputs are present, version-pinned, and architecture-compatible\n'
printf 'MarkText\t%s\t%s\t%s\n' "$version" "$(sha256_file "$MARKTEXT_BIN")" "$marktext_file"
printf 'Krate CLI\tcommit=%s\t%s\t%s\n' "$krate_commit" "$(sha256_file "$KRATE_BIN")" "$krate_file"
printf 'Krate bundle\t%s\t%s bytes\n' "$actual_bundle_sha" "$(file_bytes "$KRATE_BUNDLE")"
