// Command rdrs is a self-hosted RSS reader.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"net"
	"os"
	"os/signal"
	"syscall"

	"github.com/henry40408/rdrs/internal/config"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	code := run(ctx, stop, os.Args[1:], os.LookupEnv, os.Stdout, os.Stderr)
	stop()
	os.Exit(code)
}

// run is the testable entry point; it returns the process exit code. Stdout is
// reserved for output that is safe to collect: the setup token goes to stderr.
//
// ctx ends on the first SIGINT or SIGTERM, which starts the graceful shutdown;
// releaseSignals then restores default signal handling, so a second signal
// kills a shutdown that hangs.
func run(ctx context.Context, releaseSignals func(), args []string, lookupEnv func(string) (string, bool), _, stderr io.Writer) int {
	if err := ensureNoLegacyEnvVars(lookupEnv); err != nil {
		fmt.Fprintf(stderr, "Configuration error: %v\n", err)
		return 1
	}
	opts, err := parseOptions(args, lookupEnv, stderr)
	if errors.Is(err, flag.ErrHelp) {
		return 0
	}
	if err != nil {
		fmt.Fprintf(stderr, "Configuration error: %v\n", err)
		return 2
	}
	logger := newLogger(loggerSettings{format: opts.logFormat, level: opts.logLevel, ansi: opts.ansi}, stderr)

	cfg, err := loadConfig(lookupEnv)
	if err != nil {
		fmt.Fprintf(stderr, "Configuration error: %v\n", err)
		return 1
	}
	slog.SetDefault(logger)
	warnAboutConfig(logger, cfg)
	return serveUntilStopped(ctx, releaseSignals, cfg, logger, stderr)
}

func serveUntilStopped(ctx context.Context, releaseSignals func(), cfg config.Config, logger *slog.Logger, stderr io.Writer) int {
	app, err := newApplication(ctx, cfg, logger, stderr)
	if err != nil {
		fmt.Fprintf(stderr, "Startup error: %v\n", err)
		return 1
	}
	listener, err := net.Listen("tcp", cfg.ServerBind.String())
	if err != nil {
		_ = app.db.Shutdown(context.WithoutCancel(ctx))
		fmt.Fprintf(stderr, "Startup error: bind %s: %v\n", cfg.ServerBind, err)
		return 1
	}
	go func() {
		<-ctx.Done()
		logger.Info("received a stop signal, shutting down", "event", "shutdown.signal")
		releaseSignals()
	}()
	if err := app.serve(ctx, listener); err != nil {
		logger.Error("server failed", "event", "server.failed", "error", err)
		return 1
	}
	return 0
}

func loadConfig(lookupEnv func(string) (string, bool)) (config.Config, error) {
	cfg, err := config.Load(lookupEnv)
	if err != nil {
		return config.Config{}, err
	}
	return cfg, cfg.Validate()
}

func warnAboutConfig(logger *slog.Logger, cfg config.Config) {
	if cfg.SecretGenerated {
		logger.Warn("RDRS_SECRET is not set or too short; using a key generated for this process only. "+
			"Every signed-in browser session ends on restart, and image-proxy URLs already cached "+
			"by Google Reader clients break until the next sync. Set RDRS_SECRET (e.g. "+
			"`openssl rand -base64 32`) to keep them stable across restarts.",
			"event", "config.warning", "kind", "ephemeral_secret")
	}
	if warning, ok := cfg.WebAuthnRPWarning(); ok {
		logger.Warn(warning, "event", "config.warning", "kind", "webauthn_rp")
	}
	if warning, ok := cfg.RateLimitProxyWarning(); ok {
		logger.Warn(warning, "event", "config.warning", "kind", "rate_limit_proxy")
	}
}
