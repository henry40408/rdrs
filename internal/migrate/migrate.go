// Package migrate applies the embedded SQL migrations and keeps the
// _sqlx_migrations bookkeeping table byte-compatible with sqlx 0.9, so a
// database can move between the Rust and Go builds.
package migrate

import (
	"cmp"
	"context"
	"crypto/sha512"
	"database/sql"
	"errors"
	"fmt"
	"hash/crc32"
	"io/fs"
	"log/slog"
	"path"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/migrations"
)

const (
	tableName = "_sqlx_migrations"
	// noTransactionMarker opts a migration out of its wrapping transaction.
	noTransactionMarker = "-- no-transaction"
	// advisoryLockSalt is the constant sqlx multiplies the CRC-32 of the
	// database name by; matching it keeps Rust and Go instances mutually
	// exclusive while one of them migrates.
	advisoryLockSalt = 0x3d32ad9e
)

// Sentinel errors, matching sqlx's MigrateError variants.
var (
	ErrDirty           = errors.New("migration was partially applied; fix the database manually")
	ErrVersionMismatch = errors.New("migration was previously applied but has been modified")
	ErrVersionMissing  = errors.New("migration was applied but is missing from the embedded set")
)

// Migration is one embedded SQL file.
type Migration struct {
	Version     int64
	Description string
	SQL         string
	Checksum    []byte
	NoTx        bool
}

// Run applies every pending migration for the database's backend.
func Run(ctx context.Context, database *db.DB) error {
	dir := "sqlite"
	if database.IsPostgres() {
		dir = "postgres"
	}
	all, err := load(migrations.FS, dir)
	if err != nil {
		return err
	}
	conn, err := database.Conn(ctx)
	if err != nil {
		return fmt.Errorf("migrate: acquire connection: %w", err)
	}
	defer func() { _ = conn.Close() }()

	if database.IsPostgres() {
		unlock, err := lockPostgres(ctx, conn)
		if err != nil {
			return err
		}
		defer unlock()
	}
	return apply(ctx, conn, database.Dialect(), all)
}

// load reads dir/NNNN_some_description.sql files in version order.
func load(fsys fs.FS, dir string) ([]Migration, error) {
	entries, err := fs.ReadDir(fsys, dir)
	if err != nil {
		return nil, fmt.Errorf("migrate: read %s: %w", dir, err)
	}
	var all []Migration
	for _, entry := range entries {
		if !isMigrationFile(entry) {
			continue
		}
		if strings.HasSuffix(entry.Name(), ".up.sql") || strings.HasSuffix(entry.Name(), ".down.sql") {
			return nil, fmt.Errorf("migrate: %s: reversible migrations are not supported", entry.Name())
		}
		raw, err := fs.ReadFile(fsys, path.Join(dir, entry.Name()))
		if err != nil {
			return nil, fmt.Errorf("migrate: read %s: %w", entry.Name(), err)
		}
		migration, err := parse(entry.Name(), raw)
		if err != nil {
			return nil, err
		}
		all = append(all, migration)
	}
	slices.SortFunc(all, func(a, b Migration) int { return cmp.Compare(a.Version, b.Version) })
	return all, nil
}

// isMigrationFile mirrors sqlx, which skips directories, non-.sql files and
// names without an underscore (a README.sql, say) rather than failing.
func isMigrationFile(entry fs.DirEntry) bool {
	name := entry.Name()
	return !entry.IsDir() && strings.HasSuffix(name, ".sql") && strings.Contains(name, "_")
}

func parse(fileName string, raw []byte) (Migration, error) {
	stem := strings.TrimSuffix(fileName, ".sql")
	versionText, description, found := strings.Cut(stem, "_")
	if !found {
		return Migration{}, fmt.Errorf("migrate: %s: expected NNNN_description.sql", fileName)
	}
	version, err := strconv.ParseInt(versionText, 10, 64)
	if err != nil {
		return Migration{}, fmt.Errorf("migrate: %s: bad version: %w", fileName, err)
	}
	sum := sha512.Sum384(raw)
	text := string(raw)
	return Migration{
		Version:     version,
		Description: strings.ReplaceAll(description, "_", " "),
		SQL:         text,
		Checksum:    sum[:],
		NoTx:        strings.HasPrefix(text, noTransactionMarker),
	}, nil
}

// lockPostgres takes the session-level advisory lock sqlx uses, so concurrent
// processes migrate one at a time.
func lockPostgres(ctx context.Context, conn *sql.Conn) (unlock func(), err error) {
	var name string
	if err := conn.QueryRowContext(ctx, "SELECT current_database()").Scan(&name); err != nil {
		return nil, fmt.Errorf("migrate: current_database: %w", err)
	}
	lockID := advisoryLockSalt * int64(crc32.ChecksumIEEE([]byte(name)))
	if _, err := conn.ExecContext(ctx, "SELECT pg_advisory_lock($1)", lockID); err != nil {
		return nil, fmt.Errorf("migrate: advisory lock: %w", err)
	}
	return func() {
		// The caller's context may already be cancelled; the lock must still go.
		_, _ = conn.ExecContext(context.WithoutCancel(ctx), "SELECT pg_advisory_unlock($1)", lockID)
	}, nil
}

