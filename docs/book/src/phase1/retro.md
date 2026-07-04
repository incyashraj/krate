# Phase 1 Retrospective

**Status:** Engineering closeout complete; external validation still pending.

Phase 1 proved the smallest useful Krate loop: build one WASM component once,
run the exact same bytes through the Krate runtime on Linux, macOS, and
Windows, and package release artifacts that a real user can download and verify.

## What Shipped

- `crates/runtime` embeds Wasmtime with Component Model support.
- `crates/cli` ships the first `krate` binary with `run`, `version`, and
  `doctor` commands.
- A temporary `krate:phase1/host` WIT interface supports `print` and `exit`
  for the hello-world proof.
- `scripts/test-phase1.sh` validates runtime behavior locally and in CI.
- CI builds one shared hello `.wasm` fixture, records its SHA-256 hash, and runs
  that same artifact on Linux, macOS, and Windows.
- `v0.1.0-rc1` published five platform archives plus `SHA256SUMS`.
- Quickstart, architecture notes, benchmark baseline, Threat Model v0.1,
  ADR-0002, and ADR-0003 are published.

## What Worked

The shared-fixture CI design was the right proof. Earlier runs compared
host-built `.wasm` hashes and failed for a reason that was true but not useful:
different hosts can produce different component bytes. The real Krate promise
is that one app artifact runs everywhere, so CI now tests that directly.

Keeping Phase 1 narrow also helped. The runtime only has the
temporary host interface it needs, and UAPI design is left for Phase 2 where it
can be reviewed as a platform contract instead of a quick demo surface.

## What Took Longer Than Expected

The GitHub Actions path uncovered real launch details early: Pages needed manual
configuration, the dependency audit action lagged behind CVSS 4.0 advisories,
and artifact upload paths had to be made explicit for hidden `.wasm` files.

Release packaging also forced the project to prove naming, archive layout, and
checksum publishing sooner than planned. That was useful pressure; it made the
runtime feel less like a local experiment and more like a product foundation.

## What Changed In The Plan

Krate remains the product name, while `layer6x6` remains the development repo
name for now. The 6x6 framing is still the strategic map; Krate is the name of
the platform that grows out of solving that matrix.

The Phase 1 release gate now explicitly accepts RC tags such as `v0.1.0-rc1`.
That keeps early platform proof honest without pretending the API is stable.

## Open Risks

- One external user still needs to complete the quickstart in 10 minutes or
  less.
- Phase 0 external gates remain open: Discord, public announcement, domain, and
  external contributor PR.
- GitHub branch protection exists as a ruleset, but direct owner bypass should
  be treated as temporary while the project is founder-only.
- GitHub Actions is warning that several actions still run on Node.js 20; this
  is not failing today, but it should be cleaned up before the forced Node.js 24
  transition.

## Phase 2 Readiness

The engineering foundation is ready for Phase 2 design work. The next durable
choices are WIT versioning, UCap enforcement shape, host adapter boundaries, and
the first real UAPI modules: `io`, `fs`, `net`, `time`, and `locale`.

Do not freeze Phase 2 WIT quickly. The first app examples should pressure-test
the interfaces before the project treats them as stable v0.1 contracts.
