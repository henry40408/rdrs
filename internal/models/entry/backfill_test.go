package entry_test

import (
	"context"
	"database/sql"
	"fmt"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// addLegacy stores an entry as a pre-v10 row: content without search text.
func (f fixture) addLegacy(t *testing.T, guid string, content *string) {
	t.Helper()
	f.add(t, guid, guid, day(1))
	_, err := f.database.ExecContext(context.Background(),
		`UPDATE entry SET content = $1, content_text = NULL WHERE guid = $2 AND feed_id = $3`, content, guid, f.feedID)
	must(t, err)
}

func contentTextOf(t *testing.T, f fixture, guid string) sql.NullString {
	t.Helper()
	var text sql.NullString
	err := f.database.QueryRowContext(context.Background(),
		`SELECT content_text FROM entry WHERE guid = $1 AND feed_id = $2`, guid, f.feedID).Scan(&text)
	must(t, err)
	return text
}

func TestBackfillContentTextFillsBatchesUntilDrained(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ctx := context.Background()
		for i := 0; i < 3; i++ {
			html := "<p>Hel</p><p>lo</p>"
			f.addLegacy(t, fmt.Sprint("g", i), &html)
		}
		f.addLegacy(t, "nullcontent", nil)

		total, err := entry.CountMissingContentText(ctx, database)
		must(t, err)
		if total != 3 {
			t.Fatalf("missing = %d, want 3 (NULL content is not counted)", total)
		}
		for _, want := range []int{2, 1, 0} {
			n, err := entry.BackfillContentTextBatch(ctx, database, 2)
			must(t, err)
			if n != want {
				t.Fatalf("batch updated %d, want %d", n, want)
			}
		}
		if text := contentTextOf(t, f, "g0"); !text.Valid || text.String == "" {
			t.Errorf("content_text = %+v", text)
		}
		if text := contentTextOf(t, f, "nullcontent"); text.Valid {
			t.Errorf("NULL content must stay NULL, got %q", text.String)
		}
	})
}
