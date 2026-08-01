# RUSTSEC-2026-0222: Wasmtime advisory, needs a decision

Found by the full CI run on 2026-08-01, the first one in four weeks. Recorded
rather than silenced: an ignored security advisory is worse than a failing
check, because the failing check is the only thing still telling anyone.

## What it is

`wasmtime 43.0.2` — the engine Krate runs every app in — carries
[RUSTSEC-2026-0222](https://rustsec.org/advisories/RUSTSEC-2026-0222),
"stores can mix up type indices between engines"
([GHSA-hgjw-h833-99q9](https://github.com/bytecodealliance/wasmtime/security/advisories/GHSA-hgjw-h833-99q9)).

This is the sandbox core. A type-confusion bug there is the category that
matters most to us, because the whole product claim is that an app can only do
what it declared.

## Why it is not already fixed

The advisory's fix line is:

```
>=24.0.12, <25.0.0 OR >=36.0.13, <37.0.0 OR >=46.0.2, <47.0.0 OR >=47.0.3
```

We are on 43.0.2. There is no patch release in the 43.x line, so the smallest
move is **43 → 46**, three major versions. Wasmtime's component-model API moves
between majors, and Krate uses it directly in `crates/runtime`. This is a real
upgrade with breaking changes to work through, not `cargo update`.

## What has to be decided

Three options, in the order I would rank them:

1. **Bump rustc to 1.94, then upgrade Wasmtime.** Correct, and it removes the
   advisory rather than annotating it. Two changes, not one -- see the measured
   cost below. Still recommended, but it is a two-step job rather than an
   afternoon.
2. **Assess exploitability first, then schedule.** The bug involves values
   crossing between two engines. `crates/runtime` has exactly one
   `Engine::new` call site (`lib.rs:212`), which is a reason to think Krate may
   not reach the bug — but one call site is not the same as one engine per
   process, and nobody has traced whether a single run can end up with two. That
   tracing is the work this option means. Until it is done, "we are not
   affected" is a guess, and it is not a guess to publish.
3. **Add it to `deny.toml`'s ignore list.** Makes CI green and changes nothing
   real. Only defensible after (2), with the reasoning written next to the
   entry and a date to revisit.

What should not happen is the third option quietly, which is the path of least
resistance and the reason advisories sit for years.

## How this was missed

The dependency audit only runs in the `[full-ci]` path, gated on a commit
message tag. That tag has been used **once in the repository's history**, on
2026-07-03. So the audit had not run in four weeks, across the store, sql,
secret, desktop, and random capability work.

Worth fixing separately from the advisory itself: an audit nobody runs is an
audit nobody has.


## What it actually costs, measured

Tried both patched versions on 2026-08-01. Neither compiles:

```
wasmtime 47.0.3 -> cranelift-assembler-x64 0.134.3 requires rustc 1.94.0
wasmtime 46.0.2 -> cranelift-assembler-x64 0.133.2 requires rustc 1.94.0
```

The workspace is pinned to **rustc 1.91.1**, and every CI job pins
`dtolnay/rust-toolchain@1.91.1`. So this is not "upgrade a dependency" -- it is
a Rust toolchain bump first, then the Wasmtime API changes on top.

That is bigger than it looked, and it touches every lane: the three-OS matrix,
the cross-system bundle job, the cold install, and the generated-doc freshness
checks all build with that pinned toolchain. A toolchain bump is exactly the
kind of change that should land on its own, with a full nightly behind it,
rather than riding along with a security fix.

The Wasmtime API surface itself is small, which is the good news. The runtime
touches about a dozen real call sites -- `Linker`, `Store`, `Config`,
`component::bindgen`, `Resource` -- and 99 of the mentions are the
`wasmtime::Result` type alias, which does not change.


## Nothing upstream is blocking this

Checked on 2026-08-01: **Rust stable is 1.97.1**. The 1.94 that wasmtime 46 and
47 require has been out for a long time. We are pinned to 1.91.1 by choice --
`rust-version = "1.91"` in the workspace and `dtolnay/rust-toolchain@1.91.1` in
every CI job -- not by anything the ecosystem is missing.

So this is not "wait for the toolchain". It is a decision about when to spend a
toolchain bump, and the answer should probably be soon: the advisory is the only
failing job left in the whole system, and every other gate is green.

The sequencing still holds. Bump rustc on its own, with a full nightly behind
it, then upgrade Wasmtime. Two changes, each provable.
