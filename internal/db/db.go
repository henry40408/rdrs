// Package db is the database handle shared by every model: one pool per
// backend, a write-priority scheduler for SQLite, and the dialect forks.
package db

import (
	"context"
	"database/sql"
	"fmt"
	"log/slog"
	"net/url"
	"strings"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/stdlib"
	_ "modernc.org/sqlite" // registers the "sqlite" driver

	"github.com/henry40408/rdrs/internal/config"
)

const sqliteMaxOpenConns = 5

// sqlitePragmas are applied to every connection. WAL with synchronous=NORMAL is
// durable to checkpoint without a per-commit fsync. analysis_limit bounds
// PRAGMA optimize so ANALYZE does not read every index in full.
var sqlitePragmas = []string{
	"journal_mode(WAL)",
	"synchronous(NORMAL)",
	"busy_timeout(5000)",
	"cache_size(-20000)",
	"mmap_size(134217728)",
	"temp_store(MEMORY)",
	"analysis_limit(400)",
	"foreign_keys(1)",
}

// DB is a pool, a priority, and the SQLite scheduler shared by every handle
// derived from the same pool. Copies made by Background keep the pool.
type DB struct {
	pool     *sql.DB
	dialect  Dialect
	sched    *sched
	priority Priority
}

// Open connects to the database named by databaseURL: a file path (optionally
// prefixed with sqlite://) for SQLite, a postgres:// URL for PostgreSQL. It
// does not migrate; run migrate.Run before serving.
func Open(ctx context.Context, databaseURL string, backend config.Backend) (*DB, error) {
	if backend == config.Postgres {
		return openPostgres(ctx, databaseURL)
	}
	return openSQLite(ctx, sqliteDSN(databaseURL), sqliteMaxOpenConns)
}

// OpenMemory opens a private in-memory SQLite database. It holds one
// connection, so every query sees the same database.
func OpenMemory(ctx context.Context) (*DB, error) {
	return openSQLite(ctx, sqliteDSNFor("file::memory:"), 1)
}

func sqliteDSN(databaseURL string) string {
	path := strings.TrimSpace(databaseURL)
	path = strings.TrimPrefix(path, "sqlite://")
	path = strings.TrimPrefix(path, "sqlite:")
	// The path becomes part of a file: URI, where these three are delimiters.
	escaped := strings.NewReplacer("%", "%25", "?", "%3f", "#", "%23").Replace(path)
	return sqliteDSNFor("file:" + escaped)
}

// sqliteDSNFor appends the pragmas. _txlock=immediate makes every transaction
// BEGIN IMMEDIATE: a deferred read-to-write promotion returns SQLITE_BUSY
// without honoring busy_timeout, which concurrent feed syncs would hit.
func sqliteDSNFor(base string) string {
	query := url.Values{}
	for _, pragma := range sqlitePragmas {
		query.Add("_pragma", pragma)
	}
	query.Set("_txlock", "immediate")
	return base + "?" + query.Encode()
}

func openSQLite(ctx context.Context, dsn string, maxConns int) (*DB, error) {
	pool, err := sql.Open("sqlite", dsn)
	if err != nil {
		return nil, fmt.Errorf("open sqlite: %w", err)
	}
	pool.SetMaxOpenConns(maxConns)
	return finishOpen(ctx, pool, DialectSQLite)
}

func openPostgres(ctx context.Context, databaseURL string) (*DB, error) {
	cfg, err := pgx.ParseConfig(strings.TrimSpace(databaseURL))
	if err != nil {
		return nil, fmt.Errorf("parse postgres url: %w", err)
	}
	// Timestamps and the to_char cursor use the session zone; any zone but UTC
	// would corrupt pagination.
	cfg.RuntimeParams["timezone"] = "UTC"
	return finishOpen(ctx, stdlib.OpenDB(*cfg), DialectPostgres)
}

func finishOpen(ctx context.Context, pool *sql.DB, dialect Dialect) (*DB, error) {
	if err := pool.PingContext(ctx); err != nil {
		_ = pool.Close()
		return nil, fmt.Errorf("connect database: %w", err)
	}
	return &DB{pool: pool, dialect: dialect, sched: newSched(), priority: User}, nil
}

// Dialect is the SQL dialect of the backend.
func (d *DB) Dialect() Dialect { return d.dialect }

// IsPostgres reports whether the backend is PostgreSQL.
func (d *DB) IsPostgres() bool { return d.dialect == DialectPostgres }

