package main

import (
	"bytes"
	"context"
	"strings"
	"testing"
)

func TestRunPrintsVersion(t *testing.T) {
	var out, errOut bytes.Buffer
	code := run(context.Background(), nil, func(string) (string, bool) { return "", false }, &out, &errOut)
	if code != 0 || !strings.HasPrefix(out.String(), "rdrs ") {
		t.Fatalf("code=%d out=%q", code, out.String())
	}
}
