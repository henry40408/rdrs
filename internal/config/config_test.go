package config

import (
	"encoding/base64"
	"net/netip"
	"strings"
	"testing"
)

func lookup(vars map[string]string) func(string) (string, bool) {
	return func(key string) (string, bool) {
		v, ok := vars[key]
		return v, ok
	}
}

func fromVars(t *testing.T, vars map[string]string) Config {
	t.Helper()
	cfg, err := Load(lookup(vars))
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	return cfg
}

func loadErr(t *testing.T, vars map[string]string) string {
	t.Helper()
	_, err := Load(lookup(vars))
	if err == nil {
		t.Fatalf("Load(%v) succeeded, want error", vars)
	}
	return err.Error()
}

func baseConfig() Config {
	return Config{
		DatabaseURL:              "test.db",
		ServerBind:               netip.MustParseAddrPort("127.0.0.1:8080"),
		Secret:                   make([]byte, 32),
		WebAuthnRPID:             "localhost",
		WebAuthnRPOrigin:         "http://localhost:8080",
		WebAuthnRPName:           "rdrs",
		LoginRateLimitAttempts:   DefaultLoginRateLimitAttempts,
		LoginRateLimitWindowSecs: DefaultLoginRateLimitWindowSecs,
		HSTSMaxAge:               31_536_000,
		HSTSIncludeSubdomains:    true,
	}
}

func TestServerBind(t *testing.T) {
	def := netip.MustParseAddrPort("127.0.0.1:8080")
	for _, raw := range []string{"", "  "} {
		if got := fromVars(t, map[string]string{"RDRS_SERVER_BIND": raw}).ServerBind; got != def {
			t.Errorf("%q: got %v", raw, got)
		}
	}
	if got := fromVars(t, map[string]string{"RDRS_SERVER_BIND": "127.0.0.1:9000"}).ServerBind; got.Port() != 9000 {
		t.Errorf("got %v", got)
	}
	for _, bad := range []string{"not-an-addr", "127.0.0.1"} {
		if msg := loadErr(t, map[string]string{"RDRS_SERVER_BIND": bad}); !strings.Contains(msg, "invalid RDRS_SERVER_BIND") {
			t.Errorf("%q: %s", bad, msg)
		}
	}
}

func TestServerBindDrivesRPOrigin(t *testing.T) {
	cfg := fromVars(t, map[string]string{"RDRS_SERVER_BIND": "127.0.0.1:9137"})
	if cfg.WebAuthnRPOrigin != "http://localhost:9137" {
		t.Fatalf("got %q", cfg.WebAuthnRPOrigin)
	}
}

func TestDefaultsWithNothingConfigured(t *testing.T) {
	cfg := fromVars(t, nil)
	if cfg.DatabaseURL != "rdrs.sqlite3" || cfg.UserAgent != DefaultUserAgent() ||
		cfg.WebAuthnRPID != "localhost" || cfg.WebAuthnRPName != "rdrs" {
		t.Fatalf("defaults: %+v", cfg)
	}
	if cfg.MultiUser || cfg.CookieSecure || cfg.PublicBaseURL != "" || cfg.AuthProxyLogoutURL != "" ||
		cfg.AuthProxyEnabled() || !cfg.SecretGenerated {
		t.Fatalf("defaults: %+v", cfg)
	}
}

func TestBlankValuesCountAsUnset(t *testing.T) {
	cfg := fromVars(t, map[string]string{
		"DATABASE_URL":               "  ",
		"RDRS_USER_AGENT":            "",
		"RDRS_PUBLIC_BASE_URL":       "   ",
		"RDRS_AUTH_PROXY_LOGOUT_URL": " ",
		"RDRS_AUTH_PROXY_HEADER":     "  ",
	})
	if cfg.DatabaseURL != "rdrs.sqlite3" || cfg.UserAgent != DefaultUserAgent() ||
		cfg.PublicBaseURL != "" || cfg.AuthProxyLogoutURL != "" || cfg.AuthProxyEnabled() {
		t.Fatalf("%+v", cfg)
	}
}

