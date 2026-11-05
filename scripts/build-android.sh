#!/usr/bin/env bash
#
# Build the Krate player for Android: 64-bit ARM by default, or 32-bit ARM
# with KRATE_ANDROID_ABI=armeabi-v7a. Many low-cost phones run a 32-bit
# system even on a 64-bit chip (a Galaxy M11 lists only armeabi-v7a), and a
# Player without that library cannot even be installed there. The 32-bit
# one runs apps on wasmtime's interpreter, as iOS does: there is no 32-bit
# ARM compiler backend. scripts/package-android.sh puts every built ABI in
# one APK.
#
# Everything here exists because two traps already cost time on this repo:
# Homebrew's rustc shadows rustup's and has no Android std, so RUSTC must
# name the rustup toolchain outright; and the C build scripts (ring, zstd,
# sqlite, wasmtime) need the NDK compilers by env var or they try the host
# clang and fail.
#
# Prereqs, one time:
#   rustup target add aarch64-linux-android armv7-linux-androideabi
#   sdkmanager "ndk;27.2.12479018"
#   Build apps/krate-gram and apps/krate-wall first (krate check-app) --
#   the player embeds gram as its demo and the wall sheet as its chrome.

set -euo pipefail

NDK_VERSION="${NDK_VERSION:-27.2.12479018}"
# 26 (Android 8.0) is the floor: cpal's AAudio backend links libaaudio,
# which the NDK only ships from API 26 up. Trying 24 fails at link time.
API_LEVEL="${ANDROID_API_LEVEL:-26}"
SDK="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
NDKBIN="$SDK/ndk/$NDK_VERSION/toolchains/llvm/prebuilt/darwin-x86_64/bin"

case "${KRATE_ANDROID_ABI:-arm64-v8a}" in
  arm64-v8a) TARGET=aarch64-linux-android CLANG=aarch64-linux-android ;;
  armeabi-v7a) TARGET=armv7-linux-androideabi CLANG=armv7a-linux-androideabi ;;
  *) echo "error: KRATE_ANDROID_ABI is arm64-v8a or armeabi-v7a" >&2; exit 2 ;;
esac
# Cargo's per-target variables spell the triple in capitals with
# underscores; the cc crate's, in lower case with underscores.
UPPER=$(echo "$TARGET" | tr 'a-z-' 'A-Z_')
LOWER=$(echo "$TARGET" | tr '-' '_')

if [ ! -d "$NDKBIN" ]; then
  echo "error: NDK $NDK_VERSION not found under $SDK/ndk" >&2
  exit 1
fi

# The pinned rustup toolchain, never whatever `cargo` resolves to on PATH.
CHANNEL=$(sed -n 's/^channel = "\(.*\)"/\1/p' "$(dirname "$0")/../rust-toolchain.toml")
HOST=$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')
TC="$HOME/.rustup/toolchains/$CHANNEL-${HOST:-aarch64-apple-darwin}"
if [ ! -d "$TC" ]; then
  echo "error: rustup toolchain $CHANNEL not found at $TC" >&2
  exit 1
fi

export RUSTC="$TC/bin/rustc"
export "CARGO_TARGET_${UPPER}_LINKER=$NDKBIN/$CLANG$API_LEVEL-clang"
export "CC_$LOWER=$NDKBIN/$CLANG$API_LEVEL-clang"
export "CXX_$LOWER=$NDKBIN/$CLANG$API_LEVEL-clang++"
export "AR_$LOWER=$NDKBIN/llvm-ar"

exec "$TC/bin/cargo" build -p krate-player-android \
  --target "$TARGET" --release "$@"
