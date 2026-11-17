// Command rdrs is a self-hosted RSS reader.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"os"

	"github.com/henry40408/rdrs/internal/config"
)

func main() {
	os.Exit(run(context.Background(), os.Args[1:], os.LookupEnv, os.Stdout, os.Stderr))
}

// run is the testable entry point; it returns the process exit code. Stdout is
// reserved for output that is safe to collect: the setup token goes to stderr.
func run(_ context.Context, args []string, lookupEnv func(string) (string, bool), _, stderr io.Writer) int {
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
	warnAboutConfig(logger, cfg)
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
