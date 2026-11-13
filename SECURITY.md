# Security Policy

## Supported versions

Krate is in public beta. Current release: 0.5.4; see the
[releases page](https://github.com/incyashraj/krate/releases/latest).

Before 1.0, security fixes ship in the next release. The installer and the
in-app updater always fetch the latest release, so updating is how you get
a fix.

When v1.0 ships, we will maintain the latest minor version with security
patches. Older minor versions will receive patches for 12 months after the
next minor release.

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

Email `yashraj@krate.tech` with:

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
responsibly.

## Scope

In scope: the `krate` command line and runtime, the `.krate` format and its
validator, the permission wall (`crates/policy`, `crates/runtime`), the
desktop and browser players, Krate Studio, the release and install scripts,
and the hub at `hub.krate.tech` and `krate.tech`.

The sandbox is real: an app reaches only the capabilities the person
granted, and imports no operating-system calls of its own. Krate is not yet
adversarially hardened, so treat an app from someone you do not know with
the care you would give any program, and report anything that gets past the
wall.

How the wall works, written from the source:
[krate.tech/security](https://krate.tech/security/). The written threat
models are in [docs/book/src/phase2/threat-model-v0-2.md](docs/book/src/phase2/threat-model-v0-2.md)
and [docs/book/src/phase1/threat-model.md](docs/book/src/phase1/threat-model.md).

## Out of scope

- Vulnerabilities in third-party dependencies (report those upstream; if they
  affect Krate we still want to know).
- Social-engineering attacks against maintainers.
- Denial-of-service against GitHub infrastructure.
