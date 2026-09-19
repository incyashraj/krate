# WebAssembly for desktop apps: what actually works in 2026

**Published:** 2026-08-05

**Updated:** 2026-09-20

WebAssembly outside the browser has been "almost ready" for years. Some of that
is real progress and some is marketing, and the line between them is worth
drawing precisely if you are deciding whether to build on it.

Here is what we found building a desktop app runtime on it.

## What genuinely works

**One app file, three systems.** Krate lets developers ship the same `.krate`
file on Mac, Windows, and Linux. The native runtime handles the platform;
the application bytes stay the same. You distribute one app artifact and
test its behaviour on the systems you support.

**The app does not need its own browser engine.** Krate puts the platform
implementation in a shared runtime installed once. Each app carries its own
code and assets. Our [notes comparison](https://krate.tech/reports/) measured
a 36.6 KiB notes-app bundle, excluding the 88.6 MiB shared runtime.
That historical bundle is not a size promise for current apps: a `.krate`
download can also carry source, SDK interfaces and assets.

**Measure startup with a real workload.** Module compilation, runtime caches,
window creation and loading the user's data all contribute. The same notes
comparison measured a 237.1 ms median warm open for a 50,000-line document
on an Apple M4 Mac. That is one workload, not a startup guarantee for every
WebAssembly app.

**The component model is the part that matters.** Plain WebAssembly gives you a
sandbox with integers. The component model gives you typed interfaces across
the boundary -- strings, records, lists, results -- defined in WIT and generated
into bindings on both sides. [WIT](https://component-model.bytecodealliance.org/design/wit.html)
describes the interface contract, not the implementation. That is what makes "the app can call
`canvas2d::fill_rect`" a real statement rather than a convention about memory
offsets.

## What does not work the way people imply

**Choose your host interfaces deliberately.** WASI is itself designed around
[capability-based security](https://github.com/WebAssembly/WASI/blob/main/docs/DesignPrinciples.md).
It does not automatically give an app access to the host filesystem. The
host decides which capabilities to supply.

Krate uses its own `krate:*` interfaces so app requests go through Krate's
permission checks. Our import policy rejects `wasi:*` imports. That is a
choice about the interfaces Krate exposes, not a claim that WASI cannot
support capability security.

Keeping those imports out takes work, because the Rust standard library can
introduce WASI dependencies through paths that look unrelated to filesystem
access. Even allocation and error-handling paths need checking. Our
[chart sample](https://github.com/incyashraj/krate/tree/main/apps/krate-chart)
documents one such allocation path.

Krate's Rust guest setup uses `#![no_std]` with an allocator and panic handler.
The [developer quickstart](../quickstart.md) provides the build path; the
[porting guide](../porting.md) explains how to assess an existing project.

**Desktop UI needs host interfaces.** The component model tells you how to pass a string
across the boundary. It says nothing about how a guest opens a window, draws a
button, or receives a click. Krate supplies those interfaces; a component
built for another host's UI API is not automatically a Krate app.

That is a real cost and worth being clear-eyed about: choosing this means
choosing someone's UI world, not a standard.

**Crates assume std.** A large fraction of the ecosystem will not build
`no_std`. Image decoding, randomness, HTTP, JSON parsing -- for each one you
either find the rare `no_std`-compatible crate, or you bridge it. `getrandom`
needs a custom backend. Image decoding wants `zune-*` rather than `image`. None
of this is hard; all of it is unglamorous work nobody mentions in a blog post
about how great Wasm is.

## The honest summary

WebAssembly gives us portable application code and a boundary between the
guest and its host. A usable desktop system still needs UI interfaces,
permission enforcement and platform adapters. Package size and responsiveness
depend on what the app and runtime do, so we measure them with actual workloads.

If you are building a desktop app runtime on it, budget most of your effort for
the parts Wasm does not give you: the UI interfaces, the host adapters for
three windowing systems, and keeping the standard library out of your guests.
The portable-bytes part is the easy half.

We think it is worth it because developers can distribute one application
file across operating systems. But "just use Wasm" understates the work by a lot.

[Krate](https://krate.tech) is what we built on it: apps as single
`.krate` files that run on Mac, Windows, and Linux with a capability wall in
front of them. The [source is public](https://github.com/incyashraj/krate),
including all of the unglamorous parts.

## Try it with your app

Start with the [developer quickstart](../quickstart.md#get-krate) to install
Krate and run an app. If you already have a project, use the
[porting guide](../porting.md) to check its dependencies and host API needs.
You can also create an app with AI in [Krate Studio](https://krate.tech/studio/)
and share the resulting `.krate` file.
