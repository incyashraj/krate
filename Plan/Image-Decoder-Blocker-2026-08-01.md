# The image decoder blocker, and the one-line fix

An image viewer can now be expressed in Krate: the file picker exists, the
image widget draws on all three systems, and the contract documents both. The
port still does not build, and the reason is worth writing down because it is
not about the app.

## What happens

`zune-png` and `zune-jpeg` are the pure-Rust decoders, and they work. Under
`#![no_std]` they compile for wasm and import nothing outside `krate:*`.

A windowed app cannot be `#![no_std]`. The generated `src/bindings.rs` contains
`impl std::error::Error` for every error enum, so the crate must link std.

With std linked, adding zune to an otherwise clean GUI app takes wasi imports
from **0 to 4**:

```
wasi_snapshot_preview1  environ_get
wasi_snapshot_preview1  environ_sizes_get
wasi_snapshot_preview1  fd_write
wasi_snapshot_preview1  proc_exit
```

Isolated with one variable changed: `krate-notes` builds clean, and the same
crate with zune added and one decode call does not.

Those four are std's panic path -- format the message, write it to stderr,
exit. A decoder panics on malformed input somewhere, and that reachable panic
drags the whole path in.

## What would fix it

`-Zbuild-std-features=panic_immediate_abort` removes the message and exit
entirely. It needs nightly, and this project pins stable 1.91.1, so it is not
available today. This was tested; the first run printed a `0` that turned out
to be an empty target directory rather than a clean build, which is worth
noting because the number looked like success.

## The fix

`wit-bindgen` already has the option. Setting

```toml
[package.metadata.component.bindings]
std_feature = true
```

puts every generated `impl std::error::Error` behind `#[cfg(feature = "std")]`,
which nothing turns on. The bindings stop needing std, so a windowed app can be
`#![no_std]` -- where zune was clean all along.

Proven end to end on the failed candidate: it builds, and the component imports
**zero** `wasi:*` and 17 `krate:*` interfaces. The GUI scaffold sets it now, so
no future port meets this.

A `no_std` guest still has to own its allocator, `#[panic_handler]`, and the
`mem*` intrinsics, or componentization fails on an unresolved `env::memcmp`.
The porting agent wrote all three correctly on its own.

Two alternatives were considered and are worse. Decoding in the host puts an
image parser back in the trusted runtime, which is exactly what the
pixels-not-PNG design avoided. Writing a panic-free decoder by hand fixes one
format for one app.

## What this does not block

Everything else in the viewer works: the picker returns a token, the widget
accepts pixels, all three hosts draw them, and the app builds and passes the
import check once the decoder is removed. The gap is one dependency, in one
direction, for one reason.

## The cost of finding out

Three port runs, roughly two hours. Every one failed on my own documentation
rather than on the app:

- The contract recommended `image`, which requires std unconditionally.
- The contract's hard rule said `format!` and `Vec` leak wasi. They do not.
  That sent the agent to `#![no_std]`, which broke the bindings, which produced
  errors that read as though its own code was wrong.
- The contract named `ImagePixels` without saying which module it lives in.

Each is fixed and each now has a test. The lesson is the same one the `-sys`
detection taught this morning: advice that sounds right and was never run
against the real target is worse than no advice, because the agent has no
reason to doubt it.
