package db

import (
	"testing"

	"github.com/henry40408/rdrs/internal/config"
)

func TestDialectFor(t *testing.T) {
	if DialectFor(config.SQLite) != DialectSQLite || DialectFor(config.Postgres) != DialectPostgres {
		t.Fatal("DialectFor mapped the wrong backend")
	}
}

func TestDialectFragments(t *testing.T) {
	checks := []struct {
		name     string
		sqlite   string
		postgres string
		wantLite string
		wantPG   string
	}{
		{"LikeCI", DialectSQLite.LikeCI("t", 2), DialectPostgres.LikeCI("t", 2),
			"t LIKE $2 COLLATE NOCASE", "t ILIKE $2"},
		{"LikeCIEscape", DialectSQLite.LikeCIEscape("t", 3), DialectPostgres.LikeCIEscape("t", 3),
			`t LIKE $3 ESCAPE '\'`, `t ILIKE $3 ESCAPE '\'`},
		{"Epoch", DialectSQLite.Epoch("x"), DialectPostgres.Epoch("x"),
			"CAST(strftime('%s', x) AS INTEGER)", "EXTRACT(EPOCH FROM x)::bigint"},
		{"CursorTS", DialectSQLite.CursorTS("x"), DialectPostgres.CursorTS("x"),
			"x", "to_char(x, 'YYYY-MM-DD HH24:MI:SS')"},
		{"DaysAgo", DialectSQLite.DaysAgo("7"), DialectPostgres.DaysAgo("7"),
			"datetime('now', '-' || (7) || ' days')", "now() - make_interval(days => (7)::int)"},
		{"BoolFalse", DialectSQLite.BoolFalse(), DialectPostgres.BoolFalse(), "0", "FALSE"},
		{"IndexedBy", DialectSQLite.IndexedBy(" INDEXED BY i"), DialectPostgres.IndexedBy(" INDEXED BY i"),
			" INDEXED BY i", ""},
		{"NullSafeNe", DialectSQLite.NullSafeNe("a", "b"), DialectPostgres.NullSafeNe("a", "b"),
			"a IS NOT b", "a IS DISTINCT FROM b"},
	}
	for _, c := range checks {
		if c.sqlite != c.wantLite {
			t.Errorf("%s sqlite = %q, want %q", c.name, c.sqlite, c.wantLite)
		}
		if c.postgres != c.wantPG {
			t.Errorf("%s postgres = %q, want %q", c.name, c.postgres, c.wantPG)
		}
	}
}

func TestRewriteOnlyTouchesPostgres(t *testing.T) {
	const sql = "UPDATE t SET read_at = datetime('now'), x = datetime('now', '-1 days')"
	if DialectSQLite.Rewrite(sql) != sql {
		t.Error("SQLite SQL was rewritten")
	}
	want := "UPDATE t SET read_at = now(), x = datetime('now', '-1 days')"
	if got := DialectPostgres.Rewrite(sql); got != want {
		t.Errorf("Rewrite = %q, want %q", got, want)
	}
}
