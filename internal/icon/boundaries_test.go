package icon

import (
	"bytes"
	"context"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

func TestAsciiLowerCoversTheFullUppercaseRange(t *testing.T) {
	if got := asciiLower("AZaz@[É"); got != "azaz@[É" {
		t.Errorf("asciiLower = %q", got)
	}
}

func TestExtractFaviconBoundaries(t *testing.T) {
	base, _ := url.Parse("https://example.com/p")
	cases := []struct{ name, page, want string }{
		{"rel at the very start has no tag", `rel="icon" <link rel='icon' href="/x">`, ""},
		{"empty href is still a href", `<link rel="icon" href="">`, "https://example.com/p"},
	}
	for _, c := range cases {
		if got := extractFavicon(c.page, base); got != c.want {
			t.Errorf("%s: got %q, want %q", c.name, got, c.want)
		}
	}
}

func TestStatus299IsAccepted(t *testing.T) {
	status299 := func(contentType string, body []byte) http.HandlerFunc {
		return func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", contentType)
			w.WriteHeader(299)
			_, _ = w.Write(body)
		}
	}
	server := serve(t, map[string]http.HandlerFunc{
		"/img":         status299("image/png", pngBytes),
		"/page/":       status299("text/html", []byte(`<link rel="icon" href="/img">`)),
		"/favicon.ico": http.NotFound,
	})
	if img := loopback(t).download(context.Background(), server.URL+"/img"); img == nil {
		t.Error("download rejected status 299")
	}
	if img := loopback(t).favicon(context.Background(), server.URL+"/page/"); img == nil {
		t.Error("page lookup rejected status 299")
	}
}

func TestFetchedImageIsLoggedOnlyOnSuccess(t *testing.T) {
	var logs bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&logs, &slog.HandlerOptions{Level: slog.LevelDebug})))
	t.Cleanup(func() { slog.SetDefault(previous) })
	server := serve(t, map[string]http.HandlerFunc{"/ok": reply("image/png", pngBytes)})
	f := loopback(t)
	if f.image(context.Background(), server.URL+"/missing", "test") != nil {
		t.Fatal("missing image was returned")
	}
	if strings.Contains(logs.String(), "icon.fetched") {
		t.Errorf("failure logged as fetched: %s", logs.String())
	}
	if f.image(context.Background(), server.URL+"/ok", "test") == nil {
		t.Fatal("image not returned")
	}
	if !strings.Contains(logs.String(), "icon.fetched") {
		t.Errorf("success not logged: %s", logs.String())
	}
}
