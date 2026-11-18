package fetch

import (
	"errors"
	"testing"
)

func mustPolicy(t *testing.T, raw string) Policy {
	t.Helper()
	policy, err := ParsePolicy(raw)
	if err != nil {
		t.Fatal(err)
	}
	return policy
}

func TestEmptyPolicyAllowsNothingExtra(t *testing.T) {
	var policy Policy
	if policy.Validate(mustURL(t, "http://127.0.0.1:8080/rss.xml")) == nil {
		t.Error("loopback allowed")
	}
	if err := policy.Validate(mustURL(t, "https://example.com/rss.xml")); err != nil {
		t.Error(err)
	}
}

func TestPolicyAllowsListedIPOnly(t *testing.T) {
	policy := mustPolicy(t, "127.0.0.1")
	if err := policy.Validate(mustURL(t, "http://127.0.0.1:8080/rss.xml")); err != nil {
		t.Error(err)
	}
	if policy.Validate(mustURL(t, "http://127.0.0.2/rss.xml")) == nil {
		t.Error("a neighbouring loopback address was allowed")
	}
}

func TestPolicyAllowsListedCIDRsHostnamesAndIPv6(t *testing.T) {
	cases := []struct {
		list            string
		allowed, refuse []string
	}{
		{"192.168.0.0/16, 10.0.0.0/8", []string{"http://192.168.1.5/feed", "http://10.1.2.3/feed"}, []string{"http://172.16.0.1/feed"}},
		{"nas.local,localhost", []string{"http://nas.local/feed.xml", "http://localhost:3000/feed.xml", "http://NAS.local/x"}, []string{"http://other.local/feed.xml"}},
		{"::1", []string{"http://[::1]:8080/feed"}, nil},
		{"  ,, 127.0.0.1 ,", []string{"http://127.0.0.1/feed"}, nil},
		{"127.0.0.1", []string{"http://2130706433/feed"}, nil},
		{"NAS.Local", []string{"http://nas.local/feed"}, nil},
	}
	for _, tc := range cases {
		policy := mustPolicy(t, tc.list)
		for _, raw := range tc.allowed {
			if err := policy.Validate(mustURL(t, raw)); err != nil {
				t.Errorf("%q: %s: %v", tc.list, raw, err)
			}
		}
		for _, raw := range tc.refuse {
			if policy.Validate(mustURL(t, raw)) == nil {
				t.Errorf("%q: %s allowed", tc.list, raw)
			}
		}
	}
}

func TestPolicyNeverLiftsSchemeOrHostRules(t *testing.T) {
	policy := mustPolicy(t, "127.0.0.1,localhost")
	for _, raw := range []string{"file:///etc/passwd", "ftp://localhost/x"} {
		if err := policy.Validate(mustURL(t, raw)); !errors.Is(err, ErrInvalidScheme) {
			t.Errorf("%s: %v", raw, err)
		}
	}
	if err := policy.Validate(mustURL(t, "http:///x")); !errors.Is(err, ErrNoHost) {
		t.Errorf("no host: %v", err)
	}
}

func TestParsePolicyRejectsMalformedEntries(t *testing.T) {
	for _, raw := range []string{"192.168.0.0/99", "not a host"} {
		if _, err := ParsePolicy(raw); err == nil {
			t.Errorf("%q accepted", raw)
		}
	}
	_, err := ParsePolicy("ok.example, a/b")
	want := "invalid host, IP or CIDR in RDRS_FETCH_ALLOW_PRIVATE_HOSTS: 'a/b'"
	if err == nil || err.Error() != want {
		t.Errorf("got %v", err)
	}
}
