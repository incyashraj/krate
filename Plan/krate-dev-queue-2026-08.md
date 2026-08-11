# The development work queue — verified 2026-08-11

Every item below was checked against the code or measured on this machine
today. **Each one says how it was verified.** Where a check disproved a
claim, the claim is deleted and the correction is recorded at the bottom,
because a stale item in a work queue costs more than no item at all.

Two rules this file follows:
1. **Young is not broken.** GPU rendering started 2026-08-10; the phone
   players started 2026-08-09. Their gaps are next steps, not defects.
2. **Development only.** Traction is not this file's business.

Method note: the first draft of this file was written from impressions and
got several things wrong. This one only contains findings with a
reproducible check beside them. **The bug board is not a reliable source
on its own -- three entries I quoted from it were fixed months of commits
ago and never marked.** Verify against the code, then fix the board.

---

# TIER 1 — Verified, high leverage

## 1. Telemetry blocks every app launch for ~68 ms — K-091

**How verified:** measured on this Mac, and confirmed in source at
`crates/cli/src/usage.rs:250` and the call site `crates/cli/src/main.rs:5702`.

| | median |
|---|---|
| `krate run` (18 KB app) | **73.9 ms** |
| same, `KRATE_NO_USAGE=1` | **6.4 ms** |
| `krate --version` (binary load only) | 5.9 ms |
| runtime compile + instantiate + run | 3.3 ms (criterion) |
| steady state, already loaded | 61 µs (criterion) |

The runtime is fast. The product is not, because after the app finishes,
`usage::record_with(Action::Open, ...)` spawns a reporting thread and
**joins it against a 600 ms deadline, polling in 20 ms sleeps**. The
comment explains why the join exists (a detached thread loses the race
with process exit and the last event is never sent) -- correct reasoning,
wrong location: it sits on the path every double-click takes, after the
window has already closed.

**Fix:** queue the event to a local file; flush the queue on the next
launch. Nothing is lost and nothing waits. **Expected ~74 ms → ~7 ms.**

**Do this first.** It also cleans the signal for judging the phone work --
some of what reads as mobile lag is this, on every platform.

## 2. Thirteen of thirty-two fleet apps fail check-app — one cause dominates

**How verified:** ran `check-app` over every app in `apps/` with a
manifest. **19 pass, 13 fail.** (The board's K-025 said "four older
apps"; that entry is stale. The real number is worse and the cause is
more interesting.)

Failures by stage:

| Stage | Apps | Cause |
|---|---|---|
| **layout** | krate-clip, krate-contacts, krate-fractal, krate-keyvault, krate-nova2, krate-spriteproof, krate-weather | **All seven share one bug:** the interactive loop is bounded by a round count (`MAX_ROUNDS`), so the app closes itself while somebody is still using it |
| **usability** | krate-notes | the window closes itself after 12.6 s with nobody asking |
| **manifest** | krate-eo2, krate-mdview | asks for a capability whose interface the component never imports (e.g. `ui.dialog:file-open`) |
| **run** | krate-curl, krate-hello-gui | fails to run headless with all grants, exit 1 |

**Why this is the highest-leverage item in the repo after K-091:** eight
of the thirteen are the *same* self-closing-window bug, and this is the
example-bug class -- the AI reads these apps as reference material, so
every generated app inherits the pattern. `krate-hello-gui` failing is
especially bad: it is "the smallest GUI app" in the pack's own index, the
first thing an AI copies.

**Fix:** gate every round limit on the `quick` argument so it never fires
in a real session (the check-app message already prescribes exactly
this), then re-run the sweep and fix the manifest/run stragglers
individually.

## 3. iPhone: wire CADisplayLink, then re-measure

**How verified:** read `crates/runtime/src/phase3_gui_host.rs` present
pacing -- frames are paced by `std::thread::sleep(FRAME_BUDGET - elapsed)`
against a fixed 16.667 ms budget. There is no display-link sync anywhere
in the iOS adapter.

Sleeping a fixed budget and syncing to the display are different things:
the sleep drifts against the panel's actual refresh, which is the
remaining structural source of uneven frames after this week's five
fixes. **Order: do item 1 first** so the measurement is not polluted,
then wire CADisplayLink, then judge with the on-device log.

## 4. GPU text has never been compared to the CPU raster

