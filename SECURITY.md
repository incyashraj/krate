# Security Policy

## Supported versions

Krate is pre-alpha (Phase 1). No versions are supported for production use.

When v1.0 ships, we will maintain the latest minor version with security
patches. Older minor versions will receive patches for 12 months after the
next minor release.

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

Email `security@krate.dev` with:

- A clear description of the vulnerability.
- Steps to reproduce.
- The potential impact.
- Any suggested mitigations you have in mind.

If you prefer encrypted communication, PGP key details will be posted at
`.github/security-pgp.asc` once a key is established.

We will acknowledge receipt within **72 hours** and provide an initial
assessment within **7 days**.

## Disclosure timeline

We follow coordinated disclosure:

| Day | Action |
|-----|--------|
| 0   | Report received; acknowledgement sent. |
| 7   | Initial assessment shared with reporter. |
| 30  | Fix in active development if confirmed. |
| 90  | Public disclosure, with or without fix (whichever comes first, unless exceptional circumstances). |

Credit is given to reporters unless they request anonymity.

## Bug bounty

No monetary bounty yet. We plan to add one when there is funding to do so
responsibly (estimated: Phase 6, when the marketplace ships).

## Scope

**Phase 1** has runnable proof-of-concept code. The current threat surface is
the `krate` CLI, the `krate-runtime` Wasmtime embedding, the temporary
`krate:phase1/host` imports, release workflows, and dependency declarations.

Do not run untrusted WASM through `krate` in Phase 1. Treat
`krate run foo.wasm` like running a local developer executable. The sandbox is
real, but Krate is not adversarially hardened yet.

See the Phase 1 threat model in `docs/book/src/phase1/threat-model.md`.

## Out of scope

- Vulnerabilities in third-party dependencies (report those upstream; if they
  affect Krate we still want to know).
- Social-engineering attacks against maintainers.
- Denial-of-service against GitHub infrastructure.
