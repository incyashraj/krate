#!/usr/bin/env bash
#
# Lint the phone code on the phones' own targets: `android` (the Android
# adapter and Player for 64-bit ARM, plus the Player's Java against
# android.jar) or `ios` (the iPhone adapter and Player for the simulator
# and a device). Their real code sits behind cfg(target_os = ...), so a
# lint on the host never reads it -- and with no lane it had drifted
# (K-1205, K-1206). CI's phones job runs exactly this; run it here first.
#
#   sh scripts/lint-phones.sh android   # needs the NDK and an Android SDK
#   sh scripts/lint-phones.sh ios       # needs Xcode
#
# Both Players embed Krate's home and the wall sheet. A plain cargo build
# of each is enough to lint against; a Player you install needs them built
# by `krate check-app`, which makes them components.

set -eu
cd "$(dirname "$0")/.."

CHANNEL=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
HOST=$(rustc -vV | sed -n 's/^host: //p')
TC="$HOME/.rustup/toolchains/$CHANNEL-$HOST"
# Homebrew's rustc has no phone std; name the pinned rustup one outright.
[ -x "$TC/bin/rustc" ] && export RUSTC="$TC/bin/rustc"

for app in krate-home krate-wall; do
  wasm="apps/$app/target/wasm32-wasip1/release/$(echo "$app" | tr - _).wasm"
  [ -f "$wasm" ] || cargo build -q --release --target wasm32-wasip1 \
    --manifest-path "apps/$app/Cargo.toml"
done

case "${1:-}" in
  android)
    SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
    NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-$(ls -d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -1)}}"
    case "$(uname -s)" in
      Darwin) PREBUILT=darwin-x86_64 ;;
      *) PREBUILT=linux-x86_64 ;;
    esac
    NDKBIN="$NDK/toolchains/llvm/prebuilt/$PREBUILT/bin"
    [ -d "$NDKBIN" ] || { echo "error: no NDK at $NDK" >&2; exit 1; }
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDKBIN/aarch64-linux-android26-clang"
    export CC_aarch64_linux_android="$NDKBIN/aarch64-linux-android26-clang"
    export CXX_aarch64_linux_android="$NDKBIN/aarch64-linux-android26-clang++"
    export AR_aarch64_linux_android="$NDKBIN/llvm-ar"
    # missing_const_for_thread_local fires on every thread_local! in the
    # workspace on this target, initializers already `const` included: a
    # false positive in the macro's Android expansion, not a finding.
    cargo clippy -p krate-adapter-android -p krate-player-android --all-targets \
      --target aarch64-linux-android --release -- -D warnings \
      -A clippy::missing_const_for_thread_local
    JAR=$(ls -d "$SDK"/platforms/android-*/android.jar 2>/dev/null | sort -V | tail -1)
    [ -n "$JAR" ] || { echo "error: no android.jar under $SDK/platforms" >&2; exit 1; }
    # As scripts/package-android.sh: macOS's /usr/bin/javac is a stub.
    if [ -z "${JAVA_HOME:-}" ] && command -v brew >/dev/null; then
      export JAVA_HOME="$(brew --prefix openjdk@21)/libexec/openjdk.jdk/Contents/Home"
    fi
    [ -n "${JAVA_HOME:-}" ] && export PATH="$JAVA_HOME/bin:$PATH"
    CLASSES=$(mktemp -d)
    javac -source 8 -target 8 -Xlint:-options -Werror -bootclasspath "$JAR" \
      -d "$CLASSES" $(find crates/player-android/android/java -name '*.java')
    rm -rf "$CLASSES"
    ;;
  ios)
    for target in aarch64-apple-ios-sim aarch64-apple-ios; do
      cargo clippy -p krate-adapter-ios -p krate-player-ios --all-targets \
        --target "$target" -- -D warnings
    done
    ;;
  *)
    echo "usage: sh scripts/lint-phones.sh android|ios" >&2
    exit 2
    ;;
esac
echo "phones ($1): clean"
