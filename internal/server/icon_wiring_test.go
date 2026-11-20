package server

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
)

var iconPNG = []byte{0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A}

// iconFeedServer serves an RSS feed whose logo is /logo.png on the same
// server, answered by logo.
func iconFeedServer(t *testing.T, logo http.HandlerFunc) *httptest.Server {
	t.Helper()
	mux := http.NewServeMux()
	var base string
	mux.HandleFunc("/feed", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		_, _ = fmt.Fprintf(w, `<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
<image><url>%s/logo.png</url></image>
<item><guid>a</guid><title>A</title></item></channel></rss>`, base)
	})
	mux.HandleFunc("/logo.png", logo)
	server := httptest.NewServer(mux)
	base = server.URL
	t.Cleanup(server.Close)
	return server
}

// refreshNewFeed adds a feed on server through the app's own Syncer, the one
// manual refresh and feed creation use, and returns the stored icon (nil when
// none) with the sync error.
func refreshNewFeed(t *testing.T, server *httptest.Server) (*models.Image, error) {
	t.Helper()
	app := defaultITestApp(t)
	user := app.SeedAccount("iconowner", fixturePassword, models.RoleUser)
	ctx := context.Background()
	category, err := models.CreateCategory(ctx, app.DB, user.ID, "c")
	if err != nil {
		t.Fatal(err)
	}
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{CategoryID: category.ID, URL: server.URL + "/feed"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := app.App.Syncer.Refresh(ctx, feed.ID); err != nil {
		return nil, err
	}
	image, err := models.FindImage(ctx, app.DB, models.EntityFeed, feed.ID)
	if err != nil {
		t.Fatal(err)
	}
	return image, nil
}

func TestANewFeedGetsItsIcon(t *testing.T) {
	server := iconFeedServer(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "image/png")
		_, _ = w.Write(iconPNG)
	})

	image, err := refreshNewFeed(t, server)

	if err != nil || image == nil {
		t.Fatalf("image = %v, err = %v", image, err)
	}
	if string(image.Data) != string(iconPNG) || image.ContentType != "image/png" {
		t.Errorf("image = %+v", image)
	}
}

func TestAMissingIconDoesNotFailTheSync(t *testing.T) {
	server := iconFeedServer(t, http.NotFound)

	image, err := refreshNewFeed(t, server)

	if err != nil || image != nil {
		t.Errorf("image = %v, err = %v; want a synced feed without an icon", image, err)
	}
}

func TestAnIconFetchErrorDoesNotFailTheSync(t *testing.T) {
	server := iconFeedServer(t, func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	})

	image, err := refreshNewFeed(t, server)

	if err != nil || image != nil {
		t.Errorf("image = %v, err = %v; want a synced feed without an icon", image, err)
	}
}

// Icons ride the same SSRF policy as feeds: the allow list names 127.0.0.1
// only, so a logo on another loopback address must never be requested.
func TestAnIconOutsideTheFetchPolicyIsNotRequested(t *testing.T) {
	var hits atomic.Int32
	logoServer := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { hits.Add(1) }))
	t.Cleanup(logoServer.Close)
	_, port, _ := strings.Cut(strings.TrimPrefix(logoServer.URL, "http://"), ":")
	feedServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		_, _ = fmt.Fprintf(w, `<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
<image><url>http://127.0.0.2:%s/logo.png</url></image>
<item><guid>a</guid><title>A</title></item></channel></rss>`, port)
	}))
	t.Cleanup(feedServer.Close)

	image, err := refreshNewFeed(t, feedServer)

	if err != nil || image != nil || hits.Load() != 0 {
		t.Errorf("image = %v, err = %v, logo hits = %d; want none", image, err, hits.Load())
	}
}
