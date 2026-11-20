package feedsync

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"
)

var breakerEpoch = time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)

func TestIconBackoffSequence(t *testing.T) {
	tests := []struct {
		consecutive int
		want        time.Duration
	}{
		{1, time.Hour},
		{2, 2 * time.Hour},
		{3, 4 * time.Hour},
		{4, 8 * time.Hour},
		{5, 16 * time.Hour},
		{6, 24 * time.Hour},
		{7, 24 * time.Hour},
		{1000, 24 * time.Hour},
	}
	for _, tt := range tests {
		if got := iconBackoff(tt.consecutive); got != tt.want {
			t.Errorf("iconBackoff(%d) = %v, want %v", tt.consecutive, got, tt.want)
		}
	}
}

func TestBreakerOpensForTheBackoffAndClosesExactlyAtRetryAfter(t *testing.T) {
	tests := []struct {
		name   string
		offset time.Duration
		open   bool
	}{
		{"just after the failure", 0, true},
		{"one nanosecond before retryAfter", time.Hour - time.Nanosecond, true},
		{"exactly at retryAfter", time.Hour, false},
		{"after retryAfter", time.Hour + time.Nanosecond, false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var b iconBreaker
			b.recordFailure(1, breakerEpoch)
			if got := b.isOpen(1, breakerEpoch.Add(tt.offset)); got != tt.open {
				t.Errorf("isOpen = %v, want %v", got, tt.open)
			}
		})
	}
}

func TestBreakerIsClosedForAnUnknownFeed(t *testing.T) {
	var b iconBreaker
	b.recordFailure(1, breakerEpoch)
	if b.isOpen(2, breakerEpoch) {
		t.Error("a feed that never failed must not be skipped")
	}
}

func TestBreakerBacksOffAcrossConsecutiveFailures(t *testing.T) {
	var b iconBreaker
	now := breakerEpoch
	for _, want := range []time.Duration{
		time.Hour, 2 * time.Hour, 4 * time.Hour, 8 * time.Hour, 16 * time.Hour,
		24 * time.Hour, 24 * time.Hour,
	} {
		if got := b.recordFailure(1, now); got != want {
			t.Fatalf("pause = %v, want %v", got, want)
		}
		now = now.Add(want) // the half-open attempt happens at retryAfter
		if b.isOpen(1, now) {
			t.Fatalf("circuit still open at retryAfter after %v", want)
		}
	}
}

func TestBreakerSuccessResetsTheCount(t *testing.T) {
	var b iconBreaker
	b.recordFailure(1, breakerEpoch)
	b.recordFailure(1, breakerEpoch.Add(time.Hour))
	b.recordSuccess(1)
	if b.isOpen(1, breakerEpoch.Add(time.Hour)) {
		t.Error("success must close the circuit")
	}
	if got := b.recordFailure(1, breakerEpoch); got != time.Hour {
		t.Errorf("pause after a reset = %v, want 1h", got)
	}
}

func TestBreakerEvictsExpiredEntriesBeforeLiveOnes(t *testing.T) {
	var b iconBreaker
	for id := range int64(iconBreakerMaxEntries) {
		b.recordFailure(id, breakerEpoch)
	}
	b.recordFailure(0, breakerEpoch) // second failure: live until 2h
	later := breakerEpoch.Add(time.Hour)

	b.recordFailure(iconBreakerMaxEntries, later)

	if len(b.entries) != 2 {
		t.Fatalf("entries = %d, want the live feed 0 plus the new one", len(b.entries))
	}
	if !b.isOpen(0, later) || !b.isOpen(iconBreakerMaxEntries, later) {
		t.Error("live entries were evicted")
	}
}

func TestBreakerNeverExceedsItsCap(t *testing.T) {
	tests := []struct {
		name     string
		inserts  int
		wantSize int
	}{
		{"one below the cap", iconBreakerMaxEntries - 1, iconBreakerMaxEntries - 1},
		{"exactly the cap", iconBreakerMaxEntries, iconBreakerMaxEntries},
		{"one above the cap", iconBreakerMaxEntries + 1, iconBreakerMaxEntries},
		{"far above the cap", 2*iconBreakerMaxEntries + 10, iconBreakerMaxEntries},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var b iconBreaker
			for id := range int64(tt.inserts) {
				b.recordFailure(id, breakerEpoch) // all live: nothing is expired
				if len(b.entries) > iconBreakerMaxEntries {
					t.Fatalf("entries = %d after insert %d", len(b.entries), id)
				}
			}
			if len(b.entries) != tt.wantSize {
				t.Errorf("entries = %d, want %d", len(b.entries), tt.wantSize)
			}
			if tt.inserts > 0 && !b.isOpen(int64(tt.inserts-1), breakerEpoch) {
				t.Error("the newest entry must be kept")
			}
		})
	}
}

