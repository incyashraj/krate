# MarkText versus Krate benchmark analysis

Generated only from the raw files in this run directory. This is an
equivalent-workload comparison, not a claim of feature parity.

## Artifact size

- MarkText 0.17.1 ARM64 installed: **284.6 MiB**
- Krate notes app payload: **37,489 bytes (36.6 KiB)**
- Incremental per-app payload ratio: **7,961x smaller**
- Shared Krate Studio/runtime installed once: **88.6 MiB**
- First-app disk ratio including shared runtime: **3.21x smaller**

## Time to visible window

| Workload | Mode | App | n | Median | p95 | Range |
|---:|---|---|---:|---:|---:|---:|
| 5,000 | warm | krate | 10 | 164.0 ms | 181.7 ms | 150.0-181.7 ms |
| 5,000 | warm | marktext | 10 | 613.8 ms | 698.6 ms | 583.1-698.6 ms |
| 50,000 | warm | krate | 10 | 237.1 ms | 248.8 ms | 217.3-248.8 ms |
| 50,000 | warm | marktext | 10 | 611.5 ms | 842.8 ms | 595.0-842.8 ms |

- 5,000 lines, warm: median visible-window time is **3.74x lower** for Krate.
- 50,000 lines, warm: median visible-window time is **2.58x lower** for Krate.

## Settled resources

| Workload | App | Processes | Footprint | Average CPU |
|---:|---|---:|---:|---:|
| 5,000 | krate | 1 | 62.0 MiB | 0.10% of one core |
| 5,000 | marktext | 4 | 366.5 MiB | 0.10% of one core |
| 50,000 | krate | 1 | 178.5 MiB | 1.97% of one core |
| 50,000 | marktext | 4 | 2299.4 MiB | 7.38% of one core |

## Controlled scrolling

| Workload | App | Event rate | Average whole-tree CPU |
|---:|---|---:|---:|
| 5,000 | krate | 125.0 Hz | 24.17% of one core |
| 5,000 | marktext | 125.0 Hz | 48.23% of one core |
| 50,000 | krate | 125.0 Hz | 37.23% of one core |
| 50,000 | marktext | 125.0 Hz | 43.77% of one core |

## Energy

Not measured. Do not make a power or battery-life claim from this run.

## Rejected samples

None recorded.
