// Package config reads the process environment into a validated Config.
package config

import (
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"net/netip"
	"strconv"
	"strings"

	"github.com/henry40408/rdrs/internal/version"
)

// MinSecretLen is the shortest RDRS_SECRET, in bytes, that is accepted.
const MinSecretLen = 16

// Login rate limit defaults, applied when the variables are unset.
const (
	DefaultLoginRateLimitAttempts   = 5
	DefaultLoginRateLimitWindowSecs = 60
)

const defaultHSTSMaxAge = 31_536_000

// DefaultUserAgent is the User-Agent for outbound HTTP requests.
func DefaultUserAgent() string {
	return "RDRS/" + version.Version() + " (RSS Reader; +https://github.com/henry40408/rdrs)"
}

// CustomUserAgentSuggestions are the browser User-Agent strings the feed-edit
// form suggests for servers that reject non-browsers. The default is excluded:
// an empty field already means it.
var CustomUserAgentSuggestions = []string{
	"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
	"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Safari/605.1.15",
	"Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
}

// Backend is the database engine selected by DATABASE_URL.
type Backend int

// The supported database engines.
const (
	SQLite Backend = iota
	Postgres
)

// Config is the validated process configuration.
type Config struct {
	DatabaseURL     string
	ServerBind      netip.AddrPort
	MultiUser       bool
	Secret          []byte
	SecretGenerated bool
	UserAgent       string
	// FetchAllowPrivate is the raw RDRS_FETCH_ALLOW_PRIVATE_HOSTS value. The
	// fetch package parses it (and rejects a malformed entry) when the fetcher
	// is built.
	FetchAllowPrivate string
	WebAuthnRPID      string
	WebAuthnRPOrigin  string
	WebAuthnRPName    string
	PublicBaseURL     string
	CookieSecure      bool

	AuthProxyHeader       string
	TrustedProxyNetworks  []netip.Prefix
	AuthProxyUserCreation bool
	DisableLocalAuth      bool
	AuthProxyGroupsHeader string
	AuthProxyAdminGroup   string
	AuthProxyLogoutURL    string

	// LoginRateLimitAttempts of 0 disables the limiter.
	LoginRateLimitAttempts   uint32
	LoginRateLimitWindowSecs uint64

	HSTS                  bool
	HSTSMaxAge            uint64
	HSTSIncludeSubdomains bool
}

// Load builds the Config from a variable lookup, so tests need not touch the
// process environment.
func Load(lookupEnv func(string) (string, bool)) (Config, error) {
	env := environment{lookup: lookupEnv}
	secret, generated := loadSecret(env.raw("RDRS_SECRET"))

	bind, err := parseServerBind(env.nonblank("RDRS_SERVER_BIND"))
	if err != nil {
		return Config{}, err
	}
	networks, err := ParseTrustedNetworks(env.nonblank("RDRS_TRUSTED_PROXY_NETWORKS"))
	if err != nil {
		return Config{}, err
	}
	publicBaseURL := env.nonblank("RDRS_PUBLIC_BASE_URL")
	cookieSecure, err := parseCookieSecure(env.raw("RDRS_COOKIE_SECURE"), publicBaseURL)
	if err != nil {
		return Config{}, err
	}
	attempts, window, err := parseLoginRateLimit(env)
	if err != nil {
		return Config{}, err
	}
	hsts, maxAge, includeSubdomains, err := parseHSTSSettings(env, publicBaseURL)
	if err != nil {
		return Config{}, err
	}

	return Config{
		DatabaseURL:              orDefault(env.nonblank("DATABASE_URL"), "rdrs.sqlite3"),
		ServerBind:               bind,
		MultiUser:                env.flag("RDRS_MULTI_USER_ENABLED"),
		Secret:                   secret,
		SecretGenerated:          generated,
		UserAgent:                orDefault(env.nonblank("RDRS_USER_AGENT"), DefaultUserAgent()),
		FetchAllowPrivate:        env.nonblank("RDRS_FETCH_ALLOW_PRIVATE_HOSTS"),
		WebAuthnRPID:             orDefault(env.nonblank("RDRS_WEBAUTHN_RP_ID"), "localhost"),
		WebAuthnRPOrigin:         orDefault(env.nonblank("RDRS_WEBAUTHN_RP_ORIGIN"), "http://localhost:"+strconv.Itoa(int(bind.Port()))),
		WebAuthnRPName:           orDefault(env.nonblank("RDRS_WEBAUTHN_RP_NAME"), "rdrs"),
		PublicBaseURL:            publicBaseURL,
		CookieSecure:             cookieSecure,
		AuthProxyHeader:          env.nonblank("RDRS_AUTH_PROXY_HEADER"),
		TrustedProxyNetworks:     networks,
		AuthProxyUserCreation:    env.flag("RDRS_AUTH_PROXY_USER_CREATION"),
		DisableLocalAuth:         env.flag("RDRS_DISABLE_LOCAL_AUTH"),
		AuthProxyGroupsHeader:    env.nonblank("RDRS_AUTH_PROXY_GROUPS_HEADER"),
		AuthProxyAdminGroup:      env.nonblank("RDRS_AUTH_PROXY_ADMIN_GROUP"),
		AuthProxyLogoutURL:       env.nonblank("RDRS_AUTH_PROXY_LOGOUT_URL"),
		LoginRateLimitAttempts:   attempts,
		LoginRateLimitWindowSecs: window,
		HSTS:                     hsts,
		HSTSMaxAge:               maxAge,
		HSTSIncludeSubdomains:    includeSubdomains,
	}, nil
}

