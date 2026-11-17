package server

import (
	"net/http"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/feedsync"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/server/middleware"
	"github.com/henry40408/rdrs/internal/summarize"
	"github.com/henry40408/rdrs/internal/version"
)

// Route is one method and path. Paths use the axum notation the Rust router
// used ({id}, {*rest}), so testdata/routes.txt can be compared with them.
type Route struct {
	Method  string
	Path    string
	Handler http.HandlerFunc
}

// String is the "METHOD /path" form used in testdata/routes.txt.
func (r Route) String() string { return r.Method + " " + r.Path }

// pattern converts the axum path to a net/http pattern: {*rest} becomes
// {rest...}. The method is part of the pattern, so a known path with another
// method is a 405 with an Allow header, like axum.
func (r Route) pattern() string {
	path := r.Path
	if path == "/" {
		// A bare "/" would match every path.
		path = "/{$}"
	}
	if start := strings.Index(path, "{*"); start >= 0 {
		path = path[:start] + "{" + path[start+2:len(path)-1] + "...}"
	}
	return r.Method + " " + path
}

// routes is the single registry of what the server serves. Handlers for the
// remaining routes are added by the steps that port them.
func (a *App) routes() []Route {
	table := []Route{
		{"GET", "/health", serveHealth},
		{"GET", "/static/{*path}", serveStatic},
		{"GET", "/sw.js", serveServiceWorker},
	}
	for path := range favicons {
		table = append(table, Route{"GET", path, serveFavicon})
	}
	table = append(table, a.authRoutes()...)
	table = append(table, a.chromeRoutes()...)
	table = append(table, a.categoryRoutes()...)
	table = append(table, a.feedRoutes()...)
	table = append(table, a.mediaRoutes()...)
	table = append(table, a.entryPageRoutes()...)
	table = append(table, a.searchRoutes()...)
	table = append(table, a.statisticsRoutes()...)
	table = append(table, a.summarizerRoutes()...)
	table = append(table, a.adminRoutes()...)
	table = append(table, a.accountRoutes()...)
	table = append(table, a.entryFragmentRoutes()...)
	table = append(table, a.entryActionRoutes()...)
	table = append(table, a.markReadRoutes()...)
	table = append(table, a.entryAPIRoutes()...)
	return table
}

func serveHealth(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, struct {
		Status     string `json:"status"`
		GitVersion string `json:"git_version"`
	}{"ok", version.Version()})
}

// Options configures the layers around the routes.
type Options struct {
	// HSTS is the Strict-Transport-Security value; empty leaves it off, which
	// is right for plain HTTP deployments.
	HSTS string

	// Config, DB and Audit feed the session layers and the handlers.
	Config config.Config
	DB     *db.DB
	Audit  *audit.Logger

	// Limiter throttles credential attempts; nil builds one from Config.
	Limiter *auth.RateLimiter
	// SetupToken is the one-time code first-run setup demands, printed on
	// stderr at startup. An empty value never means "no token": setup is then
	// refused, so a wiring mistake cannot hand the first admin to anyone.
	SetupToken string
	// SkipSetupToken drops the token check; for tests only.
	SkipSetupToken bool

	// Fetcher guards every outbound request; nil builds one that allows no
	// private host.
	Fetcher *fetch.Fetcher
	// Syncer refreshes feeds on demand; nil builds one around Fetcher.
	Syncer *feedsync.Syncer
	// Events, Summaries, SummaryQueue and Cancels connect the handlers to the
	// SSE endpoint and the summary worker; nil builds fresh ones.
	Events       *events.Bus
	Summaries    *cache.Summary
	SummaryQueue chan summarize.Job
	Cancels      *summarize.Cancels
	// Now is the clock of relative times; nil is time.Now. Tests pin it.
	Now func() time.Time

	// notFound answers a path no route serves; App.Handler sets it.
	notFound http.HandlerFunc
}

// Handler mounts every registered route inside the middleware stack, outermost
// first as in the Rust router. The SSE route will sit outside the core stack:
// Timeout, Compress and ETag buffer, which would break a stream.
func Handler(options Options) http.Handler {
	return NewApp(options).Handler()
}

// Handler mounts the app's routes inside the middleware stack.
func (a *App) Handler() http.Handler {
	options := a.options
	options.notFound = a.notFoundPage
	return buildHandler(options, a.routes())
}

// fallback answers what no route matched: 405 with Allow when another method
// serves the path, like axum, else notFound (a bare 404 when it is nil).
func fallback(mux *http.ServeMux, notFound http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if allowed := allowedMethods(mux, r); len(allowed) > 0 {
			w.Header().Set("Allow", strings.Join(allowed, ","))
			w.WriteHeader(http.StatusMethodNotAllowed)
			return
		}
		if notFound == nil {
			http.NotFound(w, r)
			return
		}
		notFound(w, r)
	}
}

// allowedMethods probes the mux with each method for a route other than the
// fallback itself.
func allowedMethods(mux *http.ServeMux, r *http.Request) []string {
	var allowed []string
	for _, method := range []string{http.MethodGet, http.MethodPost, http.MethodPut, http.MethodDelete} {
		probe := r.Clone(r.Context())
		probe.Method = method
		if _, pattern := mux.Handler(probe); pattern != "" && pattern != "/" {
			allowed = append(allowed, method)
		}
	}
	return allowed
}

// notFoundPage is the router fallback: the 404 page when signed in, the login
// redirect otherwise.
func (a *App) notFoundPage(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requirePage(w, r)
	if !ok {
		return
	}
	a.renderNotFound(w, r, principal, "Page not found", "The page you're looking for doesn't exist.")
}

// buildHandler takes the route table so tests can mount probe routes behind
// the real stack.
func buildHandler(options Options, table []Route) http.Handler {
	mux := http.NewServeMux()
	for _, route := range table {
		mux.HandleFunc(route.pattern(), withRoute(route))
	}
	mux.HandleFunc("/", fallback(mux, options.notFound))
	authenticator := &auth.Authenticator{DB: options.DB, Config: options.Config, Audit: options.Audit}
	cookies := authenticator.Cookies()
	core := middleware.Chain(mux,
		middleware.CrossSiteGuard,
		middleware.SignFlashCookies(options.Config.Secret),
		middleware.ForwardAuth(auth.ForwardLogin{DB: options.DB, Config: options.Config, Audit: options.Audit}),
		middleware.SlideSession(authenticator),
		middleware.AnonymousSession(cookies),
		middleware.CSRFGuard(cookies),
		middleware.Timeout(middleware.RequestTimeout),
		middleware.Compress,
		middleware.DateHeader,
		middleware.ETag,
		middleware.NoStoreForAuthenticated,
	)
	layers := []middleware.Middleware{middleware.RequestLog}
	if options.HSTS != "" {
		layers = append(layers, middleware.HSTS(options.HSTS))
	}
	layers = append(layers, middleware.SecurityHeaders)
	return middleware.Chain(core, layers...)
}

// withRoute tells the request log which template matched.
func withRoute(route Route) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		middleware.SetRoute(r, route.Path)
		route.Handler(w, r)
	}
}
