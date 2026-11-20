package db

import (
	"fmt"
	"strings"
	"time"
)

// Dialect picks the backend-specific SQL fragment where SQLite and PostgreSQL
// disagree. Queries are otherwise written once with $N placeholders.
type Dialect int

// The supported SQL dialects.
const (
	DialectSQLite Dialect = iota
	DialectPostgres
)

// TimeBound renders t for comparison against a stored entry timestamp. SQLite
// keeps those at second precision in the space form, so the sub-second part
// would only break the text comparison; PostgreSQL stores full precision, so
// it keeps the fraction.
func (d Dialect) TimeBound(t time.Time) string {
	if d == DialectPostgres {
		return FormatRustRFC3339(t)
	}
	return FormatSpace(t)
}

// Rewrite turns SQLite-only SQL into its PostgreSQL form: datetime('now')
// becomes now(), the same instant under the pinned UTC zone. Modifier forms are
// left alone and need an explicit fork such as DaysAgo.
func (d Dialect) Rewrite(sql string) string {
	if d != DialectPostgres {
		return sql
	}
	return strings.ReplaceAll(sql, "datetime('now')", "now()")
}

// LikeCI is a case-insensitive LIKE against placeholder $param.
func (d Dialect) LikeCI(column string, param int) string {
	if d == DialectPostgres {
		return fmt.Sprintf("%s ILIKE $%d", column, param)
	}
	return fmt.Sprintf("%s LIKE $%d COLLATE NOCASE", column, param)
}

// LikeCIEscape is LikeCI with a backslash ESCAPE, for patterns built by
// LikeContains. SQLite's LIKE is already ASCII-case-insensitive.
func (d Dialect) LikeCIEscape(column string, param int) string {
	if d == DialectPostgres {
		return fmt.Sprintf(`%s ILIKE $%d ESCAPE '\'`, column, param)
	}
	return fmt.Sprintf(`%s LIKE $%d ESCAPE '\'`, column, param)
}

// Epoch is the unix-epoch-seconds expression for a timestamp.
func (d Dialect) Epoch(expr string) string {
	if d == DialectPostgres {
		return fmt.Sprintf("EXTRACT(EPOCH FROM %s)::bigint", expr)
	}
	return fmt.Sprintf("CAST(strftime('%%s', %s) AS INTEGER)", expr)
}

// CursorTS renders a timestamp as "YYYY-MM-DD HH:MM:SS" text. It is not
// sargable, so use it only as a fallback for comparisons. PostgreSQL relies on
// the session zone pinned to UTC.
func (d Dialect) CursorTS(expr string) string {
	if d == DialectPostgres {
		return fmt.Sprintf("to_char(%s, 'YYYY-MM-DD HH24:MI:SS')", expr)
	}
	return expr
}

// DaysAgo is the timestamp daysExpr (a SQL literal or column) days in the past.
func (d Dialect) DaysAgo(daysExpr string) string {
	if d == DialectPostgres {
		return fmt.Sprintf("now() - make_interval(days => (%s)::int)", daysExpr)
	}
	return fmt.Sprintf("datetime('now', '-' || (%s) || ' days')", daysExpr)
}

// BoolFalse is the FALSE literal matching the backend's LIKE result type, for
// COALESCE-ing LIKE leaves to two-valued logic.
func (d Dialect) BoolFalse() string {
	if d == DialectPostgres {
		return "FALSE"
	}
	return "0"
}

// IndexedBy is the SQLite index hint, or empty on PostgreSQL. Pass the hint
// with its leading space, for example " INDEXED BY idx_entry_unread_feed".
func (d Dialect) IndexedBy(hint string) string {
	if d == DialectPostgres {
		return ""
	}
	return hint
}

// NullSafeNe is a NULL-safe inequality between two expressions.
func (d Dialect) NullSafeNe(a, b string) string {
	if d == DialectPostgres {
		return fmt.Sprintf("%s IS DISTINCT FROM %s", a, b)
	}
	return fmt.Sprintf("%s IS NOT %s", a, b)
}