**How verified:** glyph rendering was added to
`crates/adapter-ios/src/vello_canvas.rs` on 2026-08-10 (commit 1da4e6e);
no comparison test or shoot-diff exists for it.

Both paths now draw text -- CPU through `vector_text.rs`, GPU through
parley → vello `draw_glyphs`. Nothing checks that they agree on baseline,
spacing, or weight. Text is the first thing anyone judges.

**Fix:** shoot the same app both ways and diff. `krate run --shoot`
already exists for exactly this kind of pixel proof.

## 5. Android has no GPU consumer yet

**How verified:** `supports_canvas_lists()` returns true only in
`adapter-ios`; `adapter-android` still paints through
`paint_placements` into a staging buffer.

The display-list spine in `adapter-common/src/canvas_list.rs` was built
cross-platform deliberately. Android measured 26-31 ms/frame after this
week's CPU fixes. The iOS pass is the template; the work is wgpu on
Vulkan with the same scene builder.

## 6. The authoring benchmark has no current number

**How verified:** `evidence/benchmark/RESULTS.md` is dated 2026-08-05 and
records 0/5 on authored apps; GOALS.md G1 asks for a public number.
Dozens of fixes have landed since.

**Fix:** re-run the corpus and publish whatever it says.

---

# TIER 2 — Verified, smaller

## 7. The runtime binary is 21 MB

**How verified:** measured. Also measured the obvious suspect and **it
was not the cause**: building with `--no-default-features` (dropping
whisper/speech entirely) gives 20.1 MB. **Speech costs 1 MB, not 10.**

So the size is elsewhere -- wasmtime with Cranelift, vello_cpu, parley +
fontique, sqlite, gilrs, the 3D renderer. Not urgent (one-time download),
but worth a real `cargo bloat` pass before anyone claims a cause. **I do
not currently know what the 21 MB is made of, and neither should anyone
else until it is measured.**

## 8. No frame-clock contract for apps

**How verified:** searched `wit/krate/phase3/deps/ui/ui.wit` -- no frame
event exists. Apps approximate animation timing with `wait(16)`.

In the modern-UI plan as phase 2, unbuilt. Every animated app is
currently guessing at time.

## 9. Windows code signing

**How verified:** `security find-identity` shows both a Developer ID
Application and an Apple Development certificate; `.github/workflows/release.yml`
signs, **notarizes** (`notarytool submit`) and staples the macOS build,
and the release gate verifies it.

**macOS is done and iOS installs on a real device.** The gap is Windows:
no OV/EV certificate, so SmartScreen warns. That is a purchase.

---

# Corrections to the first draft of this file

Recorded so the same mistakes are not repeated, and as evidence for why
the board needs a sweep:

| Claim | What checking found |
|---|---|
| "No Apple/Microsoft certificates; Gatekeeper warns" | **Wrong.** Developer ID exists; releases are signed AND notarized in CI. Only Windows lacks a cert. |
| "K-036: GUI app panics on stock Ubuntu" | **Stale.** `check_window_libraries()` already dlopens `libxkbcommon-x11.so` before any window and returns a plain sentence instead of a panic. |
| "K-025: four older apps fail check-app" | **Stale and understated.** Thirteen fail; seven share one root cause. |
| "K-001: no scroll in the widget path" | **Stale.** `scroll_offsets` and `clamped_scroll_offset` exist in the drawn widget path, with tests. |
| "The 21 MB binary is whisper/gamepads/sqlite" | **Unproven.** Speech is only 1 MB of it. Cause unknown until measured. |
| "GPU/phones being days old is a weakness" | **Wrong framing.** They are next steps in work that is on schedule. |

**Action item from these corrections: sweep BUGS.md.** At least three
open entries describe problems the code already solved. A board that
carries fixed bugs as open is worse than no board -- it makes every other
entry untrustworthy, which is exactly what happened here.

---

# The order I would work in

1. **K-091 telemetry** — one afternoon, 10x on the latency everyone
   feels, and it de-noises the phone measurements.
2. **The self-closing-window sweep** — one bug, eight apps, and the AI
   learns from these files.
3. **Sweep BUGS.md against the code** — cheap, and it restores the
   board's trustworthiness.
4. **CADisplayLink, then re-measure on the iPhone.**
5. **GPU text diff** — protects the thing people judge first.
6. **Android GPU consumer** — repeats the iOS pass.
7. **Re-run the benchmark** — turns a claim into a number.
