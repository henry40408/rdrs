package server

import (
	"context"
	"encoding/base64"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/imageproxy"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/pixel"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/views"
)

// mediaRoutes are the offline page, the offline manifest, the open-tracking
// pixel and the image proxy.
func (a *App) mediaRoutes() []Route {
	return []Route{
		{"GET", "/offline", serveOfflinePage},
		{"GET", "/api/offline/manifest", a.offlineManifest},
		{"GET", "/p/{token}", a.trackingPixel},
		{"GET", "/api/proxy/image", a.proxyImage},
	}
}

// offlineCacheControl is short and not immutable: the URL has no build stamp.
const offlineCacheControl = "public, max-age=3600"

// serveOfflinePage is GET /offline. It must stay user-agnostic: the Cache API
// ignores Cache-Control, so anything personal would reach the next user. The
// explicit public Cache-Control also tells the session layer to attach no
// cookie.
func serveOfflinePage(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Cache-Control", offlineCacheControl)
	writeHTML(w, http.StatusOK, views.Offline(gitVersion()))
}

type offlineEntry struct {
	ID int64 `json:"id"`
	// UpdatedAt is the cache validity token: every write that changes the
	// rendered pane bumps it.
	UpdatedAt string `json:"updated_at"`
}

type offlineManifest struct {
	CacheKey string         `json:"cache_key"`
	Keep     int64          `json:"keep"`
	Entries  []offlineEntry `json:"entries"`
}

