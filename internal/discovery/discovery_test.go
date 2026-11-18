package discovery

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/fetch"
)

const rssBody = `<?xml version="1.0"?><rss version="2.0"><channel>
<title>Example Feed</title><description>Desc</description>
<link>https://example.com</link></channel></rss>`

func loopbackParams(t *testing.T, target string) Params {
	t.Helper()
	policy, err := fetch.ParsePolicy("127.0.0.1")
	if err != nil {
		t.Fatal(err)
	}
	return Params{
		URL: target, UserAgent: "RDRS-Test/1.0", Fetcher: fetch.NewFetcher(policy),
		Retry: fetch.RetryConfig{MaxRetries: 0, InitialBackoff: time.Millisecond, MaxBackoff: time.Millisecond},
	}
}

func serve(t *testing.T, routes map[string]http.HandlerFunc) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()
	for path, handler := range routes {
		mux.HandleFunc(path, handler)
	}
	server := httptest.NewServer(mux)
	t.Cleanup(server.Close)
	return server
}

func reply(contentType, body string) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", contentType)
		_, _ = w.Write([]byte(body))
	}
}

func wantKind(t *testing.T, err error, kind Kind) *Error {
	t.Helper()
	var got *Error
	if !errors.As(err, &got) || got.Kind != kind {
		t.Fatalf("error = %v, want kind %d", err, kind)
	}
	return got
}

func TestDirectFeedByContentType(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("application/rss+xml", rssBody)})
	feed, err := Discover(context.Background(), loopbackParams(t, server.URL))
	if err != nil {
		t.Fatal(err)
	}
	if *feed.Title != "Example Feed" || *feed.Description != "Desc" || *feed.SiteURL != "https://example.com/" {
		t.Fatalf("feed = %+v", feed)
	}
	if feed.FeedURL != server.URL {
		t.Fatalf("FeedURL = %q", feed.FeedURL)
	}
}

func TestDiscoverViaHTMLLink(t *testing.T) {
	page := `<html><head><link rel="alternate" type="application/rss+xml" href="/feed.xml"></head><body>hi</body></html>`
	server := serve(t, map[string]http.HandlerFunc{
		"/":         reply("text/html", page),
		"/feed.xml": reply("application/rss+xml", rssBody),
	})
	feed, err := Discover(context.Background(), loopbackParams(t, server.URL+"/"))
	if err != nil {
		t.Fatal(err)
	}
	if feed.FeedURL != server.URL+"/feed.xml" || *feed.Title != "Example Feed" {
		t.Fatalf("feed = %+v", feed)
	}
}

func TestDiscoverInAPageNestedDeeperThanTheHTMLParserLimit(t *testing.T) {
	page := `<html><head><link rel="alternate" type="application/rss+xml" href="/feed.xml"></head><body>` +
		strings.Repeat("<div>", 600) + "x" + strings.Repeat("</div>", 600) + `</body></html>`
	server := serve(t, map[string]http.HandlerFunc{
		"/":         reply("text/html", page),
		"/feed.xml": reply("application/rss+xml", rssBody),
	})
	feed, err := Discover(context.Background(), loopbackParams(t, server.URL+"/"))
	if err != nil {
		t.Fatal(err)
	}
	if feed.FeedURL != server.URL+"/feed.xml" {
		t.Fatalf("feed = %+v", feed)
	}
}

func TestFirstFeedLinkWinsAndAtomAndXMLTypesCount(t *testing.T) {
	page := `<link rel="alternate" type="text/html" href="/no"><link rel="stylesheet" type="application/rss+xml" href="/no2">
<link rel="alternate" type="application/rss+xml"><link rel="alternate" type="application/atom+xml" href="/atom.xml">
<link rel="alternate" type="application/xml" href="/other.xml">`
	server := serve(t, map[string]http.HandlerFunc{
		"/":         reply("text/html", page),
		"/atom.xml": reply("application/atom+xml", rssBody),
	})
	feed, err := Discover(context.Background(), loopbackParams(t, server.URL))
	if err != nil {
		t.Fatal(err)
	}
	if feed.FeedURL != server.URL+"/atom.xml" {
		t.Fatalf("FeedURL = %q", feed.FeedURL)
	}
}

func TestDiscoverByBodySniffing(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("text/html", "  \n"+rssBody)})
	feed, err := Discover(context.Background(), loopbackParams(t, server.URL))
	if err != nil || *feed.Title != "Example Feed" {
		t.Fatalf("feed = %+v, err = %v", feed, err)
	}
}

