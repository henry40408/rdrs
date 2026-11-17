package server

import (
	"context"
	"fmt"
	"net/http"
	"time"

	"github.com/henry40408/rdrs/internal/events"
)

// keepAliveInterval keeps proxies from closing an idle stream.
const keepAliveInterval = 15 * time.Second

// streamingPaths are served outside the core middleware: Timeout would kill
// the stream, and Compress and ETag would buffer it.
var streamingPaths = map[string]bool{"/events": true}

func (a *App) eventRoutes() []Route {
	return []Route{{"GET", "/events", a.eventsStream}}
}

// eventsStream is GET /events: one SSE stream per open tab, carrying only the
// signed-in user's events.
func (a *App) eventsStream(w http.ResponseWriter, r *http.Request) {
	principal := a.Auth.AuthenticatePage(r)
	if principal == nil {
		// No flash here: this route sits outside the layer that signs flashes.
		http.Redirect(w, r, "/login", http.StatusSeeOther)
		return
	}
	flusher := http.NewResponseController(w)
	subscription := a.Events.Subscribe()
	defer subscription.Close()
	header := w.Header()
	header.Set("Content-Type", "text/event-stream")
	header.Set("Cache-Control", "no-cache")
	w.WriteHeader(http.StatusOK)
	if flusher.Flush() != nil {
		return
	}
	serveEvents(r.Context(), eventStream{
		out: w, flush: flusher.Flush, subscription: subscription, userID: principal.User.ID,
	})
}

// eventStream is one open connection.
type eventStream struct {
	out          http.ResponseWriter
	flush        func() error
	subscription *events.Subscription
	userID       int64
}

// serveEvents writes events until the client leaves or the bus closes (the
// server's graceful shutdown), so shutdown never waits on an open tab.
func serveEvents(ctx context.Context, stream eventStream) {
	incoming := make(chan events.Event)
	go func() {
		defer close(incoming)
		for {
			event, err := stream.subscription.Next(ctx)
			if err != nil {
				return
			}
			select {
			case incoming <- event:
			case <-ctx.Done():
				return
			}
		}
	}()
	ticker := time.NewTicker(keepAliveInterval)
	defer ticker.Stop()
	for {
		var err error
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			_, err = fmt.Fprint(stream.out, ":\n\n")
		case event, open := <-incoming:
			if !open {
				return
			}
			err = stream.send(event)
		}
		if err != nil || stream.flush() != nil {
			return
		}
	}
}

// send writes one event; another user's event is skipped, and a resync nudge
// (what a lagging subscriber gets) goes to every tab.
func (s eventStream) send(event events.Event) error {
	if event.UserID != s.userID && !event.IsResync() {
		return nil
	}
	name, data := event.Wire()
	_, err := fmt.Fprintf(s.out, "event: %s\ndata: %s\n\n", name, data)
	return err
}
