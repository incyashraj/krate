# Layer36 Go SDK

This is the first Go/TinyGo shape for the Phase 2 UAPI. It is a draft facade,
not the final generated binding proof.

The package gives Go app authors stable names for the same `io`, `fs`, `net`,
`time`, and `locale` modules used by the Rust and TypeScript tracks. The actual
TinyGo component build still needs the Go toolchain and generated WIT bindings.

```go
package main

import (
    l36io "github.com/incyashraj/layer6x6/packages/sdk-go/layer36/io"
    l36net "github.com/incyashraj/layer6x6/packages/sdk-go/layer36/net"
)

func main() {
    args := l36io.Args()
    if len(args) == 0 {
        l36io.Eprintln("usage: layer36-go-curl <url>")
        return
    }

    body, err := l36net.GetText(args[0])
    if err != nil {
        l36io.Eprintln(err.Error())
        return
    }

    l36io.Print(body)
}
```

Until TinyGo is wired, the host-call hooks fail with a clear setup error. That
keeps this package useful for API review without hiding the missing runtime
piece.

Current sample sources:

- `examples/layer36-clock/main.go`
- `examples/layer36-cat/main.go`
- `examples/layer36-curl/main.go`
