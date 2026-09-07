# ADR-0017: Which Wasmtime line Krate ships, and when it moves

**Status:** Accepted
**Date:** 2026-09-07
**Authors:** @incyashraj
**Supersedes:** -
**Superseded by:** -

---

## Context

Wasmtime is the engine every Krate app runs inside. Which version ships is a
security decision, not a maintenance chore, and it had no written policy --
so it was being made one advisory at a time.

The facts as of this decision, each checked rather than recalled:

- The workspace and the runtime crate both pin **46.0.3**
  (`Cargo.toml`, `crates/runtime/Cargo.toml`).
- `cargo deny check advisories` passes. 46.0.2 was named by RUSTSEC-2026-0268
  and RUSTSEC-2026-0269; 46.0.3 is the patch, taken in c99aabefd.
- **48.0.0 is an LTS release and requires Rust 1.95.** Krate's
  `rust-toolchain.toml` declares **1.94.1**, so moving to 48 is a toolchain
  move as well as an engine move.

The register (IC-233) also names a reasoning trap worth writing down: the two
advisories concern WASI surfaces that Krate's custom-world dependency path
does not reach, and it would have been easy to call the package unaffected
and skip the patch. That inference is not sound. A feature Krate does not use
today is not a guarantee about the code paths compiled into the binary, and
"we do not call it" is not the same as "it cannot be reached".

## Decision

**Patch releases on the current line are taken immediately.** A patch that
clears an advisory is not scheduled or batched against other work. 46.0.2 to
46.0.3 is the shape: read the advisory, take the patch, run the suite, ship.

**Reachability never substitutes for patching.** Krate does not skip a
security patch on the argument that the affected surface is unused. Absence
of a feature in our world is evidence about intent, not about the compiled
binary. If a patch is genuinely impossible to take, the reason is written in
the release evidence -- it is never inferred quietly.

**The LTS move is qualified, not assumed.** Wasmtime 48 LTS brings Rust 1.95
with it. Both move together, through the full conformance suite: the Krate
world contract, AOT and cache invalidation, bundle format compatibility (a
`.krate` written before the move must still open after it), the performance
comparison, and the browser and mobile builds. A green workspace test run is
not qualification.

**Old files keep working, or the move does not happen.** Krate's promise is
that a file someone was sent keeps opening. An engine move that strands
existing bundles is not a move Krate makes, whatever else it offers.

**Rollback is part of the move, not a reaction to it.** The previous line
stays taken and tested until the new one has shipped and settled, so a
regression is a revert rather than a scramble.

**Advisory monitoring stays automated.** `cargo deny check advisories` runs
in CI on every full run, which is what turned RUSTSEC-2026-0268 from a thing
somebody might read into a thing that fails a build.

## Consequences

Staying on 46.x while 48 is the LTS means Krate is not on the longest-support
line. That is deliberate: 48 costs a toolchain bump and a full re-qualification
across six platforms, two of which (browser, mobile) are mid-flight, and doing
it badly risks the one promise -- that old files keep opening -- that the LTS
was meant to protect.

The cost of the policy is that patch adoption has to be fast, because the
fallback of "we are on LTS so we can wait" is not available. That is the
trade, and CI's advisory check is what makes it survivable.

When 48 is qualified, this ADR is superseded by one that records the
measurements the move was made on, not merely the decision to make it.