func TestValuesAreTrimmed(t *testing.T) {
	cfg := fromVars(t, map[string]string{
		"DATABASE_URL":               "  postgres://u:p@db/rdrs  ",
		"RDRS_AUTH_PROXY_HEADER":     " Remote-User ",
		"RDRS_AUTH_PROXY_LOGOUT_URL": " https://auth.example.com/logout ",
	})
	if cfg.DatabaseURL != "postgres://u:p@db/rdrs" || cfg.Backend() != Postgres ||
		cfg.AuthProxyHeader != "Remote-User" || cfg.AuthProxyLogoutURL != "https://auth.example.com/logout" {
		t.Fatalf("%+v", cfg)
	}
}

func TestBooleanFlags(t *testing.T) {
	for _, raw := range []string{"true", "TRUE", "True", "1", " true "} {
		if !fromVars(t, map[string]string{"RDRS_MULTI_USER_ENABLED": raw}).MultiUser {
			t.Errorf("%q should enable", raw)
		}
	}
	for _, raw := range []string{"false", "0", "yes", "on", "", "  "} {
		if fromVars(t, map[string]string{"RDRS_MULTI_USER_ENABLED": raw}).MultiUser {
			t.Errorf("%q should not enable", raw)
		}
	}
}

func TestSecretSources(t *testing.T) {
	encoded := base64.StdEncoding.EncodeToString(make([]byte, 32))
	secret, generated := loadSecret(&encoded)
	if len(secret) != 32 || generated || secret[0] != 0 {
		t.Fatalf("base64: %v %v", secret, generated)
	}
	raw := strings.Repeat("!", 16)
	secret, generated = loadSecret(&raw)
	if string(secret) != raw || generated {
		t.Fatalf("raw: %q %v", secret, generated)
	}
	short := "short"
	for _, in := range []*string{&short, nil} {
		secret, generated = loadSecret(in)
		if len(secret) != 32 || !generated {
			t.Fatalf("generated: %v %v", secret, generated)
		}
	}
}

func TestServiceTokenKeyIsAbsentForGeneratedSecret(t *testing.T) {
	if _, ok := fromVars(t, nil).ServiceTokenKey(); ok {
		t.Fatal("generated secret must not be a service token key")
	}
	cfg := fromVars(t, map[string]string{"RDRS_SECRET": strings.Repeat("k", 20)})
	if key, ok := cfg.ServiceTokenKey(); !ok || string(key) != strings.Repeat("k", 20) {
		t.Fatalf("%q %v", key, ok)
	}
}

func strPtr(s string) *string { return &s }

func TestParseCookieSecureAndHSTSDerive(t *testing.T) {
	for name, parse := range map[string]func(*string, string) (bool, error){
		"cookie": parseCookieSecure,
		"hsts":   parseHSTS,
	} {
		derive := func(raw *string, base string) bool {
			got, err := parse(raw, base)
			if err != nil {
				t.Fatalf("%s: %v", name, err)
			}
			return got
		}
		if !derive(nil, "https://rdrs.example.com") || !derive(nil, "  HTTPS://x") {
			t.Errorf("%s: https must derive true", name)
		}
		if derive(nil, "http://localhost:8080") || derive(nil, "") || derive(nil, "http://https.example.com") {
			t.Errorf("%s: non-https must derive false", name)
		}
		if !derive(strPtr("true"), "http://localhost") || !derive(strPtr("1"), "") ||
			!derive(strPtr("TRUE"), "") || !derive(strPtr(" true "), "") {
			t.Errorf("%s: explicit true", name)
		}
		if derive(strPtr("false"), "https://rdrs.example.com") || derive(strPtr("0"), "https://rdrs.example.com") {
			t.Errorf("%s: explicit false", name)
		}
		if !derive(strPtr(""), "https://rdrs.example.com") || !derive(strPtr("   "), "https://rdrs.example.com") {
			t.Errorf("%s: blank is unset, not off", name)
		}
		for _, bad := range []string{"yes", "on", "off", "2", "tru"} {
			_, err := parse(strPtr(bad), "https://rdrs.example.com")
			if err == nil || !strings.Contains(err.Error(), bad) {
				t.Errorf("%s: %q: %v", name, bad, err)
			}
		}
	}
}

