# The notes-app battery: Krate replica vs MarkText, macOS

> **Evidence status:** historical internal lab note, not an independently
> reproducible result set. It records the method and engineering discoveries,
> but the original raw samples and harness were not committed. The 271 MiB
> MarkText package was x86_64. Use the architecture-matched, raw-data-retaining
> protocol in [`marktext-vs-krate/`](marktext-vs-krate/README.md) for any new
> public claim.

2026-08-16, M4 Mac, macOS 27. Continuation of
2026-08-16-paper-notes-vs-marktext-macos.md after the feel work landed
(Metal canvas presenting, clip-as-bounds text, raster cache, stable
springs, spring-glided scrolling, dithered gradients). Every number below
is from a validated run: the battery counts stray input events and rejects
polluted legs, after live-trackpad contamination produced a fake 45.7%
idle figure and a fake divergence repro earlier in the day.

## Validated results

| Metric | MarkText (Electron) | Krate replica | Verdict |
|---|---|---|---|
| App file | 107 MB download / 271 MB installed | 37 KB | win (and size is explicitly NOT the priority; feel is) |
| First-ever open | 17.1 s | ~0.2 s | win |
| Warm start -> window | 1.80 s (3 runs +-17 ms) | 0.19-0.25 s (see note) | win ~8x |
| Footprint, 5k doc open, idle | 635 MB across 5 processes | 129 MB, 1 process | win 4.9x |
| Footprint, 50k doc | 2,331 MB (and 21.2% CPU just holding it; scroll phase consistent with a renderer collapse) | 95 MB | win 24x |
| Idle CPU, 5k (validated) | 2.7% | 0.6% | win 4.5x |
| Idle CPU, 50k | 21.2% | 0.8% | win 26x |
| Warm start, 50k doc | 1.97 s | 0.25 s | win 8x |
| Scroll CPU, 5k, aimed 125Hz wheel | 81.0% (tree) | 65.3% | win, with a feedback note: both burn real CPU; ours is full-frame redraw+copy -- damage tracking is the known next step |
| Scroll pacing, 5k + 50k | no per-frame instrument on macOS (PresentMon is the Windows leg) | p50 17.3 ms / p99 17.5 ms at 5k; p50 17.6 / p99 17.9 at 50k | measured: 58 fps, jitter under 0.3 ms, both sizes |
| Frame produce cost vs doc size (headless, driven) | n/a | 5k: 0.77 ms/frame text -- 50k: 0.76 ms/frame | win: cost independent of document size |
| Scroll feel | supersmooth | confirmed good by the founder's hand after the fixes | parity reached on the row that started everything |

Note on warm start: the one 704 ms replica sample came from the first
quiet-window leg (display waking from idle); interactive re-measures sit
at 156-249 ms consistently.

## What this battery already fixed (feedback figures turned into commits)

- K-114 both roots: unsynced NSImage composite, and the clip guard cloning
  the whole canvas per text run (194 ms/frame -> 1.7 ms/frame, 114x).
- Spring divergence on stalled frames -> substepped, regression-tested.
- The 45.7% "idle burn" -> measurement pollution; real idle 0.5%.
- The 1.5 MB "210 KB" bundle -> the pack was shipping the AI's attachment
  inbox and the agent's verification frame inside the app. Privacy-class
  bug, fixed in the source collector; same app repacked: 37 KB.

## Still open

- Windows leg with PresentMon measuring BOTH apps by the same external
  instrument: staged for the Azure VM (CPU path) and the founder's PC
  (GPU path).
- Replica scroll-CPU reduction (damage tracking / partial redraw): the one
  feedback figure left on the macOS board.
