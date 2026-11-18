package fetch

import (
	"context"
	"crypto/tls"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"strings"
	"time"
)

// maxRedirects is shorter than the usual 10, to bound the time spent on a
// blocked target.
const maxRedirects = 5

// Resolver looks a name up; net.DefaultResolver satisfies it.
type Resolver interface {
	LookupNetIP(ctx context.Context, network, host string) ([]netip.Addr, error)
}

// Fetcher is a pair of pooled HTTP clients whose redirects and connections obey
// a Policy. It is safe for concurrent use.
type Fetcher struct {
	policy     Policy
	resolver   Resolver
	negotiated *http.Client
	http1Only  *http.Client
}

// NewFetcher builds the pooled clients for policy.
func NewFetcher(policy Policy) *Fetcher {
	return newFetcher(policy, net.DefaultResolver)
}

func newFetcher(policy Policy, resolver Resolver) *Fetcher {
	f := &Fetcher{policy: policy, resolver: resolver}
	f.negotiated = f.newClient(false)
	f.http1Only = f.newClient(true)
	return f
}

func (f *Fetcher) newClient(http1Only bool) *http.Client {
	transport := &http.Transport{
		Proxy:                 http.ProxyFromEnvironment,
		DialContext:           f.dialContext,
		ForceAttemptHTTP2:     !http1Only,
		MaxIdleConns:          100,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: time.Second,
	}
	if http1Only {
		// A non-nil, empty map switches HTTP/2 off.
		transport.TLSNextProto = map[string]func(string, *tls.Conn) http.RoundTripper{}
	}
	return &http.Client{Transport: transport, Timeout: DefaultTimeout, CheckRedirect: f.checkRedirect}
}

// Client is the client for a feed, honouring its HTTP/2 opt-out: it is a
// transport-level setting, hence two pools.
func (f *Fetcher) Client(http1Only bool) *http.Client {
	if http1Only {
		return f.http1Only
	}
	return f.negotiated
}

// Validate is an early URL check for a reportable error; the clients still
// re-check every hop and every connection.
func (f *Fetcher) Validate(u *url.URL) error {
	return f.policy.Validate(u)
}

// RedirectRefusedError is why a redirect was not followed.
type RedirectRefusedError struct {
	// URL is the refused target; nil when the cap was hit.
	URL *url.URL
}

func (e *RedirectRefusedError) Error() string {
	if e.URL == nil {
		return fmt.Sprintf("too many redirects (limit %d)", maxRedirects)
	}
	return "refused to follow a redirect to " + e.URL.String()
}

func (f *Fetcher) checkRedirect(next *http.Request, via []*http.Request) error {
	if len(via) >= maxRedirects {
		return &RedirectRefusedError{}
	}
	if f.policy.Validate(next.URL) != nil {
		return &RedirectRefusedError{URL: next.URL}
	}
	return nil
}

// dialContext connects only to addresses the policy accepts. It checks the
// address it is about to use, so a name that rebinds between validation and
// connection cannot reach an internal host, and an IP literal is held to the
// same rule.
func (f *Fetcher) dialContext(ctx context.Context, network, address string) (net.Conn, error) {
	host, port, err := net.SplitHostPort(address)
	if err != nil {
		return nil, err
	}
	addrs, err := f.allowedAddrs(ctx, host)
	if err != nil {
		return nil, &net.OpError{Op: "dial", Net: network, Err: err}
	}
	dialer := net.Dialer{Timeout: DefaultTimeout, KeepAlive: 30 * time.Second}
	var lastErr error
	for _, addr := range addrs {
		conn, err := dialer.DialContext(ctx, network, net.JoinHostPort(addr.String(), port))
		if err == nil {
			return conn, nil
		}
		lastErr = err
	}
	return nil, lastErr
}

// allowedAddrs resolves host and drops every answer the policy would refuse.
func (f *Fetcher) allowedAddrs(ctx context.Context, host string) ([]netip.Addr, error) {
	host = strings.ToLower(host)
	answers, err := f.lookup(ctx, host)
	if err != nil {
		return nil, err
	}
	// An allow-listed host is trusted wherever it points.
	hostAllowed := f.policy.Allows(host)
	var kept []netip.Addr
	for _, ip := range answers {
		if hostAllowed || !IsPrivateIP(ip) || f.policy.AllowsIP(ip) {
			kept = append(kept, ip)
		}
	}
	if len(kept) == 0 {
		return nil, errors.New(host + " resolves only to addresses this server may not reach")
	}
	return kept, nil
}

func (f *Fetcher) lookup(ctx context.Context, host string) ([]netip.Addr, error) {
	if ip, err := netip.ParseAddr(host); err == nil {
		return []netip.Addr{ip}, nil
	}
	// A numeric form such as 1572395042 was validated as the IP it means, and
	// the pure-Go resolver would not understand it.
	if normalized, err := hostOf(&url.URL{Host: host}); err == nil && normalized != host {
		if ip, err := netip.ParseAddr(normalized); err == nil {
			return []netip.Addr{ip}, nil
		}
	}
	return f.resolver.LookupNetIP(ctx, "ip", host)
}
