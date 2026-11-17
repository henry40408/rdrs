package server

import (
	"encoding/json"
	"errors"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"os"
	"regexp"
	"sort"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/version"
	"github.com/henry40408/rdrs/static"
)

func get(t *testing.T, target string) *httptest.ResponseRecorder {
	t.Helper()
	return do(t, http.MethodGet, target)
}

func do(t *testing.T, method, target string) *httptest.ResponseRecorder {
	t.Helper()
	rec := httptest.NewRecorder()
	Handler().ServeHTTP(rec, httptest.NewRequest(method, target, nil))
	return rec
}

func TestRegisteredRoutesAreAllInTheRustRouteList(t *testing.T) {
	data, err := os.ReadFile("testdata/routes.txt")
	if err != nil {
		t.Fatal(err)
	}
	known := map[string]bool{}
	for _, line := range strings.Split(strings.TrimSpace(string(data)), "\n") {
		known[line] = true
	}
	for _, route := range routes() {
		if !known[route.String()] {
			t.Errorf("%s is registered but is not in testdata/routes.txt", route)
		}
	}
}

// allRoutesPending is removed by the step that mounts the last handler; from
// then on a route missing from the registry fails this test.
const allRoutesPending = true

func TestEveryRustRouteIsRegistered(t *testing.T) {
	data, err := os.ReadFile("testdata/routes.txt")
	if err != nil {
		t.Fatal(err)
	}
	registered := map[string]bool{}
	for _, route := range routes() {
		registered[route.String()] = true
	}
	var missing []string
	for _, line := range strings.Split(strings.TrimSpace(string(data)), "\n") {
		if !registered[line] {
			missing = append(missing, line)
		}
	}
	sort.Strings(missing)
	if len(missing) > 0 && allRoutesPending {
		t.Skipf("%d routes not ported yet", len(missing))
	}
	if len(missing) > 0 {
		t.Errorf("routes missing from the registry:\n%s", strings.Join(missing, "\n"))
	}
}

func TestPatternConvertsCatchAllSegments(t *testing.T) {
	route := Route{Method: "GET", Path: "/a/{id}/b/{*rest}"}
	if got := route.pattern(); got != "GET /a/{id}/b/{rest...}" {
		t.Errorf("pattern = %q", got)
	}
}

func TestAnotherMethodOnAKnownPathIs405WithAllow(t *testing.T) {
	rec := do(t, http.MethodPost, "/health")
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("status = %d", rec.Code)
	}
	if !strings.Contains(rec.Header().Get("Allow"), "GET") {
		t.Errorf("Allow = %q", rec.Header().Get("Allow"))
	}
}

func TestHealth(t *testing.T) {
	rec := get(t, "/health")
	want := `{"status":"ok","git_version":"` + version.Version() + `"}`
	if rec.Code != 200 || rec.Body.String() != want {
		t.Errorf("got %d %s, want %s", rec.Code, rec.Body, want)
	}
	if rec.Header().Get("Content-Type") != "application/json" {
		t.Errorf("content type = %q", rec.Header().Get("Content-Type"))
	}
}

func TestEveryAllowListedAssetIsEmbedded(t *testing.T) {
	for _, name := range append(append([]string{serviceWorkerPath}, textAssets...), binaryAssets...) {
		if _, err := fs.Stat(static.FS, name); err != nil {
			t.Errorf("%s: %v", name, err)
		}
	}
	for _, icon := range favicons {
		if _, err := fs.Stat(static.FS, icon.file); err != nil {
			t.Errorf("%s: %v", icon.file, err)
		}
	}
}

