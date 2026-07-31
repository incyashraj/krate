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

1. **Upgrade to 46.x or 47.x.** Correct, and it removes the advisory rather
   than annotating it. Costs a day or two of API churn in the runtime, plus a
   full three-OS run to prove nothing regressed. Recommended.
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
