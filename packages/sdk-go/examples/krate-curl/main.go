package main

import (
	"os"
	"strings"

	l36io "github.com/incyashraj/layer6x6/packages/sdk-go/layer36/io"
	l36net "github.com/incyashraj/layer6x6/packages/sdk-go/layer36/net"
)

func main() {
	os.Exit(run())
}

func run() int {
	args := l36io.Args()
	if len(args) == 0 {
		_ = l36io.Eprintln("usage: layer36-go-curl <url>")
		return 2
	}

	body, err := l36net.GetText(args[0])
	if err != nil {
		message, code := classifyNetError(err)
		_ = l36io.Eprintln(message)
		return code
	}

	_ = l36io.Print(body)
	return 0
}

func classifyNetError(err error) (string, int) {
	msg := strings.ToLower(err.Error())
	switch {
	case strings.Contains(msg, "permission denied"), strings.Contains(msg, "permission-denied"):
		return "layer36-go-curl: permission denied", 5
	case strings.Contains(msg, "invalid-url"):
		return "layer36-go-curl: invalid url", 20
	case strings.Contains(msg, "body-too-large"):
		return "layer36-go-curl: response too large", 21
	case strings.Contains(msg, "timeout"):
		return "layer36-go-curl: request timed out", 21
	case strings.Contains(msg, "protocol"):
		return "layer36-go-curl: protocol error", 21
	case strings.Contains(msg, "tls-failure"):
		return "layer36-go-curl: tls handshake failed", 21
	case strings.Contains(msg, "dns-failure"):
		return "layer36-go-curl: dns lookup failed", 21
	case strings.Contains(msg, "connect-failure"):
		return "layer36-go-curl: connection failed", 21
	default:
		return "layer36-go-curl: fetch failed", 21
	}
}