// Background is a handle over the same pool and scheduler whose operations
// yield to interactive work on SQLite.
func (d *DB) Background() *DB {
	background := *d
	background.priority = Background
	return &background
}

// admit takes write-priority admission for one operation. It returns the
// release func to call when the operation ends.
func (d *DB) admit(ctx context.Context) (release func(), err error) {
	if d.dialect != DialectSQLite {
		return noRelease, nil
	}
	if d.priority == User {
		return d.sched.enterUser(), nil
	}
	return noRelease, d.sched.waitIdle(ctx)
}

// ExecContext runs a statement that returns no rows.
func (d *DB) ExecContext(ctx context.Context, query string, args ...any) (sql.Result, error) {
	release, err := d.admit(ctx)
	if err != nil {
		return nil, err
	}
	defer release()
	return d.pool.ExecContext(ctx, d.dialect.Rewrite(query), args...)
}

// QueryContext runs a query. The caller must close the returned Rows, which
// holds write-priority admission until then.
func (d *DB) QueryContext(ctx context.Context, query string, args ...any) (*Rows, error) {
	release, err := d.admit(ctx)
	if err != nil {
		return nil, err
	}
	rows, err := d.pool.QueryContext(ctx, d.dialect.Rewrite(query), args...)
	if err != nil {
		release()
		return nil, err
	}
	return &Rows{Rows: rows, release: release}, nil
}

// QueryRowContext runs a query expected to return at most one row. Admission
// is held until Scan.
func (d *DB) QueryRowContext(ctx context.Context, query string, args ...any) *Row {
	release, err := d.admit(ctx)
	if err != nil {
		return &Row{err: err, release: noRelease}
	}
	return &Row{row: d.pool.QueryRowContext(ctx, d.dialect.Rewrite(query), args...), release: release}
}

// Begin starts a write transaction and holds write-priority admission until
// Commit or Rollback. On SQLite it is BEGIN IMMEDIATE.
func (d *DB) Begin(ctx context.Context) (*Tx, error) {
	release, err := d.admit(ctx)
	if err != nil {
		return nil, err
	}
	tx, err := d.pool.BeginTx(ctx, nil)
	if err != nil {
		release()
		return nil, err
	}
	return &Tx{tx: tx, dialect: d.dialect, release: release}, nil
}

// Shutdown closes the pool. On SQLite it truncates the WAL first so no
// sidecar files linger.
func (d *DB) Shutdown(ctx context.Context) error {
	if d.dialect == DialectSQLite {
		d.checkpointBeforeClose(ctx)
	}
	return d.pool.Close()
}

func (d *DB) checkpointBeforeClose(ctx context.Context) {
	slog.Info("executing WAL checkpoint before shutdown", "event", "db.checkpoint_started")
	if _, err := d.pool.ExecContext(ctx, "PRAGMA wal_checkpoint(TRUNCATE);"); err != nil {
		slog.Error("WAL checkpoint failed", "event", "db.checkpoint_failed", "error", err)
	}
}

// Rows is sql.Rows that releases write-priority admission when done.
type Rows struct {
	*sql.Rows
	release func()
}

// Next advances to the next row, releasing admission after the last one.
func (r *Rows) Next() bool {
	if r.Rows.Next() {
		return true
	}
	r.release()
	return false
}

// Close closes the rows and releases admission. It is safe to call twice.
func (r *Rows) Close() error {
	r.release()
	return r.Rows.Close()
}

// Row is the single-row result of QueryRowContext.
type Row struct {
	row     *sql.Row
	err     error
	release func()
}

// Scan copies the row into dest and releases admission.
func (r *Row) Scan(dest ...any) error {
	defer r.release()
	if r.err != nil {
		return r.err
	}
	return r.row.Scan(dest...)
}

// Executor is what models accept, so one function works on a DB or inside a Tx.
type Executor interface {
	ExecContext(ctx context.Context, query string, args ...any) (sql.Result, error)
	QueryContext(ctx context.Context, query string, args ...any) (*Rows, error)
	QueryRowContext(ctx context.Context, query string, args ...any) *Row
	// Dialect lets dynamic SQL builders pick backend-specific fragments.
	Dialect() Dialect
}

var (
	_ Executor = (*DB)(nil)
	_ Executor = (*Tx)(nil)
)

// Conn pins one connection for work that needs session state, such as the
// migrator's PostgreSQL advisory lock. It skips write-priority admission, so
// use it only before the server starts.
func (d *DB) Conn(ctx context.Context) (*sql.Conn, error) {
	return d.pool.Conn(ctx)
}
