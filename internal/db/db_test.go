package db

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"github.com/henry40408/rdrs/internal/config"
)

func openTemp(t *testing.T) (*DB, string) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "test.sqlite3")
	database, err := Open(context.Background(), path, config.SQLite)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.pool.Close() })
	return database, path
}

func mustExec(t *testing.T, e Executor, query string, args ...any) {
	t.Helper()
	if _, err := e.ExecContext(context.Background(), query, args...); err != nil {
		t.Fatalf("%s: %v", query, err)
	}
}

func scanInt(t *testing.T, e Executor, query string, args ...any) int64 {
	t.Helper()
	var n int64
	if err := e.QueryRowContext(context.Background(), query, args...).Scan(&n); err != nil {
		t.Fatalf("%s: %v", query, err)
	}
	return n
}

func TestSQLitePragmas(t *testing.T) {
	database, _ := openTemp(t)
	var mode string
	if err := database.QueryRowContext(context.Background(), "PRAGMA journal_mode;").Scan(&mode); err != nil {
		t.Fatal(err)
	}
	if mode != "wal" {
		t.Errorf("journal_mode = %q, want wal", mode)
	}
	if got := scanInt(t, database, "PRAGMA foreign_keys;"); got != 1 {
		t.Errorf("foreign_keys = %d, want 1", got)
	}
	if got := scanInt(t, database, "PRAGMA busy_timeout;"); got != 5000 {
		t.Errorf("busy_timeout = %d, want 5000", got)
	}
}

func TestDollarPlaceholdersMayRepeat(t *testing.T) {
	database, _ := openTemp(t)
	got := scanInt(t, database, "SELECT $1 + $1 + $2", 3, 4)
	if got != 10 {
		t.Errorf("got %d, want 10", got)
	}
}

func TestConcurrentWritersDoNotSeeBusy(t *testing.T) {
	database, _ := openTemp(t)
	ctx := context.Background()
	mustExec(t, database, "CREATE TABLE counter (n INTEGER NOT NULL)")
	mustExec(t, database, "INSERT INTO counter VALUES (0)")

	const writers = 8
	var wg sync.WaitGroup
	errs := make(chan error, writers)
	for range writers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			errs <- incrementCounter(ctx, database)
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Errorf("writer failed: %v", err)
		}
	}
	if got := scanInt(t, database, "SELECT n FROM counter"); got != writers {
		t.Errorf("counter = %d, want %d", got, writers)
	}
}

// incrementCounter reads then writes, the pattern that fails with SQLITE_BUSY
// under a deferred transaction.
func incrementCounter(ctx context.Context, database *DB) error {
	tx, err := database.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	var n int64
	if err := tx.QueryRowContext(ctx, "SELECT n FROM counter").Scan(&n); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx, "UPDATE counter SET n = $1", n+1); err != nil {
		return err
	}
	return tx.Commit()
}

func TestIsUniqueViolationSQLite(t *testing.T) {
	database, _ := openTemp(t)
	ctx := context.Background()
	mustExec(t, database, "CREATE TABLE u (id INTEGER PRIMARY KEY, name TEXT UNIQUE)")
	mustExec(t, database, "INSERT INTO u (id, name) VALUES (1, 'a')")

	_, uniqueErr := database.ExecContext(ctx, "INSERT INTO u (id, name) VALUES (2, 'a')")
	if !IsUniqueViolation(uniqueErr) {
		t.Errorf("unique column violation not detected: %v", uniqueErr)
	}
	_, pkErr := database.ExecContext(ctx, "INSERT INTO u (id, name) VALUES (1, 'b')")
	if !IsUniqueViolation(pkErr) {
		t.Errorf("primary key violation not detected: %v", pkErr)
	}
	_, otherErr := database.ExecContext(ctx, "INSERT INTO missing VALUES (1)")
	if IsUniqueViolation(otherErr) || IsUniqueViolation(nil) {
		t.Error("non-unique error reported as unique violation")
	}
}

