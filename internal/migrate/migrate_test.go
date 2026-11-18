package migrate

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"testing/fstest"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/migrations"
)

const (
	fixturePath = "testdata/sqlx_migrated.sqlite3"
	vectorsPath = "../../testdata/vectors/migrations.json"
)

type appliedRow struct {
	Version     int64
	Description string
	Success     bool
	Checksum    string
}

func openFile(t *testing.T, path string) *db.DB {
	t.Helper()
	database, err := db.Open(context.Background(), path, config.SQLite)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(context.Background()) })
	return database
}

func openEmpty(t *testing.T) *db.DB {
	t.Helper()
	return openFile(t, filepath.Join(t.TempDir(), "empty.sqlite3"))
}

func openFixtureCopy(t *testing.T) *db.DB {
	t.Helper()
	raw, err := os.ReadFile(fixturePath)
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "rust.sqlite3")
	if err := os.WriteFile(path, raw, 0o600); err != nil {
		t.Fatal(err)
	}
	return openFile(t, path)
}

func readApplied(t *testing.T, database *db.DB) []appliedRow {
	t.Helper()
	rows, err := database.QueryContext(context.Background(),
		"SELECT version, description, success, checksum FROM _sqlx_migrations ORDER BY version")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = rows.Close() }()
	var out []appliedRow
	for rows.Next() {
		var row appliedRow
		var checksum []byte
		if err := rows.Scan(&row.Version, &row.Description, &row.Success, &checksum); err != nil {
			t.Fatal(err)
		}
		row.Checksum = hex.EncodeToString(checksum)
		out = append(out, row)
	}
	return out
}

func readSchema(t *testing.T, database *db.DB) map[string]string {
	t.Helper()
	rows, err := database.QueryContext(context.Background(),
		"SELECT name, COALESCE(sql, '') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'")
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = rows.Close() }()
	schema := map[string]string{}
	for rows.Next() {
		var name, ddl string
		if err := rows.Scan(&name, &ddl); err != nil {
			t.Fatal(err)
		}
		schema[name] = ddl
	}
	return schema
}

func TestRunOnEmptyDatabaseMatchesRustBuiltDatabase(t *testing.T) {
	ctx := context.Background()
	fresh := openEmpty(t)
	if err := Run(ctx, fresh); err != nil {
		t.Fatal(err)
	}
	rust := openFixtureCopy(t)

	got, want := readApplied(t, fresh), readApplied(t, rust)
	if len(got) != 16 {
		t.Fatalf("applied %d migrations, want 16", len(got))
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("row %d: got %+v, want %+v", i, got[i], want[i])
		}
	}
	gotSchema, wantSchema := readSchema(t, fresh), readSchema(t, rust)
	for name, ddl := range wantSchema {
		if gotSchema[name] != ddl {
			t.Errorf("schema object %s differs:\n got %q\nwant %q", name, gotSchema[name], ddl)
		}
	}
	if len(gotSchema) != len(wantSchema) {
		t.Errorf("schema has %d objects, want %d", len(gotSchema), len(wantSchema))
	}
}

func TestRunOnRustMigratedDatabaseIsNoOp(t *testing.T) {
	database := openFixtureCopy(t)
	before := readInstalledOn(t, database)
	if err := Run(context.Background(), database); err != nil {
		t.Fatal(err)
	}
	if after := readInstalledOn(t, database); after != before {
		t.Errorf("bookkeeping changed:\n before %q\n after  %q", before, after)
	}
}

func readInstalledOn(t *testing.T, database *db.DB) string {
	t.Helper()
	var out string
	err := database.QueryRowContext(context.Background(),
		"SELECT group_concat(version || '@' || installed_on || '@' || execution_time, ',') FROM _sqlx_migrations").Scan(&out)
	if err != nil {
		t.Fatal(err)
	}
	return out
}

func TestRunIsIdempotent(t *testing.T) {
	ctx := context.Background()
	database := openEmpty(t)
	for range 2 {
		if err := Run(ctx, database); err != nil {
			t.Fatal(err)
		}
	}
	if got := len(readApplied(t, database)); got != 16 {
		t.Errorf("rows = %d, want 16", got)
	}
}

