# MarkText versus Krate: reproducible macOS benchmark

This directory turns the 2026-08-16 notes comparison into an auditable test.
It deliberately separates three things that were previously mixed together:

1. **Artifact size**, which anyone can verify without opening either app.
2. **Equivalent-workload performance**, which requires the same document,
   machine state, architecture, and external measurement tools.
3. **Product scope**, which is not equivalent. MarkText is a mature editor with
   more features. The Krate replica implements the workflow being measured.

The historical result notes are one directory above. They are useful lab notes,
but they do not include the original raw samples or the window probe. This kit
is the replacement. A result is publishable only when `scripts/audit.py` reports
`PASS` and the complete run directory is committed unchanged.

## What this kit proves

The controlled workload is:

- one notes list and editor;
- local persistence;
- the same generated Markdown document;
- either 5,000 or 50,000 content lines;
- warm launch, settled idle, memory footprint, and controlled scrolling.

It does **not** claim that the two products have feature parity. It does not
measure Windows or Linux. It does not convert a per-app Krate file size into the
size of the shared Krate Studio/runtime installation.

## Required inputs

- Apple-silicon Mac. The default protocol refuses an x86_64 MarkText binary on
  this machine so Rosetta cannot quietly distort the result.
- MarkText 0.17.1 ARM64 from its official GitHub release.
- The exact `mark-replica.krate` bundle used for the Krate leg.
- A release Krate CLI/runtime.
- Xcode Command Line Tools (`swiftc`) and Python 3.
- Accessibility permission for Terminal only for the automated scroll test.
- `sudo` only for the optional energy run (`powermetrics`).

Copy `config.example.env` to `config.env` and set the three local paths. Do not
commit `config.env`; run outputs record canonical paths, hashes, versions, and
architectures instead.

## Run

```bash
cd evidence/benchmarks/marktext-vs-krate
cp config.example.env config.env
# edit config.env

./scripts/prepare.sh
./scripts/run-suite.sh
```

`prepare.sh` performs no application launch. It:

- generates byte-identical 5,000- and 50,000-line workloads;
- compiles the two small Swift measurement tools;
- verifies artifact hashes and native architecture;
- writes a machine and input manifest.

`run-suite.sh` creates a timestamped directory under `runs/`, uses an alternating
ABBA schedule, and retains every TSV, JSON, log, and configuration snapshot.
The default is ten warm-start samples per app and workload, plus one settled
resource sample and one controlled-scroll sample per leg.

For a non-invasive check of the kit itself:

```bash
./scripts/self-test.sh
```

## Fresh-profile runs

The old phrase “first-ever open” is not sufficiently precise. Gatekeeper,
quarantine, OS caches, display wake, and application profiles all affect it.
This kit calls such a run `fresh_profile`, creates a new profile/home for each
leg, and records it separately from `warm` runs:

```bash
BENCH_START_MODE=fresh_profile ./scripts/run-suite.sh
```

Do not combine fresh-profile and warm results in one average.

## Publication checklist

Before quoting a result publicly:

- [ ] `scripts/audit.py RUN_DIR` prints `PASS`.
- [ ] MarkText and Krate ran the same host architecture.
- [ ] Both apps used the same fixture SHA-256.
- [ ] At least ten startup samples exist for each app/workload.
- [ ] Median and p95 are reported; no cherry-picked fastest run is used.
- [ ] The process count and whole process tree were measured.
- [ ] The exact Krate bundle is included or downloadable by its recorded hash.
- [ ] Raw outputs are committed with the analysis.
- [ ] Any rejected sample remains visible with a rejection reason.
- [ ] The shared Krate Studio/runtime size is disclosed beside per-app size.
- [ ] The comparison is described as equivalent workload, not feature parity.
- [ ] A second person reproduces the run on a different Apple-silicon Mac.

## Independent reproduction

An outside tester should receive only this directory and artifact links. They
should not receive a prepared results table. Ask them to:

1. verify the hashes;
2. run `prepare.sh`, `self-test.sh`, and `run-suite.sh`;
3. publish the untouched run directory and `git rev-parse HEAD`;
4. report failures as failures rather than editing samples.

Two matching independent runs are substantially stronger proof than a screen
recording. A continuous screen recording is still useful to show the setup,
architecture check, test execution, and final audit without cuts.

## Current evidence status

The official MarkText 0.17.1 artifacts and the local 37,489-byte Krate bundle
have been independently hashed. The historical 271 MiB installed figure matches
MarkText's **x86_64** macOS package. The ARM64 package is approximately 284.6
MiB, so a new M4 performance run must use and report the ARM64 package. The old
performance table should remain labelled “historical internal measurement” until
this kit has produced and retained a clean raw run.
