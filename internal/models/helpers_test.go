package models

import (
	"context"
	"log/slog"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/migrate"
)

func TestMain(m *testing.M) {
	// Every test migrates a database; the per-migration log lines are noise.
	slog.SetDefault(slog.New(slog.DiscardHandler))
	os.Exit(m.Run())
}

// eachBackend runs fn against a freshly migrated SQLite file and, when
// TEST_DATABASE_URL is set, a freshly migrated PostgreSQL schema.
func eachBackend(t *testing.T, fn func(t *testing.T, database *db.DB)) {
	t.Helper()
	t.Run("sqlite", func(t *testing.T) {
		path := filepath.Join(t.TempDir(), "test.sqlite3")
		fn(t, openAndMigrate(t, path, config.SQLite))
	})
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		return
	}
	t.Run("postgres", func(t *testing.T) {
		database := openAndMigrate(t, url, config.Postgres)
		fn(t, database)
	})
}

func openAndMigrate(t *testing.T, url string, backend config.Backend) *db.DB {
	t.Helper()
	ctx := context.Background()
	database, err := db.Open(ctx, url, backend)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(ctx) })
	if backend == config.Postgres {
		if _, err := database.ExecContext(ctx, "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"); err != nil {
			t.Fatal(err)
		}
	}
	if err := migrate.Run(ctx, database); err != nil {
		t.Fatal(err)
	}
	return database
}

func mustUser(t *testing.T, database *db.DB, name string) *User {
	t.Helper()
	user, err := CreateUser(context.Background(), database, name, "hash", RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	return user
}

func mustCategory(t *testing.T, database *db.DB, userID int64, name string) *Category {
	t.Helper()
	category, err := CreateCategory(context.Background(), database, userID, name)
	if err != nil {
		t.Fatal(err)
	}
	return category
}

func mustFeed(t *testing.T, database *db.DB, categoryID int64, url string) *Feed {
	t.Helper()
	feed, err := CreateFeed(context.Background(), database, CreateFeedParams{CategoryID: categoryID, URL: url})
	if err != nil {
		t.Fatal(err)
	}
	return feed
}

func mustExec(t *testing.T, database *db.DB, query string, args ...any) {
	t.Helper()
	if _, err := database.ExecContext(context.Background(), query, args...); err != nil {
		t.Fatalf("%s: %v", query, err)
	}
}

func ptr[T any](v T) *T { return &v }

// sameInstant tolerates PostgreSQL's microsecond timestamp precision.
func sameInstant(a, b time.Time) bool {
	d := a.Sub(b)
	return d > -time.Millisecond && d < time.Millisecond
}

func approxNow(t *testing.T, got time.Time, within time.Duration) {
	t.Helper()
	if d := time.Since(got); d < -within || d > within {
		t.Errorf("time %v is not within %v of now", got, within)
	}
}
