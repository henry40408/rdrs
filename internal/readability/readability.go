// Package readability fetches a page and extracts its main article.
package readability

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"

	goreadability "codeberg.org/readeck/go-readability/v2"
	"golang.org/x/net/html/charset"

	"github.com/henry40408/rdrs/internal/fetch"
)

// MaxElements bounds the element nodes in a fetched document; a larger one is
// refused before any scoring work. The HTML is parsed by then, so this bounds
// the scoring and clean-up cost, not the memory of the parsed document. Large
// real pages (Wikipedia's "United States" has about 22,000 elements) leave
// about 4x headroom.
const MaxElements = 100_000

// Extracted is a readable article. Title is nil when the page gave none.
type Extracted struct {
	Title   *string
	Content string
}

// Kind says which stage failed.
type Kind int

// The failure kinds. Their texts are shown to users, so they match Rust.
const (
	KindInvalidURL Kind = iota
	KindFetch
)

// Error is a failed extraction.
type Error struct {
	Kind   Kind
	Detail string
}

func (e *Error) Error() string {
	if e.Kind == KindInvalidURL {
		return "Invalid URL"
	}
	return "Failed to fetch: " + e.Detail
}

// Params are the inputs of FetchAndExtract.
type Params struct {
	URL       string
	UserAgent string
	Fetcher   *fetch.Fetcher
	Retry     fetch.RetryConfig
}

// FetchAndExtract downloads the page and extracts its article. The SSRF guard
// runs before any network I/O; the fetcher re-checks every redirect hop and
// resolved address.
func FetchAndExtract(ctx context.Context, p Params) (*Extracted, error) {
	parsed, err := url.Parse(p.URL)
	if err != nil || p.Fetcher.Validate(parsed) != nil {
		return nil, &Error{Kind: KindInvalidURL}
	}
	page, err := p.download(ctx, parsed)
	if err != nil {
		return nil, err
	}
	title, content, err := extractArticle(page, parsed, MaxElements)
	if err != nil {
		return nil, &Error{Kind: KindFetch, Detail: "Failed to extract content: " + err.Error()}
	}
	result := &Extracted{Content: content}
	if title != "" {
		result.Title = &title
	}
	return result, nil
}

// download returns the page text decoded to UTF-8 by its declared charset.
func (p Params) download(ctx context.Context, u *url.URL) (string, error) {
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
		return "", &Error{Kind: KindFetch, Detail: err.Error()}
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return "", &Error{Kind: KindFetch, Detail: "HTTP " + strconv.Itoa(resp.StatusCode) + " " + http.StatusText(resp.StatusCode)}
	}
	text, err := decode(resp.Body, resp.Header.Get("Content-Type"))
	if err != nil {
		return "", &Error{Kind: KindFetch, Detail: err.Error()}
	}
	return text, nil
}

func decode(body io.Reader, contentType string) (string, error) {
	reader, err := charset.NewReader(body, contentType)
	if err != nil {
		return "", err
	}
	text, err := io.ReadAll(reader)
	return string(text), err
}

// extractArticle returns the title and HTML of the main content, refusing a
// document with more than maxElements elements. The parser is third-party
// code run on hostile input, so a panic becomes an error.
func extractArticle(page string, base *url.URL, maxElements int) (title, content string, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("panic: %v", r)
		}
	}()
	parser := goreadability.NewParser()
	parser.MaxElemsToParse = maxElements
	article, err := parser.Parse(strings.NewReader(page), base)
	if err != nil {
		return "", "", describe(err)
	}
	var rendered bytes.Buffer
	if err := article.RenderHTML(&rendered); err != nil {
		return "", "", err
	}
	return article.Title(), rendered.String(), nil
}

// describe words the element-limit error as the Rust parser did.
func describe(err error) error {
	if strings.HasPrefix(err.Error(), "documents too large") {
		return fmt.Errorf("too many elements: %w", err)
	}
	return err
}
