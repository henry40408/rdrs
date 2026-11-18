package db

import (
	"database/sql"
	"fmt"
	"time"
)

type timeScanner struct {
	dst  *time.Time
	null **time.Time
}

// TimeDest scans a NOT NULL timestamp column into dst, whatever form the
// backend stored it in (see ParseStored).
func TimeDest(dst *time.Time) sql.Scanner {
	return timeScanner{dst: dst}
}

// NullTimeDest scans a nullable timestamp column into dst, leaving nil for NULL.
func NullTimeDest(dst **time.Time) sql.Scanner {
	return timeScanner{null: dst}
}

// Scan implements sql.Scanner.
func (s timeScanner) Scan(value any) error {
	if value == nil && s.null != nil {
		*s.null = nil
		return nil
	}
	parsed, err := ParseStored(value)
	if err != nil {
		return fmt.Errorf("scan timestamp: %w", err)
	}
	if s.null != nil {
		*s.null = &parsed
		return nil
	}
	*s.dst = parsed
	return nil
}
