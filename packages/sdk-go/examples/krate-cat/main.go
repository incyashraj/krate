package main

import (
	kratefs "github.com/incyashraj/krate/packages/sdk-go/krate/fs"
	krateio "github.com/incyashraj/krate/packages/sdk-go/krate/io"
)

func main() {
	args := krateio.Args()
	if len(args) == 0 {
		_ = krateio.Eprintln("usage: krate-go-cat <path> [path...]")
		return
	}

	for _, file := range args {
		body, err := kratefs.ReadText(file)
		if err != nil {
			_ = krateio.Eprintln(err.Error())
			return
		}
		_ = krateio.Print(body)
	}
}
