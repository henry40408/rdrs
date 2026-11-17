package migrate

import (
	"context"
	"errors"
	"os"
	"sync"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
)

// openFreshPostgres connects to TEST_DATABASE_URL and drops everything the
// migrator creates, so the test starts from an empty schema.
func openFreshPostgres(t *testing.T) *db.DB {
	t.Helper()
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		t.Skip("TEST_DATABASE_URL not set")
	}
	ctx := context.Background()
	database, err := db.Open(ctx, url, config.Postgres)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(ctx) })
	if _, err := database.ExecContext(ctx, "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"); err != nil {
		t.Fatal(err)
	}
	return database
}

func TestPostgresRunAppliesAllAndIsIdempotent(t *testing.T) {
	ctx := context.Background()
	database := openFreshPostgres(t)
	for range 2 {
		if err := Run(ctx, database); err != nil {
			t.Fatal(err)
		}
	}
	rows := readApplied(t, database)
	if len(rows) != 16 {
		t.Fatalf("applied %d, want 16", len(rows))
	}
	for _, row := range rows {
		if !row.Success {
			t.Errorf("version %d not successful", row.Version)
		}
	}
}

func TestPostgresConcurrentRunsSerialize(t *testing.T) {
	ctx := context.Background()
	database := openFreshPostgres(t)
	var wg sync.WaitGroup
	errs := make(chan error, 3)
	for range 3 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			errs <- Run(ctx, database)
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Errorf("concurrent Run: %v", err)
		}
	}
}

func TestPostgresRejectsTamperedChecksum(t *testing.T) {
	ctx := context.Background()
	database := openFreshPostgres(t)
	if err := Run(ctx, database); err != nil {
		t.Fatal(err)
	}
	if _, err := database.ExecContext(ctx, "UPDATE _sqlx_migrations SET checksum = '\\x00' WHERE version = 2"); err != nil {
		t.Fatal(err)
	}
	if err := Run(ctx, database); !errors.Is(err, ErrVersionMismatch) {
		t.Errorf("err = %v, want ErrVersionMismatch", err)
	}
}
