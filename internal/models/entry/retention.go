package entry

import (
	"context"

	"github.com/henry40408/rdrs/internal/db"
)

// retentionVictimsSQL picks read, aged, unstarred entries of users who opted
// in. It is built per call because the interval differs by dialect.
func retentionVictimsSQL(dialect db.Dialect) string {
	cutoff := dialect.DaysAgo("us.retention_read_days")
	return `SELECT e.id
		 FROM entry e
		 JOIN feed f           ON f.id = e.feed_id
		 JOIN category c       ON c.id = f.category_id
		 JOIN user_settings us ON us.user_id = c.user_id
		 WHERE us.retention_read_days > 0
		   AND e.read_at    IS NOT NULL
		   AND e.starred_at IS NULL
		   AND e.read_at < ` + cutoff + `
		 LIMIT $1`
}

// PruneReadRetentionBatch deletes up to batchSize read, aged, unstarred
// entries of opted-in users, tombstoning each, and returns the count. One
// transaction per batch keeps tombstone and delete atomic against a concurrent
// refresh.
func PruneReadRetentionBatch(ctx context.Context, database *db.DB, batchSize int) (int64, error) {
	tx, err := database.Begin(ctx)
	if err != nil {
		return 0, err
	}
	defer func() { _ = tx.Rollback() }()

	victims, err := retentionVictims(ctx, tx, batchSize)
	if err != nil {
		return 0, err
	}
	if len(victims) == 0 {
		return 0, tx.Commit()
	}

	// Two set-based statements per batch, not two per victim, shorten the
	// SQLite write-lock window. WHERE id IN (...) also disambiguates SQLite's
	// INSERT ... SELECT ... ON CONFLICT parse.
	in := placeholders(1, len(victims))
	args := int64Args(victims)
	if _, err := tx.ExecContext(ctx,
		`INSERT INTO entry_tombstone (feed_id, guid)
		 SELECT feed_id, guid FROM entry WHERE id IN (`+in+`)
		 ON CONFLICT (feed_id, guid) DO NOTHING`, args...); err != nil {
		return 0, err
	}
	deleted, err := exec(ctx, tx, `DELETE FROM entry WHERE id IN (`+in+`)`, args...)
	if err != nil {
		return 0, err
	}
	return deleted, tx.Commit()
}

func retentionVictims(ctx context.Context, tx *db.Tx, batchSize int) ([]int64, error) {
	rows, err := tx.QueryContext(ctx, retentionVictimsSQL(tx.Dialect()), int64(batchSize))
	return collect(rows, err, func(s scanner) (int64, error) {
		var id int64
		err := s.Scan(&id)
		return id, err
	})
}
