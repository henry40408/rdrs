package server

import (
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/testutil"
	"log/slog"
)

func setupWith(t *testing.T, options Options) int {
	t.Helper()
	t.Setenv("RDRS_FAST_HASH", "1")
	key := []byte("01234567890123456789012345678901")
	options.Config = config.Config{Secret: key, MultiUser: true}
	options.DB = testutil.SQLite(t)
	options.Audit = audit.New(slog.New(slog.DiscardHandler), key)
	options.Limiter = auth.NewRateLimiter(0, 60)
	rec := httptest.NewRecorder()
	NewApp(options).Handler().ServeHTTP(rec,
		jsonRequest("POST", "/api/setup", `{"username":"root","password":"`+testPassword+`"}`))
	return rec.Code
}

func TestSetupWithoutAConfiguredTokenIsRefused(t *testing.T) {
	if got := setupWith(t, Options{}); got != http.StatusForbidden {
		t.Errorf("setup with an empty token = %d, want 403", got)
	}
}

func TestSetupWithAConfiguredTokenStillNeedsIt(t *testing.T) {
	if got := setupWith(t, Options{SetupToken: "CCCCC-CCCCC"}); got != http.StatusForbidden {
		t.Errorf("setup without the token = %d, want 403", got)
	}
}

func TestReauthChecksTheSessionBeforeTheContentType(t *testing.T) {
	f := newAuthFixture(t)
	request := httptest.NewRequest("POST", "/api/session/reauth", strings.NewReader("x"))
	if got := f.do(request).Code; got != http.StatusUnauthorized {
		t.Errorf("anonymous reauth = %d, want 401", got)
	}
}

type brokenBody struct{}

func (brokenBody) Read([]byte) (int, error) { return 0, errors.New("connection reset") }

func TestBodyReadFailureIsABadRequestNotTooLarge(t *testing.T) {
	request := httptest.NewRequest("POST", "/x", brokenBody{})
	request.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	var out map[string]any
	if decodeJSON(rec, request, &out) || rec.Code != http.StatusBadRequest {
		t.Errorf("status = %d, want 400", rec.Code)
	}
	big := httptest.NewRequest("POST", "/x", strings.NewReader(strings.Repeat("a", maxBodyBytes+1)))
	big.Header.Set("Content-Type", "application/json")
	rec = httptest.NewRecorder()
	if decodeJSON(rec, big, &out) || rec.Code != http.StatusRequestEntityTooLarge {
		t.Errorf("status = %d, want 413", rec.Code)
	}
}
