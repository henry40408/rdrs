package auth

import (
	"net/http"
	"net/http/httptest"
	"net/netip"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
)

func trustedConfig(t *testing.T, networks string) config.Config {
	t.Helper()
	parsed, err := config.ParseTrustedNetworks(networks)
	if err != nil {
		t.Fatal(err)
	}
	return config.Config{TrustedProxyNetworks: parsed}
}

func requestFrom(remote string, headers map[string]string) *http.Request {
	r := httptest.NewRequest(http.MethodGet, "/", nil)
	r.RemoteAddr = remote
	for name, value := range headers {
		r.Header.Set(name, value)
	}
	return r
}

func TestClientIP(t *testing.T) {
	cfg := trustedConfig(t, "10.0.0.0/8, 192.168.0.0/16")
	for _, tc := range []struct {
		name    string
		remote  string
		headers map[string]string
		want    string
	}{
		{"untrusted peer ignores headers", "203.0.113.9:1234", map[string]string{"X-Forwarded-For": "1.1.1.1", "X-Real-IP": "2.2.2.2"}, "203.0.113.9"},
		{"trusted peer takes the right-most untrusted hop", "10.0.0.1:80", map[string]string{"X-Forwarded-For": "9.9.9.9, 1.1.1.1, 192.168.1.1"}, "1.1.1.1"},
		{"a forged left entry is never reached", "10.0.0.1:80", map[string]string{"X-Forwarded-For": "6.6.6.6, 1.1.1.1"}, "1.1.1.1"},
		{"a malformed hop breaks the chain", "10.0.0.1:80", map[string]string{"X-Forwarded-For": "1.1.1.1, garbage"}, "10.0.0.1"},
		{"all hops trusted falls back to the peer", "10.0.0.1:80", map[string]string{"X-Forwarded-For": "10.1.1.1, 192.168.3.3"}, "10.0.0.1"},
		{"x-real-ip is the fallback", "10.0.0.1:80", map[string]string{"X-Real-IP": " 4.4.4.4 "}, "4.4.4.4"},
		{"bad x-real-ip falls back to the peer", "10.0.0.1:80", map[string]string{"X-Real-IP": "nope"}, "10.0.0.1"},
		{"zoned address is malformed", "10.0.0.1:80", map[string]string{"X-Forwarded-For": "fe80::1%eth0"}, "10.0.0.1"},
		{"ipv6 hop", "10.0.0.1:80", map[string]string{"X-Forwarded-For": "2001:db8::1"}, "2001:db8::1"},
		{"no peer is loopback", "", nil, "127.0.0.1"},
		{"unparsable peer is loopback", "not-an-address", nil, "127.0.0.1"},
		{"ipv6 peer", "[2001:db8::5]:443", nil, "2001:db8::5"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := ClientIP(cfg, requestFrom(tc.remote, tc.headers))
			if got != netip.MustParseAddr(tc.want) {
				t.Errorf("ClientIP = %v, want %s", got, tc.want)
			}
		})
	}
}

func TestUserAgentIsTruncatedAndASCIIOnly(t *testing.T) {
	long := strings.Repeat("a", 600)
	if got := UserAgent(requestFrom("", map[string]string{"User-Agent": long})); len(got) != 512 {
		t.Errorf("len = %d, want 512", len(got))
	}
	if got := UserAgent(requestFrom("", nil)); got != "" {
		t.Errorf("missing header = %q", got)
	}
	if got := UserAgent(requestFrom("", map[string]string{"User-Agent": "Mozilla/5.0"})); got != "Mozilla/5.0" {
		t.Errorf("got %q", got)
	}
	if got := UserAgent(requestFrom("", map[string]string{"User-Agent": "café"})); got != "" {
		t.Errorf("non-ASCII header = %q, want empty as Rust to_str", got)
	}
}

func TestPeerIPDropsTheZoneAndReportsAnUnknownPeer(t *testing.T) {
	peer, ok := PeerIP(requestFrom("[fe80::1%en0]:5555", nil))
	if !ok || peer != netip.MustParseAddr("fe80::1") {
		t.Errorf("zoned peer = %v, %v", peer, ok)
	}
	for _, remote := range []string{"", "@", "not-an-address"} {
		if _, ok := PeerIP(requestFrom(remote, nil)); ok {
			t.Errorf("%q must not count as a peer", remote)
		}
	}
	cfg := trustedConfig(t, "fe80::/10")
	if got := ClientIP(cfg, requestFrom("[fe80::9%en0]:1", nil)); got != netip.MustParseAddr("fe80::9") {
		t.Errorf("ClientIP = %v, want the link-local peer, not loopback", got)
	}
}
