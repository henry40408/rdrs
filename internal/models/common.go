// Package models holds the SQL for every table the application owns. Each
// function takes a db.Executor so the same code runs on a pool or inside a
// transaction, and returns nil (not an error) when a lookup finds no row.
package models

import (
	"database/sql"
	"errors"
	"fmt"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// Domain errors the HTTP layer maps to status codes.
var (
	ErrUserNotFound        = errors.New("user not found")
	ErrUsernameExists      = errors.New("username already exists")
	ErrCategoryNotFound    = errors.New("category not found")
	ErrCategoryExists      = errors.New("category already exists")
	ErrFeedNotFound        = errors.New("feed not found")
	ErrFeedExists          = errors.New("feed already exists")
	ErrUnauthorized        = errors.New("unauthorized")
	ErrAlreadyMasquerading = errors.New("already masquerading")
	ErrNotMasquerading     = errors.New("not masquerading")
)

// ValidationError is a rejected input whose message is safe to show the user.
type ValidationError struct{ Message string }

func (e *ValidationError) Error() string { return e.Message }

func validationf(format string, args ...any) error {
	return &ValidationError{Message: fmt.Sprintf(format, args...)}
}

// scanner is the Scan method shared by db.Row and db.Rows.
type scanner interface {
	Scan(dest ...any) error
}

// orNil turns "no rows" into a nil result.
func orNil[T any](value *T, err error) (*T, error) {
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return value, nil
}

// collect scans every row of a query result.
func collect[T any](rows *db.Rows, err error, scan func(scanner) (T, error)) ([]T, error) {
	if err != nil {
		return nil, err
	}
	defer func() { _ = rows.Close() }()
	var out []T
	for rows.Next() {
		item, err := scan(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, item)
	}
	return out, rows.Err()
}

// stamp renders t as the chrono-compatible text the Rust build wrote.
func stamp(t time.Time) string {
	return db.FormatRustRFC3339(t)
}

// stampPtr is stamp for a nullable timestamp.
func stampPtr(t *time.Time) any {
	if t == nil {
		return nil
	}
	return stamp(*t)
}

// rowsAffected reports how many rows a statement changed.
func rowsAffected(result sql.Result, err error) (int64, error) {
	if err != nil {
		return 0, err
	}
	return result.RowsAffected()
}
