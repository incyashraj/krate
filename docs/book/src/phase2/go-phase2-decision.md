# Go Phase 2 Decision

Go stays in Phase 2, but it is experimental for runtime parity.

That is the current decision.

## What Works

The Go SDK source is in the repo. The examples build with TinyGo:

- `layer36-clock`
- `layer36-cat`
- `layer36-curl`

The readiness recorder also captures tool versions, artifact hashes, and the
import check result:

```bash
scripts/record-phase2-go-readiness-evidence.sh
```

## What Does Not Work Yet

The compiled Go components still import `wasi:*` host APIs directly.

That means they are not promoted into the runtime fixture set yet. Layer36 Phase
2 requires promoted runtime fixtures to import `layer36:*` UAPI packages, so
policy checks stay in front of host access.

## Why We Are Not Forcing It

Forcing Go promotion now would weaken the boundary we are trying to prove.

The better decision is simple:

- keep Go examples and shape checks
- keep TinyGo smoke builds
- keep import-purity checks strict
- mark Go runtime parity as experimental for Phase 2
- carry import-pure Go runtime fixtures into the next phase if TinyGo tooling or
  our binding path becomes ready

This keeps Phase 2 honest. Rust and TypeScript continue to carry the current
runtime proof. Go remains visible, tested, and ready to advance without blocking
the UAPI freeze.

## Exit Meaning

For Phase 2 exit, Go is considered:

- usable as an SDK source and TinyGo build-smoke track
- not yet a runtime parity track
- explicitly experimental until compiled artifacts pass the Layer36 import check

The command that decides promotion is still:

```bash
scripts/promote-phase2-go-runtime-fixtures.sh
```

If it promotes the fixtures later, this decision can be revisited.
