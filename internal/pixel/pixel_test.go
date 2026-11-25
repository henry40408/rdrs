package pixel

import (
	"encoding/hex"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/secret"
)

func mustTime(t *testing.T, s string) time.Time {
	t.Helper()
	parsed, err := time.Parse(time.RFC3339, s)
	if err != nil {
		t.Fatal(err)
	}
	return parsed
}

func TestRustVectors(t *testing.T) {
	if PathPrefix != "/p/" {
		t.Fatalf("prefix %q", PathPrefix)
	}
	if hex.EncodeToString(TransparentGIF) != rustGIFHex {
		t.Fatal("GIF bytes differ from Rust")
	}
	optIn := mustTime(t, rustOptIn)
	for _, u := range rustURLVectors {
		c := Context{UserID: u.userID, EnabledAt: &optIn, Secret: testKey, BaseURL: u.baseURL}
		if got := c.URL(u.entryID); got != u.url {
			t.Errorf("URL(%d,%d) = %q, want %q", u.userID, u.entryID, got, u.url)
		}
	}
	for _, row := range rustInjectVectors {
		c := Context{UserID: 7, Secret: testKey}
		if row.enabled {
			c.EnabledAt = &optIn
		}
		got := c.MaybeInject(row.html, 42, mustTime(t, row.createdAt))
		if got != row.output {
			t.Errorf("inject(%+v) = %q, want %q", row, got, row.output)
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
