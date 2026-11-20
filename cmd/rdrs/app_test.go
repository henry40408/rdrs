package main

import (
	"bytes"
	"context"
	"path/filepath"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/fetch"
)

func TestTheBackgroundSyncerFetchesFeedIcons(t *testing.T) {
	syncer := newBackgroundSyncer(config.Config{UserAgent: "ua"}, nil, fetch.NewFetcher(fetch.Policy{}))

	if syncer.FetchIcon == nil {
		t.Error("the background syncer has no icon fetcher, so feeds it syncs never get an icon")
	}
	if syncer.DefaultUserAgent != "ua" {
		t.Errorf("DefaultUserAgent = %q", syncer.DefaultUserAgent)
	}
}

func TestNewApplicationRefusesAConfigurationItCannotServe(t *testing.T) {
	cases := []struct {
		name   string
		mutate func(cfg *config.Config, dir string)
		want   string
	}{
		{"relying party origin is not a URL", func(cfg *config.Config, _ string) { cfg.WebAuthnRPOrigin = "not a url" }, "is not an absolute URL"},
		{"relying party id is another domain", func(cfg *config.Config, _ string) {
			cfg.WebAuthnRPOrigin, cfg.WebAuthnRPID = "https://rdrs.example", "other.example"
		}, "is not the host of"},
		{"fetch allow list is malformed", func(cfg *config.Config, _ string) { cfg.FetchAllowPrivate = "not-an-address/99" }, "RDRS_FETCH_ALLOW_PRIVATE_HOSTS"},
		{"database directory is missing", func(cfg *config.Config, dir string) {
			cfg.DatabaseURL = filepath.Join(dir, "missing", "rdrs.sqlite3")
		}, "open database"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			cfg, err := loadConfig(envOf(isolatedEnv(t)))
			if err != nil {
				t.Fatal(err)
			}
			c.mutate(&cfg, t.TempDir())
			app, err := newApplication(context.Background(), cfg, newLogger(loggerSettings{}, &bytes.Buffer{}), &bytes.Buffer{})
			if err == nil || !strings.Contains(err.Error(), c.want) {
				t.Errorf("err = %v, want it to mention %q", err, c.want)
			}
			if app != nil {
				t.Error("a failed start must not return an application")
			}
		})
	}
}
