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

## What the two ports have in common

Both were the wrong shape for the tooling in some way, and each exposed a
defect that was ours rather than the agent's:

- **hexyl** invented `stdio::write` because the contract listed rules and
  prohibitions but not a single function. The SDK reference generator came out
  of that, and the byte-write it reached for turned out to be a genuine gap.
- **savings** was reported as `Frameworks: not detected` and handed the CLI
  profile, because the analyzer knew Qt, GTK, WPF, and Tauri but none of the
  Rust-native toolkits. Then it failed its permission wall against `fs.write`,
  a capability a budget calculator never requests.

The pattern across both, and across `random.bytes` before them: **the
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