func orDefault(value, fallback string) string {
	if value == "" {
		return fallback
	}
	return value
}

// environment wraps a variable lookup with the trimming rules shared by every
// setting.
type environment struct {
	lookup func(string) (string, bool)
}

// raw returns the value untrimmed and whether it was set at all.
func (e environment) raw(key string) *string {
	v, ok := e.lookup(key)
	if !ok {
		return nil
	}
	return &v
}

// nonblank returns the trimmed value; empty means unset ("FOO=" is "not configured").
func (e environment) nonblank(key string) string {
	v, _ := e.lookup(key)
	return strings.TrimSpace(v)
}

// flag is a lenient boolean for default-off settings: only "true" (any case) or
// "1" turns it on.
func (e environment) flag(key string) bool {
	v := e.nonblank(key)
	return strings.EqualFold(v, "true") || v == "1"
}

// loadSecret resolves RDRS_SECRET (base64 or raw, at least MinSecretLen bytes).
// A shorter or missing value is replaced by a random key that dies with the
// process. It is not trimmed: trimming would silently rotate a key that
// carries whitespace.
func loadSecret(raw *string) (secret []byte, generated bool) {
	if raw != nil {
		if decoded, err := base64.StdEncoding.Strict().DecodeString(*raw); err == nil && len(decoded) >= MinSecretLen {
			return decoded, false
		}
		if len(*raw) >= MinSecretLen {
			return []byte(*raw), false
		}
	}
	secret = make([]byte, 32)
	// crypto/rand.Read never returns an error on supported platforms.
	_, _ = rand.Read(secret)
	return secret, true
}

// ServiceTokenKey is the key that encrypts service tokens at rest. It is empty
// when the secret is generated, because encrypting with it would lose the
// tokens on restart.
func (c Config) ServiceTokenKey() ([]byte, bool) {
	if c.SecretGenerated {
		return nil, false
	}
	return c.Secret, true
}

func parseServerBind(raw string) (netip.AddrPort, error) {
	if raw == "" {
		return netip.AddrPortFrom(netip.AddrFrom4([4]byte{127, 0, 0, 1}), 8080), nil
	}
	addr, err := netip.ParseAddrPort(raw)
	if err != nil {
		return netip.AddrPort{}, fmt.Errorf("invalid RDRS_SERVER_BIND '%s': %w", raw, err)
	}
	return addr, nil
}