func TestHSTSMaxAge(t *testing.T) {
	base := map[string]string{"RDRS_PUBLIC_BASE_URL": "https://rdrs.example.com"}
	if got := fromVars(t, base).HSTSMaxAge; got != 31_536_000 {
		t.Fatalf("default %d", got)
	}
	base["RDRS_HSTS_MAX_AGE"] = "3600"
	if got := fromVars(t, base).HSTSMaxAge; got != 3600 {
		t.Fatalf("override %d", got)
	}
	// 0 is valid: it is the recovery path for a mis-set declaration.
	base["RDRS_HSTS_MAX_AGE"] = "0"
	if got := fromVars(t, base).HSTSMaxAge; got != 0 {
		t.Fatalf("zero %d", got)
	}
	msg := loadErr(t, map[string]string{"RDRS_HSTS_MAX_AGE": "soon"})
	if !strings.Contains(msg, "RDRS_HSTS_MAX_AGE") || !strings.Contains(msg, "soon") {
		t.Fatal(msg)
	}
}

func TestHSTSIncludeSubdomains(t *testing.T) {
	if !fromVars(t, nil).HSTSIncludeSubdomains {
		t.Fatal("default on")
	}
	if fromVars(t, map[string]string{"RDRS_HSTS_INCLUDE_SUBDOMAINS": "false"}).HSTSIncludeSubdomains {
		t.Fatal("override off")
	}
	if msg := loadErr(t, map[string]string{"RDRS_HSTS_INCLUDE_SUBDOMAINS": "sometimes"}); !strings.Contains(msg, "RDRS_HSTS_INCLUDE_SUBDOMAINS") {
		t.Fatal(msg)
	}
}

func TestHSTSHeader(t *testing.T) {
	if _, ok := baseConfig().HSTSHeader(); ok {
		t.Fatal("off by default")
	}
	cfg := baseConfig()
	cfg.HSTS = true
	if got, _ := cfg.HSTSHeader(); got != "max-age=31536000; includeSubDomains" || strings.Contains(got, "preload") {
		t.Fatalf("%q", got)
	}
	cfg.HSTSMaxAge = 100
	cfg.HSTSIncludeSubdomains = false
	if got, _ := cfg.HSTSHeader(); got != "max-age=100" {
		t.Fatalf("%q", got)
	}
}

func TestClassifyBackend(t *testing.T) {
	for in, want := range map[string]Backend{
		"rdrs.sqlite3":                      SQLite,
		"/var/lib/rdrs/data.db":             SQLite,
		"sqlite://rdrs.sqlite3":             SQLite,
		"postgres://user:pw@localhost/rdrs": Postgres,
		"postgresql://user@db:5432/rdrs":    Postgres,
		"  POSTGRES://x":                    Postgres,
	} {
		if got := ClassifyBackend(in); got != want {
			t.Errorf("%q: got %v", in, got)
		}
	}
}

func TestRedactDatabaseURL(t *testing.T) {
	for in, want := range map[string]string{
		"postgres://user:s3cr3t@db.internal:5432/rdrs": "postgres://user:***@db.internal:5432/rdrs",
		"postgres://u:p@host/rdrs?sslmode=require":     "postgres://u:***@host/rdrs?sslmode=require",
		"postgres://user:p@ss@host/rdrs":               "postgres://user:***@host/rdrs",
		"postgres://user@host/rdrs":                    "postgres://user@host/rdrs",
		"rdrs.sqlite3":                                 "rdrs.sqlite3",
		"sqlite:///data/rdrs.sqlite3":                  "sqlite:///data/rdrs.sqlite3",
		"postgres://u:p@host":                          "postgres://u:***@host",
		"postgres://host/db?x=a@b":                     "postgres://host/db?x=a@b",
		"postgres://u:p@h/db#frag@x":                   "postgres://u:***@h/db#frag@x",
		"postgres://u:p@h?x=1":                         "postgres://u:***@h?x=1",
	} {
		if got := RedactDatabaseURL(in); got != want {
			t.Errorf("%q: got %q want %q", in, got, want)
		}
	}
}

