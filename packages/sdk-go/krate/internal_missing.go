package layer36

import "errors"

// ErrGeneratedBindingsMissing is returned by the draft Go SDK until the
// TinyGo-generated WIT bindings are wired behind the public helpers.
var ErrGeneratedBindingsMissing = errors.New("layer36 Go SDK draft: generated TinyGo bindings are not wired yet")
