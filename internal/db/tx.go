package db

import (
	"context"
	"database/sql"
	"errors"
	"sync"
)

// Tx is a write transaction. It holds write-priority admission from Begin to
// Commit or Rollback, so always `defer tx.Rollback()`: after Commit it is a
// harmless no-op.
type Tx struct {
	tx      *sql.Tx
	dialect Dialect
	release func()
	done    sync.Once
}

// ExecContext runs a statement that returns no rows.
func (t *Tx) ExecContext(ctx context.Context, query string, args ...any) (sql.Result, error) {
	return t.tx.ExecContext(ctx, t.dialect.Rewrite(query), args...)
}

// QueryContext runs a query; the caller must close the Rows.
func (t *Tx) QueryContext(ctx context.Context, query string, args ...any) (*Rows, error) {
	rows, err := t.tx.QueryContext(ctx, t.dialect.Rewrite(query), args...)
	if err != nil {
		return nil, err
	}
	return &Rows{Rows: rows, release: noRelease}, nil
}

// QueryRowContext runs a query expected to return at most one row.
func (t *Tx) QueryRowContext(ctx context.Context, query string, args ...any) *Row {
	return &Row{row: t.tx.QueryRowContext(ctx, t.dialect.Rewrite(query), args...), release: noRelease}
}

// LockUserTable serializes writers of the user table for the rest of the
// transaction. PostgreSQL's default isolation lets two transactions both read
// COUNT(*) = 0 and each insert the first account. SQLite needs nothing: Begin
// already took the write lock.
func (t *Tx) LockUserTable(ctx context.Context) error {
	if t.dialect != DialectPostgres {
		return nil
	}
	_, err := t.tx.ExecContext(ctx, `LOCK TABLE "user" IN SHARE ROW EXCLUSIVE MODE`)
	return err
}

// Commit commits and releases admission.
func (t *Tx) Commit() error {
	defer t.finish()
	return t.tx.Commit()
}

// Rollback rolls back and releases admission. After Commit it does nothing.
func (t *Tx) Rollback() error {
	defer t.finish()
	err := t.tx.Rollback()
	if errors.Is(err, sql.ErrTxDone) {
		return nil
	}
	return err
}

func (t *Tx) finish() {
	t.done.Do(t.release)
}
