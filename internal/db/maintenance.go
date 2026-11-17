package db

import (
	"context"
	"fmt"
)

// vacuumFreelistRatio is the freed-page fraction that triggers a full VACUUM,
// which rewrites the whole file.
const vacuumFreelistRatio = 0.20

// Optimize refreshes stale SQLite planner statistics with PRAGMA optimize.
// Missing stats after a new migration index cause bad plans. It is a no-op on
// PostgreSQL, where autoanalyze does this.
func (d *DB) Optimize(ctx context.Context) error {
	if d.dialect != DialectSQLite {
		return nil
	}
	_, err := d.ExecContext(ctx, "PRAGMA optimize;")
	return err
}

// RunMaintenance runs after a retention prune: refresh planner stats, a VACUUM
// gated on the freelist ratio, then a truncating WAL checkpoint. It reports
// whether a VACUUM ran. SQLite only; it must run outside a transaction.
func (d *DB) RunMaintenance(ctx context.Context) (vacuumed bool, err error) {
	if d.dialect != DialectSQLite {
		return false, nil
	}
	if err := d.Optimize(ctx); err != nil {
		return false, fmt.Errorf("optimize: %w", err)
	}
	vacuumed, err = d.vacuumIfFragmented(ctx)
	if err != nil {
		return false, err
	}
	if _, err := d.ExecContext(ctx, "PRAGMA wal_checkpoint(TRUNCATE);"); err != nil {
		return vacuumed, fmt.Errorf("checkpoint: %w", err)
	}
	return vacuumed, nil
}

func (d *DB) vacuumIfFragmented(ctx context.Context) (bool, error) {
	var pageCount, freelist int64
	if err := d.QueryRowContext(ctx, "PRAGMA page_count;").Scan(&pageCount); err != nil {
		return false, fmt.Errorf("page_count: %w", err)
	}
	if err := d.QueryRowContext(ctx, "PRAGMA freelist_count;").Scan(&freelist); err != nil {
		return false, fmt.Errorf("freelist_count: %w", err)
	}
	if pageCount <= 0 || float64(freelist)/float64(pageCount) < vacuumFreelistRatio {
		return false, nil
	}
	if _, err := d.ExecContext(ctx, "VACUUM;"); err != nil {
		return false, fmt.Errorf("vacuum: %w", err)
	}
	return true, nil
}
