#!/usr/bin/env bash
#
# Build the Krate player for Android (aarch64).
#
# Everything here exists because two traps already cost time on this repo:
# Homebrew's rustc shadows rustup's and has no Android std, so RUSTC must
# name the rustup toolchain outright; and the C build scripts (ring, zstd,
# sqlite, wasmtime) need the NDK compilers by env var or they try the host
# clang and fail.
#
# Prereqs, one time:
#   rustup target add aarch64-linux-android
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
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDKBIN/aarch64-linux-android$API_LEVEL-clang"
export CC_aarch64_linux_android="$NDKBIN/aarch64-linux-android$API_LEVEL-clang"
export CXX_aarch64_linux_android="$NDKBIN/aarch64-linux-android$API_LEVEL-clang++"
export AR_aarch64_linux_android="$NDKBIN/llvm-ar"

exec "$TC/bin/cargo" build -p krate-player-android \
  --target aarch64-linux-android --release "$@"
