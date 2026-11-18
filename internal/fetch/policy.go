package fetch

import (
	"errors"
	"fmt"
	"net/netip"
	"net/url"
	"slices"
	"strings"
	"unicode"
)

// Policy is the set of hosts a deployment opted back in to, on top of
// ValidateURL's block (a LAN feed, say). Deny stays the default; the zero value
// permits nothing extra.
//
// Entries are hostnames, IPs or CIDR blocks. No DNS is resolved here, so a
// hostname entry only lifts the name-based rules.
type Policy struct {
	nets  []netip.Prefix
	hosts []string
}

// ParsePolicy reads a comma-separated allow list; empty permits nothing extra.
func ParsePolicy(raw string) (Policy, error) {
	var policy Policy
	for _, part := range strings.Split(raw, ",") {
		entry := strings.TrimSpace(part)
		if entry == "" {
			continue
		}
		if prefix, ok := parsePrefix(entry); ok {
			policy.nets = append(policy.nets, prefix)
			continue
		}
		if strings.Contains(entry, "/") || strings.IndexFunc(entry, unicode.IsSpace) >= 0 {
			return Policy{}, fmt.Errorf("invalid host, IP or CIDR in RDRS_FETCH_ALLOW_PRIVATE_HOSTS: '%s'", entry)
		}
		policy.hosts = append(policy.hosts, strings.ToLower(entry))
	}
	return policy, nil
}

// parsePrefix reads a CIDR block or a single address (a host-sized block).
func parsePrefix(entry string) (netip.Prefix, bool) {
	if prefix, err := netip.ParsePrefix(entry); err == nil && prefix.Addr().Zone() == "" {
		return prefix, true
	}
	if ip, err := netip.ParseAddr(entry); err == nil && ip.Zone() == "" {
		return netip.PrefixFrom(ip, ip.BitLen()), true
	}
	return netip.Prefix{}, false
}

// Validate is ValidateURL with the policy's exceptions applied.
//
// A bad scheme or a missing host stays fatal: no entry may turn
// file:///etc/passwd into a feed.
func (p Policy) Validate(u *url.URL) error {
	err := ValidateURL(u)
	if err == nil || errors.Is(err, ErrInvalidScheme) || errors.Is(err, ErrNoHost) {
		return err
	}
	if host, hostErr := hostOf(u); hostErr == nil && p.Allows(host) {
		return nil
	}
	return err
}

// Allows reports whether a host is allow-listed by name or network. A name
// match is all-or-nothing: the dialer then trusts whatever it resolves to.
func (p Policy) Allows(host string) bool {
	if ip, err := netip.ParseAddr(strings.Trim(host, "[]")); err == nil {
		return p.AllowsIP(ip)
	}
	return slices.Contains(p.hosts, host)
}

// AllowsIP reports whether a resolved address falls in an allow-listed
// network.
func (p Policy) AllowsIP(ip netip.Addr) bool {
	return slices.ContainsFunc(p.nets, func(prefix netip.Prefix) bool { return prefix.Contains(ip) })
}
