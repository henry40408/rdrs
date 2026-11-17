package cache

import (
	"os"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/timezone"
)

const (
	sidebarCapacity = 10_000 // covers any single host
	sidebarTTL      = 60 * time.Second
)

// SidebarCategory is one category row of the sidebar.
type SidebarCategory struct {
	ID          int64
	Name        string
	UnreadCount int64
}

// Chrome is the per-user data around every page: theme, categories and unread
// counts. It holds nothing session specific, so one entry serves every session
// of a user.
type Chrome struct {
	Theme           *string
	Categories      []SidebarCategory
	TotalUnread     int64
	TotalSummarized int64
	// TotalToday counts unread entries published on Today in Timezone.
	TotalToday int64
	Timezone   *time.Location
	Today      timezone.Date
	// UnreadByFeed lets /feeds skip its own full-table count. It is shared
	// between readers and must not be modified.
	UnreadByFeed map[int64]int64
	SidebarPrefs models.SidebarPrefs
	OfflineKeep  int64
}

// IsCurrent is false once the user's calendar day has moved on: TotalToday
// then counts yesterday, and nothing busts the cache at midnight.
func (c Chrome) IsCurrent() bool {
	return timezone.TodayIn(c.Timezone) == c.Today
}

// Generation is the bust count taken before a read-through fill and checked on
// publish, so a fill that raced a Bust is dropped.
type Generation uint64

// slot is one cache entry. chrome is nil for a tombstone: Bust keeps the slot
// so its bumped generation can reveal stale concurrent reads.
type slot struct {
	generation Generation
	chrome     *Chrome
}

// Sidebar caches Chrome per user, saving four queries per page render.
// Handlers that write chrome-affecting data call Bust; the short TTL bounds
// staleness from a missed call. BeginRead and InsertIfCurrent stop a racing
// fill from overwriting a Bust.
type Sidebar struct {
	slots   *TTL[int64, slot]
	enabled bool
}

// NewSidebar makes an enabled cache.
func NewSidebar(capacity int, ttl time.Duration) *Sidebar {
	return &Sidebar{slots: NewTTL[int64, slot](capacity, ttl), enabled: true}
}

// NewDisabledSidebar makes a cache that stores nothing, for the E2E harness:
// it seeds the database directly, bypassing the Bust calls.
func NewDisabledSidebar() *Sidebar {
	return &Sidebar{slots: NewTTL[int64, slot](0, 0)}
}

// DefaultSidebar is the production cache; RDRS_DISABLE_SIDEBAR_CACHE, set to
// anything, turns it off.
func DefaultSidebar() *Sidebar {
	if _, disabled := os.LookupEnv("RDRS_DISABLE_SIDEBAR_CACHE"); disabled {
		return NewDisabledSidebar()
	}
	return NewSidebar(sidebarCapacity, sidebarTTL)
}

// Get returns the cached chrome, or nil.
func (s *Sidebar) Get(userID int64) *Chrome {
	if !s.enabled {
		return nil
	}
	entry, _ := s.slots.Get(userID)
	return entry.chrome
}

// BeginRead snapshots the generation before a read-through fill; pass it to
// InsertIfCurrent.
func (s *Sidebar) BeginRead(userID int64) Generation {
	entry, _ := s.slots.Get(userID)
	return entry.generation
}

// InsertIfCurrent publishes chrome unless a Bust landed since since was taken.
// Publishing a stale fill would hide the write for the whole TTL.
func (s *Sidebar) InsertIfCurrent(userID int64, since Generation, chrome Chrome) {
	if !s.enabled {
		return
	}
	s.slots.Compute(userID, func(old slot, _ bool) (slot, bool) {
		if old.generation != since {
			return slot{}, false
		}
		return slot{generation: old.generation, chrome: &chrome}, true
	})
}

// Bust invalidates the user's chrome.
func (s *Sidebar) Bust(userID int64) {
	if !s.enabled {
		return
	}
	s.slots.Compute(userID, func(old slot, _ bool) (slot, bool) {
		return slot{generation: old.generation + 1}, true
	})
}
