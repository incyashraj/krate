package main

import (
	"errors"
	"testing"
)

func TestClassifyNetError(t *testing.T) {
	tests := []struct {
		name     string
		err      string
		wantLine string
		wantCode int
	}{
		{
			name:     "permission denied",
			err:      "permission-denied",
			wantLine: "layer36-go-curl: permission denied",
			wantCode: 5,
		},
		{
			name:     "invalid url",
			err:      "invalid-url",
			wantLine: "layer36-go-curl: invalid url",
			wantCode: 20,
		},
		{
			name:     "dns failure",
			err:      "dns-failure: not found",
			wantLine: "layer36-go-curl: dns lookup failed",
			wantCode: 21,
		},
		{
			name:     "fallback",
			err:      "some-unknown-error",
			wantLine: "layer36-go-curl: fetch failed",
			wantCode: 21,
		},
		{
			name:     "uppercase variant token",
			err:      "TIMEOUT",
			wantLine: "layer36-go-curl: request timed out",
			wantCode: 21,
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			gotLine, gotCode := classifyNetError(errors.New(tc.err))
			if gotLine != tc.wantLine {
				t.Fatalf("line mismatch: got %q, want %q", gotLine, tc.wantLine)
			}
			if gotCode != tc.wantCode {
				t.Fatalf("code mismatch: got %d, want %d", gotCode, tc.wantCode)
			}
		})
	}
}
