# Sample Evidence

This page explains how to record Phase 2 sample evidence.

The goal is simple: run the same three Rust sample apps on each desktop host and
compare the stdout hashes.

- `krate-clock`
- `krate-cat`
- `krate-curl`

If Linux, macOS, and Windows produce the same stdout for these fixed fixtures,
we have much better evidence that the UAPI behaves the same across hosts.

## Recorder

Run this from the repo root:

```bash
scripts/record-phase2-sample-evidence.sh
```

By default, it writes:

```text
target/phase2-sample-evidence/sample-evidence.md
```

You can choose another output path:

```bash
scripts/record-phase2-sample-evidence.sh target/phase2-sample-evidence/macos-arm64.md
```

For three-host comparison after collecting Linux, macOS, and Windows reports:

```bash
scripts/compare-phase2-sample-evidence.sh \
  target/phase2-sample-evidence/linux.md \
  target/phase2-sample-evidence/macos.md \
  target/phase2-sample-evidence/windows.md
```

If curl is blocked only because localhost binding is restricted on one host,
you can use:

```bash
scripts/compare-phase2-sample-evidence.sh \
  target/phase2-sample-evidence/linux.md \
  target/phase2-sample-evidence/macos.md \
  target/phase2-sample-evidence/windows.md \
  --allow-blocked-curl
```

## What It Runs

The recorder builds the local CLI and the three Rust sample components. Then it
runs:

1. `krate-clock` with fixed time, locale, and timezone
2. `krate-cat` against two small fixture files
3. `krate-curl` against a local HTTP fixture server

The output file records:

- git commit
- host OS and CPU architecture
- command text
- process exit code
- result status
- stdout SHA-256
- stderr SHA-256
- exact stdout snapshot

## Exit Use

For Phase 2 exit, collect one report from each required desktop host.

The important comparison is the stdout hash for each sample. Host metadata will
be different. The sample stdout hashes should match.

If a host cannot run the curl local-server fixture, record that as a blocker
instead of treating it as cross-host proof.
The recorder does this automatically when localhost binding is blocked: it keeps
clock and cat evidence, and marks curl as blocked.
The comparator can then enforce exact hash matches and fail fast on drift.
It also verifies two safety checks before hash comparison:

- each report matches the flag you pass (`--linux`, `--macos`, `--windows`)
- all three reports were recorded from the same git commit

## Hosted CI Evidence

Full hosted CI now records one sample evidence report per OS lane and uploads:

- `sample-evidence-ubuntu-latest`
- `sample-evidence-macos-latest`
- `sample-evidence-windows-latest`

Then CI runs:

- `Sample evidence compare`

The compare step currently uses `--allow-blocked-curl` so the lane can stay
useful when one hosted runner blocks localhost fixture sockets. Clock and cat
still require strict cross-host hash alignment.
When curl is blocked on one host, the comparator still checks the curl stdout
hashes from any hosts that did run it. The exception only covers the blocked
host; it does not hide drift between successful curl runs.
