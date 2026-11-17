package server

import (
	"errors"
	"net/http"
	"net/netip"
	"sync"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/feedsync"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/summarize"
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
	// AdminDBStats holds the site-wide database figures of /statistics.
	AdminDBStats *cache.AdminDBStats
	// Fetcher and Syncer reach the network for discovery, import and refresh.
	Fetcher *fetch.Fetcher
	Syncer  *feedsync.Syncer
	// Events tells open pages that the sidebar or a summary changed.
	Events *events.Bus
	// Summaries tracks summary jobs that have no finished database row yet.
	Summaries *cache.Summary
	// SummaryQueue feeds the summary worker; Cancels stops a job in flight.
	SummaryQueue chan summarize.Job
	Cancels      *summarize.Cancels
	// Now is the clock of relative times.
	Now     func() time.Time
	cookies auth.Cookies
	// externalClient reaches the user's own Linkding and Kagi hosts.
	externalClient *http.Client
	// detach runs a write that must not delay the response; tests make it
	// synchronous so the next request sees the write.
	detach  func(task func())
	pending sync.WaitGroup
}

// NewApp wires the shared dependencies from options.
func NewApp(options Options) *App {
	if options.Limiter == nil {
		options.Limiter = auth.NewRateLimiter(options.Config.LoginRateLimitAttempts, options.Config.LoginRateLimitWindowSecs)
	}
	if options.Fetcher == nil {
		options.Fetcher = fetch.NewFetcher(fetch.Policy{})
	}
	if options.Syncer == nil {
		options.Syncer = &feedsync.Syncer{
			DB: options.DB, Fetcher: options.Fetcher, DefaultUserAgent: options.Config.UserAgent,
			Retry: fetch.DefaultRetry(),
		}
	}
	if options.Now == nil {
		options.Now = time.Now
	}
	if options.Events == nil {
		options.Events = events.NewBus()
	}
	if options.Summaries == nil {
		options.Summaries = cache.NewSummary(cache.SummaryCapacity, cache.SummaryTTL)
	}
	if options.SummaryQueue == nil {
		options.SummaryQueue = summarize.NewQueue()
	}
	if options.Cancels == nil {
		options.Cancels = summarize.NewCancels()
	}
	authenticator := &auth.Authenticator{DB: options.DB, Config: options.Config, Audit: options.Audit}
	app := &App{
		Fetcher:        options.Fetcher,
		Syncer:         options.Syncer,
		Events:         options.Events,
		Summaries:      options.Summaries,
		SummaryQueue:   options.SummaryQueue,
		Cancels:        options.Cancels,
		Now:            options.Now,
		options:        options,
		Config:         options.Config,
		DB:             options.DB,
		Audit:          options.Audit,
		Auth:           authenticator,
		Limiter:        options.Limiter,
		Sidebar:        cache.DefaultSidebar(),
		AdminDBStats:   cache.NewAdminDBStats(),
		cookies:        authenticator.Cookies(),
		externalClient: fetch.NewExternalAPIClient(),
	}
	app.detach = app.goDetached
	return app
}

// goDetached runs task on its own goroutine, which Wait can join.
func (a *App) goDetached(task func()) {
	a.pending.Add(1)
	go func() {
		defer a.pending.Done()
		task()
	}()
}

// Wait blocks until every detached write has finished; shutdown calls it
// before the database closes.
func (a *App) Wait() { a.pending.Wait() }

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
