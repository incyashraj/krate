# hexyl behaviour audit and Krate comparison

Reference: [sharkdp/hexyl](https://github.com/sharkdp/hexyl), version 0.17.0,
revision `6ecc29b9c8c84d08a7e860f7f69c22b113b480ea`. Audited on macOS ARM64,
2026-09-19. Sources read: README, CLI definitions and implementation in
`src/main.rs`, `src/input.rs`, renderer `src/lib.rs`, colour/character data in
`src/colors.rs`, argument unit tests and all 41 integration tests. Upstream's
10 renderer tests, five CLI unit tests and 41 integration tests pass with
`NO_COLOR` removed from the test process environment. Its first test run had
three colour-test failures because this shell sets NO_COLOR; that was an
environment effect, not a native hexyl defect. Our harnesses normalize it.

This inventories every documented CLI option and the major I/O behaviours.
It does not claim exhaustive testing of all option combinations, operating
systems, devices, terminals or malformed files. Concrete checks and retained
differences are in `parity-v2.json` and `audit.json` in the evidence directory.
Open defects and ownership live only in repository BUGS.md, notably K-424.

## What the original actually does

hexyl is a non-interactive terminal hex viewer, not a desktop editor. It opens
a file or stdin, optionally skips/limits input, reads buffered chunks and
prints a formatted byte dump. Each eight-byte panel has a numerical view and
an optional character view. Colours distinguish null, printable ASCII,
whitespace, other ASCII and non-ASCII bytes. Identical consecutive rows can
be collapsed. It does not parse PNG/ELF structures, edit bytes, search for a
pattern, maintain a selection, or provide a file-picker GUI.

The Krate terminal path uses the attributed upstream renderer with Krate file
and stream handles replacing std I/O. Its GUI is a separate read-only viewer
we wrote. A GUI page showing 320 bytes cannot be benchmarked as if it were
equivalent to hexyl printing a whole 16 MiB file.

## Complete CLI inventory

“Pass” means the listed cases are covered by output comparison or unit tests,
not a universal compatibility guarantee. `tests/audit.py` intentionally
preserves mismatches rather than counting unsupported options as success.

| Original feature | Krate behaviour and evidence |
|---|---|
| FILE, omitted file, `-` stdin | Pass for regular files, implicit/explicit stdin, empty input, Unicode and space-containing paths. Krate paths are sandbox-relative and need a grant. |
| `-n`, `-c`, `-l`, `--length`, `--bytes` | Same rendering for byte limits, zero length, partial rows and buffer boundaries. Alias/parse unit coverage; duplicate scalar options still differ. |
| `-s`, `--skip` | Pass for positive and `+` offsets, end-relative negative file offsets, EOF/past-EOF, forward stdin skips. Negative stdin and before-start seeks reject. |
| `--block-size` | Decimal/custom units supported; blocks cannot define another block size. Original's hexadecimal block-size branch exits successfully without producing a dump; Krate parses it and dumps. This difference is retained in audit.json, not treated as equivalent. |
| Byte-count units | Bytes, hexadecimal, decimal kB/MB/GB/TB, binary KiB/MiB/GiB/TiB, block/blocks. Overflow is checked. Native rejects `b` suffix and so do we. |
| `-v`, `--no-squeezing` | Pass for repeated zero/nonzero rows, partial last rows and disabled squeezing. |
| `--color always/force/never/auto` | Always/force/never output passes. Auto is rejected because the selected guest interface has no terminal-colour query. |
| `NO_COLOR` | Native suppresses colour even for an empty variable; force overrides. Krate does not receive the host environment. Explicit `--color never` is required. |
| `HEXYL_COLOR_*` | Native has six configurable byte/offset colours including named, bright and RGB forms. Krate uses fixed defaults; every variable was probed and differs. |
| `--border unicode/ascii/none` | Pass for all three, including partial rows. |
| `-p`, `--plain` | Pass for hiding offset/characters and border/colour defaults. Explicit border/colour overrides work regardless of ordering. |
| `--no-characters`, `-C`, `--characters` | Pass for hiding/showing and order-dependent override. Plain still hides the panel. |
| `--character-table` | Default, ASCII and braille pass; `codepage-437` and `codepage-1047` are explicitly unsupported. The omitted upstream tables carry separate ISC/EPL notices; they were not silently relicensed under our MIT notice. |
| `--color-scheme default/gradient` | Pass with explicit colour output. |
| `-P`, `--no-position` | Pass. |
| `-o`, `--display-offset` | Pass for nonnegative byte-count offsets combined with skips. Despite its help wording, this upstream revision rejects negative display offsets; both reject the probed case. |
| `--panels N/auto` | Numeric 1..16 supported and tested at several widths. Native accepts larger positive counts; auto and counts above 16 are unsupported in our version. |
| `-g`, `--group-size`, `--groupsize` | 1/2/4/8-byte groups pass, including incomplete final groups. |
| `--endianness big/little`, `-e` | Pass for all group sizes, including little-endian reversal and partial groups. Conflicts with include mode are rejected, but exact error text/exit differs. |
| `-b`, `--base` | Binary, octal, decimal, hex and their names/aliases. Output passes for all four with explicit matching panels. Native's automatic binary layout usually has one panel at width 80; our default remains two. |
| `--terminal-width N` | Explicit widths pass in narrow, grouped, binary and no-character cases. Conflicts with --panels reject. Very large widths are capped at 16 panels in our app. No automatic live width detection. |
| `--print-color-table` | Not implemented; rejected. |
| `-i`, `--include` | C-array output passes for file, explicit/implicit stdin, empty input and incomplete rows. |
| `--completion` | Native bash/elvish/fish/powershell/zsh generation inspected and invoked. All five are unsupported in our app. |
| `-h`, `--help`, `-V`, `--version` | Work, but identify the Krate adaptation and its supported subset, not native hexyl. Output intentionally differs. |
| Short clusters and attached values | `-Pv`, `-n37`, `-n=37`, `-g2`, `-b2` pass. |
| Repeated options / invalid input | We reject key conflicts and invalid values, but do not reproduce clap's full duplicate detection, precedence, suggestions or exact exit-code contract. |

## I/O and error contract

- Native has ambient file access. Krate needs explicit read permission or a
  chosen-file token. Denied and outside-grant paths fail without leaking file
  bytes. Missing files and directories fail in both versions.
- Krate reads in bounded calls and buffers output in 16 KiB chunks. The GUI
  only reads its current page, except incremental search, which scans 32 KiB
  plus pattern overlap per tick. No application network or write access.
- A closed output pipe is normal termination in native hexyl. Our adaptation
  currently reports a generic I/O error and exits 1. Reproduced by consuming
  one line and closing the reader on a 4 MiB fixture. This matters for pipelines.
- Native also handles file-backed non-seekable streams via forward relative
  skipping. Named FIFOs, devices, mid-read mutation, real disk I/O failures,
  terminal resize during output and Windows console behaviour are not verified
  in this Krate adaptation. They are not part of the benchmark claim.
- Errors in our adapted renderer propagate through the guest rather than
  being ignored. The host still mediates every file/stream operation.

## GUI review

This is an extra interface, not part of upstream hexyl parity. It offers
16-column hex/ASCII, a byte inspector, selection/copy, first exact text/hex
search, paging and go-to-offset. It does not offer editing, regex, reverse
search, find-next or decoding of binary file formats.

The original UI's decorative title and teal/navy cards were removed. The new
window uses compact toolbars, a neutral surface, subdued byte categories,
fixed 16px data text, actual measured monospace advances and adaptive visible
row count. The inspector disappears at narrow widths; the data text does not
shrink. Below the fixed grid's width, clipping remains a limitation.

The host now prefers Menlo, Consolas or DejaVu Sans Mono before the generic
monospace mapping. It still renders vector glyphs at the window's backing
scale. The captured native content is 2200 pixels across for a 1100-point
window, so it is 2x, not a 1x screenshot enlarged to Retina. The full PNG also
includes the frame/shadow. View it at native scale when judging sharpness.

Tests cover actual file reads, empty files, search across a buffer boundary,
a 1 GiB sparse-file end offset, compact/large layouts and guest handlers. They
do not establish the full native mouse/keyboard, clipboard or picker path:
macOS input-injection permission is unavailable. The real window has been
rendered, visually inspected and left open for manual use.

## Performance interpretation

See the evidence report for measured numbers. The harness first hashes the
complete output for each pair, then times both programs to /dev/null with the
same options and input. It uses two warmups and nine randomized paired runs,
and retains every sample. Timing includes process launch, bundle validation,
Wasm compilation and runtime initialization, not merely the inner renderer.
The /usr/bin/time wrapper is present on both sides and is part of the wall
measurement. Peak RSS is per process as reported by macOS, not allocated
guest memory and not a system-wide memory delta.

This measures warmed-filesystem, fresh-process end-to-end execution. It does
not measure real terminal painting, true cold cache, persistent compiled
components, GUI latency, power use or isolated steady-state rendering. The
native binary is optimized at level 3; the shipping guest uses size level s;
both use LTO and one codegen unit. This is a comparison of these actual builds,
not a controlled claim about intrinsic native-vs-Wasm execution speed.

The bundle includes the GUI, editable source and SDK. Native hexyl is CLI-only.
Report both bundle size and the separately required shared runtime; a smaller
.krate download is not a smaller first installation. No claim of a faster or
lower-memory replacement is justified by the measurements so far.
