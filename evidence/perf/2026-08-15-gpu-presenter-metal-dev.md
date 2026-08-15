# GPU presenter: first pacing measurement

2026-08-15, dev Mac (Apple silicon, Metal, 60 Hz display), debug build,
`gpu_window` example (button + field + animated slider), 240 frames,
KRATE_FRAME_STATS=1:

    render p50 16.56ms p99 17.07ms
    present interval p50 16.68ms p99 17.18ms   (60.0 fps, vsync-locked)
    input-to-present p50 16.60ms p99 17.11ms   (under one frame)

Reading: the interval sits on the display's 16.67ms with a 0.5ms p99 spread
-- AutoVsync is pacing, not the CPU. "render" includes the vsync block, so
it measures waiting, not work. Acceptance asks for the same shape on a
2020-class Windows laptop at 150% (S4 does that run on real hardware).

## S4 nova-workload throughput (same machine, release build)

    bench-nova: 960x640 @1.5x, 40 sprites+text+bar, 300 frames:
    p50 1.63ms p90 2.98ms p99 3.11ms (raw render+readback, no vsync)

Reading: ~9ms of headroom under the 12ms p99 acceptance budget, and the
readback in this number is a cost the windowed path does not pay. The same
example must be re-run on the 2020-class Windows laptop for the chart that
counts.
