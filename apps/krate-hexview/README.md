# Hexview: hexyl on Krate, with a native viewer

An experimental, attributed adaptation of David Peter's hexyl renderer, plus
a new read-only graphical byte inspector. This is real Rust compiled to a
WebAssembly component using Krate's file, stream, window and canvas APIs.
There is no subprocess call to native hexyl and no web view. The native hexyl
executable is used only as a test oracle. This revision also improves the host's
generic monospace font preference; the app does not require a new WIT interface.

Upstream: https://github.com/sharkdp/hexyl

Pinned revision: `6ecc29b9c8c84d08a7e860f7f69c22b113b480ea` (0.17.0).
The upstream renderer and gradient calculations are adapted under MIT.
`LICENSE-MIT` and `NOTICE` are carried both with the source and as bundle assets.
This is not an official hexyl release and does not imply David Peter's endorsement.

## What works in this proof

| Area | Implemented scope |
| --- | --- |
| Terminal dump | hexyl's renderer, byte-category colours, positions, squeezing |
| Ranges | byte limits, positive skips, end-relative file skips, display offset, units |
| Formatting | 1..16 panels, 1/2/4/8-byte groups, endian selection, bases 2/8/10/16 |
| Output choices | Unicode/ASCII/no border, default/ASCII/braille characters, gradient |
| Other terminal modes | plain output, hide character/position panels, C-array output, stdin |
| GUI | hex/ASCII grid, category colours, byte inspector, paging, selection, offsets |
| GUI search | first exact UTF-8 text or hex-byte match, incremental 32 KiB scan, Escape to cancel |
| File I/O | read-only handles, seek/read pages rather than loading entire files |

GUI character cells deliberately use `.` for non-printable bytes, rather than
the terminal renderer's Unicode symbols. The terminal mode retains the original
symbols. This is a viewer, not a hex editor: it never modifies the input file.

## Evidence and its limits

Verified locally on macOS ARM64, using the repository's rebuilt Krate 0.4.0
development runtime, not the public release installer:

- 14 native unit tests for the renderer and argument parsing.
- 98 terminal checks: 92 successful native-vs-Krate byte-for-byte output
  comparisons, plus six error/permission checks. Output hashes are in `parity-v2.json`.
- Eight viewer checks: normal/empty files, text and hex search crossing the 32 KiB
  read boundary, the final seven bytes of a **1 GiB + 7 byte sparse file**, and
  a handler self-test for paging, selection, search, offsets and resize, and
  compact/large window captures with page-length checks.
- Native macOS window launched and photographed. It remained open for more
  than 30 seconds while the test work continued.

**Not yet verified:** native mouse/keyboard input, clipboard interaction, the
native file picker end to end, or execution on Windows/Linux. The local macOS
test process reports `AXIsProcessTrusted() == false` and
`CGPreflightPostEventAccess() == false`; OS event injection was unavailable.
The handler tests do not substitute for these native interactions. A real file
was verified through the `--file` path, not the native picker.

Performance is measured separately by `tests/benchmark.py`: eight workloads,
identical-output preflights, two warmups, nine randomized paired measurements.
See `krate-evidence/hexview-2026-09-19/RESULTS.md` and `benchmark/benchmark.json`.
Native hexyl wins the initial speed and process-memory comparison. Do not use
this adaptation to claim a performance advantage. Sparse-file tests remain
correctness evidence, not full-file scan timings.

## Deliberate differences from hexyl

- Explicit `--terminal-width` works, but no live terminal width/TTY detection,
  environment-configured colours, `NO_COLOR`,
  shell completions, colour-table printing, CP437 or CP1047 tables.
- Colour defaults to on; pass `--color never` for plain captured output.
- Two panels by default. Set `--panels N` explicitly for comparisons.
- Attached short values (`-n16`) and flag clusters (`-Pv`) now work. Explicit
  colour/border settings override plain mode in either order. Duplicate scalar
  arguments and exact clap diagnostics still differ. This is not a drop-in CLI replacement.
- File paths are sandbox-relative and explicitly granted under `input/`.
  This deliberately differs from hexyl's ambient access to host paths.
- Native binary file reading is replaced with bounded Krate I/O calls, and
  output is buffered in 16 KiB chunks. Renderer read errors propagate.
- GUI search returns the first match; no regex, reverse search or find-next yet.
- GUI uses fixed-size vector text and measured monospace cells, never a scaled
  whole-window design. It keeps 16 byte columns, shortens the visible page on
  smaller windows, and hides the inspector when there is not enough width.
  Extremely narrow windows still clip the fixed grid; there is no horizontal scrollbar.
