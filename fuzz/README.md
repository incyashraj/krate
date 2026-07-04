# Krate fuzz targets

This folder contains the first Phase 2 fuzz harness set.

## Current targets

- `manifest_parse`: fuzzes `krate_manifest::Manifest::parse`
- `logical_path_parse`: fuzzes `LogicalPath::parse` and filesystem operation intent checks
- `policy_match`: fuzzes capability grant and requirement parsing plus session-policy matching

## Run locally

Install once:

```bash
cargo install cargo-fuzz --locked
```

Then from repo root:

```bash
cargo fuzz run manifest_parse -- -max_total_time=300
cargo fuzz run logical_path_parse -- -max_total_time=300
cargo fuzz run policy_match -- -max_total_time=300
```

## Notes

- This is the first scaffold pass to satisfy Phase 2 fuzz target definition work.
- Nightly multi-hour fuzz scheduling is still tracked as a separate remaining item.