func apply(ctx context.Context, conn *sql.Conn, dialect db.Dialect, all []Migration) error {
	if err := ensureTable(ctx, conn, dialect); err != nil {
		return err
	}
	if err := checkNotDirty(ctx, conn); err != nil {
		return err
	}
	applied, err := listApplied(ctx, conn)
	if err != nil {
		return err
	}
	if err := validate(applied, all); err != nil {
		return err
	}
	for _, migration := range all {
		checksum, done := applied[migration.Version]
		if done {
			if string(checksum) != string(migration.Checksum) {
				return fmt.Errorf("migrate: version %d: %w", migration.Version, ErrVersionMismatch)
			}
			continue
		}
		if err := applyOne(ctx, conn, migration); err != nil {
			return err
		}
		slog.Info("applied migration", "event", "db.migration_applied", "version", migration.Version, "description", migration.Description)
	}
	return nil
}

func ensureTable(ctx context.Context, conn *sql.Conn, dialect db.Dialect) error {
	timestamp, blob := "TIMESTAMP", "BLOB"
	if dialect == db.DialectPostgres {
		timestamp, blob = "TIMESTAMPTZ", "BYTEA"
	}
	installedOn := "CURRENT_TIMESTAMP"
	if dialect == db.DialectPostgres {
		installedOn = "now()"
	}
	query := fmt.Sprintf(`CREATE TABLE IF NOT EXISTS %s (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on %s NOT NULL DEFAULT %s,
    success BOOLEAN NOT NULL,
    checksum %s NOT NULL,
    execution_time BIGINT NOT NULL
)`, tableName, timestamp, installedOn, blob)
	if _, err := conn.ExecContext(ctx, query); err != nil {
		return fmt.Errorf("migrate: create %s: %w", tableName, err)
	}
	return nil
}

func checkNotDirty(ctx context.Context, conn *sql.Conn) error {
	var version int64
	err := conn.QueryRowContext(ctx,
		"SELECT version FROM "+tableName+" WHERE success = false ORDER BY version LIMIT 1").Scan(&version)
	if errors.Is(err, sql.ErrNoRows) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("migrate: dirty check: %w", err)
	}
	return fmt.Errorf("migrate: version %d: %w", version, ErrDirty)
}

// listApplied maps each applied version to its recorded checksum.
func listApplied(ctx context.Context, conn *sql.Conn) (map[int64][]byte, error) {
	rows, err := conn.QueryContext(ctx, "SELECT version, checksum FROM "+tableName+" ORDER BY version")
	if err != nil {
		return nil, fmt.Errorf("migrate: list applied: %w", err)
	}
	defer func() { _ = rows.Close() }()
	applied := map[int64][]byte{}
	for rows.Next() {
		var version int64
		var checksum []byte
		if err := rows.Scan(&version, &checksum); err != nil {
			return nil, fmt.Errorf("migrate: scan applied: %w", err)
		}
		applied[version] = checksum
	}
	return applied, rows.Err()
}

// validate rejects a database that has migrations this binary does not know,
// which means it was migrated by a newer build.
func validate(applied map[int64][]byte, all []Migration) error {
	known := make(map[int64]bool, len(all))
	for _, migration := range all {
		known[migration.Version] = true
	}
	missing := make([]int64, 0)
	for version := range applied {
		if !known[version] {
			missing = append(missing, version)
		}
	}
	if len(missing) == 0 {
		return nil
	}
	slices.Sort(missing)
	return fmt.Errorf("migrate: version %d: %w", missing[0], ErrVersionMissing)
}

// applyOne runs the script and its bookkeeping row together, so a crash never
// leaves a migration applied but unrecorded. execution_time can only be known
// afterwards, hence the -1 placeholder and the follow-up update.
func applyOne(ctx context.Context, conn *sql.Conn, migration Migration) error {
	start := time.Now()
	if err := runScript(ctx, conn, migration); err != nil {
		return fmt.Errorf("migrate: version %d: %w", migration.Version, err)
	}
	_, err := conn.ExecContext(ctx,
		"UPDATE "+tableName+" SET execution_time = $1 WHERE version = $2",
		time.Since(start).Nanoseconds(), migration.Version)
	if err != nil {
		return fmt.Errorf("migrate: version %d: record execution time: %w", migration.Version, err)
	}
	return nil
}

const insertApplied = "INSERT INTO " + tableName +
	" (version, description, success, checksum, execution_time) VALUES ($1, $2, TRUE, $3, -1)"

func runScript(ctx context.Context, conn *sql.Conn, migration Migration) error {
	if migration.NoTx {
		if _, err := conn.ExecContext(ctx, migration.SQL); err != nil {
			return err
		}
		_, err := conn.ExecContext(ctx, insertApplied, migration.Version, migration.Description, migration.Checksum)
		return err
	}
	tx, err := conn.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	if _, err := tx.ExecContext(ctx, migration.SQL); err != nil {
		return err
	}
	if _, err := tx.ExecContext(ctx, insertApplied, migration.Version, migration.Description, migration.Checksum); err != nil {
		return err
	}
	return tx.Commit()
}
