package htmldepth

import (
	"errors"
	"strings"
	"testing"

	"golang.org/x/net/html"
)

func deepPage(depth int) string {
	return "<html><body>" + strings.Repeat("<div>", depth) + "<p>text</p>" + strings.Repeat("</div>", depth) + "</body></html>"
}

func TestFlattenMakesADeepPageParse(t *testing.T) {
	page := deepPage(600)
	if _, err := html.Parse(strings.NewReader(page)); !IsTooDeep(err) {
		t.Fatalf("the raw page must trip the limit, got %v", err)
	}
	flat := Flatten(page)
	if _, err := html.Parse(strings.NewReader(flat)); err != nil {
		t.Fatalf("flattened page: %v", err)
	}
	if !strings.Contains(flat, "text") || strings.Count(flat, "<div>") != MaxDepth-2 {
		t.Errorf("content lost or wrong depth: %d divs", strings.Count(flat, "<div>"))
	}
}

func TestFlattenKeepsVoidAndForeignTags(t *testing.T) {
	page := `<head><link rel="alternate" href="/f"></head><body><br><img src=a></div>` + deepPage(450)
	flat := Flatten(page)
	for _, want := range []string{`<link rel="alternate" href="/f">`, "<br>", "<img src=a>", "</div>"} {
		if !strings.Contains(flat, want) {
			t.Errorf("lost %q", want)
		}
	}
}

func TestIsTooDeepOnlyMatchesTheLimitError(t *testing.T) {
	if IsTooDeep(nil) || IsTooDeep(errors.New("other")) {
		t.Error("unrelated errors must not match")
	}
}
