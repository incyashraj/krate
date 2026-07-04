# Adapter Boundary

Phase 2 has one rule for host work:

The runtime decides policy. The adapter touches the host.

That means the runtime can check grants, normalize paths, choose limits, and map
errors. When it needs the real machine, it must go through the host adapter
crate for the current OS.

## Current Shape

```text
Krate app
    |
    v
Phase 2 UAPI call
    |
    v
Runtime policy check
    |
    v
Runtime host wrapper
    |
    v
Linux, macOS, or Windows adapter crate
    |
    v
Native OS call
```

The wrapper step matters. It gives us one clear place to keep Phase 2 behavior
stable while each host can do the native work in its own crate.

## What Is Guarded

The current boundary check covers 34 runtime wrappers:

| Area | Count | Examples |
|------|------:|----------|
| Filesystem | 16 | open, read, write, stat, list, remove, rename, canonicalize |
| Network | 5 | resolve address, connect TCP, apply timeouts, write request, read response |
| IO | 6 | stdin, stdout, stderr, stream writes, stream flushes |
| Time | 2 | clock discovery, sleep |
| Locale | 5 | locale discovery, timezone, date format, number format |

For each wrapper, `check-adapter-boundary` verifies two things:

- the runtime wrapper calls `host_os_adapter::*` on supported desktop hosts
- Linux, macOS, and Windows adapter crates expose the matching function

This is not a security proof by itself. It is a drift guard. If future runtime
work accidentally bypasses the adapter split, CI should catch it early.

## Commands

Run this from repo root:

```bash
scripts/check-adapter-boundary.sh
```

The same check runs in hosted CI and in the self-hosted full gate.

## Why This Helps Phase 2

The final Krate goal is one app contract across many systems. We do not get
there by hiding every OS difference. We get there by keeping OS-specific work
behind a small boundary and making the shared contract clear.

This guard keeps that direction honest for the CLI UAPI slice.
