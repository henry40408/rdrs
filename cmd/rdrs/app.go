package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/feedsync"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/icon"
	"github.com/henry40408/rdrs/internal/migrate"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/server"
	"github.com/henry40408/rdrs/internal/summarize"
	"github.com/henry40408/rdrs/internal/worker"
)

const (
	// serverShutdownTimeout bounds how long open requests may finish.
	serverShutdownTimeout = 10 * time.Second
	// detachedWritesTimeout bounds the wait for writes started by requests.
	detachedWritesTimeout = 5 * time.Second
	// workerShutdownTimeout is the join timeout of the Rust server.
	workerShutdownTimeout = 30 * time.Second
)

// application is the wired server plus the background workers.
type application struct {
	cfg     config.Config
	logger  *slog.Logger
	db      *db.DB
	server  *server.App
	handler http.Handler
	// workers run until workerCtx ends; workersDone is closed when all return.
	workers []func(ctx context.Context)
}

// newApplication opens the database and wires everything but the listener.
// Any error leaves nothing open.
func newApplication(ctx context.Context, cfg config.Config, logger *slog.Logger, stderr io.Writer) (*application, error) {
	// An invalid relying party fails the start, as the Rust build did, rather
	// than answering every passkey ceremony with a 500 later.
	if err := checkRelyingParty(cfg); err != nil {
		return nil, fmt.Errorf("invalid WebAuthn relying party settings: %w", err)
	}
	policy, err := fetch.ParsePolicy(cfg.FetchAllowPrivate)
	if err != nil {
		return nil, fmt.Errorf("invalid RDRS_FETCH_ALLOW_PRIVATE_HOSTS: %w", err)
	}
	database, err := openDatabase(ctx, cfg)
	if err != nil {
		return nil, err
	}
	app, err := assemble(ctx, assembly{cfg: cfg, logger: logger, db: database, policy: policy, stderr: stderr})
	if err != nil {
		_ = database.Shutdown(context.WithoutCancel(ctx))
		return nil, err
	}
	return app, nil
}

// checkRelyingParty repeats the checks webauthn-rs makes at build time that
// go-webauthn leaves to the first ceremony: the origin must parse and the RP
// ID must be its host or a parent domain of it.
func checkRelyingParty(cfg config.Config) error {
	origin, err := url.Parse(cfg.WebAuthnRPOrigin)
	if err != nil || origin.Scheme == "" || origin.Hostname() == "" {
		return fmt.Errorf("RDRS_WEBAUTHN_RP_ORIGIN %q is not an absolute URL", cfg.WebAuthnRPOrigin)
	}
	host, id := origin.Hostname(), cfg.WebAuthnRPID
	if host != id && !strings.HasSuffix(host, "."+id) {
		return fmt.Errorf("RDRS_WEBAUTHN_RP_ID %q is not the host of RDRS_WEBAUTHN_RP_ORIGIN %q or a parent domain of it", id, cfg.WebAuthnRPOrigin)
	}
	_, err = auth.NewWebAuthn(cfg)
	return err
}

func openDatabase(ctx context.Context, cfg config.Config) (*db.DB, error) {
	database, err := db.Open(ctx, cfg.DatabaseURL, cfg.Backend())
	if err != nil {
		return nil, fmt.Errorf("open database: %w", err)
	}
	if err := migrate.Run(ctx, database); err != nil {
		_ = database.Shutdown(context.WithoutCancel(ctx))
		return nil, fmt.Errorf("migrate database: %w", err)
	}
	if err := database.Optimize(ctx); err != nil {
		slog.Warn("PRAGMA optimize failed", "event", "db.optimize_failed", "error", err)
	}
	return database, nil
}

type assembly struct {
	cfg    config.Config
	logger *slog.Logger
	db     *db.DB
	policy fetch.Policy
	stderr io.Writer
}

func assemble(ctx context.Context, in assembly) (*application, error) {
	setupToken, err := announceSetupToken(ctx, in.db, in.stderr)
	if err != nil {
		return nil, err
	}
	fetcher := fetch.NewFetcher(in.policy)
	hsts, _ := in.cfg.HSTSHeader()
	app := server.NewApp(server.Options{
		HSTS:       hsts,
		Config:     in.cfg,
		DB:         in.db,
		Audit:      audit.New(in.logger, in.cfg.Secret),
		SetupToken: setupToken,
		Fetcher:    fetcher,
	})
	warnIfNoAdminForGroupMapping(ctx, in.logger, in.cfg, in.db)
	return &application{
		cfg: in.cfg, logger: in.logger, db: in.db, server: app, handler: app.Handler(),
		workers: backgroundWorkers(in, app, fetcher),
	}, nil
}

// announceSetupToken makes the one-time setup code. It is printed only while
// setup is open, and on stderr, not through the logger: log collectors retain
// log output, and a leftover token would outlive the setup it protects.
func announceSetupToken(ctx context.Context, database *db.DB, stderr io.Writer) (string, error) {
	token, err := secret.GenerateSetupToken()
	if err != nil {
		return "", fmt.Errorf("generate setup token: %w", err)
	}
	if token == "" {
		return "", errors.New("generate setup token: empty token")
	}
	users, err := models.CountUsers(ctx, database)
	if err == nil && users > 0 {
		return token, nil
	}
	_, _ = fmt.Fprintf(stderr, "no accounts yet: open /setup and enter this setup token to create the admin account\nsetup_token=%s\n", token)
	return token, nil
}

