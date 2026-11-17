package db

import (
	"errors"

	"github.com/jackc/pgx/v5/pgconn"
	"modernc.org/sqlite"
)

// Extended result codes for UNIQUE and PRIMARY KEY violations.
const (
	sqliteConstraintPrimaryKey = 1555
	sqliteConstraintUnique     = 2067
	pgUniqueViolation          = "23505"
)

// IsUniqueViolation reports a UNIQUE or primary-key violation on either backend.
func IsUniqueViolation(err error) bool {
	var lite *sqlite.Error
	if errors.As(err, &lite) {
		code := lite.Code()
		return code == sqliteConstraintUnique || code == sqliteConstraintPrimaryKey
	}
	var pg *pgconn.PgError
	if errors.As(err, &pg) {
		return pg.Code == pgUniqueViolation
	}
	return false
}
