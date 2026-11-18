package server

import (
	"bytes"
	"io/fs"
	"net/http"
	"path"
	"strings"

	"github.com/henry40408/rdrs/internal/version"
	"github.com/henry40408/rdrs/static"
)

// Replaced at serve time so nested ES-module imports get ?v=-stamped URLs; a
// bare URL would be cached forever under immutable.
const assetVersionPlaceholder = "__RDRS_ASSET_VERSION__"

// Replaced in the service worker with whether it may cache /static/.
const staticCachePlaceholder = "__RDRS_CACHE_STATIC__"

const (
	immutableCacheControl   = "public, max-age=31536000, immutable"
	unversionedCacheControl = "public, max-age=3600"
	// serviceWorkerPath is served from the root so its scope covers the site.
	serviceWorkerPath = "js/sw.js"
)

// textAssets get the version placeholder substituted. manifest.webmanifest is
// under /static/ so the session and CSRF layers skip it and the response is
// cookie-free; its start_url and scope are therefore absolute.
var textAssets = []string{
	"css/app.css",
	"manifest.webmanifest",
	"js/app.js",
	"js/behaviors.js",
	"js/admin.js",
	"js/keyboard.js",
	"js/bulk-read.js",
	"js/entries-search.js",
	"js/live-updates.js",
	"js/summary.js",
	"js/csrf.js",
	"js/client-info.js",
	"js/login.js",
	"js/passkey.js",
	"js/offline.js",
	"js/pwa.js",
	"js/setup.js",
	"js/search.js",
	"js/statistics.js",
	"js/summarizer.js",
	"js/utils.js",
	"js/components/rdrs-flash.js",
	"js/components/rdrs-kb-help.js",
	"js/components/rdrs-sidebar.js",
}

// binaryAssets are served byte for byte. The icons here are the manifest
// icons; the favicons are served from the root.
var binaryAssets = []string{
	"fonts/newsreader-latin.woff2",
	"fonts/newsreader-italic-latin.woff2",
	"fonts/archivo-latin.woff2",
	"fonts/ibm-plex-mono-400-latin.woff2",
	"fonts/ibm-plex-mono-500-latin.woff2",
	"fonts/ibm-plex-mono-600-latin.woff2",
	"fonts/noto-sans-symbols2-stars.woff2",
	"icons/icon-192.png",
	"icons/icon-512.png",
	"icons/maskable-icon-512.png",
}

// assetCacheControl is no-cache for a -dirty build: its version is identical
// across dev edits, so immutable would serve stale assets.
func assetCacheControl() string {
	if version.IsDirty() {
		return "no-cache"
	}
	return immutableCacheControl
}

func contentTypeFor(name string) string {
	switch path.Ext(name) {
	case ".css":
		return "text/css; charset=utf-8"
	case ".woff2":
		return "font/woff2"
	case ".png":
		return "image/png"
	case ".webmanifest":
		// Under nosniff, a mislabelled manifest is rejected outright.
		return "application/manifest+json"
	default:
		return "application/javascript"
	}
}

func isTextAsset(name string) bool {
	for _, candidate := range textAssets {
		if candidate == name {
			return true
		}
	}
	return false
}

func isBinaryAsset(name string) bool {
	for _, candidate := range binaryAssets {
		if candidate == name {
			return true
		}
	}
	return false
}

func readAsset(name string) []byte {
	data, err := fs.ReadFile(static.FS, name)
	if err != nil {
		// The allow-lists are checked against the embed by a test.
		panic("embedded asset missing: " + name)
	}
	return data
}

func stampVersion(data []byte) []byte {
	return bytes.ReplaceAll(data, []byte(assetVersionPlaceholder), []byte(version.Version()))
}

// serveStatic handles GET /static/{path...}.
func serveStatic(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("path")
	var body []byte
	switch {
	case isBinaryAsset(name):
		body = readAsset(name)
	case isTextAsset(name):
		body = stampVersion(readAsset(name))
	default:
		w.WriteHeader(http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", contentTypeFor(name))
	w.Header().Set("Cache-Control", assetCacheControl())
	_, _ = w.Write(body)
}

// serveServiceWorker handles GET /sw.js. The URL is unstamped (a stamped one
// would register a new worker each deploy), so it is no-cache, never immutable.
func serveServiceWorker(w http.ResponseWriter, _ *http.Request) {
	body := stampVersion(readAsset(serviceWorkerPath))
	// The Cache API ignores Cache-Control, so the worker is told directly.
	mayCache := "false"
	if assetCacheControl() != "no-cache" {
		mayCache = "true"
	}
	body = bytes.ReplaceAll(body, []byte(staticCachePlaceholder), []byte(mayCache))
	w.Header().Set("Content-Type", "application/javascript")
	w.Header().Set("Cache-Control", "no-cache")
	_, _ = w.Write(body)
}

// favicons maps a root path to its embedded file and content type.
var favicons = map[string]struct{ file, contentType string }{
	"/favicon.ico":          {"icons/favicon.ico", "image/x-icon"},
	"/favicon.svg":          {"icons/favicon.svg", "image/svg+xml"},
	"/favicon-16x16.png":    {"icons/favicon-16x16.png", "image/png"},
	"/favicon-32x32.png":    {"icons/favicon-32x32.png", "image/png"},
	"/apple-touch-icon.png": {"icons/apple-touch-icon.png", "image/png"},
}

// serveFavicon serves the icon at the request path. Only ?v=-stamped URLs are
// immutable: bare well-known paths (browser probes, the iOS touch icon) have
// no URL that changes on upgrade.
func serveFavicon(w http.ResponseWriter, r *http.Request) {
	icon, ok := favicons[r.URL.Path]
	if !ok {
		w.WriteHeader(http.StatusNotFound)
		return
	}
	cacheControl := unversionedCacheControl
	if hasVersionParam(r.URL.RawQuery) {
		cacheControl = assetCacheControl()
	}
	w.Header().Set("Content-Type", icon.contentType)
	w.Header().Set("Cache-Control", cacheControl)
	_, _ = w.Write(readAsset(icon.file))
}

// hasVersionParam matches a parameter named exactly v, not one ending in v.
func hasVersionParam(rawQuery string) bool {
	for _, param := range strings.Split(rawQuery, "&") {
		if strings.HasPrefix(param, "v=") {
			return true
		}
	}
	return false
}
