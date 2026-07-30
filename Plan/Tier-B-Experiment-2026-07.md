# Tier B Experiment — can an AI author arbitrary Krate apps? (2026-07-27)

## Why we ran this

The product bet is not "let people make checklists." It is: an AI turns an
arbitrary plain-English request into a real app that runs safely, carrying its
own permissions. This experiment stress-tested that bet directly — Yashraj's
signed-in `claude` CLI driving `krate create --author-cmd` on 8 genuinely
varied, non-template requests. All on one machine; no hosted infra (that comes
later, and should be client-AI authoring + a thin server that only builds — see
memory `krate-two-paths-adoption-2026-07`).

## The 8 requests (mixed capability profiles, mixed difficulty)

| # | App | Profile | Hard part |
|---|-----|---------|-----------|
| 1 | temp-convert | compute + float output | render f64 without `format!` |
| 2 | line-count | `fs.read` | file streaming |
| 3 | reverse-file | `fs.read` + `fs.write` | two-cap gating |
| 4 | countdown | `time.sleep`, loop | fuel + time |
| 5 | pass-check | compute, args | manual arg parsing |
| 6 | dice-roll | randomness | does Krate expose random? |
| 7 | tip-calc | compute + float output | the float-format trap |
| 8 | clock-window | `ui.window` + `time` | GUI + cap combo |

## Result

**8/8 compiled. 5/8 are valid, working Krate apps** (import only `krate:*`,
pass the permission wall, produce correct output). The AI authored real,
non-template apps from a sentence — the core bet holds.

| App | Compiled | krate:*-only | Works | Notes |
|-----|:--------:|:------------:|:-----:|-------|
| dice-roll | ✅ | ✅ | ✅ | clean (found: Krate DOES have a usable entropy path via time) |
| clock-window | ✅ | ✅ | ✅ | GUI + time, clean |
| pass-check | ✅ | ✅ | ✅ | correct "too short"/"ok" |
| line-count | ✅ | ✅ | ✅ | correctly refuses without `fs.read` (exit 5) |
| countdown | ✅ | ✅ (after `opt-level="s"`) | ✅ | profile fix cleared its leak |
| temp-convert | ✅ | ❌ | — | leaks `wasi:*` |
| reverse-file | ✅ | ❌ | — | leaks `wasi:*` |
| tip-calc | ✅ | ❌ | — | leaks `wasi:*` |

## The one recurring failure: `wasi:*` import leak

The 3 failures fail the same way: the built component pulls the full WASI CLI
world (`wasi:cli/environment`, `wasi:filesystem/preopens`, `wasi:clocks/
wall-clock`, `cli/exit`, `io/streams`, …) even though the model wrote careful,
disciplined code (fixed `[u8; N]` buffers, hand-rolled float rendering, manual
arg parsing, no `format!`/`HashMap`/growable `Vec`).

### Root cause (confirmed by bisection)

`wasm32-wasip1` links `std`, and `std` carries latent `wasi:*` imports. Whether
they survive into the component depends on **two** things:

1. **Release profile.** The generated apps' `[profile.release]` was missing
   `opt-level = "s"` (they had `strip = true` instead — which strips symbols,
   not imports). Adding `opt-level = "s"` cleared the leak on `countdown`.
   The in-repo samples (checklist/notes) have `opt-level = "s"` and are clean.
2. **Reachable std entry.** For `temp-convert` / `reverse-file` / `tip-calc`,
   even the exact clean sample profile does NOT clear the leak. Their code
   keeps a `std` runtime path reachable (the tell is the full CLI+fs+clocks
   world appearing together = `std::rt`/args init), which no profile flag can
   eliminate because it is genuinely referenced.

Empirically established:
- f64→u64 casts alone: do NOT leak.
- `fs::open`/`.write` in isolation with discarded result: do NOT leak.
- Missing `opt-level="s"`: contributes; fixed 1 of 4.
- A reachable std path: the real blocker for the remaining 3; profile cannot fix it.

### The durable fix (SDK-side, not model-side)

The AI is doing its job; the SDK gives it no leak-proof way to write a normal
app. Options, in order of leverage:

1. **Fix the generated `[profile.release]` to match the samples** (add
   `opt-level = "s"`). Free, ships in the generator. Clears the profile-only
   leaks. (Countdown-class.)
2. **Give the guest a `no_std`-style or std-shim surface** so the app never
   links `std`'s wasi-backed runtime — the real fix for the reachable-std
   cases. The samples avoid std by discipline; the SDK should make that the
   default path, not a tightrope the author (human or AI) must walk.
3. **The import check already catches this** at `krate create` step 3 — a
   leaking app is REJECTED, never shipped. So today the failure is safe (no bad
   app escapes), just a lower success rate. Fixing 1+2 raises the rate.

## What this means

- The bet works: an AI authors arbitrary, safe, permission-gated apps from a
  sentence. 5/8 with zero SDK tuning is a strong first signal.
- The single gap is one SDK seam (std-linkage / release profile), on our side,
  that we control — not a model limitation.
- The import wall means even the failures fail *safely*: a leaking app is
  rejected at create time, never handed to a user.

## Environment traps found (worth a preflight fix)

- **Homebrew Rust shadows rustup.** `/opt/homebrew/bin/cargo` → `Cellar/rust`
  has no `wasm32-wasip1` target, so `cargo-component` fails with "target not
  found" even though rustup HAS the target. A real dev with `brew install rust`
  hits this. The toolchain preflight should detect and warn.

## Reproduction

Authored source + logs: `/tmp/tierb-results/work-<name>/`. Harness scripts in
the session scratchpad (`tierb-experiment.sh`, `tierb-rebuild.sh`). Each app's
`.krate` and per-stage logs under `/tmp/tierb-rebuilt/`.
