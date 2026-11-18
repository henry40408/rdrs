package models

import (
	"context"
	"math"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

func TestTokenRefreshStartsBelowHalfTheLifetime(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 6, 10, 12, 0, 0, 0, time.UTC)
	cases := []struct {
		name        string
		remaining   time.Duration
		wantRefresh bool
	}{
		{"exactly half", apiTokenTTL / 2, false},
		{"just under half", apiTokenTTL/2 - time.Second, true},
		{"nearly all", apiTokenTTL - time.Second, false},
	}
	for _, tc := range cases {
		token := &APIToken{ExpiresAt: now.Add(tc.remaining)}
		if got := token.ComputeRefreshedExpiry(now) != nil; got != tc.wantRefresh {
			t.Errorf("api token, %s: refreshed = %v, want %v", tc.name, got, tc.wantRefresh)
		}
	}
	sessionCases := []struct {
		name        string
		remaining   time.Duration
		wantRefresh bool
	}{
		{"exactly half", sessionTTL / 2, false},
		{"just under half", sessionTTL/2 - time.Second, true},
	}
	for _, tc := range sessionCases {
		session := &Session{CreatedAt: now, ExpiresAt: now.Add(tc.remaining)}
		if got := session.ComputeRefreshedExpiry(now) != nil; got != tc.wantRefresh {
			t.Errorf("session, %s: refreshed = %v, want %v", tc.name, got, tc.wantRefresh)
		}
	}
}

func TestUsernameMayNotStartWithAControlCharacter(t *testing.T) {
	t.Parallel()
	for _, name := range []string{"\x01alice", "al\x01ice", "alice\x7f"} {
		if got, err := ValidateUsername(name); err == nil {
			t.Errorf("ValidateUsername(%q) = %q, want a refusal", name, got)
		}
	}
}

func TestSummaryResultsComeBackFromTheStatusChange(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		entryID := f.addEntry(t, "a")
		f.pending(t, entryID)

		failed, err := SetSummaryFailed(ctx, database, f.userID, entryID, "API error")
		if err != nil || failed == nil || failed.Status != SummaryFailed || failed.ErrorMessage == nil || *failed.ErrorMessage != "API error" {
			t.Fatalf("SetSummaryFailed = %+v, %v", failed, err)
		}
		completed, err := SetSummaryCompleted(ctx, database, f.userID, entryID, "the text")
		if err != nil || completed == nil || completed.Status != SummaryCompleted || completed.SummaryText == nil || *completed.SummaryText != "the text" {
			t.Fatalf("SetSummaryCompleted = %+v, %v", completed, err)
		}
	})
}

func TestSummariesExpireOnlyPastTheGivenHours(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		ages := map[string]time.Duration{"stale": 25 * time.Hour, "recent": 23 * time.Hour}
		ids := map[string]int64{}
		for name, age := range ages {
			ids[name] = f.addEntry(t, name)
			f.pending(t, ids[name])
			created := db.FormatSpace(time.Now().UTC().Add(-age))
			mustExec(t, database, `UPDATE entry_summary SET created_at = $1 WHERE entry_id = $2`, created, ids[name])
		}

		removed, err := DeleteExpiredSummaries(ctx, database, 24)
		if err != nil || removed != 1 {
			t.Fatalf("removed = %d, %v; want only the 25 hour old row", removed, err)
		}
		if ok, _ := EntrySummaryExists(ctx, database, f.userID, ids["stale"]); ok {
			t.Error("the 25 hour old summary survived")
		}
		if ok, _ := EntrySummaryExists(ctx, database, f.userID, ids["recent"]); !ok {
			t.Error("the 23 hour old summary was removed")
		}
	})
}

func TestUpdateSaveServicesReturnsTheStoredSettings(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		if _, err := UpsertUserSettings(ctx, database, user.ID, 20); err != nil {
			t.Fatal(err)
		}
		const services = `{"x":1}`
		settings, err := UpdateSaveServices(ctx, database, user.ID, services, nil)
		if err != nil || settings == nil {
			t.Fatalf("UpdateSaveServices = %+v, %v", settings, err)
		}
		if got := settings.SaveServicesJSON(nil).JSON; got != services {
			t.Errorf("returned services = %q, want %q", got, services)
		}
	})
}

func TestStorageSizesReportTheReclaimableShareOfTheFile(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		if database.Dialect() == db.DialectPostgres {
			t.Skip("free space is reported on SQLite only")
		}
		ctx := context.Background()
		mustExec(t, database, `CREATE TABLE scratch (blob_data BLOB)`)
		for range 40 {
			mustExec(t, database, `INSERT INTO scratch VALUES (zeroblob(8192))`)
		}
		mustExec(t, database, `DROP TABLE scratch`)

		pragma := func(name string) int64 {
			n, err := countScalar(ctx, database, "PRAGMA "+name)
			if err != nil {
				t.Fatal(err)
			}
			return n
		}
		pageCount, pageSize, freelist := pragma("page_count"), pragma("page_size"), pragma("freelist_count")
		if freelist == 0 {
			t.Fatal("the fixture left no free pages")
		}

		size, reclaimable, err := storageSizes(ctx, database)
		if err != nil || reclaimable == nil {
			t.Fatalf("storageSizes = %d, %+v, %v", size, reclaimable, err)
		}
		if size != pageCount*pageSize {
			t.Errorf("size = %d, want %d", size, pageCount*pageSize)
		}
		if reclaimable.Bytes != freelist*pageSize {
			t.Errorf("reclaimable bytes = %d, want %d", reclaimable.Bytes, freelist*pageSize)
		}
		want := float64(freelist) / float64(pageCount)
		if math.Abs(reclaimable.FragmentationRatio-want) > 1e-9 {
			t.Errorf("ratio = %v, want %v", reclaimable.FragmentationRatio, want)
		}
	})
}