func TestValidate(t *testing.T) {
	cfg := baseConfig()
	cfg.DatabaseURL = "postgres://user@localhost/rdrs"
	if err := cfg.Validate(); err != nil {
		t.Fatal(err)
	}
	bad := baseConfig()
	bad.AuthProxyHeader = "Remote-User"
	if bad.Validate() == nil {
		t.Fatal("header without trusted networks must fail")
	}
	bad.TrustedProxyNetworks, _ = ParseTrustedNetworks("10.0.0.0/8")
	if err := bad.Validate(); err != nil {
		t.Fatal(err)
	}
	local := baseConfig()
	local.DisableLocalAuth = true
	if local.Validate() == nil {
		t.Fatal("disable local auth without header must fail")
	}
}

func TestSetupAndAccountCreation(t *testing.T) {
	cfg := baseConfig()
	if !cfg.CanSetup(0) || cfg.CanSetup(1) || !cfg.CanCreateAccount(0) || cfg.CanCreateAccount(1) {
		t.Fatal("single user rules")
	}
	cfg.MultiUser = true
	if cfg.CanSetup(1) || !cfg.CanCreateAccount(0) || !cfg.CanCreateAccount(5) {
		t.Fatal("multi user rules")
	}
}

func TestParseTrustedNetworks(t *testing.T) {
	nets, err := ParseTrustedNetworks("10.0.0.0/8, 192.168.1.0/24 , 127.0.0.1")
	if err != nil || len(nets) != 3 || nets[2].Bits() != 32 {
		t.Fatalf("%v %v", nets, err)
	}
	if nets, err := ParseTrustedNetworks(""); err != nil || len(nets) != 0 {
		t.Fatalf("%v %v", nets, err)
	}
	if nets, _ := ParseTrustedNetworks("::1"); nets[0].Bits() != 128 {
		t.Fatalf("%v", nets)
	}
	if _, err := ParseTrustedNetworks("not-an-ip"); err == nil {
		t.Fatal("want error")
	}
}

func TestIsTrustedPeer(t *testing.T) {
	cfg := baseConfig()
	cfg.TrustedProxyNetworks, _ = ParseTrustedNetworks("10.0.0.0/8")
	if !cfg.IsTrustedPeer(netip.MustParseAddr("10.1.2.3")) || cfg.IsTrustedPeer(netip.MustParseAddr("192.168.0.1")) {
		t.Fatal("trusted peer")
	}
	if !cfg.IsTrustedPeer(netip.MustParseAddr("::ffff:10.1.2.3")) {
		t.Fatal("a v4-mapped peer must match its v4 network")
	}
}

func TestGroupMappingEnabled(t *testing.T) {
	cfg := baseConfig()
	if cfg.GroupMappingEnabled() {
		t.Fatal("off")
	}
	cfg.AuthProxyGroupsHeader, cfg.AuthProxyAdminGroup = "Remote-Groups", "admins"
	if !cfg.GroupMappingEnabled() {
		t.Fatal("on")
	}
}

func TestNoAdminGroupMappingWarning(t *testing.T) {
	cfg := baseConfig()
	cfg.AuthProxyHeader, cfg.AuthProxyGroupsHeader, cfg.AuthProxyAdminGroup = "Remote-User", "Remote-Groups", "admins"
	warning, ok := cfg.NoAdminGroupMappingWarning(0)
	if !ok || !strings.Contains(warning, "'admins'") || !strings.Contains(warning, "Remote-Groups") {
		t.Fatalf("%q", warning)
	}
	if _, ok := cfg.NoAdminGroupMappingWarning(1); ok {
		t.Fatal("an admin exists")
	}
	if _, ok := baseConfig().NoAdminGroupMappingWarning(0); ok {
		t.Fatal("mapping off")
	}
	cfg.AuthProxyHeader = ""
	if _, ok := cfg.NoAdminGroupMappingWarning(0); ok {
		t.Fatal("no forward auth")
	}
}

