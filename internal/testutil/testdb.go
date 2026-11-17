// Package testutil holds helpers shared by tests of several packages.
package testutil

import (
	"context"
	"path/filepath"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/migrate"
)

// SQLite opens a freshly migrated SQLite database in a temporary directory and
// closes it when the test ends.
func SQLite(t testing.TB) *db.DB {
	t.Helper()
	ctx := context.Background()
	database, err := db.Open(ctx, filepath.Join(t.TempDir(), "test.sqlite3"), config.SQLite)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(ctx) })
	if err := migrate.Run(ctx, database); err != nil {
		t.Fatal(err)
	}
	return database
}
