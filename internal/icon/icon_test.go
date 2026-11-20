package icon

import (
	"bytes"
	"context"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/feedsync"
	"github.com/henry40408/rdrs/internal/fetch"
)

var pngBytes = []byte{0x89, 0x50, 0x4E, 0x47}

func loopback(t *testing.T) Fetcher {
	t.Helper()
	policy, err := fetch.ParsePolicy("127.0.0.1")
	if err != nil {
		t.Fatal(err)
	}
	return Fetcher{
		Fetcher: fetch.NewFetcher(policy), UserAgent: "RDRS-Test/1.0",
		Retry: fetch.RetryConfig{InitialBackoff: time.Millisecond, MaxBackoff: time.Millisecond},
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

func reply(contentType string, body []byte) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		if contentType != "" {
			w.Header().Set("Content-Type", contentType)
		}
		_, _ = w.Write(body)
	}
}

func str(s string) *string { return &s }

func TestImageSuccess(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("image/png", pngBytes)})
	img := loopback(t).download(context.Background(), server.URL)
	if img == nil || img.ContentType != "image/png" || !bytes.Equal(img.Data, pngBytes) || img.SourceURL != server.URL {
		t.Fatalf("img = %+v", img)
	}
}

func TestImageStripsContentTypeParams(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("image/svg+xml; charset=utf-8", pngBytes)})
	img := loopback(t).download(context.Background(), server.URL)
	if img == nil || img.ContentType != "image/svg+xml" {
		t.Fatalf("img = %+v", img)
	}
}

func TestImageRejections(t *testing.T) {
	cases := map[string]http.HandlerFunc{
		"not found":    func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNotFound) },
		"not an image": reply("text/html", []byte("hi")),
		"no type": func(w http.ResponseWriter, _ *http.Request) {
			w.Header()["Content-Type"] = nil
			_, _ = w.Write(pngBytes)
		},
		"too large": reply("image/png", make([]byte, MaxSize+1)),
		"empty":     reply("image/png", nil),
	}
	for name, handler := range cases {
		server := serve(t, map[string]http.HandlerFunc{"/": handler})
		if img := loopback(t).download(context.Background(), server.URL); img != nil {
			t.Errorf("%s: img = %+v, want nil", name, img)
		}
	}
}

func TestImageExactlyAtLimit(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("image/png", make([]byte, MaxSize))})
	if img := loopback(t).download(context.Background(), server.URL); img == nil || len(img.Data) != MaxSize {
		t.Fatalf("img = %+v", img)
	}
}

func TestImageRefusesPrivateAndBadURLs(t *testing.T) {
	fetcher := loopback(t)
	for _, raw := range []string{"http://10.0.0.1/icon.png", "not a url", "ftp://example.com/i.png"} {
		if img := fetcher.download(context.Background(), raw); img != nil {
			t.Errorf("%q: img = %+v", raw, img)
		}
	}
}

func TestFaviconUsesFaviconIco(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/favicon.ico": reply("image/x-icon", pngBytes)})
	img := loopback(t).favicon(context.Background(), server.URL)
	if img == nil || img.SourceURL != server.URL+"/favicon.ico" {
		t.Fatalf("img = %+v", img)
	}
}

func TestFaviconFallsBackToHTMLLink(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{
		"/":                reply("text/html", []byte(`<link rel="icon" href="/assets/icon.png">`)),
		"/assets/icon.png": reply("image/png", pngBytes),
	})
	img := loopback(t).favicon(context.Background(), server.URL+"/")
	if img == nil || img.SourceURL != server.URL+"/assets/icon.png" {
		t.Fatalf("img = %+v", img)
	}
}

func TestFaviconReadsOnlyTheStartOfALargePage(t *testing.T) {
	const link = `<link rel="icon" href="/assets/icon.png">`
	padding := strings.Repeat("a", maxPageSize)
	cases := []struct {
		name string
		page string
		want bool
	}{
		{"link inside the cap", link + padding, true},
		{"link past the cap", padding + link, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			server := serve(t, map[string]http.HandlerFunc{
				"/":                reply("text/html", []byte(c.page)),
				"/assets/icon.png": reply("image/png", pngBytes),
			})
			img := loopback(t).favicon(context.Background(), server.URL+"/")
			if (img != nil) != c.want {
				t.Errorf("img = %+v, want found = %v", img, c.want)
			}
		})
	}
}

func TestFaviconNoneFound(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{"/": reply("text/html", []byte("<html>nothing</html>"))})
	if img := loopback(t).favicon(context.Background(), server.URL); img != nil {
		t.Fatalf("img = %+v", img)
	}
}

func TestFaviconPageErrorAndLinkToMissingImage(t *testing.T) {
	broken := serve(t, map[string]http.HandlerFunc{"/": func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(500) }})
	if img := loopback(t).favicon(context.Background(), broken.URL); img != nil {
		t.Fatalf("img = %+v", img)
	}
	dangling := serve(t, map[string]http.HandlerFunc{"/": reply("text/html", []byte(`<link rel="icon" href="/gone.png">`))})
	if img := loopback(t).favicon(context.Background(), dangling.URL); img != nil {
		t.Fatalf("img = %+v", img)
	}
}