func warnIfNoAdminForGroupMapping(ctx context.Context, logger *slog.Logger, cfg config.Config, database *db.DB) {
	admins, err := models.CountActiveAdmins(ctx, database)
	if err != nil {
		return
	}
	if warning, ok := cfg.NoAdminGroupMappingWarning(admins); ok {
		logger.Warn(warning, "event", "config.warning", "kind", "no_admin_group_mapping")
	}
}

// newBackgroundSyncer is the Syncer the periodic sync worker uses; it fetches
// feed icons like the server's own Syncer does.
func newBackgroundSyncer(cfg config.Config, background *db.DB, fetcher *fetch.Fetcher) *feedsync.Syncer {
	return &feedsync.Syncer{
		DB: background, Fetcher: fetcher, DefaultUserAgent: cfg.UserAgent, Retry: fetch.DefaultRetry(),
		FetchIcon: icon.ForProduction(fetcher),
	}
}

// backgroundWorkers lists every loop that must finish before the database
// closes. Each works on a background database handle so it yields to requests.
func backgroundWorkers(in assembly, app *server.App, fetcher *fetch.Fetcher) []func(context.Context) {
	background := in.db.Background()
	syncWorker := &worker.Sync{
		Syncer: newBackgroundSyncer(in.cfg, background, fetcher),
		DB:     background, Sidebar: app.Sidebar, Events: app.Events,
	}
	key, _ := in.cfg.ServiceTokenKey()
	summaries := summarize.NewWorker(summarize.Worker{
		DB: in.db, Cache: app.Summaries, Sidebar: app.Sidebar, Cancels: app.Cancels,
		Events: app.Events, ServiceTokenKey: key,
	})
	return []func(context.Context){
		syncWorker.Run,
		func(ctx context.Context) { summaries.Run(ctx, app.SummaryQueue) },
		(&worker.Cleanup{DB: background}).Run,
		(&worker.Retention{DB: background}).Run,
		func(ctx context.Context) { worker.BackfillContentText(ctx, background) },
	}
}

// serve runs until ctx ends, then shuts down in two stages: first the HTTP
// server stops taking requests and open ones finish, then the workers are
// joined and the database closes. Shutdown also closes the event bus, which
// ends SSE streams that Shutdown would otherwise wait for.
func (a *application) serve(ctx context.Context, listener net.Listener) error {
	workerCtx, stopWorkers := context.WithCancel(context.WithoutCancel(ctx))
	defer stopWorkers()

	workersDone := a.startWorkers(workerCtx)
	if recovered := summarize.RecoverIncomplete(workerCtx, a.db, a.server.SummaryQueue, a.server.Summaries); recovered > 0 {
		a.logger.Info("recovered incomplete summary jobs", "event", "summary.recovered", "count", recovered)
	}

	httpServer := &http.Server{
		Handler:           a.handler,
		ReadHeaderTimeout: 10 * time.Second,
	}
	a.logger.Info("starting server", "event", "server.starting", "addr", listener.Addr().String())
	serveErr := make(chan error, 1)
	go func() { serveErr <- httpServer.Serve(listener) }()

	var result error
	select {
	case result = <-serveErr:
	case <-ctx.Done():
	}
	a.stopServer(httpServer)
	if errors.Is(result, http.ErrServerClosed) {
		result = nil
	}
	a.logger.Info("server stopped, initiating graceful shutdown", "event", "server.stopped")

	stopWorkers()
	a.waitForWorkers(workersDone)
	a.waitForDetachedWrites()
	if err := a.db.Shutdown(context.WithoutCancel(ctx)); err != nil {
		a.logger.Error("database shutdown failed", "event", "db.shutdown_failed", "error", err)
	}
	a.logger.Info("graceful shutdown complete", "event", "shutdown.complete")
	return result
}

func (a *application) startWorkers(ctx context.Context) <-chan struct{} {
	var group sync.WaitGroup
	for _, run := range a.workers {
		group.Add(1)
		go func() {
			defer group.Done()
			run(ctx)
		}()
	}
	done := make(chan struct{})
	go func() {
		group.Wait()
		close(done)
	}()
	return done
}

// stopServer ends the streams at once, then gives requests a grace period
// before closing their connections. Request contexts stay alive meanwhile, so
// a request that is still running can finish its database writes.
func (a *application) stopServer(httpServer *http.Server) {
	a.server.Events.Close()
	shutdownCtx, cancel := context.WithTimeout(context.Background(), serverShutdownTimeout)
	defer cancel()
	if err := httpServer.Shutdown(shutdownCtx); err != nil {
		a.logger.Warn("server did not drain in time; closing connections", "event", "server.shutdown_timeout", "error", err)
		_ = httpServer.Close()
	}
}

func (a *application) waitForWorkers(done <-chan struct{}) {
	a.logger.Info("waiting for background tasks to complete", "event", "shutdown.waiting", "timeout_s", int(workerShutdownTimeout.Seconds()))
	select {
	case <-done:
		a.logger.Info("all background tasks completed", "event", "shutdown.tasks_completed")
	case <-time.After(workerShutdownTimeout):
		a.logger.Warn("background tasks did not complete in time", "event", "shutdown.timeout", "timeout_s", int(workerShutdownTimeout.Seconds()))
	}
}

// waitForDetachedWrites lets the writes that requests started in the
// background finish before the database closes.
func (a *application) waitForDetachedWrites() {
	done := make(chan struct{})
	go func() {
		a.server.Wait()
		close(done)
	}()
	select {
	case <-done:
		a.logger.Info("detached writes completed", "event", "shutdown.writes_completed")
	case <-time.After(detachedWritesTimeout):
		a.logger.Warn("detached writes did not complete in time", "event", "shutdown.writes_timeout", "timeout_s", int(detachedWritesTimeout.Seconds()))
	}
}
