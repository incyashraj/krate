#!/bin/sh
# Type-check krate-cli and krate-runtime for another OS from this machine:
#   sh scripts/cross-cc/check.sh x86_64-unknown-linux-gnu
#   sh scripts/cross-cc/check.sh x86_64-pc-windows-msvc
# Nothing is linked, so C-backed crates (ring, zstd-sys, v4l2-sys) only need
# a compiler and headers, which zig carries for both targets. The Linux
# system libraries found through pkg-config get stub .pc files: a type-check
# asks whether they exist, never what is in them.
set -e
target="$1"
here=$(cd "$(dirname "$0")" && pwd)
case "$target" in
  x86_64-unknown-linux-gnu)
    zl=$(zig env | sed -n 's/.*\.lib_dir = "\(.*\)",/\1/p')
    inc="$zl/libc/include"
    export CC_x86_64_unknown_linux_gnu="$here/linux-cc"
    export AR_x86_64_unknown_linux_gnu="$here/linux-ar"
    export BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_gnu="--target=$target -nostdinc -isystem $zl/include -isystem $inc/x86-linux-gnu -isystem $inc/generic-glibc -isystem $inc/x86-linux-any -isystem $inc/any-linux-any"
    export PKG_CONFIG_ALLOW_CROSS=1
    export PKG_CONFIG_PATH="$here/pkgconfig"
    export PKG_CONFIG_LIBDIR="$here/pkgconfig"
    ;;
  x86_64-pc-windows-msvc)
    export CC_x86_64_pc_windows_msvc="$here/windows-cc"
    export AR_x86_64_pc_windows_msvc="$here/windows-ar"
    ;;
  *)
    echo "unknown target: $target" >&2
    exit 2
    ;;
esac
exec cargo check -q -p krate-cli -p krate-runtime --all-targets --target "$target"
