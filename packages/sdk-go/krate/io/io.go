package io

import (
	"strings"

	layer36 "github.com/incyashraj/layer6x6/packages/sdk-go/layer36"
)

var (
	ArgsRawHook     = func() string { return "" }
	StdoutWriteHook = func([]byte) error { return layer36.ErrGeneratedBindingsMissing }
	StderrWriteHook = func([]byte) error { return layer36.ErrGeneratedBindingsMissing }
)

func Args() []string {
	raw := ArgsRawHook()
	if raw == "" {
		return nil
	}

	parts := strings.Split(raw, "\n")
	args := parts[:0]
	for _, arg := range parts {
		if arg != "" {
			args = append(args, arg)
		}
	}

	return args
}

func Print(value string) error {
	return StdoutWriteHook([]byte(value))
}

func Println(value string) error {
	return Print(value + "\n")
}

func Eprint(value string) error {
	return StderrWriteHook([]byte(value))
}

func Eprintln(value string) error {
	return Eprint(value + "\n")
}
