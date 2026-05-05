# ADR-0009: Sandbox Link-Semantics Guardrails

**Status:** Accepted  
**Date:** 2026-05-05  
**Authors:** @incyashraj  
**Supersedes:** -  
**Superseded by:** -

---

## Context

Phase 2 relies on filesystem sandboxing for all app-visible file operations.
Before native host I/O, the runtime resolves logical paths under a configured
sandbox root and checks capabilities.

That model is not enough by itself if link-like path segments can redirect path
traversal at the filesystem layer. On Unix this is mostly symbolic links. On
Windows, reparse points include symbolic links and junction-style redirects.
If we only guard one of those forms, behavior can drift between hosts and leave
surprising sandbox traversal gaps.

---

## Decision

We will treat link semantics as blocked during sandbox traversal across hosts.
The runtime must deny path traversal when any visited segment is a symbolic
link on Unix-like hosts, or a reparse-point segment on Windows.

Final-file open paths also keep no-follow behavior (`O_NOFOLLOW` on Unix and
`FILE_FLAG_OPEN_REPARSE_POINT` on Windows), so both traversal-time and
open-time checks stay aligned.

---

## Alternatives considered

### Check only final path segment

Rejected. It protects against one class of swap, but still allows traversal
through linked parent segments, which can change target resolution before final
open.

### Canonicalize once and trust that result

Rejected as a full strategy. Canonicalization is useful, but by itself it does
not express a policy about link semantics, and can drift across host behavior.

### Keep Unix symlink checks and skip Windows reparse checks

Rejected. It creates host parity gaps in the exact place where Layer36 needs
predictable cross-platform sandbox behavior.

---

## Consequences

### Positive

- Sandbox traversal rules are more consistent across Linux, macOS, and Windows.
- Junction-style path hops on Windows are blocked by policy, not by accident.
- Runtime behavior is easier to reason about in security reviews and tests.

### Negative

- Some legitimate workflows that rely on in-sandbox links are denied in Phase 2.
- Windows behavior is intentionally conservative, which may require later
  allowlist-style refinement.

### Neutral

- This does not replace later hardening work such as persistent policy state,
  signed bundles, or broader TOCTOU defenses planned in later phases.

---

## Revisiting

Revisit if we introduce a richer link policy in later phases, or if we need
explicit allowlist behavior for trusted in-sandbox links. Any revision must
preserve cross-host parity and keep default behavior safe.

---

## References

- [Rust `std::fs::symlink_metadata`](https://doc.rust-lang.org/std/fs/fn.symlink_metadata.html)
- [Rust `std::os::windows::fs::MetadataExt`](https://doc.rust-lang.org/std/os/windows/fs/trait.MetadataExt.html)
- [Windows file attribute constants](https://learn.microsoft.com/windows/win32/fileio/file-attribute-constants)

