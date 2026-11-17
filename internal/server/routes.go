package server

import (
	"net/http"
	"strings"

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
	if start := strings.Index(path, "{*"); start >= 0 {
		path = path[:start] + "{" + path[start+2:len(path)-1] + "...}"
	}
	return r.Method + " " + path
}

// routes is the single registry of what the server serves. Handlers for the
// remaining routes are added by the steps that port them.
func routes() []Route {
	table := []Route{
		{"GET", "/health", serveHealth},
		{"GET", "/static/{*path}", serveStatic},
		{"GET", "/sw.js", serveServiceWorker},
	}
	for path := range favicons {
		table = append(table, Route{"GET", path, serveFavicon})
	}
	return table
}

func serveHealth(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, struct {
		Status     string `json:"status"`
		GitVersion string `json:"git_version"`
	}{"ok", version.Version()})
}

// Handler mounts every registered route.
func Handler() http.Handler {
	mux := http.NewServeMux()
	for _, route := range routes() {
		mux.HandleFunc(route.pattern(), route.Handler)
	}
	return mux
}
