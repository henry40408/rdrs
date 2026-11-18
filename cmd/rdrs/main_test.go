package main

import (
	"bytes"
	"context"
	"net"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"testing"
	"time"
)

func runWith(t *testing.T, args []string, env map[string]string) (code int, stdout, stderr string) {
	t.Helper()
	var out, errOut bytes.Buffer
	code = run(context.Background(), func() {}, args, envOf(env), &out, &errOut)
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

// lockedBuffer lets a test read what run writes while it is still running.
type lockedBuffer struct {
	mu  sync.Mutex
	buf bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.Write(p)
}

func (b *lockedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buf.String()
}

// isolatedEnv keeps a started server away from the working directory and the
// real home: the database lives in the test's temp directory.
func isolatedEnv(t *testing.T) map[string]string {
	t.Helper()
	dir := t.TempDir()
	t.Setenv("HOME", dir)
	return map[string]string{
		"DATABASE_URL":     filepath.Join(dir, "rdrs.sqlite3"),
		"RDRS_SERVER_BIND": "127.0.0.1:0",
	}
}

// runServer starts run, waits for the "starting server" line, stops it with a
// context cancel (the shutdown path) and returns the exit code and stderr.
func runServer(t *testing.T, env map[string]string) (int, string) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var stdout, stderr lockedBuffer
	done := make(chan int, 1)
	go func() { done <- run(ctx, func() {}, nil, envOf(env), &stdout, &stderr) }()
	deadline := time.After(20 * time.Second)
	for !strings.Contains(stderr.String(), "server.starting") {
		select {
		case code := <-done:
			return code, stderr.String()
		case <-deadline:
			t.Fatalf("server did not start: %s", stderr.String())
		case <-time.After(10 * time.Millisecond):
		}
	}
	cancel()
	select {
	case code := <-done:
		if stdout.String() != "" {
			t.Fatalf("stdout=%q", stdout.String())
		}
		return code, stderr.String()
	case <-time.After(40 * time.Second):
		t.Fatalf("shutdown hung: %s", stderr.String())
	}
	return -1, ""
}

func TestRunServesThenShutsDownGracefully(t *testing.T) {
	code, stderr := runServer(t, isolatedEnv(t))
	if code != 0 {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
	for _, want := range []string{"ephemeral_secret", "config.warning", "shutdown.tasks_completed", "shutdown.complete"} {
		if !strings.Contains(stderr, want) {
			t.Errorf("stderr lacks %q: %s", want, stderr)
		}
	}
}

func TestRunPrintsSetupTokenOnStderrWhenNoAccountExists(t *testing.T) {
	_, stderr := runServer(t, isolatedEnv(t))
	if !regexp.MustCompile(`(?m)^setup_token=[A-Z0-9]{5}-[A-Z0-9]{5}$`).MatchString(stderr) {
		t.Fatalf("no setup token line: %s", stderr)
	}
}

func TestRunReleasesTheDatabaseWithoutSidecarFiles(t *testing.T) {
	env := isolatedEnv(t)
	if code, stderr := runServer(t, env); code != 0 {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
	for _, suffix := range []string{"-wal", "-shm"} {
		if info, err := os.Stat(env["DATABASE_URL"] + suffix); err == nil && info.Size() > 0 {
			t.Errorf("%s left behind with %d bytes", suffix, info.Size())
		}
	}
}

func TestRunRefusesAnInvalidWebAuthnRelyingParty(t *testing.T) {
	env := isolatedEnv(t)
	env["RDRS_WEBAUTHN_RP_ID"] = "example.com"
	env["RDRS_WEBAUTHN_RP_ORIGIN"] = "not a url"
	code, stderr := runServer(t, env)
	if code != 1 || !strings.Contains(stderr, "WebAuthn") {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
}

func TestRunRefusesARelyingPartyIDOutsideTheOriginHost(t *testing.T) {
	env := isolatedEnv(t)
	env["RDRS_WEBAUTHN_RP_ID"] = "example.org"
	env["RDRS_WEBAUTHN_RP_ORIGIN"] = "https://rss.example.com"
	code, stderr := runServer(t, env)
	if code != 1 || !strings.Contains(stderr, "RDRS_WEBAUTHN_RP_ID") {
		t.Fatalf("code=%d stderr=%q", code, stderr)
	}
}

func TestRunFailsWhenTheAddressIsTaken(t *testing.T) {
	taken, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer taken.Close()
	env := isolatedEnv(t)
	env["RDRS_SERVER_BIND"] = taken.Addr().String()
	code, stderr := runServer(t, env)
	if code != 1 || !strings.Contains(stderr, "bind") {
		t.Fatalf("code=%d stderr=%q", code, stderr)
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
