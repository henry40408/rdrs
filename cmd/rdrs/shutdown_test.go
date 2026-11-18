package main

import (
	"bytes"
	"context"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"
)

// startedApplication serves handler (in place of the real one) on a local
// port and returns its address, a stop function that triggers shutdown and
// returns serve's result, and the log.
func startedApplication(t *testing.T, handler http.Handler) (app *application, addr string, stop func() error, log *lockedBuffer) {
	t.Helper()
	cfg, err := loadConfig(envOf(isolatedEnv(t)))
	if err != nil {
		t.Fatal(err)
	}
	log = &lockedBuffer{}
	app, err = newApplication(context.Background(), cfg, newLogger(loggerSettings{}, log), &bytes.Buffer{})
	if err != nil {
		t.Fatal(err)
	}
	app.handler = handler
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	done := make(chan error, 1)
	go func() { done <- app.serve(ctx, listener) }()
	stop = func() error {
		cancel()
		select {
		case err := <-done:
			return err
		case <-time.After(30 * time.Second):
			t.Fatal("serve did not return")
			return nil
		}
	}
	return app, listener.Addr().String(), stop, log
}

// Shutdown used to cancel every request context, so a request still running
// could not finish its write, while a stream had to end for Shutdown to return.
func TestShutdownLetsARunningRequestWriteWhileStreamsEnd(t *testing.T) {
	var app *application
	inFlight := make(chan struct{})
	release := make(chan struct{})
	writeErr := make(chan error, 1)
	mux := http.NewServeMux()
	mux.HandleFunc("/slow", func(w http.ResponseWriter, r *http.Request) {
		close(inFlight)
		<-release
		_, err := app.db.ExecContext(r.Context(), "SELECT 1")
		writeErr <- err
	})
	mux.HandleFunc("/stream", func(w http.ResponseWriter, r *http.Request) {
		subscription := app.server.Events.Subscribe()
		defer subscription.Close()
		w.WriteHeader(http.StatusOK)
		_ = http.NewResponseController(w).Flush()
		_, _ = subscription.Next(r.Context())
	})
	app, addr, stop, _ := startedApplication(t, mux)

	slowDone := make(chan error, 1)
	go func() {
		response, err := http.Get("http://" + addr + "/slow")
		if err == nil {
			_ = response.Body.Close()
		}
		slowDone <- err
	}()
	<-inFlight
	stream, err := http.Get("http://" + addr + "/stream")
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Body.Close()

	stopped := make(chan error, 1)
	go func() { stopped <- stop() }()
	// Shutdown must not return while /slow runs, and must not wait for the stream.
	select {
	case <-stopped:
		t.Fatal("serve returned while a request was still running")
	case <-time.After(300 * time.Millisecond):
	}
	close(release)
	if err := <-writeErr; err != nil {
		t.Fatalf("a running request lost its context at shutdown: %v", err)
	}
	if err := <-slowDone; err != nil {
		t.Fatalf("running request failed: %v", err)
	}
	select {
	case err := <-stopped:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("the open stream blocked shutdown")
	}
}

// The database closes only after the detached writes are joined, as
// server.App.Wait promises.
func TestShutdownJoinsDetachedWritesBeforeTheDatabaseCloses(t *testing.T) {
	_, _, stop, log := startedApplication(t, http.NotFoundHandler())
	if err := stop(); err != nil {
		t.Fatal(err)
	}
	output := log.String()
	writes := strings.Index(output, "shutdown.writes_completed")
	complete := strings.Index(output, "shutdown.complete")
	if writes < 0 || complete < 0 || writes > complete {
		t.Fatalf("detached writes were not joined before shutdown completed: %s", output)
	}
}
