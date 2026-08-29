# Krate Go SDK

This is the first Go/TinyGo shape for the Phase 2 UAPI. It is a draft facade,
not the final generated binding proof.

The package gives Go app authors stable names for the same `io`, `fs`, `net`,
`time`, and `locale` modules used by the Rust and TypeScript tracks. The actual
TinyGo component build still needs the Go toolchain and generated WIT bindings.

```go
package main

import (
    krateio "github.com/incyashraj/krate/packages/sdk-go/krate/io"
    kratenet "github.com/incyashraj/krate/packages/sdk-go/krate/net"
)

func main() {
    args := krateio.Args()
    if len(args) == 0 {
        krateio.Eprintln("usage: krate-go-curl <url>")
        return
    }

    body, err := kratenet.GetText(args[0])
    if err != nil {
        krateio.Eprintln(err.Error())
        return
    }

    krateio.Print(body)
}
```

Until TinyGo is wired, the host-call hooks fail with a clear setup error. That
keeps this package useful for API review without hiding the missing runtime
piece.

Current sample sources:

- `examples/krate-clock/main.go`
- `examples/krate-cat/main.go`
- `examples/krate-curl/main.go`
