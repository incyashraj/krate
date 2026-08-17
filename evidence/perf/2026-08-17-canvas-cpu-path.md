# The canvas CPU path, before and after (2026-08-17)

The complaint: a super-mario-like canvas game, published from a Windows PC,
unplayable there on v0.1.33 while acceptable on an M4 Mac. Profiled with a
symbolized build; the guest was innocent. Three host loops were the frame:

| Cost | Before | After |
|---|---|---|
| to_image ARGB->RGBA (per frame, full canvas) | 4.3 ms | 0.34 ms |
| draw_image scale-blit | per-pixel float sampling | packed-row memcpy (opaque path) |
| events::wait(16) actual return | ~20 ms (10 ms park slices) | ~16 ms (park clamped to deadline) |

Fix commit: f2e3aad4.

## Measured, Mac (M4, windowed, same published bundle)

- Before: 30 ms/frame (33 fps) at 29% of a core
- After: 18.5 ms/frame (54 fps) at 12% of a core

## Measured, Windows (2-core Azure VM, no GPU -- the CPU fallback path)

Same bundle fetched from the hub, 20 s runs, KRATE_CPU_PRESENT=1 for the
old binary so both take the CPU path:

- v0.1.30 (installed): 14.6 CPU-seconds over 20 s (73% of a core); no
  frame stats in that build. At default settings it does not run at all on
  this machine: the GPU surface path panics ("Surface::configure: Invalid
  surface"), fixed since by 5b7ffbf0.
- main (v0.1.35): 7.6 CPU-seconds over 20 s (38% of a core), frame gap
  p50 20.1 ms / p99 22.2 ms -- 50 fps sustained on two cores with no GPU.

The two pixel loops cut here are exactly what a weak Windows machine
chokes on; machines with a working GPU take the vello path and skip them
for widget scenes, but every canvas app's publish still crossed them.

## The founder's Iris Xe desktop, live over ssh (2026-08-17)

The same game on real hardware surfaced three more thieves, fixed in
6f73ece5, f426cff7 and ae3e4316:

1. The winit pump repainted every window on every event check; on the GPU
   path each repaint blocked a vsync. Measured: 61,431ms of pump per 60
   frames (3,708 calls), under 2.4fps, three cores pinned. After: pump
   4-100ms per 60 frames.
2. The swapchain presented AutoVsync inside the guest's publish, stacking
   a second 16ms clock on the game's own pacing. Moved to AutoNoVsync
   (mailbox); pacing stays with the host frame budget.
3. Full-window canvas frames went through the whole vello scene pipeline
   with a fresh image upload per frame (~18-20ms on Iris Xe). Now a
   persistent texture, one write, one scaling blit, same as the macOS
   Metal path.

Verdict runs on the desktop: frame gap 1,132ms -> 38ms -> 32ms across the
three fixes; the founder's side-by-side against the M4 Mac (same game,
both machines at once, judged by the game's own on-screen clock): play
feel indistinguishable, 1-2s of game-clock drift over 30s. Remaining
headroom (publish sync p50 12-14ms) belongs to K-112's GPU presenter
work proper.
