# The GPU presenter: Krate apps that feel native everywhere

Written 2026-08-15. This is the plan of record for K-112 and plan section
7.5. The bar, set explicitly: a person comparing a Krate app on Windows to
the same app on a Mac, or to the native apps beside it, must find nothing to
forgive. Not "good for a portable runtime" -- indistinguishable. Adoption is
the whole game, and adoption is lost in the first ten seconds of jank.

## Where we actually stand (surveyed, not guessed)

| Platform | Today | Quality |
|---|---|---|
| macOS | native AppKit adapter (widgets) + CPU-composited canvas | good, but painted surfaces are 1x (K-111) |
| iOS | **vello 0.9 on wgpu/Metal** (`adapter-ios/src/vello_canvas.rs`, 826 lines) | the proof the GPU path works; iOS itself is ON HOLD as a target until desktop numbers justify mobile (founder decision 2026-08-15) -- this code is reference material |
| Windows | winit + shared CPU painter (`vello_cpu` for text) + softbuffer blit | correct since v0.1.25, visibly slower than the Mac |
| Linux | same CPU path as Windows | same gap |
| Android | vello_cpu + softbuffer | same gap |

The decisive fact: **the placement contract already isolates the
presenter.** Every adapter hands `paint_placements` the same
`WidgetPlacement` list. iOS proves vello renders our scenes on a GPU. The
work is a port along an existing seam, not a redesign.

## Architecture

One new crate, `crates/presenter-gpu`:

    placements: &[WidgetPlacement], scale, interaction
        -> scene build (ported from adapter-ios vello_canvas)
        -> vello::Renderer on wgpu surface (DX12/Vulkan/Metal/GL via wgpu)
        -> present, vsync-paced

- **Scene building is shared, backends are not.** The placements->Scene
  translation becomes one function used by GPU vello everywhere it lands.
  The existing `vello_cpu` path keeps working as the automatic fallback
  wherever a usable GPU adapter is not found -- correctness never depends
  on a driver.
- **Present mode:** `AutoVsync`. The lag reported was pacing as much as
  raster; a GPU frame presented on vsync is the "native feel" the user is
  comparing against.
- **Text:** parley shaping is already in the tree (iOS + vector_text);
  glyph runs go to vello exactly as iOS does.
- **Canvas/images:** vello image resources, uploaded once and reused --
  re-uploading per frame is the classic port mistake and is called out as
  a review item because a variant of it already shipped once (the K-062
  white-screen was the vello painter dropping Image/Canvas pixels).

## Stages, each independently shippable

1. **S1 - crate + offscreen proof (macOS-verifiable).** presenter-gpu
   renders a placement list to a wgpu texture, read back to PNG, image-diffed
   against the CPU painter's output for the same list. This is the harness
   every later stage is judged by, and it runs on this Mac via Metal.
2. **S2 - Windows/Linux wiring.** The winit adapters route
   `draw_placements` through presenter-gpu when a wgpu adapter initializes,
   softbuffer otherwise. Feature-gated; env `KRATE_CPU_PRESENT=1` forces the
   fallback for A/B and support.
3. **S3 - frame pacing.** Redraw driven by vsync presents, not by
   paint-when-poked; input latency measured (event to present).
4. **S4 - the charts.** A benchmark app (moving sprites + text + canvas,
   the nova workload) with a frame-time HUD; recorded numbers in
   evidence/perf/ per platform per release. "Top every chart" starts with
   having charts.
5. **S5 - macOS painted surfaces join.** The same presenter replaces the
   NSImageView CPU composite for canvas apps, which also closes K-111's
   Retina softness. Native widgets stay native.

## Acceptance, in numbers

- 60 fps sustained on the nova workload on a 2020-class Windows laptop at
  150% scaling, frame time p99 < 12 ms (measured by S4, recorded in
  evidence/perf/).
- Zero image-diff regressions against the CPU painter on the S1 corpus.
- Input-to-photon under two frames at 60 Hz on S3's measurement.
- The fallback path stays green on machines with no usable GPU.

## Workstations

Per GOALS.md convention; claim before working.

- **W-GPU-1 (scene port):** extract iOS scene building into presenter-gpu,
  S1 harness. No windowing knowledge needed.
- **W-GPU-2 (surface + wiring):** wgpu surface lifecycle in the winit
  adapters, resize/scale/lost-surface handling, S2 + S3.
- **W-GPU-3 (evidence):** S4 benchmark app + perf recording + the
  side-by-side methodology, so claims about smoothness are measurements.

## Risks, named

- wgpu on old Intel iGPUs and remote desktops: the fallback is the answer;
  never gate opening an app on GPU init succeeding.
- Binary size: wgpu+vello adds megabytes; measure in S2, budget is +6 MB.
- Two vello versions (0.9 GPU vs vello_cpu 0.0.9) drifting: pin together in
  the workspace, one upgrade PR moves both.