func TestFetchPrefersIconURLThenLogoThenFavicon(t *testing.T) {
	server := serve(t, map[string]http.HandlerFunc{
		"/icon":        reply("image/png", []byte("icon")),
		"/logo":        reply("image/png", []byte("logo")),
		"/favicon.ico": reply("image/x-icon", []byte("fav")),
	})
	fetcher := loopback(t)
	ctx := context.Background()
	all := Candidates{IconURL: str(server.URL + "/icon"), LogoURL: str(server.URL + "/logo"), SiteURL: str(server.URL)}
	if img := fetcher.Fetch(ctx, all); string(img.Data) != "icon" {
		t.Fatalf("img = %+v", img)
	}
	all.IconURL = str(server.URL + "/missing")
	if img := fetcher.Fetch(ctx, all); string(img.Data) != "logo" {
		t.Fatalf("img = %+v", img)
	}
	all.LogoURL = nil
	if img := fetcher.Fetch(ctx, all); string(img.Data) != "fav" {
		t.Fatalf("img = %+v", img)
	}
	if img := fetcher.Fetch(ctx, Candidates{}); img != nil {
		t.Fatalf("img = %+v", img)
	}
}

func TestFetchRefusesPrivateCandidates(t *testing.T) {
	fetcher := Fetcher{Fetcher: fetch.NewFetcher(fetch.Policy{}), UserAgent: "x"}
	img := fetcher.Fetch(context.Background(), Candidates{
		IconURL: str("http://127.0.0.1:1/i.png"), SiteURL: str("http://127.0.0.1:1/"),
	})
	if img != nil {
		t.Fatalf("img = %+v", img)
	}
}

func TestForSyncPassesPerFeedUserAgent(t *testing.T) {
	var agent string
	server := serve(t, map[string]http.HandlerFunc{"/i": func(w http.ResponseWriter, r *http.Request) {
		agent = r.Header.Get("User-Agent")
		reply("image/png", pngBytes)(w, r)
	}})
	hook := loopback(t).ForSync()
	got, err := hook(context.Background(), feedsync.IconCandidates{IconURL: str(server.URL + "/i"), UserAgent: "Feed-UA"})
	if err != nil || got == nil || agent != "Feed-UA" || got.SourceURL != server.URL+"/i" {
		t.Fatalf("got = %+v, err = %v, agent = %q", got, err, agent)
	}
	none, err := hook(context.Background(), feedsync.IconCandidates{})
	if none != nil || err != nil {
		t.Fatalf("none = %+v, err = %v", none, err)
	}
}

func TestExtractHref(t *testing.T) {
	cases := []struct{ tag, want string }{
		{`<link rel="icon" href="/favicon.ico">`, "/favicon.ico"},
		{`<link href='/icon.png' rel='icon'>`, "/icon.png"},
		{`<link HREF="/up.png">`, "/up.png"},
		{`<link href="/unclosed>`, ""},
		{`<link href="/unclosed' href='/x'>`, ""},
		{`<link nothing>`, ""},
		{"<link title=\"İİ\" href=\"/a.png\">", "/a.png"},
		{"<link title=\"KK\" href=\"/a.png\">", "/a.png"},
	}
	for _, c := range cases {
		got, _ := extractHref(c.tag)
		if got != c.want {
			t.Errorf("extractHref(%q) = %q, want %q", c.tag, got, c.want)
		}
	}
}

func TestResolveURL(t *testing.T) {
	base, _ := url.Parse("https://example.com/path/page")
	cases := map[string]string{
		"/favicon.ico":               "https://example.com/favicon.ico",
		"icon.png":                   "https://example.com/path/icon.png",
		"//cdn.example.com/icon.png": "https://cdn.example.com/icon.png",
		"https://other.com/icon.png": "https://other.com/icon.png",
		"http://other.com/icon.png":  "http://other.com/icon.png",
	}
	for href, want := range cases {
		if got := resolveURL(href, base); got != want {
			t.Errorf("resolveURL(%q) = %q, want %q", href, got, want)
		}
	}
	if got := resolveURL("http://[bad", base); got == "" {
		t.Error("absolute URLs are passed through unchecked")
	}
	if got := resolveURL("%zz", base); got != "" {
		t.Errorf("unresolvable href = %q, want empty", got)
	}
}

func TestExtractFavicon(t *testing.T) {
	base, _ := url.Parse("https://example.com")
	cases := []struct{ name, page, want string }{
		{"icon", "<html><head>\n<link rel=\"icon\" href=\"/static/favicon.ico\">\n</head></html>", "https://example.com/static/favicon.ico"},
		{"shortcut", `<link rel="shortcut icon" href="https://cdn.example.com/icon.png">`, "https://cdn.example.com/icon.png"},
		{"single quotes", `<link rel='icon' href='/s.png'>`, "https://example.com/s.png"},
		{"uppercase", `<LINK REL="icon" HREF="/u.png">`, "https://example.com/u.png"},
		{"growing char before", "<title>İ</title><link rel=\"icon\" href=\"/a.png\">", "https://example.com/a.png"},
		{"shrinking char before", "<title>KKK</title><link rel=\"icon\" href=\"/a.png\">", "https://example.com/a.png"},
		{"growing char inside", "<link rel=\"icon\" title=\"İİ\" href=\"/a.png\">", "https://example.com/a.png"},
		{"shrinking char inside", "<link rel=\"icon\" title=\"KK\" href=\"/a.png\">", "https://example.com/a.png"},
		{"no link tag", `rel="icon" href="/x.png"`, ""},
		{"no tag end", `<link rel="icon" href="/x.png"`, ""},
		{"first pattern without href tries next", `<link rel="icon"><link rel='shortcut icon' href='/n.png'>`, "https://example.com/n.png"},
		{"none", "<html></html>", ""},
	}
	for _, c := range cases {
		if got := extractFavicon(c.page, base); got != c.want {
			t.Errorf("%s: got %q, want %q", c.name, got, c.want)
		}
	}
}
