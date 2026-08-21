# Paper Notes (Krate) vs MarkText (Electron), macOS

> **Evidence status:** historical internal lab note. The original raw samples
> and `winprobe.swift` were not committed. The 271 MiB installed size below has
> since been traced to MarkText's x86_64 package, so its Apple-silicon
> performance numbers must not be presented as an architecture-matched public
> benchmark. The replacement ARM64 protocol and fail-closed audit live in
> [`marktext-vs-krate/`](marktext-vs-krate/README.md).

2026-08-16, M-series Mac, macOS 27. First head-to-head against a shipping
offline cross-platform app. MarkText 0.17.1 chosen because it is free,
fully offline, ships on Mac/Windows/Linux, and is Electron -- the incumbent
way to ship a desktop app everywhere, i.e. the thing Krate competes with.

Both apps satisfy the same spec: notes list left, editor right, local
autosave, and the same 5,000-line / 495 KB document loaded.

## Numbers

| Metric | MarkText (Electron) | Paper Notes (Krate) | Ratio |
|---|---|---|---|
| Download size | 107 MB | 210 KB (+ Krate once) | 522x |
| Installed size | 271 MB | 210 KB | 1320x |
| First-ever open | 17.1 s | 0.16 s | 107x |
| Warm start -> window | 1.80 s (3 runs, +-17 ms) | 0.156 s | 11.5x |
| Memory footprint, doc open | 635 MB (5 processes) | 117 MB (1 process) | 5.4x |
| Idle CPU (60 s avg, whole tree) | 0.4% | 1.1% | 0.36x (loss) |
| Scroll/feel (watched, same person) | supersmooth | line jumps + swap flicker | decisive loss |

## Method (rerunnable)

- Window time: swiftc probe polling CGWindowListCopyWindowInfo for the
  spawned pid owning a layer-0 on-screen window, 15 ms resolution
  (scratchpad/bench/winprobe.swift).
- Footprint: macOS `footprint` per process, summed over the app's whole
  process tree (Electron runs five processes; counting only the main one
  under-reports it 3.7x).
- Idle CPU: 12 samples of summed `ps %cpu` over 60 s, app focused,
  untouched, after a 30 s settle.
- The Krate store was seeded with the same document
  (`~/.krate/store/dev.krate.offline_notes.kv`, key `note.0`).

## The honest verdict

Krate wins every number that can be printed. MarkText wins the feel the
moment a person touches the scroll wheel, and that is the row adoption
actually reads. Filed as K-114 (macOS canvas presents without vsync -- the
vanish/reappear; S5 of Plan/GPU-Presenter.md is the fix) and K-115 (the
authoring pack never teaches pixel-offset scrolling, so every generated
app scrolls by whole lines on every platform).

The two idle-CPU tenths of a percent we lose come from the same place as
the feel: the app repaints on a timer instead of presenting on vsync only
when something changed.

Windows leg (PresentMon frame times for both apps, GPU and CPU paths)
pending: staged on the Azure VM for the CPU/fallback story, and on the
founder's physical PC for the GPU story.
