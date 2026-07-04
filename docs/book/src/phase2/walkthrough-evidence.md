# Timed Walkthrough Evidence

Phase 2 has one human proof gate: a Rust developer should be able to write and
run a small UAPI CLI app in 30 minutes or less.

The code cannot prove this by itself. We need one outside reviewer to follow the
Rust walkthrough and record what happened.

## Rehearse The Walkthrough Locally

Before asking an outside reviewer, run the local rehearsal:

```bash
scripts/check-phase2-rust-walkthrough-rehearsal.sh
```

It runs the same core path the reviewer will use:

- `krate doctor`
- CLI build
- Rust cat component build
- manifest generation
- manifest explanation
- granted file read
- denied missing-grant path

The rehearsal writes:

```text
target/phase2-walkthrough/rehearsal.md
```

This does not close the human gate. It only proves the current walkthrough is
ready for a reviewer.

## Generate The Packet

Run this from the repo root:

```bash
scripts/record-phase2-walkthrough-template.sh
```

It writes:

```text
target/phase2-walkthrough/walkthrough-template.md
```

The template records the commit under review and gives the reviewer one place to
fill in timing, step results, notes, and the final pass or fail.

## Check The Filled Packet

After the reviewer fills the packet, run:

```bash
scripts/check-phase2-walkthrough-evidence.sh target/phase2-walkthrough/walkthrough-template.md
```

The checker does not replace the human review. It only checks that the packet is
usable evidence:

- metadata fields are filled
- result is `pass` or `fail`
- total minutes is a number
- a passing run is 30 minutes or less
- every walkthrough step has a reviewer result

## What The Reviewer Does

The reviewer follows:

[Your First UAPI App In Rust](../uapi/first-rust-cli.md)

They start a timer before checking tools and stop it after the missing-grant
denial path works.

The pass rule is simple:

```text
Rust developer, new to Krate, completes the walkthrough in 30 minutes or less.
```

Private help from us does not count. Docs fixes after the run are welcome, but
the original timing result should stay honest.

## What To Save

Save:

- the filled walkthrough template
- the checker output
- the terminal transcript or log
- the commit hash used for the run
- any notes about confusing wording or missing setup steps

Once that evidence exists, `P2E-12` can move from pending to done or partial,
depending on the result.
