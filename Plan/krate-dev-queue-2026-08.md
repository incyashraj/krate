# The development work queue — what to fix next, and why

Written 2026-08-11, rewritten the same day after a correction worth
keeping: **this is a work plan, not a verdict.** Krate is eight days old
in this repo and four months old as an idea. Almost everything below is
either the next commit in work that is already moving, or a real defect
worth naming. Neither is a criticism of the project.

Two rules for this file:
1. **Young is not broken.** GPU rendering started yesterday. The phone
   players started days ago. Work that has just begun gets described as
   "next step", never as "weakness".
2. **Development only.** Traction, users and go-to-market are Yashraj's
   lane and do not appear here.

---

# TIER 1 — Highest leverage, do these first

## 1. Telemetry blocks every launch for ~68 ms

**The biggest single win available right now, and it is a one-afternoon
fix.**

Measured today on this Mac:

| | median |
|---|---|
| `krate run` (18 KB app) | **73.9 ms** |
| same, `KRATE_NO_USAGE=1` | **6.4 ms** |
| `krate --version` (binary load only) | 5.9 ms |
| runtime compile + instantiate + run | 3.3 ms |
| steady state, already loaded | 61 µs |

The runtime is genuinely fast -- 3.3 ms to compile and start a component,
61 microseconds of steady-state work. **91% of what a person waits for is
an HTTP round-trip we block on.** `crates/cli/src/usage.rs:250` joins the
reporting thread against a 600 ms deadline, which was the correct fix for
a real bug (a detached thread lost the race with process exit) placed on
the one path that should never wait.

**Fix:** queue the event to a local file, flush it on the next launch.
Nothing is lost and nothing waits. **Expected: ~74 ms → ~7 ms, a 10x
improvement in the only latency a user can feel.** Filed as K-091.

This also matters for judging the phone work: some of what reads as
mobile lag is this, on every platform.

## 2. iPhone: wire CADisplayLink, then re-measure

The phone work is days old and moving fast -- five real causes found and
fixed with device evidence in two days (watchdog kills, thermal
throttling, a starved touch pipeline, a self-inflicted stall, and the
whole CPU→GPU renderer). The next step is known: **frames are paced with
a 16 ms sleep instead of synced to the display.** CADisplayLink is the
correct clock on iOS and would remove the last structural source of
uneven frames.

Order matters here: do item 1 first, then wire the display link, then
judge with the on-device log before writing more code. Two of the last
five fixes came from measuring instead of guessing.

## 3. GPU text needs a fidelity pass

Glyphs landed yesterday through parley → vello. They have not been
compared against the CPU raster for spacing, weight, or baseline
placement. Text is where "looks cheap" comes from, so a side-by-side
screenshot diff on the same app is worth doing before anyone else sees
it.

## 4. Android gets the GPU consumer next

The display-list spine was built cross-platform on purpose; only iOS
consumes it today. Android still CPU-rasterizes at 26-31 ms/frame after
this week's fixes. The work is mostly shaped like the iOS pass already
done -- wgpu on Vulkan instead of Metal, same scene builder, same list.

## 5. Re-run the authoring benchmark

The corpus and harness exist (W16). The last recorded run was 2026-08-05,
before a lot of fixes; G1 in GOALS.md asks for a public number and there
is no current one. Re-running it is a few hours and it turns "most apps
work" into a number we can stand behind.

---

# TIER 2 — Real defects, cheap to fix, high first-impression cost

## 6. A GUI app panics on stock Ubuntu (K-036)

`libxkbcommon-x11.so` missing on a clean install means a Linux user's
first app crashes. Static link or dlopen with a clear message.

## 7. Four of our own apps fail check-app at the run stage (K-025)

Our reference fleet does not fully pass our own gate. This is the
example-bug class -- generated apps learn from these, so it is the highest
leverage per line changed anywhere in the repo.

## 8. Layout collapses past four controls in a row (K-018)

A limit an AI-authored app will hit and produce something visibly broken.

## 9. Development history leaks into generated apps (K-029)

Generated apps carry this repo's fingerprints.

## 10. Running an app from its source dir writes data into the repo (K-023)

Sandbox root resolution picks the wrong place in a common developer case.

## 11. A window sometimes will not close from its own close button (K-032)

Intermittent, and it ends trust immediately when it happens.

## 12. No scroll in the widget path (K-001)

Canvas apps scroll (krate-gram proves it); the widget path does not.
Referenced in GOALS.md as a G2 blocker.

---

# TIER 3 — Structural work, worth planning now

## 13. The runtime binary is 21 MB while apps are 15-30 KB

Not fatal -- it is a one-time download -- but whisper (speech), gilrs
(gamepads), sqlite and the 3D renderer are linked into every install
whether or not any app uses them. Feature-gating or lazy-loading would
cut both the download and the process start.

## 14. No frame-clock contract for apps

Apps approximate animation timing with `wait(16)`. A real `frame` event
carrying a timestamp is in the modern-UI plan (phase 2) and unbuilt.
Everything animated is currently guessing at time.

## 15. Windows code signing

macOS is **done** -- Developer ID certificate, signed and notarized in CI,
stapled, and verified by the release gate, so a downloaded Krate opens
clean on a stranger's Mac. iOS has an Apple Development certificate and
installs on a real device. Windows has no OV/EV certificate, so
SmartScreen still warns there. That is a purchase, not engineering.

## 16. Documentation carries the bus factor

275 commits of context lives in one head, mitigated deliberately by the
bug board, GOALS.md and the plan docs. Worth keeping that discipline
exactly as it is -- it is what makes the project legible to anyone else,
including future me after a context loss.

## 17. Not hardened against hostile code -- keep saying so

The wall stops honest apps from overreaching; it is not an adversarial
security boundary and is correctly disclosed everywhere. Worth protecting
that discipline: the moment someone treats Krate as a security product
and gets burned, the story changes.

## 18. Authoring rides other people's CLIs

Claude, Codex, Gemini, Copilot and Grok can each change their interfaces.
Multi-provider support is the mitigation and it already exists; a thin
contract test per provider would catch a break before a user does.

---

# The order I would work in

1. **K-091, telemetry off the launch path** — one afternoon, 10x on the
   number every user feels, on every platform at once.
2. **CADisplayLink on iOS, then re-measure** — the last known structural
   source of uneven frames.
3. **GPU text fidelity diff** — cheap, and it protects the thing people
   judge first.
4. **K-036 Ubuntu crash + K-025 the four failing fleet apps** — small,
   and both are first-contact failures.
5. **Android GPU consumer** — mostly a repeat of the iOS pass.
6. **Re-run the benchmark** — turns a claim into a number.

Items 1-4 are days, not weeks. Item 5 is the one that makes both phone
platforms tell the same story.
