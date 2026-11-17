package models

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// ErrSummaryNotFound is returned when a status change targets no row.
var ErrSummaryNotFound = errors.New("entry summary not found")

// SummaryStatus is the processing state of an entry summary.
type SummaryStatus string

// The stored status values.
const (
	SummaryPending    SummaryStatus = "pending"
	SummaryProcessing SummaryStatus = "processing"
	SummaryCompleted  SummaryStatus = "completed"
	SummaryFailed     SummaryStatus = "failed"
)

// ParseSummaryStatus returns false for a value that is not a status.
func ParseSummaryStatus(s string) (SummaryStatus, bool) {
	switch status := SummaryStatus(s); status {
	case SummaryPending, SummaryProcessing, SummaryCompleted, SummaryFailed:
		return status, true
	}
	return "", false
}

// EntrySummary is a row of the entry_summary table.
type EntrySummary struct {
	ID           int64
	UserID       int64
	EntryID      int64
	Status       SummaryStatus
	SummaryText  *string
	ErrorMessage *string
	CreatedAt    time.Time
	UpdatedAt    time.Time
}

const entrySummaryColumns = "id, user_id, entry_id, status, summary_text, error_message, created_at, updated_at"

func scanEntrySummary(row scanner) (EntrySummary, error) {
	var s EntrySummary
	var status string
	err := row.Scan(&s.ID, &s.UserID, &s.EntryID, &status, &s.SummaryText, &s.ErrorMessage,
		db.TimeDest(&s.CreatedAt), db.TimeDest(&s.UpdatedAt))
	// An unknown value reads as failed, so a bad row is retried, not stuck.
	s.Status = SummaryFailed
	if parsed, ok := ParseSummaryStatus(status); ok {
		s.Status = parsed
	}
	return s, err
}

// FindEntrySummary looks up a user's summary of an entry.
func FindEntrySummary(ctx context.Context, e db.Executor, userID, entryID int64) (*EntrySummary, error) {
	summary, err := scanEntrySummary(e.QueryRowContext(ctx,
		`SELECT `+entrySummaryColumns+` FROM entry_summary WHERE user_id = $1 AND entry_id = $2`,
		userID, entryID))
	return orNil(&summary, err)
}

// reloadEntrySummary reads back a row a statement just changed.
func reloadEntrySummary(ctx context.Context, e db.Executor, userID, entryID int64) (*EntrySummary, error) {
	summary, err := FindEntrySummary(ctx, e, userID, entryID)
	if err != nil {
		return nil, err
	}
	if summary == nil {
		return nil, ErrSummaryNotFound
	}
	return summary, nil
}

// UpsertPendingSummary resets an existing row to pending, so retries reuse one
// record.
func UpsertPendingSummary(ctx context.Context, e db.Executor, userID, entryID int64) (*EntrySummary, error) {
	_, err := e.ExecContext(ctx,
		`INSERT INTO entry_summary (user_id, entry_id, status) VALUES ($1, $2, 'pending')
		 ON CONFLICT(user_id, entry_id) DO UPDATE SET
		     status = 'pending', summary_text = NULL, error_message = NULL, updated_at = $3`,
		userID, entryID, stamp(time.Now()))
	if err != nil {
		return nil, err
	}
	return reloadEntrySummary(ctx, e, userID, entryID)
}

// changeSummary runs an UPDATE of one summary row, whose placeholders are
// $1 user, $2 entry and then extra, and fails with ErrSummaryNotFound when it
// matches nothing.
func changeSummary(ctx context.Context, e db.Executor, set string, userID, entryID int64, extra ...any) error {
	args := append([]any{userID, entryID}, extra...)
	affected, err := rowsAffected(e.ExecContext(ctx,
		`UPDATE entry_summary SET `+set+` WHERE user_id = $1 AND entry_id = $2`, args...))
	if err != nil {
		return err
	}
	if affected == 0 {
		return ErrSummaryNotFound
	}
	return nil
}

// SetSummaryProcessing marks the summary as being generated.
func SetSummaryProcessing(ctx context.Context, e db.Executor, userID, entryID int64) error {
	return changeSummary(ctx, e, "status = 'processing', updated_at = $3", userID, entryID, stamp(time.Now()))
}

