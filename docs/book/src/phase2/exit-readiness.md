# Phase 2 Exit Readiness

This page explains the quick readiness command for Phase 2.

The command does not decide that Phase 2 is complete. It reads the exit ledger
and gives a plain summary of what is done, what has proof in progress, and what
is still blocked.

```bash
scripts/phase2-exit-readiness.sh
```

For the full review view, including every open proof item and its next step:

```bash
scripts/phase2-exit-readiness.sh --all
```

It reports:

- how many exit gates are tracked
- how many are fully done
- how many are strong drafts
- how many have proof in progress
- how many are pending or blocked
- which gates still need final proof or a decision
- with `--all`, the next step for every open gate

## Why This Exists

Phase 2 has many proof files now. That is good, but it can become hard to see
the whole picture from one page.

The readiness command keeps the answer repeatable. Instead of guessing from
memory, it reads `docs/book/src/phase2/exit-evidence.md` and prints the current
gate state.

The default output stays short so daily progress checks are easy to read. The
`--all` output is for exit review and handoff, where hiding the lower-priority
open gates would make the status less clear.

## Current Shape

The important split is:

- Fully done gates are complete for the current Phase 2 scope.
- Strong draft and partial gates have real work behind them, but still need a
  final review, cross-host proof, or a human decision.
- Pending and blocked gates are the hard remaining items.

This matches the current state of Phase 2: the runtime and UAPI shape are
strong, while formal exit still needs proof from more than one machine and one
outside walkthrough. Go now has an explicit Phase 2 decision: it stays tested,
but runtime parity is experimental until its compiled components import only
`krate:*`.

## When To Run It

Run it before:

- deciding whether Phase 2 is ready to freeze
- asking what is left in Phase 2
- creating a handoff status file
- opening the Phase 3 kickoff issue
- updating the Phase 2 retrospective draft after final review

For deeper review, also run:

```bash
scripts/record-phase2-exit-bundle.sh --strict
scripts/check-phase2-exit-evidence.sh
scripts/check-phase2-freeze-decision.sh
scripts/phase2-exit-readiness.sh --all
```

The readiness command is the map. The exit bundle is the evidence packet.
The freeze decision packet keeps the final human decision separate from the
automated checks.
