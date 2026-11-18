package readability

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/fetch"
)

const articleHTML = `<!DOCTYPE html>
<html>
  <head><title>The Readable Title</title></head>
  <body>
    <nav>skip me</nav>
    <article>
      <h1>The Readable Title</h1>
      <p>This is the first substantial paragraph of the article body that
      readability should retain because it carries the real content.</p>
      <p>A second paragraph with even more meaningful prose so the extractor
      is confident this is the main article region and not boilerplate.</p>
    </article>
    <footer>copyright boilerplate</footer>
  </body>
</html>`

func loopbackParams(t *testing.T, target string) Params {
	t.Helper()
	policy, err := fetch.ParsePolicy("127.0.0.1")
	if err != nil {
		t.Fatal(err)
	}
	return Params{
		URL: target, UserAgent: "RDRS-Test/1.0", Fetcher: fetch.NewFetcher(policy),
		Retry: fetch.RetryConfig{InitialBackoff: time.Millisecond, MaxBackoff: time.Millisecond},
	}
}

func serve(t *testing.T, handler http.HandlerFunc) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	return server
}

func page(body string) http.HandlerFunc {
	return func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		_, _ = w.Write([]byte(body))
	}
}

func asError(t *testing.T, err error) *Error {
	t.Helper()
	var got *Error
	if !errors.As(err, &got) {
		t.Fatalf("err = %v, want *Error", err)
	}
	return got
}

func TestRejectsMalformedBlockedAndNonHTTPURLs(t *testing.T) {
	fetcher := fetch.NewFetcher(fetch.Policy{})
	for _, raw := range []string{"not a url", "http://localhost/article", "ftp://example.com/article"} {
		_, err := FetchAndExtract(context.Background(), Params{URL: raw, Fetcher: fetcher})
		if got := asError(t, err); got.Kind != KindInvalidURL || got.Error() != "Invalid URL" {
			t.Errorf("%s: err = %v", raw, err)
		}
	}
}

func TestExtractsTitleAndContentAndSendsUserAgent(t *testing.T) {
	var agent string
	server := serve(t, func(w http.ResponseWriter, r *http.Request) {
		agent = r.Header.Get("User-Agent")
		page(articleHTML)(w, r)
	})
	got, err := FetchAndExtract(context.Background(), loopbackParams(t, server.URL+"/article"))
	if err != nil {
		t.Fatal(err)
	}
	if got.Title == nil || *got.Title != "The Readable Title" {
		t.Fatalf("title = %v", got.Title)
	}
	if !strings.Contains(got.Content, "first substantial paragraph") || strings.Contains(got.Content, "skip me") {
		t.Fatalf("content = %s", got.Content)
	}
	if agent != "RDRS-Test/1.0" {
		t.Fatalf("User-Agent = %q", agent)
	}
}

func TestExtractsFromAPageNestedDeeperThanTheHTMLParserLimit(t *testing.T) {
	deep := "<html><head><title>Deep</title></head><body>" + strings.Repeat("<div>", 600) +
		"<p>" + strings.Repeat("A long enough sentence for the scorer to keep. ", 20) + "</p>" +
		strings.Repeat("</div>", 600) + "</body></html>"
	server := serve(t, page(deep))
	got, err := FetchAndExtract(context.Background(), loopbackParams(t, server.URL+"/deep"))
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(got.Content, "A long enough sentence") {
		t.Fatalf("content = %.200s", got.Content)
	}
}

func TestEmptyTitleBecomesNil(t *testing.T) {
	body := `<!DOCTYPE html><html><head></head><body><article>
<p>Body-only content with no title anywhere in the document, long
enough that the extractor keeps it as the main article region.</p>
<p>Another paragraph of prose to reinforce the content region.</p>
</article></body></html>`
	server := serve(t, page(body))
	got, err := FetchAndExtract(context.Background(), loopbackParams(t, server.URL))
	if err != nil {
		t.Fatal(err)
	}
	if got.Title != nil {
		t.Fatalf("title = %q, want nil", *got.Title)
	}
}

func TestNonSuccessStatus(t *testing.T) {
	server := serve(t, func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNotFound) })
	_, err := FetchAndExtract(context.Background(), loopbackParams(t, server.URL+"/missing"))
	if got := asError(t, err); got.Kind != KindFetch || got.Detail != "HTTP 404 Not Found" {
		t.Fatalf("err = %v", err)
	}
}

func TestDecodesTheDeclaredCharset(t *testing.T) {
	// "é" in ISO-8859-1 is the single byte 0xE9.
	body := strings.ReplaceAll(articleHTML, "first substantial", "caf\xe9 substantial")
	server := serve(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/html; charset=iso-8859-1")
		_, _ = w.Write([]byte(body))
	})
	got, err := FetchAndExtract(context.Background(), loopbackParams(t, server.URL))
	if err != nil || !strings.Contains(got.Content, "café substantial") {
		t.Fatalf("got = %+v, err = %v", got, err)
	}
}

func TestElementLimit(t *testing.T) {
	base, _ := url.Parse("https://example.com/")
	spans := strings.Repeat("<span>x</span>", 100)
	doc := "<html><head></head><body><article><p>" + spans + "</p></article></body></html>"
	_, _, err := extractArticle(doc, base, 50)
	if err == nil || !strings.Contains(err.Error(), "too many elements") {
		t.Fatalf("err = %v", err)
	}
	if _, _, err := extractArticle(doc, base, 1000); err != nil {
		t.Fatal(err)
	}
}

func TestOversizedDocumentIsAFetchError(t *testing.T) {
	body := "<html><head></head><body><article><p>" + strings.Repeat("<i></i>", MaxElements+1) + "</p></article></body></html>"
	server := serve(t, page(body))
	_, err := FetchAndExtract(context.Background(), loopbackParams(t, server.URL))
	got := asError(t, err)
	if got.Kind != KindFetch || !strings.HasPrefix(got.Detail, "Failed to extract content: too many elements") {
		t.Fatalf("err = %v", err)
	}
}

func TestFixtureIsUnderTheProductionLimit(t *testing.T) {
	base, _ := url.Parse("https://example.com/")
	title, content, err := extractArticle(articleHTML, base, MaxElements)
	if err != nil || title != "The Readable Title" || !strings.Contains(content, "first substantial paragraph") {
		t.Fatalf("title = %q, content = %q, err = %v", title, content, err)
	}
}
