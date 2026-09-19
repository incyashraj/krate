# grex bundle metadata

The committed `grex.krate` contains the existing ported WebAssembly program and
its manifest. `grex.manifest.toml` is the reviewable manifest used to repack it.
The port's source tree is not included in this repository; changing this
manifest does not rebuild or change the program.

Direct inputs, `quick`, and `--help` need no filesystem grant. `--file` and `-f`
need the optional `fs.read:./input/**` capability. Making it optional lets other
input modes start; it does not grant file access or bypass the runtime's scope.

To regenerate the bundle after a manifest change:

```sh
python3 scripts/test-grex.py --krate /absolute/path/to/krate --repack
```

This extracts the existing component, packs it with `krate pack`, verifies the
component bytes are identical and the manifest matches, then runs eight real
input/permission checks in isolated temporary profiles. It requires a runtime
compatible with the bundle; the metadata repair was tested on public v0.5.0.

Without `--repack`, the same command only verifies the existing bundle. The
cross-platform ported-app replay invokes these checks without blanket grants.