func TestStaticContentTypesAndCaching(t *testing.T) {
	cases := map[string]string{
		"/static/css/app.css":                         "text/css; charset=utf-8",
		"/static/js/app.js":                           "application/javascript",
		"/static/fonts/archivo-latin.woff2":           "font/woff2",
		"/static/icons/icon-192.png":                  "image/png",
		"/static/manifest.webmanifest":                "application/manifest+json",
		"/static/js/components/rdrs-flash.js":         "application/javascript",
		"/static/icons/maskable-icon-512.png":         "image/png",
		"/static/fonts/ibm-plex-mono-600-latin.woff2": "font/woff2",
	}
	for target, contentType := range cases {
		rec := get(t, target)
		if rec.Code != 200 {
			t.Errorf("%s: status %d", target, rec.Code)
			continue
		}
		if got := rec.Header().Get("Content-Type"); got != contentType {
			t.Errorf("%s: content type %q, want %q", target, got, contentType)
		}
		if got := rec.Header().Get("Cache-Control"); got != assetCacheControl() {
			t.Errorf("%s: cache control %q", target, got)
		}
	}
}

func TestStaticRefusesNamesOutsideTheAllowList(t *testing.T) {
	for _, target := range []string{
		"/static/js/sw.js",          // root only
		"/static/icons/favicon.ico", // root only
		"/static/fonts/OFL-Archivo.txt",
		"/static/nope.js",
	} {
		if rec := get(t, target); rec.Code != http.StatusNotFound {
			t.Errorf("%s: status %d, want 404", target, rec.Code)
		}
	}
}

func TestJSImportsAreVersionStamped(t *testing.T) {
	stamp := ".js?v=" + version.Version()
	importLine := regexp.MustCompile(`(?m)^\s*(?:import\s+(?:[^'"]*\sfrom\s+)?|\}\s*from\s+)['"]([^'"]+)['"]`)
	checked := 0
	for _, name := range textAssets {
		if !strings.HasSuffix(name, ".js") {
			continue
		}
		body := get(t, "/static/"+name).Body.String()
		if strings.Contains(body, assetVersionPlaceholder) {
			t.Errorf("%s: placeholder not substituted", name)
		}
		for _, match := range importLine.FindAllStringSubmatch(body, -1) {
			checked++
			if specifier := match[1]; strings.Contains(specifier, ".js") && !strings.HasSuffix(specifier, stamp) {
				t.Errorf("%s: import %q lacks the ?v= cache-buster", name, specifier)
			}
		}
	}
	if checked < 5 {
		t.Errorf("only %d imports found; the scanner is broken", checked)
	}
}

func TestServiceWorker(t *testing.T) {
	rec := get(t, "/sw.js")
	if rec.Code != 200 || rec.Header().Get("Cache-Control") != "no-cache" ||
		rec.Header().Get("Content-Type") != "application/javascript" {
		t.Fatalf("status %d headers %v", rec.Code, rec.Header())
	}
	body := rec.Body.String()
	if strings.Contains(body, assetVersionPlaceholder) || strings.Contains(body, staticCachePlaceholder) {
		t.Error("a placeholder was not substituted")
	}
	if !strings.Contains(body, version.Version()) {
		t.Error("worker body must carry the build version")
	}
	wantCache := "'true' === 'true'"
	if assetCacheControl() == "no-cache" {
		wantCache = "'false' === 'true'"
	}
	if !strings.Contains(body, wantCache) {
		t.Errorf("worker lacks %s", wantCache)
	}
	for _, want := range []string{"'/offline'", "'/entries/offline'"} {
		if !strings.Contains(body, want) {
			t.Errorf("worker does not name %s", want)
		}
	}
}

func TestServiceWorkerOnlyNamesServableStaticURLs(t *testing.T) {
	body := string(readAsset(serviceWorkerPath))
	named := 0
	for _, match := range regexp.MustCompile(`/static/([^"'`+"`"+`?\s>]+)`).FindAllStringSubmatch(body, -1) {
		named++
		if name := match[1]; !isTextAsset(name) && !isBinaryAsset(name) {
			t.Errorf("sw.js names /static/%s but nothing serves it", name)
		}
	}
	if named == 0 {
		t.Error("the worker is expected to name at least one static asset")
	}
}

