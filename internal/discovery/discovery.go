// Package discovery finds the feed behind a URL the user typed: either the URL
// is a feed, or its HTML page links to one.
package discovery

import (
	"context"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	"golang.org/x/net/html"

	"github.com/henry40408/rdrs/internal/feedparse"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/htmldepth"
)

// Feed is what discovery learned about a feed.
type Feed struct {
	FeedURL     string
	Title       *string
	Description *string
	SiteURL     *string
}

// Kind says which stage failed.
type Kind int

// The failure kinds. Their texts are shown to users, so they match Rust.
const (
	KindInvalidURL Kind = iota
	KindFetch
	KindNoFeed
	KindParse
)

// Error is a failed discovery.
type Error struct {
	Kind   Kind
	Detail string
}

func (e *Error) Error() string {
	switch e.Kind {
	case KindInvalidURL:
		return "Invalid URL"
	case KindFetch:
		return "Failed to fetch: " + e.Detail
	case KindNoFeed:
		return "No feed found at URL"
	default:
		return "Failed to parse feed: " + e.Detail
	}
}

// Params are the inputs of Discover.
type Params struct {
	URL       string
	UserAgent string
	Fetcher   *fetch.Fetcher
	Retry     fetch.RetryConfig
}

type page struct {
	contentType string
	body        []byte
}

// Discover resolves the URL to a feed. The SSRF guard runs before any network
// I/O, and again on a feed link taken from a page.
func Discover(ctx context.Context, p Params) (*Feed, error) {
	pageURL, err := p.validURL(p.URL)
	if err != nil {
		return nil, err
	}
	first, err := p.get(ctx, pageURL)
	if err != nil {
		return nil, err
	}
	if isFeedContentType(strings.ToLower(first.contentType)) || looksLikeFeed(first.body) {
		return parseFeed(p.URL, first)
	}
	feedURL, err := findFeedLink(first.body, pageURL)
	if err != nil {
		return nil, err
	}
	linked, err := p.validURL(feedURL)
	if err != nil {
		return nil, err
	}
	second, err := p.get(ctx, linked)
	if err != nil {
		return nil, err
	}
	return parseFeed(feedURL, second)
}

func (p Params) validURL(raw string) (*url.URL, error) {
	u, err := url.Parse(raw)
	if err != nil || p.Fetcher.Validate(u) != nil {
		return nil, &Error{Kind: KindInvalidURL}
	}
	return u, nil
}

func (p Params) get(ctx context.Context, u *url.URL) (*page, error) {
	resp, err := fetch.SendWithRetry(ctx, fetch.SendParams{
		Client: p.Fetcher.Client(false),
		Config: p.Retry,
		Build: func(ctx context.Context) (*http.Request, error) {
			req, err := http.NewRequestWithContext(ctx, http.MethodGet, u.String(), nil)
			if err != nil {
				return nil, err
			}
			req.Header.Set("User-Agent", p.UserAgent)
			return req, nil
		},
	})
	if err != nil {
		return nil, &Error{Kind: KindFetch, Detail: err.Error()}
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return nil, &Error{Kind: KindFetch, Detail: "HTTP " + strconv.Itoa(resp.StatusCode) + " " + http.StatusText(resp.StatusCode)}
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &Error{Kind: KindFetch, Detail: err.Error()}
	}
	return &page{contentType: resp.Header.Get("Content-Type"), body: body}, nil
}

func isFeedContentType(contentType string) bool {
	for _, marker := range []string{"application/rss", "application/atom", "application/xml", "text/xml"} {
		if strings.Contains(contentType, marker) {
			return true
		}
	}
	return false
}

func looksLikeFeed(body []byte) bool {
	trimmed := strings.TrimLeft(string(body), " \t\r\n\f\v")
	for _, prefix := range []string{"<?xml", "<rss", "<feed", "<RDF"} {
		if strings.HasPrefix(trimmed, prefix) {
			return true
		}
	}
	return false
}

var feedLinkTypes = map[string]bool{
	"application/rss+xml":  true,
	"application/atom+xml": true,
	"application/xml":      true,
}

// findFeedLink returns the first alternate link, in document order, that
// points at a feed. A link whose href cannot be resolved is an error, as the
// first match decides.
func findFeedLink(body []byte, base *url.URL) (string, error) {
	doc, err := html.Parse(strings.NewReader(string(body)))
	if htmldepth.IsTooDeep(err) {
		doc, err = html.Parse(strings.NewReader(htmldepth.Flatten(string(body))))
	}
	if err != nil {
		return "", &Error{Kind: KindNoFeed}
	}
	href, found := firstFeedHref(doc)
	if !found {
		return "", &Error{Kind: KindNoFeed}
	}
	resolved, err := base.Parse(strings.TrimSpace(href))
	if err != nil {
		return "", &Error{Kind: KindInvalidURL}
	}
	return resolved.String(), nil
}

func firstFeedHref(n *html.Node) (string, bool) {
	if n.Type == html.ElementNode && n.Data == "link" {
		if href, ok := feedLinkHref(n); ok {
			return href, true
		}
	}
	for c := n.FirstChild; c != nil; c = c.NextSibling {
		if href, ok := firstFeedHref(c); ok {
			return href, true
		}
	}
	return "", false
}

func feedLinkHref(n *html.Node) (string, bool) {
	var rel, kind, href string
	var hasHref bool
	for _, a := range n.Attr {
		switch a.Key {
		case "rel":
			rel = a.Val
		case "type":
			kind = a.Val
		case "href":
			href, hasHref = a.Val, true
		}
	}
	if rel != "alternate" || !feedLinkTypes[kind] || !hasHref {
		return "", false
	}
	return href, true
}

func parseFeed(feedURL string, p *page) (*Feed, error) {
	decoded, err := feedparse.Decode(p.body, p.contentType)
	if err != nil {
		return nil, &Error{Kind: KindParse, Detail: err.Error()}
	}
	parsed, err := feedparse.Parse(decoded)
	if err != nil {
		return nil, &Error{Kind: KindParse, Detail: err.Error()}
	}
	return &Feed{FeedURL: feedURL, Title: parsed.Title, Description: parsed.Description, SiteURL: siteURL(parsed)}, nil
}

// siteURL is the first link whose rel is "alternate" or absent, which skips
// an Atom "self" link and RSS comments links.
func siteURL(parsed *feedparse.Feed) *string {
	for _, link := range parsed.Links {
		if link.Target == feedparse.TargetNone && (link.Rel == nil || *link.Rel == "alternate") {
			href := link.Href
			return &href
		}
	}
	return nil
}
