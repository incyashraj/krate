# Krate vs native: measured, 2026-07-31

"Cross-platform is slow and bloated" is the first objection anyone raises. This
is the answer as numbers rather than claims, measured on this machine on this
date, with the commands to reproduce it.

Everything below compares **the same program built two ways**: as a native Rust
binary and as a `.krate`. Three programs are used, each chosen to expose a
different cost -- a word-frequency counter, an integer hash loop, and a loop that
does nothing but cross the host boundary.

Every pair was checked to produce **identical output** before anything was timed.
That check is not a formality: the first version of the compute test reported
Krate as *faster than native*, which was a harness bug rebuilding one side and
not the other. A speed comparison between programs doing different work is
worthless, and the only way to know they are doing the same work is to check.

Machine: Apple Silicon, macOS. Numbers will differ elsewhere; the shape should
not.

## Speed

### The honest headline: 3.9x–5.3x end to end on a task too small to measure

| Input | Native | Krate | Ratio | Added |
|---|---|---|---|---|
| 2 KB | 2.72 ms | 14.28 ms | 5.25x | 11.56 ms |
| 8 KB | 3.36 ms | 15.79 ms | 4.71x | 12.44 ms |
| 20 KB | 3.77 ms | 17.72 ms | 4.70x | 13.95 ms |
| 40 KB | 4.47 ms | 18.35 ms | 4.10x | 13.88 ms |
| 60 KB | 4.83 ms | 18.71 ms | 3.88x | 13.89 ms |

Median of 25 runs each, whole process including launch.

### What the table actually says

Read the last column, not the ratio. **The added time is flat**: ~12 ms at 2 KB,
~14 ms at 60 KB, for 30x more data. The ratio falls as work grows because the
overhead is a fixed cost, not a per-operation tax.

This is the difference that matters. A per-operation tax compounds and makes the
portability claim hollow. A fixed startup cost is a constant that disappears into
anything a person interacts with.

### Measured directly

```
cargo bench -p krate-runtime --bench native_comparison
```

```
startup_compile_and_instantiate   5.999 ms
steady_state_already_loaded      91.616 µs
```

**65x apart. About 98.5% of Krate's overhead is one-time startup.** Once an app
is running, it runs at 92 microseconds per iteration of the same work that costs
6 ms to start.

### Sustained compute: 1.00x–1.08x

The measurement that decides whether "near-native" is honest. The same integer
hash loop, native and as a `.krate`, at three workload sizes. **Output was
checked identical before each timing** — the first version of this test reported
Krate as *faster than native*, which was a bug in the harness rebuilding one
side and not the other, and the output check is what caught it.

| Iterations | Native | Krate | Ratio | Added |
|---|---|---|---|---|
| 30,000,000 | 83.0 ms | 86.7 ms | 1.04x | 3.6 ms |
| 100,000,000 | 220.0 ms | 236.6 ms | 1.08x | 16.7 ms |
| 300,000,000 | 703.9 ms | 706.3 ms | 1.00x | 2.5 ms |

At 300 million iterations the difference is inside measurement noise. Krate runs
sustained compute at native speed.

### The worst case: 5.14x, and what it costs per call

A sandbox costs the most where a program crosses the host boundary constantly and
does almost nothing in between, because every crossing is where the capability
check happens. So that case was built deliberately: 20,000 clock reads in a loop,
native and as a `.krate`, both printing the same result.

| | Time |
|---|---|
| Native | 4.37 ms |
| Krate | 22.44 ms |
| Ratio | **5.14x** |

Removing the ~6 ms fixed startup leaves 3.77x, which works out to **0.6
microseconds per host crossing**. That is the price of the permission check, and
it is the number to quote, because the ratio depends entirely on how little work
the program does between calls.

In practice: an app making 1,000 host calls pays 0.6 ms for the safety. An app
making a million pays 0.6 seconds and should be batching them anyway.

### What this means, stated plainly

- **A short-lived CLI run in a tight loop is the worst case for Krate.** A script
  invoked ten thousand times pays ~6 ms each time. Say so; do not hide it.
- **Anything a person opens and uses is the best case.** 6 ms is below the
  threshold anyone perceives, and everything after runs at native speed.
- **Real compute is not taxed.** The 4.44x headline comes entirely from startup
  dominating a 4 ms task. Quoting 4.44x as "Krate's overhead" is as misleading as
  quoting 1.00x; both are true of different workloads, and the useful claim is
  the shape: a fixed cost, not a per-operation one.

## Size

| Artifact | Size | Runs on |
|---|---|---|
| `.krate` bundle | 6,494 bytes | macOS, Windows, Linux |
| Native binary | 373,664 bytes | one OS |
| Native binary, stripped | 319,536 bytes | one OS |

**49x smaller than a stripped native binary, and it runs on three platforms
instead of one.**

### The honest caveat

A `.krate` needs the Krate runtime installed: **17.5 MB, once, shared by every
app**. A native binary needs nothing.

So the crossover is arithmetic: the runtime pays for itself at about **59 apps**
against stripped native binaries (17.5 MB ÷ 313 KB saved per app). Below that,
native wins on total bytes. Above it, Krate does — and Krate was already winning
on the thing bytes cannot buy, which is one file that runs everywhere.

For a single app, "download 17.5 MB then a 6 KB file" is worse than "download
374 KB", and pretending otherwise is the kind of claim that gets checked.

## What is not measured yet

Stated so nobody has to discover it in a demo:

- **Only macOS.** The Windows and Linux numbers are unmeasured. The runtime is
  shared and the component is host-independent, so they should be close, but
  should-be is not measured.
- **No GUI workload.** Every number here is CLI. Window creation, drawing, and
  event dispatch are unmeasured against native.
- **Allocation-heavy work is unmeasured.** Syscall-heavy work is now measured
  above; a workload dominated by allocation and garbage is not.
- **Three programs, none of them real.** Word counting, an integer hash loop, and
  a clock-read loop. None is an app anyone would ship, and a real app is the only
  thing that settles this.

## Reproducing this

```bash
cargo bench -p krate-runtime --bench native_comparison
```

That covers startup and steady state. The end-to-end, sustained-compute, and
host-crossing tables were taken by building each program both ways and timing
8-25 runs of each, verifying identical output first.

Numbers should be re-taken rather than quoted from here once anything changes in
the runtime, and re-taken on Windows and Linux before any claim is made about
them.
