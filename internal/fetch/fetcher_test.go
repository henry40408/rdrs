package fetch

import (
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"strings"
	"testing"
)

// fakeResolver answers from a table; an unknown name is an error.
type fakeResolver map[string][]netip.Addr

func (r fakeResolver) LookupNetIP(_ context.Context, _, host string) ([]netip.Addr, error) {
	if addrs, ok := r[host]; ok {
		return addrs, nil
	}
	return nil, errors.New("no such host " + host)
}

func ips(addrs ...string) []netip.Addr {
	out := make([]netip.Addr, len(addrs))
	for i, a := range addrs {
		out[i] = netip.MustParseAddr(a)
	}
	return out
}

func allowedFor(t *testing.T, policy string, resolver fakeResolver, host string) ([]netip.Addr, error) {
	t.Helper()
	return newFetcher(mustPolicy(t, policy), resolver).allowedAddrs(context.Background(), host)
}

func TestARebindingNameIsRefused(t *testing.T) {
	_, err := allowedFor(t, "", fakeResolver{"rebind.example": ips("127.0.0.1")}, "rebind.example")
	if err == nil || !strings.Contains(err.Error(), "may not reach") {
		t.Errorf("got %v", err)
	}
}

func TestAHostNamedByThePolicyKeepsItsAnswers(t *testing.T) {
	got, err := allowedFor(t, "nas.example", fakeResolver{"nas.example": ips("10.0.0.5")}, "nas.example")
	if err != nil || len(got) != 1 {
		t.Errorf("got %v, %v", got, err)
	}
}

func TestANameResolvingIntoAnAllowedNetworkIsKept(t *testing.T) {
	got, err := allowedFor(t, "10.0.0.0/8", fakeResolver{"nas.example": ips("10.0.0.5", "192.168.0.1")}, "nas.example")
	if err != nil || len(got) != 1 || got[0].String() != "10.0.0.5" {
		t.Errorf("got %v, %v", got, err)
	}
}

func TestOnlyPublicAnswersSurvive(t *testing.T) {
	got, err := allowedFor(t, "", fakeResolver{"mixed.example": ips("127.0.0.1", "93.184.216.34", "::1")}, "mixed.example")
	if err != nil || len(got) != 1 || got[0].String() != "93.184.216.34" {
		t.Errorf("got %v, %v", got, err)
	}
}

func TestAnIPLiteralIsHeldToTheSameRule(t *testing.T) {
	if _, err := allowedFor(t, "", nil, "10.0.0.1"); err == nil {
		t.Error("private literal dialled")
	}
	if _, err := allowedFor(t, "10.0.0.0/8", nil, "10.0.0.1"); err != nil {
		t.Error(err)
	}
}

func TestAResolverFailureIsADialError(t *testing.T) {
	f := newFetcher(Policy{}, fakeResolver{})
	_, err := f.dialContext(context.Background(), "tcp", "unknown.example:80")
	var opErr *net.OpError
	if !errors.As(err, &opErr) || opErr.Op != "dial" {
		t.Errorf("got %v", err)
	}
}

func get(t *testing.T, client *http.Client, target string) (string, error) {
	t.Helper()
	response, err := client.Get(target)
	if err != nil {
		return "", err
	}
	defer func() { _ = response.Body.Close() }()
	body, err := io.ReadAll(response.Body)
	return string(body), err
}

func TestADefaultFetcherCannotReachLoopback(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("x")) }))
	defer server.Close()
	if _, err := get(t, NewFetcher(Policy{}).Client(false), server.URL); err == nil {
		t.Error("loopback reached")
	}
}

func TestAnAllowedHostIsFetched(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("<rss/>")) }))
	defer server.Close()
	for _, http1Only := range []bool{false, true} {
		body, err := get(t, NewFetcher(mustPolicy(t, "127.0.0.1")).Client(http1Only), server.URL)
		if err != nil || body != "<rss/>" {
			t.Errorf("http1Only=%v: %q, %v", http1Only, body, err)
		}
	}
}

func TestARedirectPointingInwardIsRefused(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, "http://10.0.0.1/secret", http.StatusFound)
	}))
	defer server.Close()
	_, err := get(t, NewFetcher(mustPolicy(t, "127.0.0.1")).Client(false), server.URL)
	var refused *RedirectRefusedError
	if !errors.As(err, &refused) || refused.URL == nil ||
		refused.Error() != "refused to follow a redirect to http://10.0.0.1/secret" {
		t.Errorf("got %v", err)
	}
	if IsTransientError(err) {
		t.Error("a refused redirect must not be retried")
	}
}

func TestARedirectThatStaysInBoundsIsFollowed(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/start", func(w http.ResponseWriter, r *http.Request) { http.Redirect(w, r, "/final", http.StatusFound) })
	mux.HandleFunc("/final", func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("<rss/>")) })
	server := httptest.NewServer(mux)
	defer server.Close()
	body, err := get(t, NewFetcher(mustPolicy(t, "127.0.0.1")).Client(false), server.URL+"/start")
	if err != nil || body != "<rss/>" {
		t.Errorf("%q, %v", body, err)
	}
}

func TestARedirectLoopStopsAtTheCap(t *testing.T) {
	hops := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hops++
		http.Redirect(w, r, "/again", http.StatusFound)
	}))
	defer server.Close()
	_, err := get(t, NewFetcher(mustPolicy(t, "127.0.0.1")).Client(false), server.URL)
	var refused *RedirectRefusedError
	if !errors.As(err, &refused) || refused.Error() != "too many redirects (limit 5)" {
		t.Fatalf("got %v", err)
	}
	if hops != maxRedirects {
		t.Errorf("served %d requests, want %d", hops, maxRedirects)
	}
}

func TestTheTwoClientsArePools(t *testing.T) {
	f := NewFetcher(Policy{})
	if f.Client(false) == f.Client(true) || f.Client(false) != f.negotiated {
		t.Error("clients are not distinct")
	}
	if f.Validate(mustURL(t, "http://localhost/")) == nil {
		t.Error("Validate let localhost through")
	}
}