func TestRollbackAfterCommitIsNoOp(t *testing.T) {
	database, _ := openTemp(t)
	mustExec(t, database, "CREATE TABLE t (n INTEGER)")
	tx, err := database.Begin(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	mustExec(t, tx, "INSERT INTO t VALUES (1)")
	if err := tx.Commit(); err != nil {
		t.Fatal(err)
	}
	if err := tx.Rollback(); err != nil {
		t.Errorf("Rollback after Commit = %v, want nil", err)
	}
	if got := scanInt(t, database, "SELECT COUNT(*) FROM t"); got != 1 {
		t.Errorf("rows = %d, want 1", got)
	}
}

func TestRollbackDiscardsWrites(t *testing.T) {
	database, _ := openTemp(t)
	mustExec(t, database, "CREATE TABLE t (n INTEGER)")
	tx, err := database.Begin(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	mustExec(t, tx, "INSERT INTO t VALUES (1)")
	if err := tx.Rollback(); err != nil {
		t.Fatal(err)
	}
	if got := scanInt(t, database, "SELECT COUNT(*) FROM t"); got != 0 {
		t.Errorf("rows = %d, want 0", got)
	}
}

func TestRowsAndRowReleaseAdmission(t *testing.T) {
	database, _ := openTemp(t)
	ctx := context.Background()
	mustExec(t, database, "CREATE TABLE t (n INTEGER)")
	mustExec(t, database, "INSERT INTO t VALUES (1), (2)")

	rows, err := database.QueryContext(ctx, "SELECT n FROM t")
	if err != nil {
		t.Fatal(err)
	}
	for rows.Next() {
	}
	if err := rows.Close(); err != nil {
		t.Fatal(err)
	}
	_ = scanInt(t, database, "SELECT COUNT(*) FROM t")

	// A background operation only proceeds once every user admission is gone.
	if err := database.Background().sched.waitIdle(ctx); err != nil {
		t.Fatalf("admission leaked: %v", err)
	}
}

func TestRunMaintenanceVacuumsFragmentedFile(t *testing.T) {
	database, _ := openTemp(t)
	ctx := context.Background()
	mustExec(t, database, "CREATE TABLE blob (b BLOB)")
	for range 40 {
		mustExec(t, database, "INSERT INTO blob VALUES (zeroblob(65536))")
	}
	mustExec(t, database, "DELETE FROM blob")

	vacuumed, err := database.RunMaintenance(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if !vacuumed {
		t.Error("expected VACUUM after freeing most pages")
	}
	vacuumed, err = database.RunMaintenance(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if vacuumed {
		t.Error("second run must not vacuum a compact file")
	}
}

func TestShutdownRemovesWAL(t *testing.T) {
	database, path := openTemp(t)
	mustExec(t, database, "CREATE TABLE t (n INTEGER)")
	mustExec(t, database, "INSERT INTO t VALUES (1)")
	if err := database.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path + "-wal"); !os.IsNotExist(err) {
		t.Errorf("-wal file still present after shutdown: %v", err)
	}
}

func TestOpenMemory(t *testing.T) {
	database, err := OpenMemory(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = database.pool.Close() }()
	mustExec(t, database, "CREATE TABLE t (n INTEGER)")
	mustExec(t, database, "INSERT INTO t VALUES (7)")
	if got := scanInt(t, database, "SELECT n FROM t"); got != 7 {
		t.Errorf("got %d, want 7", got)
	}
}

func TestOperationsOnAClosedPoolReturnTheError(t *testing.T) {
	ctx := context.Background()
	database, _ := openTemp(t)
	if err := database.Shutdown(ctx); err != nil {
		t.Fatal(err)
	}
	if _, err := database.ExecContext(ctx, "SELECT 1"); err == nil {
		t.Error("ExecContext: want an error")
	}
	if rows, err := database.QueryContext(ctx, "SELECT 1"); err == nil {
		_ = rows.Close()
		t.Error("QueryContext: want an error")
	}
	if err := database.QueryRowContext(ctx, "SELECT 1").Scan(new(int64)); err == nil {
		t.Error("QueryRowContext: want an error")
	}
	if _, err := database.Begin(ctx); err == nil {
		t.Error("Begin: want an error")
	}
	if err := database.Optimize(ctx); err == nil {
		t.Error("Optimize: want an error")
	}
	if _, err := database.RunMaintenance(ctx); err == nil {
		t.Error("RunMaintenance: want an error")
	}
}

func TestBackgroundOperationsStopWhenTheContextIsCancelled(t *testing.T) {
	database, _ := openTemp(t)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	background := database.Background()
	// Hold user admission so the background handle has to wait.
	release := database.sched.enterUser()
	defer release()
	if _, err := background.ExecContext(ctx, "SELECT 1"); err == nil {
		t.Error("ExecContext: want the context error")
	}
	if rows, err := background.QueryContext(ctx, "SELECT 1"); err == nil {
		_ = rows.Close()
		t.Error("QueryContext: want the context error")
	}
	if err := background.QueryRowContext(ctx, "SELECT 1").Scan(new(int64)); err == nil {
		t.Error("QueryRowContext: want the context error")
	}
	if _, err := background.Begin(ctx); err == nil {
		t.Error("Begin: want the context error")
	}
}

func TestOpenRejectsBadTargets(t *testing.T) {
	ctx := context.Background()
	if _, err := Open(ctx, filepath.Join(t.TempDir(), "missing-dir", "x.sqlite3"), config.SQLite); err == nil {
		t.Error("sqlite in a missing directory: want an error")
	}
	if _, err := Open(ctx, "postgres://%zz", config.Postgres); err == nil {
		t.Error("malformed postgres URL: want an error")
	}
}

func TestTxOperationsAfterCommitReturnTheError(t *testing.T) {
	ctx := context.Background()
	database, _ := openTemp(t)
	tx, err := database.Begin(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if err := tx.Commit(); err != nil {
		t.Fatal(err)
	}
	if rows, err := tx.QueryContext(ctx, "SELECT 1"); err == nil {
		_ = rows.Close()
		t.Error("QueryContext after commit: want an error")
	}
	if err := tx.LockUserTable(ctx); err != nil {
		t.Errorf("LockUserTable is a no-op on SQLite, got %v", err)
	}
}
