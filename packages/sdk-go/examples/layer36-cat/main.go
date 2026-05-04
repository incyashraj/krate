package main

import (
	l36fs "github.com/incyashraj/layer6x6/packages/sdk-go/layer36/fs"
	l36io "github.com/incyashraj/layer6x6/packages/sdk-go/layer36/io"
)

func main() {
	args := l36io.Args()
	if len(args) == 0 {
		_ = l36io.Eprintln("usage: layer36-go-cat <path> [path...]")
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
