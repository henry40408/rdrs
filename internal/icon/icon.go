// Package icon finds the image that represents a feed. Every failure is
// cosmetic, so a candidate that cannot be used is logged at debug level and
// skipped.
package icon

import (
	"context"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strings"

	"github.com/henry40408/rdrs/internal/fetch"
)

// MaxSize is the largest icon kept, in bytes.
const MaxSize = 256 * 1024

// Image is an icon ready to store.
type Image struct {
	Data        []byte
	ContentType string
	SourceURL   string
}

// Candidates are where a feed's icon may be found, tried in this order:
// IconURL, LogoURL, then the site's own favicon.
type Candidates struct {
	IconURL *string
	LogoURL *string
	SiteURL *string
}

// Fetcher downloads icons through the SSRF-guarded client.
type Fetcher struct {
	Fetcher   *fetch.Fetcher
	Retry     fetch.RetryConfig
	UserAgent string
}

// Fetch returns the first usable icon, or nil when there is none.
func (f Fetcher) Fetch(ctx context.Context, c Candidates) *Image {
	if c.IconURL != nil {
		if img := f.image(ctx, *c.IconURL, "icon_url"); img != nil {
			return img
		}
	}
	if c.LogoURL != nil {
		if img := f.image(ctx, *c.LogoURL, "logo_url"); img != nil {
			return img
		}
	}
	if c.SiteURL != nil {
		return f.favicon(ctx, *c.SiteURL)
	}
	return nil
}

// rejected logs why an image was dropped. detail is one extra field, or nil.
func rejected(reason, raw string, detail ...slog.Attr) {
	slog.Debug("image rejected", "event", "icon.rejected", "reason", reason, "url", raw, "detail", slog.GroupValue(detail...))
}

// get sends one guarded GET bounded by the icon timeout; the caller closes the
// body and calls cancel.
func (f Fetcher) get(ctx context.Context, raw string) (*http.Response, context.CancelFunc, error) {
	ctx, cancel := context.WithTimeout(ctx, fetch.IconTimeout)
	resp, err := fetch.SendWithRetry(ctx, fetch.SendParams{
		Client: f.Fetcher.Client(false),
		Config: f.Retry,
		Build: func(ctx context.Context) (*http.Request, error) {
			req, err := http.NewRequestWithContext(ctx, http.MethodGet, raw, nil)
			if err != nil {
				return nil, err
			}
			req.Header.Set("User-Agent", f.UserAgent)
			return req, nil
		},
	})
	if err != nil {
		cancel()
		return nil, nil, err
	}
	return resp, cancel, nil
}

func (f Fetcher) image(ctx context.Context, raw, source string) *Image {
	img := f.download(ctx, raw)
	if img != nil {
		slog.Debug("fetched feed icon", "event", "icon.fetched", "source", source, "url", raw)
	}
	return img
}

// download validates the URL, then keeps the body only when it is a non-empty
// image within the size limit.
func (f Fetcher) download(ctx context.Context, raw string) *Image {
	parsed, err := url.Parse(raw)
	if err != nil {
		rejected("unparsable", raw)
		return nil
	}
	if err := f.Fetcher.Validate(parsed); err != nil {
		rejected("blocked_url", raw, slog.Any("error", err))
		return nil
	}
	resp, cancel, err := f.get(ctx, raw)
	if err != nil {
		slog.Debug("failed to fetch image", "event", "icon.fetch_failed", "url", raw, slog.Any("error", err))
		return nil
	}
	defer cancel()
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		rejected("status", raw, slog.Int("status", resp.StatusCode))
		return nil
	}
	contentType := mediaType(resp.Header.Get("Content-Type"))
	if !strings.HasPrefix(contentType, "image/") {
		rejected("content_type", raw, slog.String("content_type", contentType))
		return nil
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, MaxSize+1))
	if err != nil {
		slog.Debug("failed to read image body", "event", "icon.fetch_failed", "url", raw, slog.Any("error", err))
		return nil
	}
	if len(data) > MaxSize {
		rejected("too_large", raw)
		return nil
	}
	if len(data) == 0 {
		rejected("empty", raw)
		return nil
	}
	return &Image{Data: data, ContentType: contentType, SourceURL: raw}
}

func mediaType(header string) string {
	before, _, _ := strings.Cut(header, ";")
	return strings.TrimSpace(before)
}

// favicon tries /favicon.ico, then the icon the site's HTML links to.
func (f Fetcher) favicon(ctx context.Context, siteURL string) *Image {
	base, err := url.Parse(siteURL)
	if err != nil {
		return nil
	}
	if err := f.Fetcher.Validate(base); err != nil {
		rejected("blocked_url", siteURL, slog.Any("error", err))
		return nil
	}
	ico, err := base.Parse("/favicon.ico")
	if err != nil {
		return nil
	}
	if img := f.image(ctx, ico.String(), "favicon_ico"); img != nil {
		return img
	}
	page := f.page(ctx, siteURL)
	if page == "" {
		return nil
	}
	linked := extractFavicon(page, base)
	if linked == "" {
		return nil
	}
	return f.image(ctx, linked, "html_link")
}

// page returns the site's HTML, or "" when it cannot be had.
func (f Fetcher) page(ctx context.Context, siteURL string) string {
	resp, cancel, err := f.get(ctx, siteURL)
	if err != nil {
		return ""
	}
	defer cancel()
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return ""
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return ""
	}
	return string(body)
}