// offlineManifest is GET /api/offline/manifest: ids and validity tokens only.
func (a *App) offlineManifest(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	userID := principal.User.ID
	keep, err := models.OfflineKeep(r.Context(), a.DB, userID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	set, err := entry.ListOfflineSet(r.Context(), a.DB, userID, keep)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	manifest := offlineManifest{CacheKey: secret.OfflineID(a.Config.Secret, userID), Keep: keep, Entries: []offlineEntry{}}
	for i := range set {
		manifest.Entries = append(manifest.Entries, offlineEntry{ID: set[i].ID, UpdatedAt: rfc3339Offset(set[i].UpdatedAt)})
	}
	writeJSON(w, http.StatusOK, manifest)
}

// --- GET /p/{token} ---

type pixelToken struct {
	userID, entryID int64
	sig             string
}

// parsePixelToken parses "{user}-{entry}-{sig}.gif"; a malformed token is
// answered like a bad signature.
func parsePixelToken(raw string) (pixelToken, bool) {
	body, ok := strings.CutSuffix(raw, ".gif")
	if !ok {
		return pixelToken{}, false
	}
	parts := strings.Split(body, "-")
	if len(parts) != 3 {
		return pixelToken{}, false
	}
	userID, err := strconv.ParseInt(parts[0], 10, 64)
	if err != nil {
		return pixelToken{}, false
	}
	entryID, err := strconv.ParseInt(parts[1], 10, 64)
	if err != nil {
		return pixelToken{}, false
	}
	return pixelToken{userID: userID, entryID: entryID, sig: parts[2]}, true
}

// writePixel answers every request alike, hit or miss: always 200 so probing
// cannot tell a valid token apart, and no-store so every open reports.
func writePixel(w http.ResponseWriter) {
	w.Header().Set("Content-Type", "image/gif")
	w.Header().Set("Cache-Control", "no-store, max-age=0")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(pixel.TransparentGIF)
}

// trackingPixel is GET /p/{token}. The path HMAC is the whole authority.
func (a *App) trackingPixel(w http.ResponseWriter, r *http.Request) {
	defer writePixel(w)
	token, ok := parsePixelToken(r.PathValue("token"))
	if !ok || !secret.VerifyPixelSig(a.Config.Secret, token.userID, token.entryID, token.sig) {
		return
	}
	// Opt-in and ownership are enforced by the statement, so a stale token
	// after opt-out writes nothing.
	if _, err := models.RecordEntryOpen(r.Context(), a.DB, token.userID, token.entryID); err != nil {
		slog.Warn("failed to record entry open", "event", "entry.record_open_failed", "error", err)
	}
}

// --- GET /api/proxy/image ---

const (
	maxProxiedImageBytes = 10 * 1024 * 1024
	// defaultImageCacheControl is used only when the origin names none.
	defaultImageCacheControl = "public, max-age=86400"
)

// chooseCacheControl mirrors the origin's non-empty Cache-Control, so
// no-store and the like win.
func chooseCacheControl(origin string) string {
	if trimmed := strings.TrimSpace(origin); trimmed != "" {
		return trimmed
	}
	return defaultImageCacheControl
}

func decodeBase64URL(encoded string) (string, bool) {
	raw, err := base64.RawURLEncoding.DecodeString(encoded)
	return string(raw), err == nil
}

// notModified answers a revalidation whose validator is the signature. It runs
// after the signature check, or an unsigned URL could get a 304 echoing
// attacker input.
func notModified(w http.ResponseWriter, r *http.Request, signature string) bool {
	etag := `"` + signature + `"`
	got := r.Header.Get("If-None-Match")
	if got != etag && got != "*" {
		return false
	}
	if strings.ContainsAny(etag, "\r\n") {
		return false
	}
	w.Header().Set("ETag", etag)
	w.Header().Set("Cache-Control", defaultImageCacheControl)
	w.WriteHeader(http.StatusNotModified)
	return true
}

type proxyRequest struct {
	url, referrer, signature string
	hasReferrer              bool
}

// parseProxyRequest decodes and verifies the query, answering its failure.
func (a *App) parseProxyRequest(w http.ResponseWriter, r *http.Request) (proxyRequest, bool) {
	query := r.URL.Query()
	if !query.Has("url") || !query.Has("s") {
		rejection(w, http.StatusBadRequest, "Failed to deserialize query string: missing field `"+missingProxyField(query)+"`")
		return proxyRequest{}, false
	}
	req := proxyRequest{signature: query.Get("s")}
	var ok bool
	if req.url, ok = decodeBase64URL(query.Get("url")); !ok {
		writeError(w, ErrInvalidImageURL)
		return proxyRequest{}, false
	}
	if query.Has("r") {
		req.hasReferrer = true
		if req.referrer, ok = decodeBase64URL(query.Get("r")); !ok {
			writeError(w, ErrInvalidImageURL)
			return proxyRequest{}, false
		}
	}
	return req, true
}

func missingProxyField(query url.Values) string {
	if !query.Has("url") {
		return "url"
	}
	return "s"
}

func (a *App) verifyProxy(req proxyRequest) bool {
	if req.hasReferrer {
		return imageproxy.VerifySignatureWithReferrer(a.Config.Secret, req.url, req.referrer, req.signature)
	}
	return imageproxy.VerifySignature(a.Config.Secret, req.url, req.signature)
}

// proxyImage is GET /api/proxy/image?url=<b64>&s=<sig>[&r=<b64>].
func (a *App) proxyImage(w http.ResponseWriter, r *http.Request) {
	req, ok := a.parseProxyRequest(w, r)
	if !ok {
		return
	}
	if !a.verifyProxy(req) {
		writeError(w, ErrInvalidSignature)
		return
	}
	// Early SSRF refusal; the fetcher re-checks every redirect hop and address.
	target, err := url.Parse(req.url)
	if err != nil || a.Fetcher.Validate(target) != nil {
		writeError(w, ErrInvalidImageURL)
		return
	}
	if notModified(w, r, req.signature) {
		return
	}
	a.serveProxied(w, r, target, req)
}

func (a *App) fetchImage(r *http.Request, target *url.URL, req proxyRequest) (*http.Response, error) {
	return fetch.SendWithRetry(r.Context(), fetch.SendParams{
		Client: a.Fetcher.Client(false),
		Config: a.requestRetry,
		Build: func(ctx context.Context) (*http.Request, error) {
			outgoing, err := http.NewRequestWithContext(ctx, http.MethodGet, target.String(), nil)
			if err != nil {
				return nil, err
			}
			outgoing.Header.Set("User-Agent", a.Config.UserAgent)
			if req.hasReferrer {
				outgoing.Header.Set("Referer", req.referrer)
			}
			return outgoing, nil
		},
	})
}

// serveProxied fetches the image and relays it.
func (a *App) serveProxied(w http.ResponseWriter, r *http.Request, target *url.URL, req proxyRequest) {
	response, err := a.fetchImage(r, target, req)
	if err != nil {
		writeError(w, FetchError("Image fetch failed: "+err.Error()))
		return
	}
	defer func() { _ = response.Body.Close() }()
	if response.StatusCode < 200 || response.StatusCode > 299 {
		writeError(w, FetchError("Image fetch failed: HTTP "+strconv.Itoa(response.StatusCode)))
		return
	}
	contentType := response.Header.Get("Content-Type")
	if contentType == "" {
		contentType = "application/octet-stream"
	}
	if !isValidImageType(contentType) {
		writeError(w, ErrUnsupportedImageType)
		return
	}
	if response.ContentLength > maxProxiedImageBytes {
		writeError(w, ErrImageTooLarge)
		return
	}
	body, err := io.ReadAll(io.LimitReader(response.Body, maxProxiedImageBytes+1))
	if err != nil {
		writeError(w, FetchError("Image fetch failed: "+err.Error()))
		return
	}
	if len(body) > maxProxiedImageBytes {
		writeError(w, ErrImageTooLarge)
		return
	}
	// The bytes decide the type, not the origin's label.
	sniffed := sniffImageType(body)
	if sniffed == "" {
		writeError(w, ErrUnsupportedImageType)
		return
	}
	w.Header().Set("Content-Type", sniffed)
	w.Header().Set("Cache-Control", chooseCacheControl(response.Header.Get("Cache-Control")))
	w.Header().Set("ETag", `"`+req.signature+`"`)
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(body)
}

// isValidImageType accepts application/octet-stream only as unlabelled;
// sniffImageType must then recognise the bytes.
func isValidImageType(contentType string) bool {
	lower := strings.ToLower(contentType)
	return strings.HasPrefix(lower, "image/") || lower == "application/octet-stream"
}

var imageSignatures = []struct{ magic, mime string }{
	{"\x89PNG\r\n\x1a\n", "image/png"},
	{"\xff\xd8\xff", "image/jpeg"},
	{"GIF87a", "image/gif"},
	{"GIF89a", "image/gif"},
	{"BM", "image/bmp"},
	{"\x00\x00\x01\x00", "image/x-icon"},
	{"II*\x00", "image/tiff"},
	{"MM\x00*", "image/tiff"},
}

// sniffImageType is the type the first bytes are, or "" for anything not an
// image. SVG is allowed: nosniff and the CSP stop its scripts.
func sniffImageType(body []byte) string {
	for _, signature := range imageSignatures {
		if strings.HasPrefix(string(body), signature.magic) {
			return signature.mime
		}
	}
	if len(body) >= 12 && string(body[:4]) == "RIFF" && string(body[8:12]) == "WEBP" {
		return "image/webp"
	}
	if len(body) >= 8 && string(body[4:8]) == "ftyp" {
		return sniffISOBrand(body)
	}
	if isSVG(body) {
		return "image/svg+xml"
	}
	return ""
}

// sniffISOBrand reads the brand of an ISO-BMFF ftyp box.
func sniffISOBrand(body []byte) string {
	if len(body) < 12 {
		return ""
	}
	switch string(body[8:12]) {
	case "avif", "avis":
		return "image/avif"
	case "heic", "heix", "mif1":
		return "image/heic"
	}
	return ""
}

// isSVG looks for the root element in the leading bytes: SVG has no magic.
func isSVG(body []byte) bool {
	const sniffLimit = 1024
	head := body[:min(len(body), sniffLimit)]
	if !utf8.Valid(head) {
		return false
	}
	text := strings.ToLower(strings.TrimLeftFunc(string(head), unicode.IsSpace))
	if strings.HasPrefix(text, "<svg") {
		return true
	}
	prolog := strings.HasPrefix(text, "<?xml") || strings.HasPrefix(text, "<!doctype") || strings.HasPrefix(text, "<!--")
	return prolog && strings.Contains(text, "<svg")
}