func TestManifestIsValidAndItsIconsAreServable(t *testing.T) {
	var manifest struct {
		StartURL string `json:"start_url"`
		Scope    string `json:"scope"`
		Display  string `json:"display"`
		Icons    []struct {
			Src     string `json:"src"`
			Sizes   string `json:"sizes"`
			Purpose string `json:"purpose"`
		} `json:"icons"`
	}
	if err := json.Unmarshal(get(t, "/static/manifest.webmanifest").Body.Bytes(), &manifest); err != nil {
		t.Fatal(err)
	}
	if manifest.StartURL != "/" || manifest.Scope != "/" || manifest.Display != "standalone" {
		t.Errorf("manifest = %+v", manifest)
	}
	maskable := false
	for _, icon := range manifest.Icons {
		if !strings.Contains(icon.Src, "?v="+version.Version()) {
			t.Errorf("%s must be version-stamped", icon.Src)
		}
		target, _, _ := strings.Cut(icon.Src, "?")
		if get(t, target).Code != 200 {
			t.Errorf("manifest names %s, a 404", target)
		}
		maskable = maskable || icon.Purpose == "maskable"
	}
	if !maskable {
		t.Error("manifest lacks a maskable icon")
	}
}

func TestFavicons(t *testing.T) {
	cases := map[string]string{
		"/favicon.ico":          "image/x-icon",
		"/favicon.svg":          "image/svg+xml",
		"/favicon-16x16.png":    "image/png",
		"/favicon-32x32.png":    "image/png",
		"/apple-touch-icon.png": "image/png",
	}
	for target, contentType := range cases {
		rec := get(t, target)
		if rec.Code != 200 || rec.Header().Get("Content-Type") != contentType {
			t.Errorf("%s: %d %q", target, rec.Code, rec.Header().Get("Content-Type"))
		}
		if got := rec.Header().Get("Cache-Control"); got != unversionedCacheControl {
			t.Errorf("%s: bare request cache control %q", target, got)
		}
	}
}

func TestFaviconVersionParameter(t *testing.T) {
	for query, versioned := range map[string]bool{
		"v=abc": true, "foo=1&v=abc": true, "": false, "size=32": false, "rev=abc": false,
	} {
		if got := hasVersionParam(query); got != versioned {
			t.Errorf("hasVersionParam(%q) = %v", query, got)
		}
	}
	rec := get(t, "/favicon.ico?v=abc")
	if got := rec.Header().Get("Cache-Control"); got != assetCacheControl() {
		t.Errorf("stamped request cache control %q", got)
	}
}

func TestWriteErrorBodiesAndStatuses(t *testing.T) {
	cases := []struct {
		err    error
		status int
		body   string
	}{
		{ErrInvalidCredentials, 401, `{"error":"Invalid credentials"}`},
		{ErrReauthenticationRequired, 403, `{"error":"Reauthentication required"}`},
		{FetchError("a <b> & c"), 502, `{"error":"a <b> & c"}`},
		{ValidationError("bad"), 400, `{"error":"bad"}`},
		{NotFoundError("gone"), 404, `{"error":"gone"}`},
		{PasskeyAuthenticationFailed("no"), 401, `{"error":"no"}`},
		{DatabaseError(errors.New("secret detail")), 500, `{"error":"Database error"}`},
		{errors.New("secret detail"), 500, `{"error":"Internal server error"}`},
	}
	for _, c := range cases {
		rec := httptest.NewRecorder()
		writeError(rec, c.err)
		if rec.Code != c.status || rec.Body.String() != c.body {
			t.Errorf("%v: got %d %s, want %d %s", c.err, rec.Code, rec.Body, c.status, c.body)
		}
	}
}

func TestTooManyRequestsSendsRetryAfter(t *testing.T) {
	rec := httptest.NewRecorder()
	writeError(rec, TooManyRequests(42))
	if rec.Code != 429 || rec.Header().Get("Retry-After") != "42" || rec.Body.String() != `{"error":"Too many requests"}` {
		t.Errorf("got %d %v %s", rec.Code, rec.Header(), rec.Body)
	}
	rec = httptest.NewRecorder()
	writeError(rec, ErrForbidden)
	if rec.Header().Get("Retry-After") != "" {
		t.Error("Retry-After sent without a window")
	}
}
