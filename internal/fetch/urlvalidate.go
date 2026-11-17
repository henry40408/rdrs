// Package fetch is the HTTP client for every attacker-influenced fetch.
//
// A vetted URL can still redirect inward or resolve (or rebind) to a private
// address, so the guard lives in the client: every redirect hop is
// re-validated against the Policy, and every connection is dialled only to an
// address the Policy accepts, leaving no check-then-connect window.
package fetch

import (
	"errors"
	"net/netip"
	"net/url"
	"strconv"
	"strings"
)

// The reasons ValidateURL refuses a URL; the texts are shown to users.
var (
	ErrInvalidScheme = errors.New("URL scheme must be http or https")
	ErrNoHost        = errors.New("URL has no host")
	ErrBlockedHost   = errors.New("URL host is blocked")
	ErrPrivateIP     = errors.New("URL points to a private IP")
)

// ValidateURL is the shared SSRF guard for user-supplied URLs: http(s) only,
// never localhost, loopback, .local/.internal, or a private or reserved range.
func ValidateURL(u *url.URL) error {
	if u.Scheme != "http" && u.Scheme != "https" {
		return ErrInvalidScheme
	}
	host, err := hostOf(u)
	if err != nil {
		return err
	}
	if host == "localhost" || host == "127.0.0.1" || host == "::1" {
		return ErrBlockedHost
	}
	if strings.HasSuffix(host, ".local") || strings.HasSuffix(host, ".internal") {
		return ErrBlockedHost
	}
	if ip, err := netip.ParseAddr(host); err == nil && IsPrivateIP(ip) {
		return ErrPrivateIP
	}
	return nil
}

// hostOf is the URL's host as the WHATWG URL parser, which the Rust build
// used, would report it: lowercase, with a numeric IPv4 form such as
// 2130706433 or 0x7f.1 rewritten to dotted decimal. A host ending in a number
// that is not a valid IPv4 address is refused as blocked.
func hostOf(u *url.URL) (string, error) {
	host := strings.ToLower(u.Hostname())
	if host == "" {
		return "", ErrNoHost
	}
	if strings.Contains(host, ":") || !endsInNumber(host) {
		return host, nil
	}
	ip, ok := parseLooseIPv4(host)
	if !ok {
		return "", ErrBlockedHost
	}
	return ip.String(), nil
}

// endsInNumber is the WHATWG "ends in a number" check.
func endsInNumber(host string) bool {
	parts := strings.Split(host, ".")
	if parts[len(parts)-1] == "" {
		if len(parts) == 1 {
			return false
		}
		parts = parts[:len(parts)-1]
	}
	last := parts[len(parts)-1]
	if last != "" && strings.Trim(last, "0123456789") == "" {
		return true
	}
	_, isHex := hexDigits(last)
	return isHex
}

// hexDigits returns what follows a 0x or 0X prefix, and whether it was there
// and was all hex digits.
func hexDigits(part string) (string, bool) {
	digits, found := strings.CutPrefix(strings.ToLower(part), "0x")
	return digits, found && strings.Trim(digits, "0123456789abcdef") == ""
}

// parseLooseIPv4 is the WHATWG IPv4 parser: one to four parts, each decimal,
// 0x hexadecimal or 0-prefixed octal, the last one filling the remaining bytes.
func parseLooseIPv4(host string) (netip.Addr, bool) {
	parts := strings.Split(host, ".")
	if parts[len(parts)-1] == "" && len(parts) > 1 {
		parts = parts[:len(parts)-1]
	}
	if len(parts) > 4 {
		return netip.Addr{}, false
	}
	numbers := make([]uint64, len(parts))
	for i, part := range parts {
		n, ok := parseIPv4Part(part)
		if !ok {
			return netip.Addr{}, false
		}
		numbers[i] = n
	}
	last := len(numbers) - 1
	for _, n := range numbers[:last] {
		if n > 255 {
			return netip.Addr{}, false
		}
	}
	if numbers[last] >= 1<<(8*(4-uint(last))) {
		return netip.Addr{}, false
	}
	value := numbers[last]
	for i, n := range numbers[:last] {
		value += n << (8 * (3 - uint(i)))
	}
	return netip.AddrFrom4([4]byte{byte(value >> 24), byte(value >> 16), byte(value >> 8), byte(value)}), true
}

func parseIPv4Part(part string) (uint64, bool) {
	if part == "" {
		return 0, false
	}
	base, digits := 10, part
	if hex, isHex := hexDigits(part); isHex {
		base, digits = 16, hex
	} else if len(part) > 1 && part[0] == '0' {
		base, digits = 8, part[1:]
	}
	if digits == "" {
		return 0, true
	}
	n, err := strconv.ParseUint(digits, base, 64)
	return n, err == nil
}

// IsPrivateIP reports an address the server must never reach: not publicly
// routable, and wider than RFC 1918 (Tailscale's CGNAT, for one). It is listed
// by hand, as the standard library has no "is global". The Rust build
// deliberately did not block NAT64, 6to4 or fec0::/10, and neither does this.
func IsPrivateIP(ip netip.Addr) bool {
	if ip.Is4In6() {
		return isPrivateIPv4(ip.Unmap())
	}
	if ip.Is4() {
		return isPrivateIPv4(ip)
	}
	segments := ip.As16()
	first := uint16(segments[0])<<8 | uint16(segments[1])
	second := uint16(segments[2])<<8 | uint16(segments[3])
	return ip.IsLoopback() || ip.IsUnspecified() ||
		first&0xfe00 == 0xfc00 || // fc00::/7 unique-local
		first&0xffc0 == 0xfe80 || // fe80::/10 link-local
		first == 0x2001 && second == 0x0db8 || // 2001:db8::/32 documentation
		ip.IsMulticast()
}

func isPrivateIPv4(ip netip.Addr) bool {
	o := ip.As4()
	return ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalUnicast() || ip == netip.IPv4Unspecified() ||
		o[0] == 100 && o[1] >= 64 && o[1] < 128 || // 100.64.0.0/10 CGNAT
		o[0] == 198 && (o[1] == 18 || o[1] == 19) || // 198.18.0.0/15 benchmarking
		o[0] == 192 && o[1] == 0 && o[2] == 2 || // documentation ranges
		o[0] == 198 && o[1] == 51 && o[2] == 100 ||
		o[0] == 203 && o[1] == 0 && o[2] == 113 ||
		o[0] >= 224 // multicast, reserved and broadcast
}
