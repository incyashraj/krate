# ADR-0010: Locale and Timezone Discovery Fallbacks (Phase 2)

**Status:** Accepted  
**Date:** 2026-05-05  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Phase 2 needs stable locale and timezone behavior for CLI apps that use
`layer36:locale`. The first implementation relied mostly on `LC_ALL`, `LANG`,
and `TZ`, with strict normalization and deterministic formatting.

In practice, many host environments do not set all three values in a
consistent way. Some shells set only `LANGUAGE` or `LC_MESSAGES`. macOS often
exposes locale hints through `AppleLocale`. Unix systems without `TZ` still
carry timezone intent in `/etc/localtime` symlink targets.

Without defined fallback order, behavior can drift between hosts and make
cross-host test output or app behavior less predictable.

---

## Decision

We will keep deterministic normalization rules and expand environment discovery
with a strict fallback order in Phase 2.

Locale fallback order is:
`LC_ALL` → `LANG` → `LC_MESSAGES` → first `LANGUAGE` token → `AppleLocale`
→ default `en-US`.

Timezone fallback order is:
explicit override → `TZ` → Unix `/etc/localtime` zoneinfo symlink inference
→ default `UTC`.

All discovered values still pass through strict normalization before they reach
runtime-facing locale/timezone APIs.

---

## Alternatives considered

### Keep only `LC_ALL` / `LANG` / `TZ`

Rejected. Too brittle for real host environments and leaves obvious gaps where
locale or timezone is available but ignored.

### Use host-specific APIs only

Rejected for Phase 2 scope. Native API discovery per OS is useful long-term,
but adds complexity and platform-specific code paths too early.

### Use broad heuristics without strict normalization

Rejected. It risks accepting malformed values and making behavior harder to
reason about in tests and security review.

---

## Consequences

### Positive

- Better practical locale/timezone behavior when primary env vars are absent.
- More predictable cross-host behavior due to explicit fallback order.
- Keeps one shared implementation in `adapter-common` for this phase.

### Negative

- Still not full host-native locale/timezone fidelity.
- `/etc/localtime` inference helps only when localtime is a parseable zoneinfo
  symlink target.

### Neutral

- Later phases can replace or augment discovery with deeper OS-native paths
  without changing the normalization contract.

---

## Revisiting

Revisit when per-OS native locale/timezone discovery is introduced, or when
ICU4X-grade formatting and richer locale metadata become required at runtime.
Any revision must preserve deterministic fallback semantics for tests.

---

## References

- [GNU gettext `LANGUAGE` behavior](https://www.gnu.org/software/gettext/manual/html_node/The-LANGUAGE-variable.html)
- [POSIX locale environment variables](https://pubs.opengroup.org/onlinepubs/9699919799/)
- [tzdb zoneinfo layout](https://data.iana.org/time-zones/tz-link.html)

