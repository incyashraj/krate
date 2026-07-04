package net

import krate "github.com/incyashraj/krate/packages/sdk-go/krate"

type HTTPMethod string

const (
	HTTPGet     HTTPMethod = "get"
	HTTPPost    HTTPMethod = "post"
	HTTPPut     HTTPMethod = "put"
	HTTPDelete  HTTPMethod = "delete"
	HTTPPatch   HTTPMethod = "patch"
	HTTPHead    HTTPMethod = "head"
	HTTPOptions HTTPMethod = "options"
)

type Header struct {
	Name  string
	Value string
}

type Request struct {
	Method        HTTPMethod
	URL           string
	Headers       []Header
	Body          []byte
	TimeoutMillis *uint32
}

type Response struct {
	Status  uint16
	Headers []Header
	Body    []byte
}

var (
	GetHook   = func(string) ([]byte, error) { return nil, krate.ErrGeneratedBindingsMissing }
	FetchHook = func(Request) (Response, error) { return Response{}, krate.ErrGeneratedBindingsMissing }
)

func Get(url string) ([]byte, error) {
	return GetHook(url)
}

func GetText(url string) (string, error) {
	body, err := Get(url)
	if err != nil {
		return "", err
	}

	return string(body), nil
}

func Fetch(req Request) (Response, error) {
	return FetchHook(req)
}
