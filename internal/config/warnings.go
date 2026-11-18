package config

import (
	"fmt"
	"strings"
)

// WebAuthnRPWarning warns when the RP origin is localhost or disagrees with
// RDRS_PUBLIC_BASE_URL, either of which breaks passkeys.
func (c Config) WebAuthnRPWarning() (string, bool) {
	if strings.Contains(c.WebAuthnRPOrigin, "localhost") {
		return fmt.Sprintf("RDRS_WEBAUTHN_RP_ORIGIN is still '%s'. Passkeys will be rejected from any other "+
			"origin — set RDRS_WEBAUTHN_RP_ID and RDRS_WEBAUTHN_RP_ORIGIN to your deployment domain.",
			c.WebAuthnRPOrigin), true
	}
	if c.PublicBaseURL != "" &&
		strings.TrimRight(c.PublicBaseURL, "/") != strings.TrimRight(c.WebAuthnRPOrigin, "/") {
		return fmt.Sprintf("RDRS_WEBAUTHN_RP_ORIGIN ('%s') does not match RDRS_PUBLIC_BASE_URL ('%s'). Passkeys "+
			"may be rejected — align RDRS_WEBAUTHN_RP_ORIGIN with the URL users access.",
			c.WebAuthnRPOrigin, c.PublicBaseURL), true
	}
	return "", false
}

// RateLimitProxyWarning warns when rate limiting runs without trusted proxies:
// behind a proxy every visitor would share one bucket.
func (c Config) RateLimitProxyWarning() (string, bool) {
	if c.LoginRateLimitAttempts == 0 || len(c.TrustedProxyNetworks) > 0 {
		return "", false
	}
	return "RDRS_LOGIN_RATE_LIMIT_ATTEMPTS is enabled but RDRS_TRUSTED_PROXY_NETWORKS is " +
		"empty. Without a trusted-proxy list rdrs keys the credential rate limiter on " +
		"the TCP peer, so behind a reverse proxy every visitor shares one bucket and a " +
		"single abuser can lock out all users. Set RDRS_TRUSTED_PROXY_NETWORKS to the " +
		"proxy's address(es) so X-Forwarded-For is honoured.", true
}

// NoAdminGroupMappingWarning warns when group mapping is on but no active admin
// exists. It stays silent without forward-auth, where mapping is inert.
func (c Config) NoAdminGroupMappingWarning(activeAdminCount int64) (string, bool) {
	if !c.AuthProxyEnabled() || !c.GroupMappingEnabled() || activeAdminCount > 0 {
		return "", false
	}
	return fmt.Sprintf("Forward-auth group mapping is enabled but no active admin account exists. Roles "+
		"are synced from the proxy on every login, so add the first user to the "+
		"'%s' group in your identity provider (it must arrive in the %s header) and have "+
		"them sign out and sign in again.",
		c.AuthProxyAdminGroup, c.AuthProxyGroupsHeader), true
}
