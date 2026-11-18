package server

import (
	"context"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/search"
	"github.com/henry40408/rdrs/internal/timezone"
)

func TestUserLocationIsTheStoredTimezone(t *testing.T) {
	app := defaultITestApp(t)
	user := app.SeedAccount("traveller", fixturePassword, models.RoleUser)
	if err := models.UpdateTimezone(context.Background(), app.DB, user.ID, "Asia/Taipei"); err != nil {
		t.Fatal(err)
	}

	if got := app.App.userLocation(context.Background(), user.ID).String(); got != "Asia/Taipei" {
		t.Errorf("location = %q, want Asia/Taipei", got)
	}
}

func TestUserLocationIsUTCWithoutSettings(t *testing.T) {
	app := defaultITestApp(t)
	user := app.SeedAccount("homebody", fixturePassword, models.RoleUser)

	if got := app.App.userLocation(context.Background(), user.ID).String(); got != "UTC" {
		t.Errorf("location = %q, want UTC", got)
	}
}

func TestSnapshotDayIsTheDayThePageWasRendered(t *testing.T) {
	now := time.Date(2026, 6, 10, 12, 0, 0, 0, time.UTC)
	app := newITestAppWith(t, Options{Config: itestConfig(), Now: func() time.Time { return now }})
	taipei, err := time.LoadLocation("Asia/Taipei")
	if err != nil {
		t.Fatal(err)
	}
	// 20:00 UTC on the 8th is already the 9th in Taipei.
	snapshot := "2026-6-8 20:0:0"

	got := app.App.snapshotDay(&snapshot, taipei)
	if want := (timezone.Date{Year: 2026, Month: time.June, Day: 9}); got != want {
		t.Errorf("snapshot day = %+v, want %+v", got, want)
	}
}

func TestSnapshotDayFallsBackToNow(t *testing.T) {
	now := time.Date(2026, 6, 10, 20, 0, 0, 0, time.UTC)
	app := newITestAppWith(t, Options{Config: itestConfig(), Now: func() time.Time { return now }})
	taipei, err := time.LoadLocation("Asia/Taipei")
	if err != nil {
		t.Fatal(err)
	}
	want := timezone.Date{Year: 2026, Month: time.June, Day: 11}

	if got := app.App.snapshotDay(nil, taipei); got != want {
		t.Errorf("no snapshot = %+v, want %+v", got, want)
	}
	garbled := "not a time"
	if got := app.App.snapshotDay(&garbled, taipei); got != want {
		t.Errorf("garbled snapshot = %+v, want %+v", got, want)
	}
}

func TestSearchErrorMessageCharacterPosition(t *testing.T) {
	t.Parallel()
	const query = "aé日b" // byte offsets: a=0, é=1..2, 日=3..5, b=6; length 7
	cases := []struct {
		name     string
		offset   int
		position int
	}{
		{"start", 0, 1},
		{"after one ASCII character", 1, 2},
		{"inside a two byte character", 2, 1},
		{"after a two byte character", 3, 3},
		{"inside a three byte character", 4, 1},
		{"last character", 6, 4},
		{"end of input", 7, 5},
		{"past the end", 8, 1},
	}
	for _, tc := range cases {
		err := &search.ParseError{Position: tc.offset, Message: "bad"}
		want := "Search syntax error (near character " + string(rune('0'+tc.position)) + "): bad"
		if got := searchErrorMessage(query, err); got != want {
			t.Errorf("%s: %q, want %q", tc.name, got, want)
		}
	}
}
