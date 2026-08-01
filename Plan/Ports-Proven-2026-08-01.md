# Ports proven, 2026-08-01

Two third-party programs, neither written by us, ported end to end and run.
This is the evidence behind "a small or medium app can be made portable", and
it is deliberately specific: two ports is not a pattern, and saying "any app"
on the strength of it would be disproved by the third person who tries.

## 1. hexyl — a command-line hex viewer

| | |
|---|---|
| Source | [sharkdp/hexyl](https://github.com/sharkdp/hexyl), ~2,400 lines of Rust |
| Result | 14,872 byte `.krate` |
| Repair attempts | 0 |
| Imports | 6 `krate:*`, **0 `wasi:*`** |

Correct hex output, including on input that is not valid UTF-8 (`ff fe 80`, and
the broken sequence `c3 28`) — the case that a text-only API cannot express and
which forced `stdio::write` into the SDK.

The permission wall refused to run it until `fs.read:input/**` was granted, and
named that capability in the refusal.

## 2. bank-savings-calculator — a desktop GUI app

| | |
|---|---|
| Source | [ahtalbi/bank-savings-calculator](https://github.com/ahtalbi/bank-savings-calculator), 490 lines, eframe/egui |
| Result | 13,791 byte `.krate` |
| Repair attempts | 1 |
| Imports | 9 `krate:*` including the full UI stack, **0 `wasi:*`** |

The business logic ported exactly rather than approximately. Budget categories
in the original are 25 / 5 / 40 / 20 / 10 percent; the ported app splits a 4000
input into 1000 / 200 / 1600 / 800 / 400, with the same five labels.

The window is real, not dead code. Timing the two paths on the same machine:

```
  -- quick     0.1s   computes, prints, exits
  interactive 11.4s   opens the window, enters the event loop, waits ~10s
                      for input with nobody there, closes cleanly, exits 0
```

If the window path were unreachable both would be 0.1s.

The agent chose to add `store.kv` on its own so the budget persists across
runs, which was verified, and marked it required — so withholding it correctly
refuses the app.

## 3. ddh — a duplicate file finder

| | |
|---|---|
| Source | [darakian/ddh](https://github.com/darakian/ddh), 756 lines, filesystem-heavy |
| Result | 17,778 byte `.krate` |
| Repair attempts | 0 |
| Imports | 6 `krate:*`, **0 `wasi:*`** |

Walks a directory, reads every file in it, and compares them. Correctly reports
two identical 8,220-byte files as one shared instance and a third as unique.

This is the shape that showed `fs.list` is a separate grant from `fs.read` --
listing a directory and reading a file are different questions to put in front
of a person, and the analyzer could not see the difference until this port.

## What the ports have in common

Both were the wrong shape for the tooling in some way, and each exposed a
defect that was ours rather than the agent's:

- **ddh** was handed `input/sample.txt` when the contract had promised it
  `quick`. It handled `quick` correctly and takes directories, so it failed
  after building, packing, and passing every other check.
- **hexyl** invented `stdio::write` because the contract listed rules and
  prohibitions but not a single function. The SDK reference generator came out
  of that, and the byte-write it reached for turned out to be a genuine gap.
- **savings** was reported as `Frameworks: not detected` and handed the CLI
  profile, because the analyzer knew Qt, GTK, WPF, and Tauri but none of the
  Rust-native toolkits. Then it failed its permission wall against `fs.write`,
  a capability a budget calculator never requests.

The pattern across all three, and across `random.bytes` before them: **the
bottleneck has never been what the runtime can do. It has been what the tooling
could see.** Three findings, three times the same shape.

## What this does not prove

- **Two programs, two shapes.** A CLI byte-pusher and a GUI form. Nothing yet
  about apps that are mostly network, mostly database, or that draw to a
  canvas — `canvas` is macOS-only today, so a canvas app would fail.
- **Ported on macOS.** Both `.krate` files should run anywhere the runtime does,
  and the components are host-independent, but neither has been opened on
  Windows or Linux by hand.
- **Nothing about size.** hexyl is 2,400 lines and savings is 490. "Medium" is
  not yet a tested claim.

## Reproducing

```bash
krate port <source> --prepare <workspace> --agent claude --to <app.krate>
```

Both runs used that command with no manual edits to the candidate.


## 4. rss-forwarder — the network shape

| | |
|---|---|
| Source | [morphy2k/rss-forwarder](https://github.com/morphy2k/rss-forwarder), 1,590 lines, `reqwest` over HTTPS |
| Result | 18,084 byte `.krate` |
| Repair attempts | 1 |
| Imports | 9 `krate:*`, **0 `wasi:*`** |

Reads a TOML feed table, fetches over HTTPS, parses RSS and Atom, and forwards
to a Discord or Slack webhook:

```
rssfwd: watching 1 feed
  rust -> discord (every 3600s)
rssfwd: rust: recorded baseline, no items forwarded
```

The interesting part is the permission list. It declared **a grant per host**
rather than a wildcard:

```
net.connect:blog.rust-lang.org:443
net.connect:github.blog:443
net.connect:discord.com:443
net.connect:hooks.slack.com:443
```

Withholding one of them refuses the run and names exactly which hosts are
missing. That is per-host network permission working end to end on a real
program, and it is the thing a wildcard `net.connect:*` would have thrown away.

### The first attempt, and what it fixed

The first run of this port failed after building and packing: `clean_text`
stripped tags before decoding entities, so `&lt;p&gt;Newer entry&lt;/p&gt;` came
out as `<p>Newer entry</p>` with the markup intact rather than as `Newer entry`.
The app's own self-check said so in one line.

**It got zero repair attempts**, because the repair loop wrapped only the build
and import checks -- verification ran after packing, outside it. A syntax error
got two attempts and a wrong answer got none.

Verification is now inside the same budget. This port then took **one** attempt
and passed, which is the fix working on the case that exposed it.

### An older note, kept

**Not counted as proven** on the first attempt. The port builds, packs, and does the hard parts: it
parses RSS and Atom, sorts newest-first, and builds both Discord and Slack
payloads. It fails its own self-check on one thing:

```
parsed RSS feed "Example Feed" with 2 items
rssfwd: quick check failed: RSS description not cleaned
parsed Atom feed "Atom Example" with 1 items
built discord payload (232 bytes) and slack payload (589 bytes)
```

The bug is an ordering mistake in `clean_text`: it strips tags first and decodes
entities second. The input is `&lt;p&gt;Newer entry&lt;/p&gt;`, so at strip time
there are no tags to strip -- they are still encoded -- and decoding then
produces `<p>Newer entry</p>` with the markup intact. Reversed, it gives
`Newer entry`.

This is the first port defect today that is genuinely the agent's code rather
than something the tooling failed to tell it. Worth having: it means the
tooling is no longer the limiting factor on this shape.

### What it exposed about the repair loop

**The port got zero repair attempts.** The loop wraps
`validate_port_candidate` -- build and import checks -- and verification runs
after packing, outside it. So a candidate that compiles cleanly and computes
the wrong answer is never sent back, even though that is the most repairable
failure there is: the app's own self-check names the problem in one line.

A build error gets two attempts. A wrong answer gets none. That is backwards,
and it is the next thing to fix on this path.


## 5. envelope — the database shape

| | |
|---|---|
| Source | [mattrighetti/envelope](https://github.com/mattrighetti/envelope), 3,795 lines, sqlx + tokio |
| Result | 19,495 byte `.krate` |
| Imports | 6 `krate:*`, **0 `wasi:*`** |

An environment-variable manager backed by SQLite. Three capabilities working
together in one program:

```
envelope quick check
added 2 variables to quick-a
  API_KEY=sk_test_123
  DEBUG_MODE=true
diff quick-a quick-b: 1 differing key(s)
soft-deleted DEBUG_MODE from quick-a
reverted DEBUG_MODE in quick-a
random and secret round-trip ok
```

Real SQL through `store.sql`, encrypted values through `store.secret`, and
`random.bytes` -- the capability that did not exist this morning.

It was blocked for a while by the harness rather than by the app: the
verification run hands a file path to anything declaring `fs.read`, which fits a
file reader and does not fit a subcommand CLI. The app printed its usage and
exited non-zero having passed every other check. The harness now tries `quick`
as well, and this port passes unchanged.

At 3,795 lines it is also the largest source ported so far, though still short
of the 5,000-line bar the gate sets for "medium".


## 6. grex — genuinely medium, 5,396 lines

| | |
|---|---|
| Source | [pemistahl/grex](https://github.com/pemistahl/grex), **5,396 lines** |
| Result | 54,084 byte `.krate` |
| Repair attempts | 1 |
| Imports | 6 `krate:*`, **0 `wasi:*`** |

A regular-expression generator: give it example strings, it produces a regex
matching exactly those. The hardest algorithm of any port so far, and it works.

```
$ krate run grex.krate -- quick
^a(?:bc?)?$
```

That regex is correct, checked rather than eyeballed: it matches `a`, `ab`, and
`abc`, and rejects `abcd`, `b`, and the empty string. Producing a
correctly-factored alternation from example strings is the whole point of the
program, and the port does it.

This closes the "medium sized" question, which had been an untested word in
every claim we made. 5,396 lines in, 2,333 out, 54 KB packed.
