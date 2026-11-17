package main

import (
	"flag"
	"io"
	"log/slog"
	"strings"
)

// options are the settings read before the main configuration is loaded: they
// decide how to log the configuration errors.
type options struct {
	logFormat logFormat
	logLevel  slog.Level
	// ansi is on unless NO_COLOR is set, even to an empty value.
	ansi bool
}

// parseOptions reads --log-format (env RDRS_LOG_FORMAT, default full) and
// RDRS_LOG_LEVEL (env only, default info). The flag wins over the environment.
func parseOptions(args []string, lookupEnv func(string) (string, bool), usageOut io.Writer) (options, error) {
	fromEnv, _ := lookupEnv("RDRS_LOG_FORMAT")
	fromEnv = strings.TrimSpace(fromEnv)
	if fromEnv == "" {
		fromEnv = string(formatFull)
	}

	flags := flag.NewFlagSet("rdrs", flag.ContinueOnError)
	flags.SetOutput(usageOut)
	rawFormat := flags.String("log-format", fromEnv, "log output format: full, compact, pretty or json")
	if err := flags.Parse(args); err != nil {
		return options{}, err
	}
	format, err := parseLogFormat(*rawFormat)
	if err != nil {
		return options{}, err
	}

	rawLevel, _ := lookupEnv("RDRS_LOG_LEVEL")
	level, err := parseLogLevel(rawLevel)
	if err != nil {
		return options{}, err
	}
	_, noColor := lookupEnv("NO_COLOR")
	return options{logFormat: format, logLevel: level, ansi: !noColor}, nil
}
