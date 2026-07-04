# Fuzz Evidence

Phase 2 needs fuzz proof before exit because the UAPI boundary handles paths,
manifests, and capability patterns.

The fuzz evidence recorder runs the Phase 2 fuzz smoke command and writes one
markdown report with the commit, host, target list, duration, tool versions,
result, and log tail.

```bash
scripts/record-phase2-fuzz-evidence.sh --strict
```

Default output:

```text
target/phase2-fuzz-evidence/fuzz-evidence.md
```

For a quick local check, use a short run:

```bash
scripts/record-phase2-fuzz-evidence.sh --strict --max-total-time 5
```

For final Phase 2 review, run the same recorder on the final candidate with a
larger time value on the self-hosted runner:

```bash
scripts/record-phase2-fuzz-evidence.sh --strict --max-total-time 14400
```

That example means four hours per fuzz target.

## What It Records

The report includes:

- git commit
- host operating system and architecture
- fuzz targets
- seconds per target
- dry-run flag
- cargo-fuzz, rustup, and nightly rustc details
- command exit code
- fuzz log tail

## Include It In The Exit Bundle

```bash
scripts/record-phase2-exit-bundle.sh --strict --include-fuzz
```

The final review bundle includes fuzz evidence too:

```bash
scripts/record-phase2-exit-bundle.sh --final-review
```

Set `KRATE_FUZZ_MAX_TOTAL_TIME` first if the final review should use a longer
soak window.

## Current Reading

A short fuzz smoke is useful during normal development. A longer self-hosted
soak remains one of the final Phase 2 proof items.
