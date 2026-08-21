# Benchmark artifact

The exact historical Krate notes replica is staged automatically into every run
directory as `artifacts/mark-replica.krate`. Its expected SHA-256 is:

```text
f2f8027ed356e8217e2e1d2764a8befe2d02be58fef5f4e1ce4bd932f074be44
```

The bundle is 37,489 bytes and carries its source. Before the benchmark is made
public, copy that staged file into this directory or attach it to the benchmark
release. A hash without retrievable bytes is identification, not reproduction.

MarkText is not redistributed here. `scripts/download-marktext-arm64.sh`
downloads the official 0.17.1 ARM64 release and verifies its pinned hash.