- The GUI currently shows Mac shortcut labels; Ctrl also works in its handlers.

## Build

From the repository root, using Rust 1.94.1 and cargo-component 0.21.1:

```sh
$HOME/.cargo/bin/cargo component build --release --manifest-path apps/krate-hexview/Cargo.toml
mkdir -p krate-evidence/hexview-2026-09-19
./target/release/krate pack --manifest apps/krate-hexview/manifest.toml \
  -o krate-evidence/hexview-2026-09-19/Hexview.krate \
  apps/krate-hexview/target/wasm32-wasip1/release/krate_hexview.wasm
```

The app generates bindings for the frozen phase-3 interface and uses the SDK
for allocator/panic support without enabling its GUI binding set. This avoids
the explicit-world linker issue recorded in **BUGS.md K-422**. Keep output
outside the source folder because of **BUGS.md K-421**. Neither runtime defect
was patched for this proof. Other working-tree changes were left untouched.

## Open and use

```sh
./target/release/krate run --native-window \
  --grant ui.window:create --grant ui.dialog:file-open --grant ui.clipboard:write \
  --grant io.args --grant io.stdout --grant io.stderr \
  krate-evidence/hexview-2026-09-19/Hexview.krate
```

Starts with a clearly labelled built-in byte fixture. **Open file** invokes the
native picker; the chosen file is opened read-only through its token. Next and
Previous page; click a byte to inspect it. Cmd/Ctrl-F searches, Cmd/Ctrl-G jumps,
Cmd/Ctrl-C copies the selection as hex. Arrow keys, Home/End and PageUp/PageDown
are supported. Search accepts text or `hex: 89 50 4e 47`.

For a terminal dump, put the input in a sandbox's `input/` directory:

```sh
./target/release/krate run --headless --sandbox-root /path/to/sandbox \
  --grant io.args --grant io.stdout --grant io.stderr --grant 'fs.read:input/**' \
  krate-evidence/hexview-2026-09-19/Hexview.krate -- \
  --dump --color never --panels 2 -n 256 input/example.bin
```

Stdin additionally needs `--grant io.stdin`; pass `-` as the filename.
GUI automation can preload `--file input/example.bin`, choose `--offset 0x1000`
or `--find 'hex: 50 4b'`. `--snapshot` exits after drawing; `--probe` prints
the actual page and selection for the verification harness.

## Reproduce verification

Build native hexyl at the pinned revision first. With `HEXYL` pointing at that
binary and `BUNDLE` at the built bundle:

```sh
$HOME/.cargo/bin/cargo test --manifest-path apps/krate-hexview/Cargo.toml
python3 apps/krate-hexview/tests/parity.py --krate target/release/krate \
  --hexyl "$HEXYL" --bundle "$BUNDLE" --report /tmp/hexview-parity.json
python3 apps/krate-hexview/tests/viewer.py --krate target/release/krate \
  --bundle "$BUNDLE" --output /tmp/hexview-viewer-checks
python3 apps/krate-hexview/tests/audit.py --krate target/release/krate \
  --hexyl "$HEXYL" --bundle "$BUNDLE" --report /tmp/hexview-audit.json
python3 apps/krate-hexview/tests/benchmark.py --krate target/release/krate \
  --hexyl "$HEXYL" --bundle "$BUNDLE" --output /tmp/hexview-benchmark
```

## Before contacting the maintainer

1. Manually verify the native controls, file picker, resize and clipboard.
2. Run the same bundle and equivalent tests on Windows and Linux.
3. Retain the measured **terminal-to-terminal** comparison and its failures;
   do not substitute GUI paging for hexyl dumping an entire file.
4. Use fixed real and synthetic inputs, including incompressible and repeated
   data. Match options, output destination, limits, build configuration and host.
5. The current benchmark is full-process time with warmed filesystem caches.
   It is not a true cold-cache test or an isolated steady-state renderer test.
   Peak process RSS is measured separately from guest linear memory.
6. Include Krate runtime/JIT cost. Separate bundle download size, component size,
   the shared installed runtime and total first-install footprint. A small
   bundle is not evidence of a smaller complete installation.
7. Native hexyl is already Rust. Do not assume a Wasm adaptation beats it.
   Permission control and distribution may be the result even if it is slower.

The GUI is an additional interface to demonstrate Krate capabilities; its own
responsiveness should be evaluated separately. Send neither performance claims
nor an unsolicited upstream contribution until the evidence is ready.