func TestBreakerUpdatingAKnownFeedAtTheCapEvictsNothing(t *testing.T) {
	var b iconBreaker
	for id := range int64(iconBreakerMaxEntries) {
		b.recordFailure(id, breakerEpoch)
	}
	b.recordFailure(7, breakerEpoch)
	if len(b.entries) != iconBreakerMaxEntries {
		t.Errorf("entries = %d, want %d", len(b.entries), iconBreakerMaxEntries)
	}
	if got := b.entries[7].consecutive; got != 2 {
		t.Errorf("consecutive = %d, want 2", got)
	}
}

func TestBreakerIsSafeForConcurrentUse(t *testing.T) {
	var b iconBreaker
	var wg sync.WaitGroup
	for worker := range int64(8) {
		wg.Go(func() {
			for i := range int64(2000) {
				id := (worker*31 + i) % 5000
				b.isOpen(id, breakerEpoch)
				b.recordFailure(id, breakerEpoch)
				if i%3 == 0 {
					b.recordSuccess(id)
				}
			}
		})
	}
	wg.Wait()
	if len(b.entries) > iconBreakerMaxEntries {
		t.Errorf("entries = %d", len(b.entries))
	}
}

func TestSyncingAFeedWithAFailingIconTwiceFetchesOnce(t *testing.T) {
	failures := map[string]func(context.Context, IconCandidates) (*FetchedIcon, error){
		"error":    func(context.Context, IconCandidates) (*FetchedIcon, error) { return nil, errors.New("boom") },
		"nil icon": func(context.Context, IconCandidates) (*FetchedIcon, error) { return nil, nil },
	}
	for name, fetch := range failures {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t, serveFeed)
			clock := breakerEpoch
			f.syncer.Now = func() time.Time { return clock }
			calls := 0
			f.syncer.FetchIcon = func(ctx context.Context, c IconCandidates) (*FetchedIcon, error) {
				calls++
				return fetch(ctx, c)
			}
			ctx := context.Background()
			for range 2 {
				if _, err := f.syncer.Refresh(ctx, f.feed.ID); err != nil {
					t.Fatal(err)
				}
			}
			if calls != 1 {
				t.Fatalf("FetchIcon calls = %d, want 1", calls)
			}

			clock = clock.Add(time.Hour) // half-open: one more attempt
			if _, err := f.syncer.Refresh(ctx, f.feed.ID); err != nil {
				t.Fatal(err)
			}
			if calls != 2 {
				t.Errorf("FetchIcon calls after the pause = %d, want 2", calls)
			}
			if _, err := f.syncer.Refresh(ctx, f.feed.ID); err != nil {
				t.Fatal(err)
			}
			if calls != 2 {
				t.Errorf("the second failure must open the circuit again, calls = %d", calls)
			}
		})
	}
}

func TestASuccessfulIconClosesTheCircuit(t *testing.T) {
	f := newFixture(t, serveFeed)
	f.syncer.Now = func() time.Time { return breakerEpoch }
	f.syncer.iconBreaker.recordFailure(f.feed.ID, breakerEpoch.Add(-time.Hour))
	f.syncer.FetchIcon = func(context.Context, IconCandidates) (*FetchedIcon, error) {
		return &FetchedIcon{Data: []byte("png"), ContentType: "image/png", SourceURL: "https://x.example/i.png"}, nil
	}

	if _, err := f.syncer.Refresh(context.Background(), f.feed.ID); err != nil {
		t.Fatal(err)
	}

	if len(f.syncer.iconBreaker.entries) != 0 {
		t.Errorf("entries = %v, want none after a success", f.syncer.iconBreaker.entries)
	}
}

func TestSyncerNowDefaultsToTheWallClock(t *testing.T) {
	var s Syncer
	before := time.Now()
	got := s.now()
	if got.Before(before) || time.Since(got) > time.Minute {
		t.Errorf("now = %v", got)
	}
}