// SetSummaryCompleted stores the generated text.
func SetSummaryCompleted(ctx context.Context, e db.Executor, userID, entryID int64, text string) (*EntrySummary, error) {
	err := changeSummary(ctx, e,
		"status = 'completed', summary_text = $3, error_message = NULL, updated_at = $4",
		userID, entryID, text, stamp(time.Now()))
	if err != nil {
		return nil, err
	}
	return reloadEntrySummary(ctx, e, userID, entryID)
}

// SetSummaryFailed stores why generation failed.
func SetSummaryFailed(ctx context.Context, e db.Executor, userID, entryID int64, message string) (*EntrySummary, error) {
	err := changeSummary(ctx, e,
		"status = 'failed', error_message = $3, updated_at = $4",
		userID, entryID, message, stamp(time.Now()))
	if err != nil {
		return nil, err
	}
	return reloadEntrySummary(ctx, e, userID, entryID)
}

// DeleteEntrySummary reports whether a row was removed.
func DeleteEntrySummary(ctx context.Context, e db.Executor, userID, entryID int64) (bool, error) {
	affected, err := rowsAffected(e.ExecContext(ctx,
		`DELETE FROM entry_summary WHERE user_id = $1 AND entry_id = $2`, userID, entryID))
	return affected > 0, err
}

// SummaryStatusesFor maps each entry that has a summary to its status, for a
// whole list page. A variable-length IN list cannot be a static literal, so
// this is one index-covered point query per entry.
func SummaryStatusesFor(ctx context.Context, e db.Executor, userID int64, entryIDs []int64) (map[int64]SummaryStatus, error) {
	statuses := make(map[int64]SummaryStatus, len(entryIDs))
	for _, entryID := range entryIDs {
		var text string
		err := e.QueryRowContext(ctx,
			`SELECT status FROM entry_summary WHERE user_id = $1 AND entry_id = $2`, userID, entryID).Scan(&text)
		if errors.Is(err, sql.ErrNoRows) {
			continue
		}
		if err != nil {
			return nil, err
		}
		if status, ok := ParseSummaryStatus(text); ok {
			statuses[entryID] = status
		}
	}
	return statuses, nil
}

// IncompleteSummary is a pending or processing row left by a shutdown.
type IncompleteSummary struct {
	UserID  int64
	EntryID int64
	Link    string
}

// FindIncompleteSummaries lists rows to re-queue at startup.
func FindIncompleteSummaries(ctx context.Context, e db.Executor) ([]IncompleteSummary, error) {
	rows, err := e.QueryContext(ctx,
		`SELECT es.user_id, es.entry_id, e.link
		 FROM entry_summary es
		 INNER JOIN entry e ON es.entry_id = e.id
		 WHERE es.status IN ('pending', 'processing') AND e.link IS NOT NULL`)
	return collect(rows, err, func(row scanner) (IncompleteSummary, error) {
		var s IncompleteSummary
		err := row.Scan(&s.UserID, &s.EntryID, &s.Link)
		return s, err
	})
}

// DeleteExpiredSummaries removes summaries created more than hours ago.
//
// The cutoff is bound as the space-separated form the created_at default
// writes: the RFC 3339 form sorts after a space on the same day, which would
// expire fresh rows.
func DeleteExpiredSummaries(ctx context.Context, e db.Executor, hours int64) (int64, error) {
	cutoff := time.Now().UTC().Add(-time.Duration(hours) * time.Hour)
	return rowsAffected(e.ExecContext(ctx,
		`DELETE FROM entry_summary WHERE created_at < $1`, db.FormatSpace(cutoff)))
}

// CountCompletedSummaries counts a user's completed summaries; the count is
// covered by idx_entry_summary_user_status.
func CountCompletedSummaries(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	var count int64
	err := e.QueryRowContext(ctx,
		`SELECT COUNT(*) FROM entry_summary WHERE user_id = $1 AND status = 'completed'`, userID).Scan(&count)
	return count, err
}

// EntrySummaryExists reports whether a summary row of any status exists.
func EntrySummaryExists(ctx context.Context, e db.Executor, userID, entryID int64) (bool, error) {
	var count int64
	err := e.QueryRowContext(ctx,
		`SELECT COUNT(*) FROM entry_summary WHERE user_id = $1 AND entry_id = $2`, userID, entryID).Scan(&count)
	return count > 0, err
}