func TestWebAuthnRPWarning(t *testing.T) {
	if _, ok := baseConfig().WebAuthnRPWarning(); !ok {
		t.Fatal("localhost must warn")
	}
	deployed := baseConfig()
	deployed.WebAuthnRPID, deployed.WebAuthnRPOrigin = "rdrs.example.com", "https://rdrs.example.com"
	if _, ok := deployed.WebAuthnRPWarning(); ok {
		t.Fatal("deployed must not warn")
	}
	deployed.PublicBaseURL = "https://rdrs.example.com/"
	if _, ok := deployed.WebAuthnRPWarning(); ok {
		t.Fatal("trailing slash is ignored")
	}
	deployed.PublicBaseURL = "https://reader.example.com"
	if _, ok := deployed.WebAuthnRPWarning(); !ok {
		t.Fatal("mismatch must warn")
	}
}

func TestLoginRateLimitDefaultsAndOverride(t *testing.T) {
	cfg := fromVars(t, map[string]string{"RDRS_LOGIN_RATE_LIMIT_ATTEMPTS": "  ", "RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS": ""})
	if cfg.LoginRateLimitAttempts != 5 || cfg.LoginRateLimitWindowSecs != 60 {
		t.Fatalf("%+v", cfg)
	}
	cfg = fromVars(t, map[string]string{"RDRS_LOGIN_RATE_LIMIT_ATTEMPTS": "10", "RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS": "120"})
	if cfg.LoginRateLimitAttempts != 10 || cfg.LoginRateLimitWindowSecs != 120 {
		t.Fatalf("%+v", cfg)
	}
	if fromVars(t, map[string]string{"RDRS_LOGIN_RATE_LIMIT_ATTEMPTS": "0"}).LoginRateLimitAttempts != 0 {
		t.Fatal("0 disables the limiter")
	}
}

func TestLoginRateLimitErrors(t *testing.T) {
	for _, tc := range []struct{ key, value string }{
		{"RDRS_LOGIN_RATE_LIMIT_ATTEMPTS", "five"},
		{"RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS", "-1"},
		{"RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS", "0"},
	} {
		msg := loadErr(t, map[string]string{tc.key: tc.value})
		if !strings.Contains(msg, tc.key) || !strings.Contains(msg, tc.value) {
			t.Errorf("%s=%s: %s", tc.key, tc.value, msg)
		}
	}
}

func TestRateLimitProxyWarning(t *testing.T) {
	if _, ok := baseConfig().RateLimitProxyWarning(); !ok {
		t.Fatal("limiter without proxies must warn")
	}
	withProxies := baseConfig()
	withProxies.TrustedProxyNetworks, _ = ParseTrustedNetworks("10.0.0.0/8")
	if _, ok := withProxies.RateLimitProxyWarning(); ok {
		t.Fatal("proxies configured")
	}
	disabled := baseConfig()
	disabled.LoginRateLimitAttempts = 0
	if _, ok := disabled.RateLimitProxyWarning(); ok {
		t.Fatal("limiter disabled")
	}
}

func TestCustomUserAgentSuggestions(t *testing.T) {
	if len(CustomUserAgentSuggestions) == 0 {
		t.Fatal("empty")
	}
	for _, ua := range CustomUserAgentSuggestions {
		for _, r := range ua {
			if r < 0x20 || r > 0x7e {
				t.Errorf("%q cannot be sent as a header value", ua)
			}
		}
		if ua == DefaultUserAgent() {
			t.Error("the default belongs to the empty field, not the list")
		}
	}
}

func TestFetchAllowPrivateIsPassedThroughRaw(t *testing.T) {
	cfg := fromVars(t, map[string]string{"RDRS_FETCH_ALLOW_PRIVATE_HOSTS": " 192.168.0.0/16,nas.local "})
	if cfg.FetchAllowPrivate != "192.168.0.0/16,nas.local" {
		t.Fatalf("%q", cfg.FetchAllowPrivate)
	}
}
