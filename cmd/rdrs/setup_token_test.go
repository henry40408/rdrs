package main

import (
	"bytes"
	"context"
	"net/http"
	"net/http/httptest"
	"regexp"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/migrate"
)

func openTestDatabase(t *testing.T) *db.DB {
	t.Helper()
	database, err := db.OpenMemory(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(context.Background()) })
	if err := migrate.Run(context.Background(), database); err != nil {
		t.Fatal(err)
	}
	return database
}

// The server refuses setup for an empty token, so main must never hand it one.
func TestAnnounceSetupTokenNeverReturnsAnEmptyToken(t *testing.T) {
	var stderr bytes.Buffer
	token, err := announceSetupToken(context.Background(), openTestDatabase(t), &stderr)
	if err != nil || token == "" {
		t.Fatalf("token=%q err=%v", token, err)
	}
	if !strings.Contains(stderr.String(), "setup_token="+token) {
		t.Fatalf("stderr=%q", stderr.String())
	}
}

func postSetup(t *testing.T, handler http.Handler, token string) int {
	t.Helper()
	body := `{"username":"root","password":"correct horse battery staple","setup_token":"` + token + `"}`
	request := httptest.NewRequest("POST", "/api/setup", strings.NewReader(body))
	request.Header.Set("Content-Type", "application/json")
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, request)
	return recorder.Code
}

// Setup is only open to whoever read the token off stderr: an empty or wrong
// token must fail closed, and the announced one must work.
func TestApplicationRequiresTheAnnouncedSetupToken(t *testing.T) {
	cfg, err := loadConfig(envOf(isolatedEnv(t)))
	if err != nil {
		t.Fatal(err)
	}
	var stderr bytes.Buffer
	app, err := newApplication(context.Background(), cfg, newLogger(loggerSettings{}, &stderr), &stderr)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = app.db.Shutdown(context.Background()) })
	announced := regexp.MustCompile(`setup_token=(\S+)`).FindStringSubmatch(stderr.String())
	if announced == nil {
		t.Fatalf("no token announced: %q", stderr.String())
	}
	for _, wrong := range []string{"", "AAAAA-AAAAA"} {
		if code := postSetup(t, app.handler, wrong); code == http.StatusCreated {
			t.Fatalf("setup accepted token %q", wrong)
		}
	}
	if code := postSetup(t, app.handler, announced[1]); code != http.StatusCreated {
		t.Fatalf("setup with the announced token = %d", code)
	}
}