func TestHTMLWithoutFeedLink(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("text/html", "<html><body>none</body></html>")})
	_, err := Discover(context.Background(), loopbackParams(t, server.URL))
	if got := wantKind(t, err, KindNoFeed); got.Error() != "No feed found at URL" {
		t.Fatal(got)
	}
}

func TestRejectsBadURLs(t *testing.T) {
	for _, raw := range []string{"not a url", "ftp://example.com/feed", "http://10.0.0.1/feed", "http://[::1]/x"} {
		params := loopbackParams(t, raw)
		_, err := Discover(context.Background(), params)
		if got := wantKind(t, err, KindInvalidURL); got.Error() != "Invalid URL" {
			t.Fatal(got)
		}
	}
}

func TestRejectsFeedLinkPointingInward(t *testing.T) {
	page := `<link rel="alternate" type="application/rss+xml" href="http://169.254.169.254/feed">`
	server := serve(t, map[string]http.HandlerFunc{"/": reply("text/html", page)})
	_, err := Discover(context.Background(), loopbackParams(t, server.URL))
	wantKind(t, err, KindInvalidURL)
}

func TestFirstFetchStatusError(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}})
	_, err := Discover(context.Background(), loopbackParams(t, server.URL))
	if got := wantKind(t, err, KindFetch); got.Error() != "Failed to fetch: HTTP 404 Not Found" {
		t.Fatal(got)
	}
}

func TestDiscoveredFetchStatusError(t *testing.T) {
	page := `<link rel="alternate" type="application/rss+xml" href="/feed.xml">`
	server := serve(t, map[string]http.HandlerFunc{
		"/": reply("text/html", page),
		"/feed.xml": func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusInternalServerError)
		},
	})
	_, err := Discover(context.Background(), loopbackParams(t, server.URL))
	wantKind(t, err, KindFetch)
}

func TestMalformedFeedBody(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("application/rss+xml", "<rss><channel>")})
	_, err := Discover(context.Background(), loopbackParams(t, server.URL))
	wantKind(t, err, KindParse)
}

func TestSendsUserAgent(t *testing.T) {
	var got string
	server := serve(t, map[string]http.HandlerFunc{"/": func(w http.ResponseWriter, r *http.Request) {
		got = r.Header.Get("User-Agent")
		reply("application/rss+xml", rssBody)(w, r)
	}})
	if _, err := Discover(context.Background(), loopbackParams(t, server.URL)); err != nil {
		t.Fatal(err)
	}
	if got != "RDRS-Test/1.0" {
		t.Fatalf("User-Agent = %q", got)
	}
}

func TestSiteURLSkipsSelfLink(t *testing.T) {
	atom := `<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title>T</title><id>x</id>
<link rel="self" href="https://example.com/feed"/><link href="https://example.com/"/></feed>`
	server := serve(t, map[string]http.HandlerFunc{"/": reply("application/atom+xml", atom)})
	feed, err := Discover(context.Background(), loopbackParams(t, server.URL))
	if err != nil || *feed.SiteURL != "https://example.com/" {
		t.Fatalf("feed = %+v, err = %v", feed, err)
	}
}

func TestFeedContentTypes(t *testing.T) {
	for _, ok := range []string{"application/rss+xml", "application/atom+xml", "application/xml", "text/xml"} {
		if !isFeedContentType(ok) {
			t.Errorf("%s should be a feed type", ok)
		}
	}
	if isFeedContentType("text/html") {
		t.Error("text/html is not a feed type")
	}
}

func TestLooksLikeFeed(t *testing.T) {
	for _, ok := range []string{"<?xml version", "<rss version=", "<feed xmlns=", "<RDF:RDF"} {
		if !looksLikeFeed([]byte(ok)) {
			t.Errorf("%q should look like a feed", ok)
		}
	}
	for _, no := range []string{"random text", "<html>"} {
		if looksLikeFeed([]byte(no)) {
			t.Errorf("%q should not look like a feed", no)
		}
	}
}

func TestRelativeLinkResolvedAgainstPage(t *testing.T) {
	page := `<link rel="alternate" type="application/rss+xml" href="feed.xml">`
	server := serve(t, map[string]http.HandlerFunc{
		"/blog/":         reply("text/html", page),
		"/blog/feed.xml": reply("application/rss+xml", rssBody),
	})
	feed, err := Discover(context.Background(), loopbackParams(t, server.URL+"/blog/"))
	if err != nil || feed.FeedURL != server.URL+"/blog/feed.xml" {
		t.Fatalf("feed = %+v, err = %v", feed, err)
	}
}
