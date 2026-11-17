package main

import (
	"strings"
	"testing"
)

func envOf(vars map[string]string) func(string) (string, bool) {
	return func(key string) (string, bool) {
		v, ok := vars[key]
		return v, ok
	}
}

func TestNoLegacyVarsPasses(t *testing.T) {
	if err := ensureNoLegacyEnvVars(envOf(map[string]string{"DATABASE_URL": "x", "RDRS_SECRET": "y"})); err != nil {
		t.Fatal(err)
	}
}

func TestRustLogIsRetiredEvenWhenEmpty(t *testing.T) {
	for _, value := range []string{"", "debug", "error,rdrs=info"} {
		err := ensureNoLegacyEnvVars(envOf(map[string]string{"RUST_LOG": value}))
		if err == nil || !strings.Contains(err.Error(), "  RUST_LOG -> RDRS_LOG_LEVEL") {
			t.Fatalf("RUST_LOG=%q: %v", value, err)
		}
	}
}

func TestLegacyVarsAreAllReportedAtOnce(t *testing.T) {
	err := ensureNoLegacyEnvVars(envOf(map[string]string{
		"IMAGE_PROXY_SECRET": strings.Repeat("x", 32),
		"AUTH_PROXY_HEADER":  "Remote-User",
		"SERVER_BIND":        "0.0.0.0:8080",
	}))
	if err == nil {
		t.Fatal("want error")
	}
	for _, want := range []string{
		"these environment variables no longer exist; rename (or unset) them to continue:\n",
		"  IMAGE_PROXY_SECRET -> RDRS_SECRET",
		"  AUTH_PROXY_HEADER -> RDRS_AUTH_PROXY_HEADER",
		"  SERVER_BIND -> RDRS_SERVER_BIND",
	} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("missing %q in %v", want, err)
		}
	}
}

func TestRetiredSignupFlagsPointAtAdmin(t *testing.T) {
	for _, name := range []string{"RDRS_SIGNUP_ENABLED", "SIGNUP_ENABLED"} {
		err := ensureNoLegacyEnvVars(envOf(map[string]string{name: "true"}))
		if err == nil || !strings.Contains(err.Error(), "  "+name+" -> ") || !strings.Contains(err.Error(), "/admin") {
			t.Fatalf("%s: %v", name, err)
		}
	}
}

func TestDatabaseURLKeepsItsBareName(t *testing.T) {
	if err := ensureNoLegacyEnvVars(envOf(map[string]string{"DATABASE_URL": "postgres://u:p@db/rdrs"})); err != nil {
		t.Fatal(err)
	}
}
