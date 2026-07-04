# UAPI Freeze Review Evidence

This page explains the repeatable review report for the Phase 2 UAPI freeze
candidate.

The freeze candidate is the current set of WIT files under `wit/krate/phase2`.
Those files define the public contract for `io`, `fs`, `net`, `time`, and
`locale`. Once we call that contract frozen, app authors should be able to build
against it without names, error shapes, resources, or capability meanings
changing underneath them.

## What The Recorder Checks

Run:

```bash
scripts/record-phase2-uapi-freeze-review.sh --strict
```

The recorder writes:

```text
target/phase2-uapi-freeze-review/uapi-freeze-review.md
```

It checks:

- the Phase 2 WIT package shape
- the generated UAPI reference freshness
- the freeze evidence page freshness
- the freeze lock freshness
- the freeze lock checker
- the adapter-boundary guard
- the Phase 2 exit-ledger guard

This is useful because a freeze review can drift if people only read a checklist.
The recorder makes the current contract state concrete. If the WIT files change,
the generated reference, evidence page, and hash lock must move together.

## What A Passing Report Means

A passing report means the current UAPI candidate is internally consistent.

It does not mean Phase 2 is complete. The remaining exit work still includes
cross-host evidence, language-track evidence, benchmark and fuzz evidence, and
one outside walkthrough.

## Self Hosted CI

The manual self-hosted full gate records this report too. That gives us a local
runner proof for the exact same freeze candidate we review in the docs.

The report should be read beside:

- [UAPI Freeze Review](uapi-freeze-review.md)
- [UAPI Freeze Evidence](uapi-freeze-evidence.md)
- [UAPI Freeze Lock](uapi-freeze-lock.md)
- [Phase 2 Exit Evidence](exit-evidence.md)

## How We Use It

Before a final freeze decision:

1. Run the recorder in strict mode.
2. Confirm the working tree only contains expected review output.
3. Check the exit ledger for anything still partial, pending, or blocked.
4. Record the freeze decision in the Phase 2 plan.
5. Add an ADR only if the review changes a rule that future phases depend on.

This keeps the freeze decision honest. We are not freezing because the code
feels ready. We freeze only when the contract, generated docs, adapter boundary,
and review evidence agree.
