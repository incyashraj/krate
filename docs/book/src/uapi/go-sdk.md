# Go SDK

The Go SDK is now started at `packages/sdk-go`. This is the first Go/TinyGo
shape for Phase 2, not the final component proof.

## Current Status

What exists now:

- A Go module at `packages/sdk-go`.
- Public packages for `io`, `fs`, `net`, `time`, and `locale`.
- Example source files for Go clock, cat, and curl-style CLI apps.
- A dependency-free shape check that guards the package layout.

What still needs proof:

- Install Go and TinyGo.
- Generate or wire WIT bindings behind the public helpers.
- Build a real Go component.
- Run that component through `krate run`.
- Add fixture-backed tests like the Rust samples.

## Example

```go
package main

import (
    l36io "github.com/incyashraj/layer6x6/packages/sdk-go/krate/io"
    l36net "github.com/incyashraj/layer6x6/packages/sdk-go/krate/net"
)

func main() {
    args := l36io.Args()
    if len(args) == 0 {
        _ = l36io.Eprintln("usage: krate-go-curl <url>")
        return
    }

    body, err := l36net.GetText(args[0])
    if err != nil {
        _ = l36io.Eprintln(err.Error())
        return
    }

    _ = l36io.Print(body)
}
```

The longer examples live here:

- `packages/sdk-go/examples/krate-clock/main.go`
- `packages/sdk-go/examples/krate-cat/main.go`
- `packages/sdk-go/examples/krate-curl/main.go`

## Tooling

Run:

```bash
krate doctor
```

For this track, these lines should be present:

```text
tinygo          ...
go              ...
```

If either is missing, the Go runtime proof is blocked. The rest of Krate can
continue without them.

## Current Check

The normal CI path runs a small package shape check:

```bash
node packages/sdk-go/scripts/check-shape.mjs
```

This does not compile a component. It catches simple mistakes such as missing
helper packages, wrong module path, accidental `wasi:*` imports, or missing
public helper names.
