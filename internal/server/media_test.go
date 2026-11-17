package server

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/imageproxy"
	"github.com/henry40408/rdrs/internal/pixel"
)

func TestParsePixelToken(t *testing.T) {
	token, ok := parsePixelToken("7-42-abcdef.gif")
	if !ok || token.userID != 7 || token.entryID != 42 || token.sig != "abcdef" {
		t.Fatalf("token = %+v, %v", token, ok)
	}
	for _, raw := range []string{"7-42-abcdef", "7-42.gif", "7-42-ab-cd.gif", "x-42-abcdef.gif", "7-x-abcdef.gif", "-7-42-abcdef.gif", ".gif", ""} {
		if _, ok := parsePixelToken(raw); ok {
			t.Errorf("%q should not parse", raw)
		}
	}
}

func TestPixelAnswersEveryTokenAlike(t *testing.T) {
	f := newAuthFixture(t)
	for _, token := range []string{"garbage", "1-1-00.gif", "1-1-" + strings.Repeat("0", 32) + ".gif"} {
		rec := f.do(httptest.NewRequest(http.MethodGet, "/p/"+token, nil))
		if rec.Code != http.StatusOK || rec.Header().Get("Content-Type") != "image/gif" ||
			rec.Header().Get("Cache-Control") != "no-store, max-age=0" || rec.Body.String() != string(pixel.TransparentGIF) {
			t.Errorf("%s: %d %v", token, rec.Code, rec.Header())
		}
		if rec.Header().Get("Set-Cookie") != "" {
			t.Errorf("%s: a pixel must not set a cookie", token)
		}
	}
}

func TestOfflinePageIsPubliclyCached(t *testing.T) {
	f := newAuthFixture(t)
	rec := f.do(httptest.NewRequest(http.MethodGet, "/offline", nil))
	if rec.Code != http.StatusOK || rec.Header().Get("Cache-Control") != "public, max-age=3600" {
		t.Errorf("%d %v", rec.Code, rec.Header())
	}
}

func TestOfflineManifestListsNothingWhenOff(t *testing.T) {
	f := newSignedIn(t)
	rec := f.get("/api/offline/manifest")
	var manifest offlineManifest
	if err := json.Unmarshal(rec.Body.Bytes(), &manifest); err != nil {
		t.Fatal(err)
	}
	if rec.Code != http.StatusOK || manifest.CacheKey == "" || manifest.Keep != 0 || manifest.Entries == nil || len(manifest.Entries) != 0 {
		t.Errorf("%d %s", rec.Code, rec.Body)
	}
	if anonymous := f.do(httptest.NewRequest(http.MethodGet, "/api/offline/manifest", nil)); anonymous.Code != http.StatusUnauthorized {
		t.Errorf("anonymous = %d", anonymous.Code)
	}
}

func TestUnknownPathIsA404PageOnlyWhenSignedIn(t *testing.T) {
	f := newSignedIn(t)
	rec := f.get("/no/such/page")
	if rec.Code != http.StatusNotFound || !strings.Contains(rec.Body.String(), "Page not found") {
		t.Errorf("signed in: %d", rec.Code)
	}
	anonymous := f.do(httptest.NewRequest(http.MethodGet, "/no/such/page", nil))
	if anonymous.Code != http.StatusSeeOther {
		t.Errorf("anonymous = %d", anonymous.Code)
	}
}

func proxyTarget(f *authFixture, rawURL string) string {
	query := url.Values{
		"url": {base64.RawURLEncoding.EncodeToString([]byte(rawURL))},
		"s":   {imageproxy.SignURL(f.secret, rawURL)},
	}
	return "/api/proxy/image?" + query.Encode()
}

func TestProxyRefusesBadRequestsBeforeFetching(t *testing.T) {
	f := newAuthFixture(t)
	good := proxyTarget(f, "https://example.com/a.png")
	cases := map[string]struct {
		target string
		status int
	}{
		"missing s":         {"/api/proxy/image?url=aa", http.StatusBadRequest},
		"bad base64":        {"/api/proxy/image?url=!!!&s=x", http.StatusBadRequest},
		"bad signature":     {strings.Replace(good, "s=", "s=00", 1), http.StatusBadRequest},
		"bad referrer":      {good + "&r=!!!", http.StatusBadRequest},
		"referrer unsigned": {good + "&r=" + base64.RawURLEncoding.EncodeToString([]byte("https://x")), http.StatusBadRequest},
	}
	for name, c := range cases {
		if rec := f.do(httptest.NewRequest(http.MethodGet, c.target, nil)); rec.Code != c.status {
			t.Errorf("%s: %d %s", name, rec.Code, rec.Body)
		}
	}
}

func TestProxyRevalidatesBySignature(t *testing.T) {
	f := newAuthFixture(t)
	rawURL := "https://example.com/a.png"
	signature := imageproxy.SignURL(f.secret, rawURL)
	request := httptest.NewRequest(http.MethodGet, proxyTarget(f, rawURL), nil)
	request.Header.Set("If-None-Match", `"`+signature+`"`)
	rec := f.do(request)
	if rec.Code != http.StatusNotModified || rec.Header().Get("Etag") != `"`+signature+`"` {
		t.Errorf("%d %v", rec.Code, rec.Header())
	}
}

func TestSniffImageType(t *testing.T) {
	cases := map[string]string{
		"\x89PNG\r\n\x1a\nrest":              "image/png",
		"\xff\xd8\xffrest":                   "image/jpeg",
		"GIF89a..":                           "image/gif",
		"RIFFxxxxWEBPrest":                   "image/webp",
		"RIFFxxxxWAVErest":                   "",
		"\x00\x00\x00\x18ftypavif":           "image/avif",
		"\x00\x00\x00\x18ftypheic":           "image/heic",
		"\x00\x00\x00\x18ftypmp42":           "",
		"  <svg xmlns=''/>":                  "image/svg+xml",
		"<?xml version='1.0'?><SVG></SVG>":   "image/svg+xml",
		"<?xml version='1.0'?><html></html>": "",
		"<html><svg></svg></html>":           "",
		"\xff\xfe<svg>":                      "",
		"":                                   "",
	}
	for body, want := range cases {
		if got := sniffImageType([]byte(body)); got != want {
			t.Errorf("sniff(%q) = %q, want %q", body, got, want)
		}
	}
}

func TestChooseCacheControl(t *testing.T) {
	if chooseCacheControl("  no-store ") != "no-store" || chooseCacheControl(" ") != defaultImageCacheControl {
		t.Error("the origin's directive wins; blank falls back")
	}
}
