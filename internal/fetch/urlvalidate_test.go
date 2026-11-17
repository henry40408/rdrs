package fetch

import (
	"errors"
	"net/netip"
	"net/url"
	"testing"
)

func mustURL(t *testing.T, raw string) *url.URL {
	t.Helper()
	u, err := url.Parse(raw)
	if err != nil {
		t.Fatal(err)
	}
	return u
}

func TestValidateURLAcceptsPublicHTTPAndHTTPS(t *testing.T) {
	for _, raw := range []string{"https://example.com/article", "http://example.com/article", "https://EXAMPLE.com/"} {
		if err := ValidateURL(mustURL(t, raw)); err != nil {
			t.Errorf("%s: %v", raw, err)
		}
	}
}

func TestValidateURLRejects(t *testing.T) {
	cases := []struct {
		raw  string
		want error
	}{
		{"http://localhost/article", ErrBlockedHost},
		{"http://LOCALHOST/article", ErrBlockedHost},
		{"http://127.0.0.1/article", ErrBlockedHost},
		{"http://[::1]/article", ErrBlockedHost},
		{"http://myhost.local/article", ErrBlockedHost},
		{"http://server.internal/article", ErrBlockedHost},
		{"http://10.0.0.1/image.jpg", ErrPrivateIP},
		{"http://172.16.0.1/image.jpg", ErrPrivateIP},
		{"http://192.168.1.1/article", ErrPrivateIP},
		{"http://169.254.1.1/image.jpg", ErrPrivateIP},
		{"http://255.255.255.255/image.jpg", ErrPrivateIP},
		{"http://192.0.2.1/image.jpg", ErrPrivateIP},
		{"http://198.51.100.1/image.jpg", ErrPrivateIP},
		{"http://203.0.113.1/image.jpg", ErrPrivateIP},
		{"http://0.0.0.0/image.jpg", ErrPrivateIP},
		{"http://[::]/image.jpg", ErrPrivateIP},
		{"http://[::ffff:127.0.0.1]/", ErrPrivateIP},
		{"ftp://example.com/file", ErrInvalidScheme},
		{"file:///etc/passwd", ErrInvalidScheme},
		{"http:///path", ErrNoHost},
		// The WHATWG parser the Rust build used rewrites these to loopback.
		{"http://2130706433/", ErrBlockedHost},
		{"http://0x7f.1/", ErrBlockedHost},
		{"http://0177.0.0.1/", ErrBlockedHost},
		{"http://127.1/", ErrBlockedHost},
		{"http://10.1/", ErrPrivateIP},
		{"http://1.2.3.4.5/", ErrBlockedHost},
		{"http://256.256.256.256/", ErrBlockedHost},
	}
	for _, tc := range cases {
		if err := ValidateURL(mustURL(t, tc.raw)); !errors.Is(err, tc.want) {
			t.Errorf("%s: got %v, want %v", tc.raw, err, tc.want)
		}
	}
}

func TestValidateURLKeepsNumericLookingNames(t *testing.T) {
	for _, raw := range []string{"http://1.2.3.4.example.com/", "http://a1/", "http://8.8.8.8/", "http://example.com./"} {
		if err := ValidateURL(mustURL(t, raw)); err != nil {
			t.Errorf("%s: %v", raw, err)
		}
	}
}

func TestErrorTexts(t *testing.T) {
	for err, want := range map[error]string{
		ErrInvalidScheme: "URL scheme must be http or https",
		ErrNoHost:        "URL has no host",
		ErrBlockedHost:   "URL host is blocked",
		ErrPrivateIP:     "URL points to a private IP",
	} {
		if err.Error() != want {
			t.Errorf("got %q, want %q", err, want)
		}
	}
}

func TestIsPrivateIP(t *testing.T) {
	private := []string{
		"::", "::1", "100.64.0.1", "100.127.255.254", "198.18.0.1", "198.19.255.255", "240.0.0.1", "224.0.0.1",
		"fd00::1", "fe80::1", "2001:db8::1", "ff02::1", "::ffff:127.0.0.1", "::ffff:192.168.1.1",
	}
	public := []string{
		"2606:4700::1111", "100.63.255.255", "100.128.0.1", "198.17.255.255", "198.20.0.1", "172.67.219.169",
		"::ffff:1.1.1.1", "1.1.1.1", "172.32.0.1", "0.0.0.1", "192.0.3.1",
		// Deliberately unblocked, as in the Rust build.
		"64:ff9b::7f00:1", "fec0::1", "2002:7f00:1::1",
	}
	for _, addr := range private {
		if !IsPrivateIP(netip.MustParseAddr(addr)) {
			t.Errorf("%s must be private", addr)
		}
	}
	for _, addr := range public {
		if IsPrivateIP(netip.MustParseAddr(addr)) {
			t.Errorf("%s must stay reachable", addr)
		}
	}
}