func TestRunRecordsExecutionTime(t *testing.T) {
	database := openEmpty(t)
	if err := Run(context.Background(), database); err != nil {
		t.Fatal(err)
	}
	var unset int64
	err := database.QueryRowContext(context.Background(),
		"SELECT COUNT(*) FROM _sqlx_migrations WHERE execution_time <= 0").Scan(&unset)
	if err != nil {
		t.Fatal(err)
	}
	if unset != 0 {
		t.Errorf("%d rows still have the placeholder execution_time", unset)
	}
}

func TestRunRejectsTamperedChecksum(t *testing.T) {
	ctx := context.Background()
	database := openFixtureCopy(t)
	if _, err := database.ExecContext(ctx, "UPDATE _sqlx_migrations SET checksum = x'00' WHERE version = 3"); err != nil {
		t.Fatal(err)
	}
	if err := Run(ctx, database); !errors.Is(err, ErrVersionMismatch) {
		t.Errorf("err = %v, want ErrVersionMismatch", err)
	}
}

func TestRunRejectsDirtyDatabase(t *testing.T) {
	ctx := context.Background()
	database := openFixtureCopy(t)
	if _, err := database.ExecContext(ctx, "UPDATE _sqlx_migrations SET success = 0 WHERE version = 5"); err != nil {
		t.Fatal(err)
	}
	if err := Run(ctx, database); !errors.Is(err, ErrDirty) {
		t.Errorf("err = %v, want ErrDirty", err)
	}
}

func TestRunRejectsUnknownAppliedVersion(t *testing.T) {
	ctx := context.Background()
	database := openFixtureCopy(t)
	_, err := database.ExecContext(ctx,
		"INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (999, 'future', 1, x'00', 1)")
	if err != nil {
		t.Fatal(err)
	}
	if err := Run(ctx, database); !errors.Is(err, ErrVersionMissing) {
		t.Errorf("err = %v, want ErrVersionMissing", err)
	}
}

func TestEmbeddedChecksumsMatchRustVectors(t *testing.T) {
	raw, err := os.ReadFile(vectorsPath)
	if err != nil {
		t.Fatal(err)
	}
	var vectors map[string][]struct {
		Description string `json:"description"`
		File        string `json:"file"`
		SHA384      string `json:"sha384"`
		Version     int64  `json:"version"`
	}
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatal(err)
	}
	for dir, want := range vectors {
		got, err := load(migrations.FS, dir)
		if err != nil {
			t.Fatal(err)
		}
		if len(got) != len(want) {
			t.Fatalf("%s: %d migrations, want %d", dir, len(got), len(want))
		}
		for i, vector := range want {
			if got[i].Version != vector.Version ||
				got[i].Description != vector.Description ||
				hex.EncodeToString(got[i].Checksum) != vector.SHA384 {
				t.Errorf("%s/%s: got version %d %q %x", dir, vector.File, got[i].Version, got[i].Description, got[i].Checksum)
			}
		}
	}
}

func TestLoadOrdersByVersionAndDetectsNoTransaction(t *testing.T) {
	fsys := fstest.MapFS{
		"d/0010_late_one.sql":   {Data: []byte("SELECT 1;")},
		"d/0002_early_one.sql":  {Data: []byte("-- no-transaction\nSELECT 2;")},
		"d/README.md":           {Data: []byte("ignored")},
		"d/0003_third_item.sql": {Data: []byte("SELECT 3;")},
	}
	got, err := load(fsys, "d")
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 3 || got[0].Version != 2 || got[1].Version != 3 || got[2].Version != 10 {
		t.Fatalf("unexpected order: %+v", got)
	}
	if !got[0].NoTx || got[1].NoTx {
		t.Errorf("NoTx = %v, %v; want true, false", got[0].NoTx, got[1].NoTx)
	}
	if got[0].Description != "early one" {
		t.Errorf("description = %q", got[0].Description)
	}
}

func TestLoadRejectsBadFileName(t *testing.T) {
	for _, name := range []string{"d/abc_desc.sql", "d/0001_x.up.sql", "d/0001_x.down.sql"} {
		fsys := fstest.MapFS{name: {Data: []byte("SELECT 1;")}}
		if _, err := load(fsys, "d"); err == nil {
			t.Errorf("%s: expected error", name)
		}
	}
}

// sqlx skips a .sql file with no underscore, so README.sql must not stop
// startup.
func TestLoadSkipsNameWithoutUnderscore(t *testing.T) {
	fsys := fstest.MapFS{"d/README.sql": {Data: []byte("not a migration")}}
	got, err := load(fsys, "d")
	if err != nil || len(got) != 0 {
		t.Fatalf("got %v, %v", got, err)
	}
}

