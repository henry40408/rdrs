// Package auth holds the request-time authentication core: the session and
// CSRF cookies, the sliding-session rules, the credential rate limiter and the
// client address a request really came from.
package auth

import (
	"net"
	"net/http"
	"net/netip"
	"strings"

	"github.com/henry40408/rdrs/internal/config"
)

// maxUserAgentChars bounds the User-Agent stored with a session.
const maxUserAgentChars = 512

// PeerIP is the address of the TCP peer. A request without a parsable peer
// (tests calling the handler directly) counts as loopback.
func PeerIP(r *http.Request) netip.Addr {
	if peer, ok := parsePeer(r.RemoteAddr); ok {
		return peer
	}
	return netip.AddrFrom4([4]byte{127, 0, 0, 1})
}

func parsePeer(remoteAddr string) (netip.Addr, bool) {
	host, _, err := net.SplitHostPort(remoteAddr)
	if err != nil {
		host = remoteAddr
	}
	peer, err := netip.ParseAddr(host)
	if err != nil || peer.Zone() != "" {
		return netip.Addr{}, false
	}
	return peer, true
}

// ClientIP is the originating client address. Forwarding headers are honoured
// only from a trusted peer, and X-Forwarded-For is read right to left because
// the left-most entry is forgeable.
func ClientIP(cfg config.Config, r *http.Request) netip.Addr {
	peer, ok := parsePeer(r.RemoteAddr)
	if !ok {
		return netip.AddrFrom4([4]byte{127, 0, 0, 1})
	}
	if !cfg.IsTrustedPeer(peer) {
		return peer
	}
	if ip, found := untrustedForwardedFor(cfg, headerText(r, "X-Forwarded-For")); found {
		return ip
	}
	if ip, ok := parseHeaderIP(headerText(r, "X-Real-IP")); ok {
		return ip
	}
	return peer
}

// untrustedForwardedFor returns the right-most hop that is not a trusted
// proxy. A malformed hop breaks the trust chain, so the scan stops there.
func untrustedForwardedFor(cfg config.Config, forwardedFor string) (netip.Addr, bool) {
	if forwardedFor == "" {
		return netip.Addr{}, false
	}
	hops := strings.Split(forwardedFor, ",")
	for i := len(hops) - 1; i >= 0; i-- {
		ip, ok := parseHeaderIP(hops[i])
		if !ok {
			return netip.Addr{}, false
		}
		if !cfg.IsTrustedPeer(ip) {
			return ip, true
		}
	}
	return netip.Addr{}, false
}

func parseHeaderIP(text string) (netip.Addr, bool) {
	ip, err := netip.ParseAddr(strings.TrimSpace(text))
	if err != nil || ip.Zone() != "" {
		return netip.Addr{}, false
	}
	return ip, true
}

// headerText is a header value as the Rust side read it: only visible ASCII
// (and tab) counts, anything else is as good as absent.
func headerText(r *http.Request, name string) string {
	value := r.Header.Get(name)
	for i := 0; i < len(value); i++ {
		if c := value[i]; (c < 0x20 && c != '\t') || c > 0x7e {
			return ""
		}
	}
	return value
}

// UserAgent is the request's User-Agent cut to the stored length.
func UserAgent(r *http.Request) string {
	text := headerText(r, "User-Agent")
	if len(text) <= maxUserAgentChars {
		return text
	}
	return text[:maxUserAgentChars]
}
