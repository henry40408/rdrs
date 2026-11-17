package main

import (
	"bytes"
	"context"
	"strings"
	"testing"
)

func runWith(t *testing.T, args []string, env map[string]string) (code int, stdout, stderr string) {
	t.Helper()
	var out, errOut bytes.Buffer
	code = run(context.Background(), args, envOf(env), &out, &errOut)
	return code, out.String(), errOut.String()
}

func TestRunRefusesRetiredRustLog(t *testing.T) {
	code, _, stderr := runWith(t, nil, map[string]string{"RUST_LOG": ""})
	if code != 1 || !strings.Contains(stderr, "  RUST_LOG -> RDRS_LOG_LEVEL") {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
}

func TestRunRejectsBadLogLevel(t *testing.T) {
	code, _, stderr := runWith(t, nil, map[string]string{"RDRS_LOG_LEVEL": "trace"})
	if code == 0 || !strings.Contains(stderr, "RDRS_LOG_LEVEL") {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
}

func TestRunReportsConfigurationErrors(t *testing.T) {
	code, _, stderr := runWith(t, nil, map[string]string{"RDRS_SERVER_BIND": "nope"})
	if code != 1 || !strings.Contains(stderr, "Configuration error: invalid RDRS_SERVER_BIND") {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
	code, _, stderr = runWith(t, nil, map[string]string{"RDRS_DISABLE_LOCAL_AUTH": "true"})
	if code != 1 || !strings.Contains(stderr, "RDRS_DISABLE_LOCAL_AUTH") {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
}

func TestRunWarnsAboutEphemeralSecretOnStderrOnly(t *testing.T) {
	code, stdout, stderr := runWith(t, nil, nil)
	if code != 0 || stdout != "" {
		t.Fatalf("code=%d stdout=%q", code, stdout)
	}
	if !strings.Contains(stderr, "ephemeral_secret") || !strings.Contains(stderr, "config.warning") {
		t.Fatalf("stderr=%q", stderr)
	}
}

func TestRunHelpExitsCleanly(t *testing.T) {
	if code, _, _ := runWith(t, []string{"-h"}, nil); code != 0 {
		t.Fatalf("code=%d", code)
	}
}

func TestRunRejectsUnknownFlag(t *testing.T) {
	if code, _, _ := runWith(t, []string{"--nope"}, nil); code != 2 {
		t.Fatalf("code=%d", code)
	}
}
