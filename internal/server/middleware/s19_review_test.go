package middleware

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/testutil"
)

func TestForwardAuthRefusesAZonedLinkLocalAndAnUnknownPeer(t *testing.T) {
	cfg := forwardConfig(t)
	networks, err := config.ParseTrustedNetworks("127.0.0.1/32")
	if err != nil {
		t.Fatal(err)
	}
	cfg.TrustedProxyNetworks = networks
	for _, remote := range []string{"", "@", "[fe80::1%en0]:5555"} {
		database := testutil.SQLite(t)
		request := forwardRequest("/", "mallory")
		request.RemoteAddr = remote
		rec := serveReq(forwardHandler(t, cfg, database), request)
		if rec.Code != http.StatusOK || rec.Header().Get("Set-Cookie") != "" {
			t.Errorf("peer %q: %d %v", remote, rec.Code, rec.Header())
		}
	}
}

func TestForwardAuthTrustsALinkLocalPeerInsideATrustedNetwork(t *testing.T) {
	cfg := forwardConfig(t)
	networks, err := config.ParseTrustedNetworks("fe80::/10")
	if err != nil {
		t.Fatal(err)
	}
	cfg.TrustedProxyNetworks = networks
	request := forwardRequest("/", "alice")
	request.RemoteAddr = "[fe80::1%en0]:5555"
	rec := serveReq(forwardHandler(t, cfg, testutil.SQLite(t)), request)
	if rec.Code != http.StatusSeeOther {
		t.Errorf("status = %d", rec.Code)
	}
}

func TestForwardAuthRedirectNeverBecomesSchemeRelative(t *testing.T) {
	database := testutil.SQLite(t)
	request := forwardRequest("/", "alice")
	request.URL.Path = "//evil.example/x"
	request.RequestURI = "//evil.example/x"
	rec := serveReq(forwardHandler(t, forwardConfig(t), database), request)
	if got := rec.Header().Get("Location"); got != "/evil.example/x" {
		t.Errorf("Location = %q", got)
	}
}

func TestCSRFExemptionComparesTheEscapedPath(t *testing.T) {
	cookies := auth.Cookies{Secret: rootSecret}
	request := httptest.NewRequest(http.MethodPost, "/invite/x%2Faccounts%2FClientLogin", nil)
	request.AddCookie(cookies.Session("token"))
	rec := serveReq(CSRFGuard(cookies)(http.HandlerFunc(okHandler)), request)
	if rec.Code != http.StatusForbidden {
		t.Errorf("status = %d, want 403", rec.Code)
	}
}

func TestTimeoutKeepsHeadersOfAHandlerThatWritesNothing(t *testing.T) {
	quiet := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		http.SetCookie(w, &http.Cookie{Name: "a", Value: "b"})
		w.Header().Set("X-Marker", "kept")
	})
	rec := serve(Timeout(time.Second)(quiet), "GET", "/")
	if rec.Code != http.StatusOK || rec.Header().Get("X-Marker") != "kept" || rec.Header().Get("Set-Cookie") == "" {
		t.Errorf("got %d %v", rec.Code, rec.Header())
	}
}
