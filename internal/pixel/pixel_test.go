package pixel

import (
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/secret"
)

type vectors struct {
	Secret string `json:"secret"`
	Prefix string `json:"prefix"`
	OptIn  string `json:"opt_in"`
	URLs   []struct {
		UserID  int64   `json:"user_id"`
		EntryID int64   `json:"entry_id"`
		BaseURL *string `json:"base_url"`
		URL     string  `json:"url"`
	} `json:"urls"`
	Inject []struct {
		Enabled   bool   `json:"enabled"`
		CreatedAt string `json:"created_at"`
		HTML      string `json:"html"`
		Output    string `json:"output"`
	} `json:"inject"`
	GIFHex string `json:"gif_hex"`
}

func loadVectors(t *testing.T) vectors {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("..", "..", "testdata", "vectors", "pixel.json"))
	if err != nil {
		t.Fatal(err)
	}
	var v vectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func mustTime(t *testing.T, s string) time.Time {
	t.Helper()
	parsed, err := time.Parse(time.RFC3339, s)
	if err != nil {
		t.Fatal(err)
	}
	return parsed
}

func TestRustVectors(t *testing.T) {
	v := loadVectors(t)
	key := []byte(v.Secret)
	if v.Prefix != PathPrefix {
		t.Fatalf("prefix %q", v.Prefix)
	}
	if hex.EncodeToString(TransparentGIF) != v.GIFHex {
		t.Fatal("GIF bytes differ from Rust")
	}
	optIn := mustTime(t, v.OptIn)
	for _, u := range v.URLs {
		c := Context{UserID: u.UserID, EnabledAt: &optIn, Secret: key}
		if u.BaseURL != nil {
			c.BaseURL = *u.BaseURL
		}
		if got := c.URL(u.EntryID); got != u.URL {
			t.Errorf("URL(%d,%d) = %q, want %q", u.UserID, u.EntryID, got, u.URL)
		}
	}
	for _, row := range v.Inject {
		c := Context{UserID: 7, Secret: key}
		if row.Enabled {
			c.EnabledAt = &optIn
		}
		got := c.MaybeInject(row.HTML, 42, mustTime(t, row.CreatedAt))
		if got != row.Output {
			t.Errorf("inject(%+v) = %q, want %q", row, got, row.Output)
		}
	}
}

var testKey = []byte("0123456789abcdef0123456789abcdef")

func TestInjectedPixelCarriesAVerifiableSignature(t *testing.T) {
	optIn := time.Now().Add(-time.Hour)
	c := Context{UserID: 7, EnabledAt: &optIn, Secret: testKey}
	out := c.MaybeInject("<p>Article</p>", 42, time.Now())
	if !strings.HasPrefix(out, "<p>Article</p>") || !strings.Contains(out, `width="1" height="1"`) {
		t.Fatal(out)
	}
	rest := strings.SplitN(out, "/p/7-42-", 2)[1]
	sig := strings.SplitN(rest, ".gif", 2)[0]
	if !secret.VerifyPixelSig(testKey, 7, 42, sig) {
		t.Fatalf("signature %q does not verify", sig)
	}
}

func TestEntryCreatedAtTheOptInInstantGetsAPixel(t *testing.T) {
	optIn := time.Now()
	c := Context{UserID: 7, EnabledAt: &optIn, Secret: testKey}
	if got := c.MaybeInject("<p>x</p>", 1, optIn); got == "<p>x</p>" {
		t.Fatal("entry created exactly at opt-in must be counted")
	}
	if got := c.MaybeInject("<p>x</p>", 1, optIn.Add(-time.Second)); got != "<p>x</p>" {
		t.Fatal("entry older than the opt-in must not get a pixel")
	}
}

func TestEmptyContentStaysEmpty(t *testing.T) {
	optIn := time.Now().Add(-time.Hour)
	c := Context{UserID: 7, EnabledAt: &optIn, Secret: testKey}
	for _, html := range []string{"", " \n\t"} {
		if got := c.MaybeInject(html, 42, time.Now()); got != html {
			t.Errorf("%q became %q", html, got)
		}
	}
}

func TestBaseURLMakesThePixelAbsoluteWithOneSlash(t *testing.T) {
	optIn := time.Now().Add(-time.Hour)
	c := Context{UserID: 7, EnabledAt: &optIn, Secret: testKey, BaseURL: "https://rdrs.example.com/"}
	out := c.MaybeInject("<p>Article</p>", 42, time.Now())
	if !strings.Contains(out, `src="https://rdrs.example.com/p/7-42-`) || strings.Contains(out, "com//p/") {
		t.Fatal(out)
	}
}

func TestDisabledContext(t *testing.T) {
	c := Disabled(7, testKey)
	if c.IsEnabled() {
		t.Fatal("disabled context reports enabled")
	}
	if got := c.MaybeInject("<p>x</p>", 1, time.Now()); got != "<p>x</p>" {
		t.Fatal(got)
	}
}

// IsEnabled reports whether this reader tracks opens at all.
func (c Context) IsEnabled() bool { return c.EnabledAt != nil }
