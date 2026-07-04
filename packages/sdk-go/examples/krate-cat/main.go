package main

import (
	l36fs "github.com/incyashraj/krate/packages/sdk-go/krate/fs"
	l36io "github.com/incyashraj/krate/packages/sdk-go/krate/io"
)

func main() {
	args := l36io.Args()
	if len(args) == 0 {
		_ = l36io.Eprintln("usage: krate-go-cat <path> [path...]")
		return
	}

	for _, file := range args {
		body, err := l36fs.ReadText(file)
		if err != nil {
			_ = l36io.Eprintln(err.Error())
			return
		}
		_ = l36io.Print(body)
	}
}
