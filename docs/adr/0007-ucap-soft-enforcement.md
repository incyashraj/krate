# ADR-0007: UCap v0.1 Soft Enforcement

**Status:** Accepted  
**Date:** 2026-05-04  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Krate apps need access to useful host features: files, network, time, locale,
stdin, stdout, and stderr. That access cannot be implicit. The whole point of
Krate is that the runtime sits between an app and the host, so access must be
declared, granted, checked, and explainable.

Phase 2 is still a CLI phase. There is no GUI grant prompt, signed bundle,
persistent policy database, or app store trust chain yet. We still need a real
permission model now, because every UAPI call added in this phase must pass
through the same shape that later phases will harden.

---

## Decision

We will implement UCap v0.1 as session-scoped soft enforcement: apps declare
capabilities in `manifest.toml`, the CLI resolves launch grants for one run, and
the runtime checks the required capability at every current UAPI entry before
calling a host adapter.

Phase 2 capability strings use this format:

```text
<module>.<action>[:<resource>]
```

Examples:

```text
io.stdout
fs.read:./data/**
net.connect:127.0.0.1:8080
```

Default-granted capabilities are low-risk developer conveniences such as
`io.stdout`, `io.stderr`, `io.args`, `time.clock`, and `locale.info`.
Filesystem and network access require explicit grants.

---

## Alternatives considered

### No Capability Enforcement Until Phase 3

Rejected. That would make Phase 2 samples easier, but it would teach the wrong
runtime shape. Retrofitting policy after apps already call host adapters directly
would be risky.

### Full Persistent Policy Database In Phase 2

Rejected. Persistent decisions, revocation, identities, signatures, and user
profiles belong to later phases. Phase 2 needs the UAPI boundary and launch-time
policy shape first.

### All Capabilities Require Explicit Grants

Rejected for Phase 2. Requiring grants for stdout, stderr, current time, and
locale would make simple CLI apps noisy without adding much safety. These
defaults can be revisited before v1.0.

---

## Consequences

### Positive

- Every meaningful host access path has a policy checkpoint.
- CLI tests can prove denied calls stop before host adapter work.
- Manifests become useful early, before bundle signing exists.
- Later UI prompts and policy databases can build on the same capability
  strings.

### Negative

- The model is not complete security yet. Manifests are unsigned, grants are
  per-run, and there is no revocation flow.
- Default grants need careful review before the API is frozen.
- Path matching and network scoping must be hardened over time.

### Neutral

- Phase 2 uses the term "soft enforcement" because it is real at the UAPI
  boundary but not yet backed by signed distribution or persistent identity.

---

## Revisiting

Revisit default grants before UAPI v0.1 freeze, before Phase 3 GUI prompts, and
again before signed bundles in Phase 6. Revisit immediately if tests reveal a
UAPI path that can reach a host adapter without a policy check.

---

## References

- [Krate Phase 2 Plan](../../Plan/Phase-2-Plan.md)
- [Generated UAPI reference](../book/src/reference/uapi/index.md)
- [Capability-Based Security](https://en.wikipedia.org/wiki/Capability-based_security)