func TestRunFailsOnClosedDatabase(t *testing.T) {
	database := openEmpty(t)
	if err := database.Shutdown(context.Background()); err != nil {
		t.Fatal(err)
	}
	if err := Run(context.Background(), database); err == nil {
		t.Fatal("expected an error from a closed database")
	}
}

func TestLoadFailsWhenDirectoryIsMissing(t *testing.T) {
	if _, err := load(fstest.MapFS{}, "missing"); err == nil {
		t.Fatal("expected an error for a missing directory")
	}
}

func TestParseRejectsNameWithoutUnderscore(t *testing.T) {
	if _, err := parse("0001.sql", nil); err == nil {
		t.Fatal("expected an error")
	}
}

func applyOn(t *testing.T, database *db.DB, all []Migration) error {
	t.Helper()
	ctx := context.Background()
	conn, err := database.Conn(ctx)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	return apply(ctx, conn, database.Dialect(), all)
}

func migrationOf(t *testing.T, name, script string) Migration {
	t.Helper()
	m, err := parse(name, []byte(script))
	if err != nil {
		t.Fatal(err)
	}
	return m
}

func TestApplyRollsBackAFailingTransactionalMigration(t *testing.T) {
	database := openEmpty(t)
	bad := migrationOf(t, "0001_bad.sql", "CREATE TABLE half_done (id INTEGER);\nSELECT * FROM no_such_table;")
	if err := applyOn(t, database, []Migration{bad}); err == nil {
		t.Fatal("expected the failing script to return an error")
	}
	var tables int64
	err := database.QueryRowContext(context.Background(),
		"SELECT COUNT(*) FROM sqlite_master WHERE name = 'half_done'").Scan(&tables)
	if err != nil || tables != 0 {
		t.Errorf("half_done tables = %d, err = %v; want the script rolled back", tables, err)
	}
	if len(readApplied(t, database)) != 0 {
		t.Error("a failed migration must not be recorded")
	}
}

func TestApplyRunsNoTransactionMigrationAndRecordsIt(t *testing.T) {
	database := openEmpty(t)
	good := migrationOf(t, "0001_plain.sql", "-- no-transaction\nCREATE TABLE plain (id INTEGER);")
	if err := applyOn(t, database, []Migration{good}); err != nil {
		t.Fatal(err)
	}
	if applied := readApplied(t, database); len(applied) != 1 || !applied[0].Success {
		t.Fatalf("applied = %+v", applied)
	}
}

func TestApplyReportsFailingNoTransactionMigration(t *testing.T) {
	database := openEmpty(t)
	bad := migrationOf(t, "0001_bad.sql", "-- no-transaction\nSELECT * FROM no_such_table;")
	if err := applyOn(t, database, []Migration{bad}); err == nil {
		t.Fatal("expected an error")
	}
	if len(readApplied(t, database)) != 0 {
		t.Error("a failed migration must not be recorded")
	}
}

func TestApplyFailsWhenTheBookkeepingTableIsUnusable(t *testing.T) {
	database := openEmpty(t)
	ctx := context.Background()
	// A table with the bookkeeping name already exists, so CREATE TABLE IF NOT EXISTS
	// succeeds while every later read of its missing columns fails.
	if _, err := database.ExecContext(ctx, "CREATE TABLE "+tableName+" (version TEXT)"); err != nil {
		t.Fatal(err)
	}
	if err := applyOn(t, database, nil); err == nil {
		t.Fatal("expected an error reading a table without the expected columns")
	}
}

func TestApplyScansBadChecksumColumnAsError(t *testing.T) {
	database := openEmpty(t)
	ctx := context.Background()
	if _, err := database.ExecContext(ctx, "CREATE TABLE "+tableName+
		" (version TEXT, checksum BLOB, success BOOLEAN)"); err != nil {
		t.Fatal(err)
	}
	if _, err := database.ExecContext(ctx, "INSERT INTO "+tableName+" VALUES ('not a number', x'00', 1)"); err != nil {
		t.Fatal(err)
	}
	if err := applyOn(t, database, nil); err == nil {
		t.Fatal("expected a scan error for a non-numeric version")
	}
}
