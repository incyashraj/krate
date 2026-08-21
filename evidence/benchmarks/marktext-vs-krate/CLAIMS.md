# Claim policy for this benchmark

## Allowed after a passing audited run

- “On the same [5,000/50,000]-line notes workload, on [machine], Krate used X
  and MarkText 0.17.1 ARM64 used Y.”
- “The individual Krate app payload was X bytes. MarkText occupied Y MiB after
  installation. Krate Studio/runtime occupied Z MiB once.”
- “Across ten alternating warm starts, median time to a visible window was X
  for Krate and Y for MarkText; p95 was A and B.”
- “The raw scripts, inputs, hashes, samples, rejected runs, and analysis are
  available in [link].”

Every sentence must name the machine, operating system, app versions, workload,
measurement boundary, and whether a number is a median, p95, or single settled
sample.

## Not allowed from this test

- “Krate is 7,000x smaller” without saying this is per-app payload versus the
  installed comparison app and disclosing the shared Krate installation.
- “Krate uses 24x less RAM” without naming the exact 50,000-line workload and
  whole-process-tree measurement.
- “Krate is faster than Electron” as a universal statement.
- “Krate and MarkText are the same product.”
- Any Windows, Linux, battery-life, or energy claim without a separate run.
- The fastest observed sample presented as typical performance.
- A percentage derived from rounded display numbers when exact raw values exist.

## Historical numbers

The 2026-08-16 Markdown files are historical internal lab notes. Their 271 MiB
installed MarkText size corresponds to the x86_64 package. Until a clean ARM64
run is retained through this kit, performance numbers from those notes must be
labelled “internal measurement” rather than “independently reproducible.”
