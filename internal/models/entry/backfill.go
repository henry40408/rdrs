package entry

import (
	"context"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/textutil"
)

const missingContentTextWhere = `content_text IS NULL AND content IS NOT NULL`

// CountMissingContentText counts legacy rows whose search text was never
// computed. NULL content stays NULL and is not counted.
func CountMissingContentText(ctx context.Context, database *db.DB) (int64, error) {
	var total int64
	err := database.QueryRowContext(ctx, `SELECT COUNT(*) FROM entry WHERE `+missingContentTextWhere).Scan(&total)
	return total, err
}

type legacyContent struct {
	id      int64
	content string
}

// BackfillContentTextBatch fills up to batchSize rows in one transaction and
// returns how many it updated; fewer than batchSize means drained.
func BackfillContentTextBatch(ctx context.Context, database *db.DB, batchSize int) (int, error) {
	tx, err := database.Begin(ctx)
	if err != nil {
		return 0, err
	}
	defer func() { _ = tx.Rollback() }()

	rows, err := tx.QueryContext(ctx, `SELECT id, content FROM entry WHERE `+missingContentTextWhere+` LIMIT $1`, int64(batchSize))
	batch, err := collect(rows, err, func(s scanner) (legacyContent, error) {
		var row legacyContent
		err := s.Scan(&row.id, &row.content)
		return row, err
	})
	if err != nil {
		return 0, err
	}
	for _, row := range batch {
		if _, err := tx.ExecContext(ctx, `UPDATE entry SET content_text = $1 WHERE id = $2`,
			textutil.StripToSearchText(row.content), row.id); err != nil {
			return 0, err
		}
	}
	return len(batch), tx.Commit()
}
