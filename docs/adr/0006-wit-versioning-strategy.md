# ADR-0006: WIT Versioning Strategy

**Status:** Accepted  
**Date:** 2026-05-04  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Krate Phase 2 introduces the first real UAPI modules: `io`, `fs`, `net`,
`time`, and `locale`. These modules are written in WIT and become the contract
between apps, SDKs, and host adapters.

The versioning rule must be clear before the UAPI is frozen. If one module needs
to change later, we should not force every other module to move with it. At the
same time, app authors should not need to guess which module versions work
together.

The Component Model already gives us package names with versions, such as
`krate:fs@0.1.0`. The decision is how Krate uses that version field.

---

## Decision

We will version each Krate WIT package with semver, starting at `0.1.0`, and
we will treat the Phase 2 `cli` world as the compatibility set that names the
exact module versions an app imports together.

For Phase 2 this means:

- `krate:io@0.1.0`
- `krate:fs@0.1.0`
- `krate:net@0.1.0`
- `krate:time@0.1.0`
- `krate:locale@0.1.0`
- `krate:app@0.1.0`

Patch releases may clarify docs or add compatible behavior. Breaking WIT shape
changes require a new minor version before `1.0`, and a new major version after
`1.0`.

---

## Alternatives considered

### One Global UAPI Version

Rejected. A single `krate:uapi@0.1.0` package would be simple at first, but it
would make small future changes expensive. A filesystem-only update should not
force network, locale, and time packages to move.

### No Versions Until v1.0

Rejected. The first public SDKs and sample apps need stable import names now.
Leaving versions out would make early app artifacts harder to reason about and
harder to migrate.

### Date-Based Versions

Rejected. Dates are useful for snapshots, but they do not tell app authors
whether a change is compatible. Semver is easier to explain and easier to check
in tooling.

---

## Consequences

### Positive

- Each UAPI module can evolve at its own pace.
- SDKs can document the exact WIT package versions they wrap.
- The `cli` world gives app authors one clear compatibility set.
- Future migration tools can compare package versions directly.

### Negative

- More package versions must be tracked in generated bindings and docs.
- The project must be disciplined about what counts as compatible.
- Version bumps need review, not casual edits.

### Neutral

- Phase 2 remains pre-`1.0`, so minor versions may still carry breaking changes.
  Those changes must still be explicit and documented.

---

## Revisiting

Revisit this if WIT tooling changes its versioning model, or if per-module
versioning creates more developer confusion than it removes. Any change must
include a migration plan for existing apps and SDKs.

---

## References

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/)
- [WIT packages](https://component-model.bytecodealliance.org/design/packages.html)
- [Semantic Versioning](https://semver.org/)
