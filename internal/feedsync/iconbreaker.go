package feedsync

import (
	"sync"
	"time"
)

const (
	iconBackoffBase = time.Hour
	iconBackoffMax  = 24 * time.Hour
	// iconBackoffMaxShift is the doubling count after which 1 h exceeds 24 h.
	iconBackoffMaxShift = 5
	// iconBreakerMaxEntries bounds the memory held for deleted feeds.
	iconBreakerMaxEntries = 4096
)

// iconBreaker is a circuit breaker per feed ID for icon fetches, so a feed
// whose icon cannot be fetched is not retried (up to five requests) on every
// sync. Each consecutive failure doubles the pause, from one hour up to 24
// hours. When the pause ends one attempt is allowed: success closes the
// circuit, failure opens it again with the next pause.
//
// The state is in memory only and starts empty after a restart, by design.
// The zero value is ready to use and safe for concurrent use.
type iconBreaker struct {
	mu      sync.Mutex
	entries map[int64]iconFailure
}

type iconFailure struct {
	consecutive int
	retryAfter  time.Time
}

// isOpen reports whether the feed's icon must not be fetched yet.
func (b *iconBreaker) isOpen(feedID int64, now time.Time) bool {
	b.mu.Lock()
	defer b.mu.Unlock()
	return now.Before(b.entries[feedID].retryAfter)
}

// recordFailure opens the circuit for the next backoff and returns the pause.
func (b *iconBreaker) recordFailure(feedID int64, now time.Time) time.Duration {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.entries == nil {
		b.entries = make(map[int64]iconFailure)
	}
	failure, known := b.entries[feedID]
	if !known && len(b.entries) >= iconBreakerMaxEntries {
		b.makeRoom(now)
	}
	failure.consecutive++
	pause := iconBackoff(failure.consecutive)
	failure.retryAfter = now.Add(pause)
	b.entries[feedID] = failure
	return pause
}

// recordSuccess closes the circuit.
func (b *iconBreaker) recordSuccess(feedID int64) {
	b.mu.Lock()
	defer b.mu.Unlock()
	delete(b.entries, feedID)
}

// makeRoom drops expired entries first, then arbitrary ones until one slot is
// free. The caller holds the lock.
func (b *iconBreaker) makeRoom(now time.Time) {
	for id, failure := range b.entries {
		if !now.Before(failure.retryAfter) {
			delete(b.entries, id)
		}
	}
	for id := range b.entries {
		if len(b.entries) < iconBreakerMaxEntries {
			return
		}
		delete(b.entries, id)
	}
}

// iconBackoff is 1 h for the first failure, doubling up to 24 h. The shift is
// clamped so that a long failure streak cannot overflow the duration.
func iconBackoff(consecutive int) time.Duration {
	return min(iconBackoffBase<<min(consecutive-1, iconBackoffMaxShift), iconBackoffMax)
}
