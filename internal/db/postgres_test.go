package db

import (
	"context"
	"os"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
)

func openPostgresForTest(t *testing.T) *DB {
	t.Helper()
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		t.Skip("TEST_DATABASE_URL not set")
	}
	database, err := Open(context.Background(), url, config.Postgres)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.pool.Close() })
	return database
}

func TestPostgresSessionIsUTC(t *testing.T) {
	database := openPostgresForTest(t)
	var zone string
	if err := database.QueryRowContext(context.Background(), "SHOW TIME ZONE").Scan(&zone); err != nil {
		t.Fatal(err)
	}
	if zone != "UTC" {
		t.Errorf("time zone = %q, want UTC", zone)
	}
}

func TestPostgresUniqueViolationAndLock(t *testing.T) {
	database := openPostgresForTest(t)
	ctx := context.Background()
	tx, err := database.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = tx.Rollback() }()
	mustExec(t, tx, `CREATE TEMP TABLE "user" (id INT PRIMARY KEY, name TEXT UNIQUE)`)
	if err := tx.LockUserTable(ctx); err != nil {
		t.Fatalf("LockUserTable: %v", err)
	}
	mustExec(t, tx, `INSERT INTO "user" VALUES (1, 'a')`)
	_, err = tx.ExecContext(ctx, `INSERT INTO "user" VALUES (2, 'a')`)
	if !IsUniqueViolation(err) {
		t.Errorf("unique violation not detected: %v", err)
	}
}
