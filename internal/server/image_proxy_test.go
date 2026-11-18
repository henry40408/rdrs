package server

import (
	"encoding/base64"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/imageproxy"
)

const pngMagic = "\x89PNG\r\n\x1a\n"

// signedProxyPath builds a proxy URL signed with the test secret; a non-empty
// referrer is signed in as well.
func signedProxyPath(rawURL, referrer string) string {
	secret := make([]byte, 32)
	query := url.Values{"url": {base64.RawURLEncoding.EncodeToString([]byte(rawURL))}}
	if referrer == "" {
		query.Set("s", imageproxy.SignURL(secret, rawURL))
	} else {
		query.Set("r", base64.RawURLEncoding.EncodeToString([]byte(referrer)))
		query.Set("s", imageproxy.SignURLWithReferrer(secret, rawURL, referrer))
	}
	return "/api/proxy/image?" + query.Encode()
}

func upstream(t *testing.T, handler http.HandlerFunc) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	return server
}

func TestProxyRelaysAnImageWithTheSniffedType(t *testing.T) {
	var gotReferer, gotUserAgent string
	origin := upstream(t, func(w http.ResponseWriter, r *http.Request) {
		gotReferer, gotUserAgent = r.Header.Get("Referer"), r.Header.Get("User-Agent")
		w.Header().Set("Content-Type", "application/octet-stream")
		_, _ = w.Write([]byte(pngMagic + "rest"))
	})
	app := defaultITestApp(t)
	rawURL := origin.URL + "/a"

	response := app.Get(signedProxyPath(rawURL, "https://blog.example/post"))

	if response.Code != http.StatusOK {
		t.Fatalf("status = %d: %s", response.Code, response.Text())
	}
	if got := response.Header().Get("Content-Type"); got != "image/png" {
		t.Errorf("Content-Type = %q, want the sniffed image/png", got)
	}
	signature := imageproxy.SignURLWithReferrer(make([]byte, 32), rawURL, "https://blog.example/post")
	if got := response.Header().Get("ETag"); got != `"`+signature+`"` {
		t.Errorf("ETag = %q", got)
	}
	if !strings.HasPrefix(response.Text(), pngMagic) {
		t.Error("body was not relayed")
	}
	if gotReferer != "https://blog.example/post" || gotUserAgent != "RDRS-Test/1.0" {
		t.Errorf("upstream saw Referer %q, User-Agent %q", gotReferer, gotUserAgent)
	}
}

func TestProxyRefusesUnusableUpstreamResponses(t *testing.T) {
	cases := map[string]struct {
		handler http.HandlerFunc
		status  int
		message string
	}{
		"upstream error status": {func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusNotFound)
		}, http.StatusBadGateway, "HTTP 404"},
		"not an image type": {func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", "text/html")
			_, _ = w.Write([]byte("<html></html>"))
		}, http.StatusBadRequest, "Unsupported image type"},
		"image label on other bytes": {func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", "image/png")
			_, _ = w.Write([]byte("<script>alert(1)</script>"))
		}, http.StatusBadRequest, "Unsupported image type"},
		"declared length over the limit": {func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", "image/png")
			w.Header().Set("Content-Length", "20000000")
		}, http.StatusBadRequest, "Image too large"},
		"streamed body over the limit": {func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("Content-Type", "image/png")
			chunk := []byte(pngMagic + strings.Repeat("x", 1<<20))
			for range 11 {
				_, _ = w.Write(chunk)
				w.(http.Flusher).Flush()
			}
		}, http.StatusBadRequest, "Image too large"},
	}
	for name, c := range cases {
		t.Run(name, func(t *testing.T) {
			origin := upstream(t, c.handler)
			response := defaultITestApp(t).Get(signedProxyPath(origin.URL+"/a", ""))
			if response.Code != c.status || !strings.Contains(response.Text(), c.message) {
				t.Errorf("got %d %s, want %d containing %q", response.Code, response.Text(), c.status, c.message)
			}
		})
	}
}

func TestProxyRefusesPrivateTargetsOutsideTheAllowList(t *testing.T) {
	response := defaultITestApp(t).Get(signedProxyPath("http://10.0.0.1/a.png", ""))
	if response.Code != http.StatusBadRequest || !strings.Contains(response.Text(), "Invalid image URL") {
		t.Errorf("got %d %s", response.Code, response.Text())
	}
}

func TestProxyReportsAnUnreachableOrigin(t *testing.T) {
	origin := upstream(t, func(http.ResponseWriter, *http.Request) {})
	rawURL := origin.URL + "/a"
	origin.Close()
	response := defaultITestApp(t).Get(signedProxyPath(rawURL, ""))
	if response.Code != http.StatusBadGateway {
		t.Errorf("got %d %s, want 502", response.Code, response.Text())
	}
}

func TestSniffImageTypeContainerBrands(t *testing.T) {
	cases := map[string]string{
		"\x00\x00\x00\x18ftypavif": "image/avif",
		"\x00\x00\x00\x18ftypheic": "image/heic",
		"\x00\x00\x00\x18ftypmp42": "",
		"\x00\x00\x00\x18ftyp":     "",
	}
	for body, want := range cases {
		if got := sniffImageType([]byte(body)); got != want {
			t.Errorf("sniffImageType(%q) = %q, want %q", body, got, want)
		}
	}
}
