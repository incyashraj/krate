# The `#![no_std]` guest — deterministic `krate:*`-only apps (2026-07-27)

## Why

The Tier B experiment (`Plan/Tier-B-Experiment-2026-07.md`) proved an AI can
author arbitrary Krate apps, but 3/8 leaked `wasi:*` imports. Root cause,
confirmed by bisection: generated apps are built as **`std` binaries on
`wasm32-wasip1`**. `std`'s runtime (`lang_start`, panicking, thread init,
`std::io`) carries latent `wasi:*` imports. Whether dead-code elimination
strips them is luck-dependent — which is why the leak is intermittent
(`opt-level = "s"` cleared one app; the WIT `fs-error` string was a red herring
that did not clear the rest).

The only **deterministic** fix: the guest never links `std`. A `#![no_std]` +
`alloc` guest has no std runtime to leak, so *every* app is `krate:*`-only by
construction — 8/8, not by DCE luck.

## Scope (all on `slice/no-std-guest` off certified main)

1. **Guest SDK (`crates/bindings-rust`)** becomes `#![no_std]` + `extern crate
   alloc`.
   - `Vec`, `String` come from `alloc`, not `std` — keep them via
     `alloc::{vec::Vec, string::String}`.
   - Replace any `std::io` / `std::` path. The SDK's stdio/fs/streams already
     go through the generated `krate:*` bindings, so this is mostly import
     rewrites, not logic changes.
   - `bindings.rs` (wit-bindgen output) already targets `wit_bindgen_rt` and is
     `no_std`-compatible; verify its `use` paths.
   - `panic_handler`: a no_std cdylib needs one. Provide a minimal
     `#[panic_handler]` that aborts (matches `panic = "abort"`), gated so it
     does not conflict when the crate is built for tests on the host.

2. **Generator templates (`crates/author`)**: both the CLI and GUI `src/lib.rs`
   templates start with `#![no_std]` + `extern crate alloc`, and the generated
   `Cargo.toml` keeps `panic = "abort"`, `lto = true`, `opt-level = "s"`.

3. **Samples (`apps/krate-checklist`, `apps/krate-notes`, and the CLI samples)**
   convert to `#![no_std]` so the shipped samples are the reference for the
   discipline the SDK now enforces, not sidesteps.

4. **The author CONTRACT.md** (in `crates/cli`): update the "one hard rule"
   text — the constraint is now structural (`#![no_std]` handles it), so the
   contract tells the agent it does not have to hand-avoid `std`; the template
   already is `no_std`.

## Proof (the bar for "done")

- The 3 previously-leaking Tier B apps (temp-convert, reverse-file, tip-calc),
  rebuilt on the new templates, import only `krate:*`.
- Re-run the full 8-request Tier B batch: target 8/8 clean.
- Samples still build and run on all 3 OS (byte-identical where they were).
- Full 3-OS matrix green.
- The `create` import-check still rejects anything that somehow leaks (defence
  in depth stays).

## Risks / watch-items

- A `no_std` cdylib panic handler must not double-define when host unit tests
  compile the crate for the host target — gate with
  `#[cfg(target_arch = "wasm32")]` or a feature.
- wit-bindgen's generated code must not pull `std`; if it does, pin the
  generate options (`std_feature` off) and regenerate.
- Keep the change behavioural-neutral: same `krate:*` surface, same manifests,
  same runtime — only the guest's std linkage changes.
