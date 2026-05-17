# Phase 2 Retrospective Draft

**Status:** Draft until Phase 2 exit review passes.
**Written:** May 17, 2026
**Author:** incyashraj

This page is the working retrospective for Phase 2. It is not the final
retrospective yet. The final version should be written after the UAPI freeze
review, the outside walkthrough, and the last evidence bundle are complete.

## What Shipped

Phase 2 turned Layer36 from a runtime proof into a useful CLI platform slice.

The main shipped pieces are:

- a Phase 2 UAPI for `io`, `fs`, `net`, `time`, and `locale`
- manifest parsing and capability strings
- launch grants through `--grant`, `--auto-grant`, prompts, and cap dumps
- runtime policy checks before host adapter calls
- host adapter split across Linux, macOS, and Windows crates
- Rust sample apps for clock, cat, and curl
- Rust SDK packaging and smoke evidence
- TypeScript and Go SDK scaffolds with fixture evidence paths
- cross-host evidence recorders for samples, adapters, UCap, benchmarks, and
  language variants
- CI freshness checks for generated UAPI docs, freeze evidence, and freeze lock
- GitHub Pages docs for the current Phase 2 state

## What Did Not Ship Yet

These are still open before Phase 2 can close.

| Area | Current state | Next step |
|---|---|---|
| UAPI freeze | Strong draft | Run final freeze review after the remaining proof is collected. |
| Cross-host evidence | Partial | Keep Linux, macOS, and Windows reports aligned for one commit. |
| Go runtime parity | Experimental | Keep TinyGo smoke builds, but wait for import-pure Layer36 components before promotion. |
| Long fuzz soak | Partial | Run a longer self-hosted fuzz pass on the final candidate. |
| Timed outside walkthrough | Pending | Ask a Rust developer new to Layer36 to run the walkthrough and pass the filled-packet checker. |

## UAPI Lessons

The UAPI direction is still right. Apps should call a small Layer36 surface, not
direct host APIs. That keeps the later 6 by 6 host matrix possible.

The main lesson is that the WIT contract must be treated like a product surface,
not an internal file. We now generate reference docs, freeze evidence, and a
freeze lock because manual review alone is too easy to miss.

## Adapter Lessons

The adapter split is useful even before Phase 3. Moving filesystem, network,
time, locale, and stdio host calls behind adapter crates made the runtime less
host-shaped and easier to review.

The next lesson for Phase 3 is to keep this boundary strict. Windowing and input
will have more host differences than CLI calls, so host-specific behavior should
stay in adapter crates as early as possible.

## Binding Lessons

Rust is the strongest path today. It has working samples, SDK smoke evidence,
and the cleanest runtime path.

TypeScript is useful and should stay in the fixture lane. Its value is that many
app developers already know the language, but the generated binding shape needs
careful testing.

Go should stay experimental for runtime parity in Phase 2. The current TinyGo
artifacts build, but still import WASI host APIs. That is acceptable only because
the project now records the limitation clearly and keeps import-purity checks in
place.

## UCap Lessons

Capability checks are already doing real work. The deny-before-adapter matrix is
especially important because it proves denied calls stop before native host work
can happen.

The prompt flow is enough for CLI apps. Phase 3 should not reuse terminal-style
prompts for GUI apps. It needs a system UI grant flow with clear wording and a
better place to explain why an app wants access.

## Performance Lessons

The current dispatch path is small enough for Phase 2. Startup and dispatch
benchmarks exist, but the final threshold should be based on same-commit
cross-host evidence, not one local machine.

Phase 3 should start with frame-time and first-paint measurement from day one.
GUI performance problems become expensive if measurement arrives late.

## Documentation Lessons

The docs are now part of the product. The public site explains what works, what
is experimental, and what remains. That is important for a project this early
because honest docs prevent confusion.

The next improvement is to keep final review pages short and direct. Long plans
are useful for implementation, but exit pages should tell a reviewer what to run,
what passed, and what still needs judgment.

## Build Plan Changes To Carry Forward

- Keep evidence recorders for every formal phase gate.
- Keep generated reference pages under CI freshness checks.
- Keep Go runtime parity experimental until import-pure components exist.
- Keep Phase 3 blocked until Phase 2 has one filled outside walkthrough packet.
- Keep normal hosted CI cheap, with full proof runs manual or self-hosted.

## Phase 3 Notes Before Start

Before Phase 3 starts, confirm:

- Phase 2 exit evidence is green for the final commit.
- UAPI v0.1 is frozen and no Phase 3 work changes it.
- The Phase 3 kickoff issue links to this retrospective and the final exit
  bundle.
- Phase 3 starts with WIT and ADR work for UI, graphics, input, and accessibility
  before implementation.