// ParseTrustedNetworks parses comma-separated CIDRs or bare IPs (a bare IP
// becomes /32 or /128); blanks are ignored.
func ParseTrustedNetworks(raw string) ([]netip.Prefix, error) {
	var nets []netip.Prefix
	for _, part := range strings.Split(raw, ",") {
		s := strings.TrimSpace(part)
		if s == "" {
			continue
		}
		if prefix, err := netip.ParsePrefix(s); err == nil {
			nets = append(nets, prefix)
			continue
		}
		addr, err := netip.ParseAddr(s)
		if err != nil {
			return nil, fmt.Errorf("invalid CIDR or IP in RDRS_TRUSTED_PROXY_NETWORKS: '%s'", s)
		}
		nets = append(nets, netip.PrefixFrom(addr, addr.BitLen()))
	}
	return nets, nil
}

func isHTTPS(publicBaseURL string) bool {
	return strings.HasPrefix(strings.ToLower(strings.TrimLeft(publicBaseURL, " \t\r\n")), "https://")
}

// parseCookieSecure decides whether the session cookie gets Secure: explicit
// RDRS_COOKIE_SECURE, else derived from an https RDRS_PUBLIC_BASE_URL. An
// unrecognized value is an error, because reading a typo as "off" would strip
// Secure from an HTTPS deployment.
func parseCookieSecure(raw *string, publicBaseURL string) (bool, error) {
	return parseBoolDerived(raw, "RDRS_COOKIE_SECURE", isHTTPS(publicBaseURL))
}

// parseHSTS decides whether to send HSTS, derived like parseCookieSecure. HSTS
// is sticky, so only true/false/1/0 are accepted.
func parseHSTS(raw *string, publicBaseURL string) (bool, error) {
	return parseBoolDerived(raw, "RDRS_HSTS", isHTTPS(publicBaseURL))
}

// parseBoolDerived is a strict boolean: blank gives derived, true/1/false/0
// (any case) are accepted, anything else is an error.
func parseBoolDerived(raw *string, name string, derived bool) (bool, error) {
	if raw == nil {
		return derived, nil
	}
	v := strings.TrimSpace(*raw)
	switch {
	case v == "":
		return derived, nil
	case strings.EqualFold(v, "true") || v == "1":
		return true, nil
	case strings.EqualFold(v, "false") || v == "0":
		return false, nil
	}
	return false, fmt.Errorf("invalid %s '%s': expected one of true, false, 1, 0", name, v)
}

func parseHSTSSettings(env environment, publicBaseURL string) (hsts bool, maxAge uint64, includeSubdomains bool, err error) {
	if hsts, err = parseHSTS(env.raw("RDRS_HSTS"), publicBaseURL); err != nil {
		return
	}
	maxAge = defaultHSTSMaxAge
	if v := env.nonblank("RDRS_HSTS_MAX_AGE"); v != "" {
		if maxAge, err = parseUint(v, 64); err != nil {
			err = fmt.Errorf("invalid RDRS_HSTS_MAX_AGE '%s': %w", v, err)
			return
		}
	}
	includeSubdomains, err = parseBoolDerived(env.raw("RDRS_HSTS_INCLUDE_SUBDOMAINS"), "RDRS_HSTS_INCLUDE_SUBDOMAINS", true)
	return
}

// parseUint accepts a leading "+" like Rust's integer parser did.
func parseUint(s string, bits int) (uint64, error) {
	return strconv.ParseUint(strings.TrimPrefix(s, "+"), 10, bits)
}

func parseLoginRateLimit(env environment) (attempts uint32, window uint64, err error) {
	attempts = DefaultLoginRateLimitAttempts
	if v := env.nonblank("RDRS_LOGIN_RATE_LIMIT_ATTEMPTS"); v != "" {
		n, perr := parseUint(v, 32)
		if perr != nil {
			return 0, 0, fmt.Errorf("invalid RDRS_LOGIN_RATE_LIMIT_ATTEMPTS '%s': %w", v, perr)
		}
		attempts = uint32(n)
	}
	window = DefaultLoginRateLimitWindowSecs
	v := env.nonblank("RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS")
	if v == "" {
		return attempts, window, nil
	}
	if window, err = parseUint(v, 64); err != nil {
		return 0, 0, fmt.Errorf("invalid RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS '%s': %w", v, err)
	}
	if window == 0 {
		return 0, 0, errors.New("invalid RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS '0': the window must be at " + //nolint:staticcheck // ST1005: user-facing text kept verbatim from the Rust build
			"least 1 second; a zero-length window elapses instantly and silently " +
			"disables throttling. Use RDRS_LOGIN_RATE_LIMIT_ATTEMPTS=0 to disable the " +
			"limiter deliberately.")
	}
	return attempts, window, nil
}

