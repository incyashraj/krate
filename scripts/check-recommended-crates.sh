#!/usr/bin/env sh
# Build every crate the contract recommends, the way a guest actually builds.
#
# This exists because an image viewer port failed on advice I wrote. The
# contract said to use the `image` crate with pure-Rust features, which is true
# as far as it goes: no C, no -sys crates, builds clean for the host. It also
# requires `std` unconditionally, so linking it pulls wasi:cli, wasi:filesystem
# and wasi:io into the component, and the import check rejects it -- after the
# build succeeds, which makes it look like a Krate bug rather than a dependency
# choice. Four repair attempts went into rewriting app code that was never the
# problem.
#
# The advice had been checked for "is it pure Rust" and not for "does the built
# module stay free of wasi imports". That is the question this asks, and it has
# to be asked of the *compiled output*, not of whether the build succeeds: the
# first version of this script built each crate under no_std and passed `image`
# happily, because a crate that is merely linked pulls nothing in. The import
# only appears once the decoder is actually called.
#
#   sh scripts/check-recommended-crates.sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Each case: crate, version, and a line of code that exercises its real entry
# point. Exercising it is the whole point -- a crate that is only linked pulls
# nothing in, so a case that does not call the decoder proves nothing.
passed=0
failed=0

check() {
  name="$1"
  dep="$2"
  body="$3"

  dir="$WORK/$name"
  mkdir -p "$dir/src"
  cat > "$dir/Cargo.toml" <<EOF
[package]
name = "check_$name"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
$dep

[workspace]
EOF
  # An exported function, so the decoder is reachable and its imports are real
  # rather than dead-stripped. The allocator and panic handler mirror what the
  # guest SDK provides: a no_std cdylib has to own both, and owning them is
  # exactly what keeps std's versions -- and their wasi imports -- out.
  cat > "$dir/src/lib.rs" <<EOF
#![no_std]
extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};

struct Bump;

// Never frees. This module exists to be compiled and inspected, not run, so
// the simplest allocator that satisfies the trait is the right one.
static mut ARENA: [u8; 1 << 20] = [0; 1 << 20];
static mut NEXT: usize = 0;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = core::ptr::addr_of_mut!(ARENA) as *mut u8;
        let start = (NEXT + layout.align() - 1) & !(layout.align() - 1);
        if start + layout.size() > (1 << 20) {
            return core::ptr::null_mut();
        }
        NEXT = start + layout.size();
        base.add(start)
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOC: Bump = Bump;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

$body
EOF

  if ! (cd "$dir" && cargo build -q --release --target wasm32-wasip1 2>"$dir/err"); then
    # A duplicate `panic_impl` or `alloc_error_handler` is not a quirk of this
    # harness -- it is the finding. Those lang items come from std, so a crate
    # that collides with the ones above has linked std, and a guest that links
    # std imports the whole wasi surface and is rejected. `image` fails here.
    if grep -q "duplicate lang item" "$dir/err"; then
      echo "  $name: REQUIRES std; a guest using this is rejected at the import check"
      grep -m1 "duplicate lang item" "$dir/err" | sed 's/^/      /'
    else
      echo "  $name: FAILED to build"
      head -6 "$dir/err" | sed 's/^/      /'
    fi
    failed=$((failed + 1))
    return 0
  fi

  wasm="$dir/target/wasm32-wasip1/release/check_$name.wasm"
  # The same question the port's import gate asks: does anything outside
  # krate:* end up in the module.
  leaks="$(wasm-tools print "$wasm" 2>/dev/null | grep -oE '\(import "wasi[^"]*"' | sort -u || true)"
  if [ -n "$leaks" ]; then
    echo "  $name: LEAKS wasi imports; a guest using this is rejected"
    echo "$leaks" | sed 's/^/      /'
    failed=$((failed + 1))
  else
    echo "  $name: ok"
    passed=$((passed + 1))
  fi
}

# Without wasm-tools the import inspection silently finds nothing, and a
# missing tool would read as "every crate is clean" -- the exact false pass
# this script exists to prevent.
if ! command -v wasm-tools >/dev/null 2>&1; then
  echo "wasm-tools is not installed; cannot inspect module imports" >&2
  echo "install it with: cargo install wasm-tools" >&2
  exit 1
fi

echo "Building every crate the contract recommends:"

check png \
  'zune-png = { version = "0.5", default-features = false }
zune-core = { version = "0.5", default-features = false }' \
  'use alloc::vec::Vec;
pub fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut d = zune_png::PngDecoder::new(zune_core::bytestream::ZCursor::new(bytes));
    let pixels = d.decode_raw().ok()?;
    let (w, h) = d.dimensions()?;
    Some((w as u32, h as u32, pixels))
}'

check jpeg \
  'zune-jpeg = { version = "0.5", default-features = false }
zune-core = { version = "0.5", default-features = false }' \
  'use alloc::vec::Vec;
pub fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut d = zune_jpeg::JpegDecoder::new(zune_core::bytestream::ZCursor::new(bytes));
    d.decode().ok()
}'

echo
echo "passed: $passed   failed: $failed"
[ "$failed" -eq 0 ] || exit 1
