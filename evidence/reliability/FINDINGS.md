# AI authoring reliability -- first measured run

Date: 2026-08-04/05. Machine: M-series Mac. Binary: `target/release/krate`
built from main, invoked by absolute path (a `krate` on PATH is an older
installed release and would have measured the wrong thing).

## The number

**14 of 14 requests that actually reached the AI produced a working app.**

Every app passed all six `check-app` stages: layout, manifest, build, imports
(zero non-`krate:*` imports), run, and paint-a-frame. Mean time about 3
minutes. Mean size about 850 lines of Rust.

## The 47 failures were not Krate

62 rows were recorded. 47 of them failed in 2-3 seconds -- far too fast for an
AI to have written anything. Every one of those 47 transcripts contains:

    "rate_limit_info":{"status":"rejected"

The Claude account ran out of quota partway through the run. The agent process
started, was refused by the API, and exited. No code was written, no build was
attempted, and no `check-app` stage was reached (all 47 recorded stage 0).

Counting those as authoring failures would give a 23% pass rate, which would be
a lie about the system. They are excluded above and the raw rows are kept in
`results-2026-08-04.tsv` so the exclusion is auditable rather than hidden.

**This is a harness gap worth fixing before the next run:** a rate-limit
rejection should be recorded as `skipped`, not `fail`, and the run should pause
and retry rather than burning through the remaining corpus in 90 seconds.

## What the earlier fixes bought

The first six requests ran on a pre-fix binary; everything after ran post-fix.
Two fixes landed during the run, both now on main:

1. The GUI template shipped no `krate` dependency, so every windowed app the
   agent wrote had to discover the missing SDK through a failed build and add
   it back by hand. Measured at 100% frequency before the fix.
2. `check-app` now repairs a `no_std` app that lost the dependency, reading the
   SDK path out of the WIT target entry already in the Cargo.toml rather than
   guessing it.

## The open finding: there is no refusal path

`validate_create_request` (`crates/cli/src/main.rs`) checks only that a request
is at least 3 characters. Nothing screens for whether Krate can serve it.

The agent prompt says "Do not stop until `check-app` prints OK" and never tells
the agent it may refuse. The only guard against a wrong app is a byte-identical
comparison against the starter skeleton, which catches an agent that wrote
nothing -- not one that wrote something plausible and wrong.

None of the six `check-app` stages compares the app to the request. They are
all mechanical: does it build, does it import only `krate:*`, does it run, does
it paint.

So a request like "download my email and show me the unread ones" would most
likely produce a plausible-looking mail-reader UI over local state that builds,
runs, and exits 0. The harness would record that as a pass.

**This was not measured.** The impossible requests sit at the end of the corpus
(96-100) and the account ran out of quota before reaching them. They need a
hand-inspected run, because the exit code cannot tell a good app from a
convincing wrong one.

## Correction to an earlier claim

An earlier draft of this work reported that the runtime has no TLS and that a
weather-style app would pass every check and then fail for real users. **That
was wrong.** TLS ships via `ureq` with its `tls` feature (`Cargo.toml:81`), the
`fetch_over_tls` path in `crates/runtime/src/lib.rs` is live in every default
build, and a real handshake to example.com returns 200. There is now a test at
`crates/runtime/tests/https_reaches_a_real_host.rs` proving it.

It is genuinely hard to confirm by reading, which is how it was misread: no
rustls or native-tls appears in any Cargo.toml, because ureq carries TLS
itself.

## Next

1. Fix the harness to treat rate-limit rejection as `skipped` and to pause.
2. Re-run the remaining corpus on a fresh quota.
3. Hand-inspect requests 96-100 (the impossible ones) rather than trusting exit
   codes.
4. Decide the refusal shape: the cheapest honest version is a screen before
   authoring that names what Krate cannot do and stops, rather than spending
   three minutes producing something wrong.

## Krate Mode: what is verified and what is not (2026-08-05)

Verified without a model, by me:

- The published `docs/krate-mode.md` is byte-identical to what
  `krate krate-mode` generates, so it cannot silently teach a dead API. I
  confirmed the drift test fails when the file is edited, then restored it.
- Both worked examples are the real shipped sources of `apps/krate-clock` and
  `apps/krate-checklist`, inlined via `include_str!`, not invented for the
  prompt.
- The page is live at https://krate.tech/docs/pages/krate-mode.html and the raw
  prompt at https://krate.tech/krate-mode.md.

**Not verified by me:** that a fresh model given this prompt produces an app
that passes `check-app`. The workstation that built it reports two apps passing
first try (a cooking unit converter, 820 lines; a cron expression explainer,
1016 lines), but I could not reproduce that independently because both AI
accounts on this machine are currently unusable:

- Claude: `OAuth session expired and could not be refreshed`
- Codex: `The 'gpt-5.6-sol' model requires a newer version of Codex`

This is the single most important claim about Krate Mode and it rests on one
report. Re-run it the moment an account works:

    cd /tmp && mkdir kmtest && cd kmtest
    claude -p "$(cat docs/krate-mode.md)

    Now write: <some app nobody has built>. Write the three files here." \
      --allowed-tools Read,Write,Edit --permission-mode bypassPermissions
    krate check-app .