// ClassifyBackend maps postgres:// and postgresql:// URLs to Postgres and
// everything else (a path, or sqlite://) to SQLite.
func ClassifyBackend(databaseURL string) Backend {
	lower := strings.ToLower(strings.TrimLeft(databaseURL, " \t\r\n"))
	if strings.HasPrefix(lower, "postgres://") || strings.HasPrefix(lower, "postgresql://") {
		return Postgres
	}
	return SQLite
}

// Backend is the database engine selected by DatabaseURL.
func (c Config) Backend() Backend { return ClassifyBackend(c.DatabaseURL) }

// RedactDatabaseURL replaces the password in a URL with *** for display.
func RedactDatabaseURL(databaseURL string) string {
	scheme, rest, ok := strings.Cut(databaseURL, "://")
	if !ok {
		return databaseURL
	}
	authorityEnd := strings.IndexAny(rest, "/?#")
	if authorityEnd < 0 {
		authorityEnd = len(rest)
	}
	authority, tail := rest[:authorityEnd], rest[authorityEnd:]
	// Split at the last '@' so an unencoded '@' in the password is not leaked.
	at := strings.LastIndex(authority, "@")
	if at < 0 {
		return databaseURL
	}
	userinfo, host := authority[:at], authority[at+1:]
	user, _, hasPassword := strings.Cut(userinfo, ":")
	if !hasPassword {
		return databaseURL
	}
	return scheme + "://" + user + ":***@" + host + tail
}

// AuthProxyEnabled reports whether forward-auth (trusted-header) login is on.
func (c Config) AuthProxyEnabled() bool { return c.AuthProxyHeader != "" }

// GroupMappingEnabled reports whether group to role mapping is active.
func (c Config) GroupMappingEnabled() bool {
	return c.AuthProxyGroupsHeader != "" && c.AuthProxyAdminGroup != ""
}

// IsTrustedPeer reports whether ip (the TCP peer) is inside a trusted proxy network.
func (c Config) IsTrustedPeer(ip netip.Addr) bool {
	ip = ip.Unmap()
	for _, network := range c.TrustedProxyNetworks {
		if network.Contains(ip) {
			return true
		}
	}
	return false
}

// Validate checks cross-field invariants at startup and returns the first problem.
func (c Config) Validate() error {
	if c.AuthProxyEnabled() && len(c.TrustedProxyNetworks) == 0 {
		return errors.New("RDRS_AUTH_PROXY_HEADER is set but RDRS_TRUSTED_PROXY_NETWORKS is empty. " + //nolint:staticcheck // ST1005: user-facing text kept verbatim from the Rust build
			"Refusing to trust an identity header without a trusted-source check.")
	}
	if c.DisableLocalAuth && !c.AuthProxyEnabled() {
		return errors.New("RDRS_DISABLE_LOCAL_AUTH is set but RDRS_AUTH_PROXY_HEADER is not configured. " + //nolint:staticcheck // ST1005: user-facing text kept verbatim from the Rust build
			"This would leave no way to log in via the browser.")
	}
	return nil
}

// CanSetup reports whether first-run setup is open: only with zero accounts.
func (c Config) CanSetup(userCount int64) bool { return userCount == 0 }

// CanCreateAccount reports whether an admin may create another account.
func (c Config) CanCreateAccount(userCount int64) bool { return userCount == 0 || c.MultiUser }

// HSTSHeader returns the Strict-Transport-Security value, or false when HSTS is
// off. It never contains preload: the preload list is effectively irreversible.
func (c Config) HSTSHeader() (string, bool) {
	if !c.HSTS {
		return "", false
	}
	value := "max-age=" + strconv.FormatUint(c.HSTSMaxAge, 10)
	if c.HSTSIncludeSubdomains {
		value += "; includeSubDomains"
	}
	return value, true
}
