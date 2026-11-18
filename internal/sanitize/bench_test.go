package sanitize

import (
	"fmt"
	"strings"
	"testing"
)

// Port of examples/bench_sanitize.rs: a representative large article, many
// images and links. Compare runs with benchstat.

// benchArticle builds paragraphs interleaved with images (some lazy, some
// tracking pixels) and links (some with tracking parameters).
func benchArticle(blocks int) string {
	var s strings.Builder
	s.WriteString("<h1>A Representative Article</h1>")
	for i := range blocks {
		fmt.Fprintf(&s, "<p>Paragraph %d with some <strong>bold</strong> and <em>italic</em> text, "+
			"plus an <a href=\"https://example.com/page%d?utm_source=feed&amp;utm_medium=rss&amp;id=%d\">tracked link</a> "+
			"and a <a href=\"https://other.example.org/article/%d\">clean link</a>.</p>", i, i, i, i)
		fmt.Fprintf(&s, "<figure><img src=\"https://cdn.example.com/img/%d.jpg?w=1200&amp;h=800\" "+
			"alt=\"Image %d\" width=\"1200\" height=\"800\"><figcaption>Caption %d</figcaption></figure>", i, i, i)
		fmt.Fprintf(&s, "<img src=\"data:image/svg+xml,%%3Csvg%%3E%%3C/svg%%3E\" "+
			"data-lazy-src=\"https://cdn.example.com/lazy/%d.jpg\" alt=\"Lazy %d\">", i, i)
		fmt.Fprintf(&s, "<img src=\"https://pixel.tracker.com/p%d.gif\" width=\"1\" height=\"1\">", i)
	}
	return s.String()
}

func BenchmarkSanitizeHTML(b *testing.B) {
	html := benchArticle(120)
	ctx := Context{
		Secret:       []byte("benchmark_secret_key_32_bytes!!!"),
		BaseURL:      "https://example.com/post",
		ProxyBaseURL: "https://rdrs.example.com",
	}
	b.SetBytes(int64(len(html)))
	b.ReportAllocs()
	for b.Loop() {
		_ = HTML(html, ctx)
	}
}
