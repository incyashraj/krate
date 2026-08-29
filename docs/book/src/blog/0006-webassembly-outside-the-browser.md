# WebAssembly for desktop apps: what actually works in 2026

**Published:** 2026-08-05

WebAssembly outside the browser has been "almost ready" for years. Some of that
is real progress and some is marketing, and the line between them is worth
drawing precisely if you are deciding whether to build on it.

Here is what we found building a desktop app runtime on it.

## What genuinely works

**One binary, three systems.** This is the promise and it holds. The same
`.wasm` bytes run unmodified on macOS, Windows, and Linux, on both Intel and
ARM. No conditional compilation, no per-platform CI matrix, no three sets of
release artifacts.

**The size is real.** A desktop app with a window, buttons, saved state, and
drawing is typically 15 to 40 KB. Not a typo, and not a hello-world -- that is
a working tool with persistence. An Electron equivalent is four orders of
magnitude larger.

**Startup is not a problem.** A component loads and paints its first frame in
tens of milliseconds. The old objection that Wasm is slow to start was about
large modules and cold JIT; for apps this size it does not apply.

**The component model is the part that matters.** Plain WebAssembly gives you a
sandbox with integers. The component model gives you typed interfaces across
the boundary -- strings, records, lists, results -- defined in WIT and generated
into bindings on both sides. That is what makes "the app can call
`canvas2d::fill_rect`" a real statement rather than a convention about memory
offsets.

## What does not work the way people imply

**WASI is not free.** WASI gives a guest a POSIX-shaped world: files, clocks,
environment variables, sockets. That is exactly what you do not want if the
point is a capability sandbox -- it hands over a filesystem and asks the host to
police it afterwards.

Avoiding it is harder than it sounds, because the Rust standard library pulls
WASI in through paths that look unrelated. `std::fs` obviously. But also, in
practice: an allocation-error handler, a panic formatter, a timer someone
called once. We spent real time tracking down twenty leaked `wasi:*` imports
that traced back to `Vec::with_capacity`'s out-of-memory path.

The fix is building the guest `#![no_std]` with your own allocator and panic
handler. It works, and it is not the getting-started experience anyone
advertises.

**There is no UI story.** The component model tells you how to pass a string
across the boundary. It says nothing about how a guest opens a window, draws a
button, or receives a click. Every project doing this invents its own
interfaces, and they are all incompatible.

That is a real cost and worth being clear-eyed about: choosing this means
choosing someone's UI world, not a standard.

**Crates assume std.** A large fraction of the ecosystem will not build
`no_std`. Image decoding, randomness, HTTP, JSON parsing -- for each one you
either find the rare `no_std`-compatible crate, or you bridge it. `getrandom`
needs a custom backend. Image decoding wants `zune-*` rather than `image`. None
of this is hard; all of it is unglamorous work nobody mentions in a blog post
about how great Wasm is.

## The honest summary

WebAssembly delivers on portability and size, completely and impressively. It
delivers nothing on user interface, and its default system interface actively
works against a capability model.

If you are building a desktop app runtime on it, budget most of your effort for
the parts Wasm does not give you: the UI interfaces, the host adapters for
three windowing systems, and keeping the standard library out of your guests.
The portable-bytes part is the easy half.

We think it is worth it, because the alternative is shipping three binaries and
a code-signing certificate. But "just use Wasm" understates the work by a lot.

[Krate](https://krate.tech) is what we built on it: apps as single
`.krate` files that run on Mac, Windows, and Linux with a capability wall in
front of them. The [source is public](https://github.com/incyashraj/krate),
including all of the unglamorous parts.
