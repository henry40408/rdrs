package server

import (
	"errors"
	"net/http"
	"net/netip"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/version"
)

// App is what every handler shares: the Rust AppState.
type App struct {
	options Options
	Config  config.Config
	DB      *db.DB
	Audit   *audit.Logger
	Auth    *auth.Authenticator
	Limiter *auth.RateLimiter
	// Sidebar caches the per-user chrome around every page.
	Sidebar *cache.Sidebar
	cookies auth.Cookies
}

// NewApp wires the shared dependencies from options.
func NewApp(options Options) *App {
	if options.Limiter == nil {
		options.Limiter = auth.NewRateLimiter(options.Config.LoginRateLimitAttempts, options.Config.LoginRateLimitWindowSecs)
	}
	authenticator := &auth.Authenticator{DB: options.DB, Config: options.Config, Audit: options.Audit}
	return &App{
		options: options,
		Config:  options.Config,
		DB:      options.DB,
		Audit:   options.Audit,
		Auth:    authenticator,
		Limiter: options.Limiter,
		Sidebar: cache.DefaultSidebar(),
		cookies: authenticator.Cookies(),
	}
}

// gitVersion is the build version shown in pages and asset URLs.
func gitVersion() string { return version.Version() }

// clientIP is the originating address, honouring forwarding headers only from
// a trusted proxy.
func (a *App) clientIP(r *http.Request) netip.Addr { return auth.ClientIP(a.Config, r) }

// request describes the caller for the audit log.
func (a *App) request(r *http.Request) audit.Request {
	return audit.Request{IP: a.clientIP(r).String(), UserAgent: auth.UserAgent(r)}
}

// requireUser authenticates an API request, answering the failure itself.
func (a *App) requireUser(w http.ResponseWriter, r *http.Request) (*auth.Principal, bool) {
	principal, err := a.Auth.Authenticate(r)
	if err != nil {
		writeError(w, authError(err))
		return nil, false
	}
	return principal, true
}

// authError maps an authentication failure to its API error.
func authError(err error) error {
	switch {
	case errors.Is(err, auth.ErrUnauthorized):
		return ErrUnauthorized
	case errors.Is(err, auth.ErrUserDisabled):
		return ErrUserDisabled
	case errors.Is(err, auth.ErrForbidden):
		return ErrForbidden
	case errors.Is(err, auth.ErrReauthRequired):
		return ErrReauthenticationRequired
	}
	return DatabaseError(err)
}
