package cache

import (
	"time"

	"github.com/henry40408/rdrs/internal/models"
)

const (
	// SummaryCapacity and SummaryTTL are what the application uses.
	SummaryCapacity = 1000
	SummaryTTL      = 24 * time.Hour
)

// SummaryEntry is the in-flight or recent state of one entry's summary.
type SummaryEntry struct {
	Status       models.SummaryStatus
	SummaryText  *string
	ErrorMessage *string
	CreatedAt    time.Time
}

type summaryKey struct{ userID, entryID int64 }

// Summary remembers where each summary job stands, so a page can show
// progress without a database row for work that has not finished.
type Summary struct {
	entries *TTL[summaryKey, SummaryEntry]
}

// NewSummary makes a cache of at most capacity entries living for ttl.
func NewSummary(capacity int, ttl time.Duration) *Summary {
	return &Summary{entries: NewTTL[summaryKey, SummaryEntry](capacity, ttl)}
}

// Get returns the entry's state, if cached.
func (s *Summary) Get(userID, entryID int64) (SummaryEntry, bool) {
	return s.entries.Get(summaryKey{userID, entryID})
}

// GetStatus returns only the status, if cached.
func (s *Summary) GetStatus(userID, entryID int64) (models.SummaryStatus, bool) {
	entry, found := s.Get(userID, entryID)
	return entry.Status, found
}

func (s *Summary) set(userID, entryID int64, entry SummaryEntry) {
	entry.CreatedAt = time.Now().UTC()
	s.entries.Put(summaryKey{userID, entryID}, entry)
}

// SetPending marks the job as queued.
func (s *Summary) SetPending(userID, entryID int64) {
	s.set(userID, entryID, SummaryEntry{Status: models.SummaryPending})
}

// SetProcessing marks the job as running.
func (s *Summary) SetProcessing(userID, entryID int64) {
	s.set(userID, entryID, SummaryEntry{Status: models.SummaryProcessing})
}

// SetCompleted stores the finished text.
func (s *Summary) SetCompleted(userID, entryID int64, text string) {
	s.set(userID, entryID, SummaryEntry{Status: models.SummaryCompleted, SummaryText: &text})
}

// SetFailed stores the failure message.
func (s *Summary) SetFailed(userID, entryID int64, message string) {
	s.set(userID, entryID, SummaryEntry{Status: models.SummaryFailed, ErrorMessage: &message})
}

// Remove forgets the entry.
func (s *Summary) Remove(userID, entryID int64) {
	s.entries.Delete(summaryKey{userID, entryID})
}

// AdminDBStatsTTL is how long the statistics page's site-wide database figures
// may lag. Nothing a request does moves them, so the TTL is the whole
// invalidation strategy.
const AdminDBStatsTTL = 60 * time.Second

// AdminDBStats memoizes models.GetAdminDatabaseStats. One slot serves every
// admin and period: the full COUNT(*)s and PRAGMAs dominated the page's cost.
type AdminDBStats struct {
	slot *TTL[struct{}, models.AdminDatabaseStats]
}

// NewAdminDBStats makes the cache.
func NewAdminDBStats() *AdminDBStats {
	return &AdminDBStats{slot: NewTTL[struct{}, models.AdminDatabaseStats](1, AdminDBStatsTTL)}
}

// Get returns the cached figures, if fresh.
func (a *AdminDBStats) Get() (models.AdminDatabaseStats, bool) {
	return a.slot.Get(struct{}{})
}

// Put stores the figures.
func (a *AdminDBStats) Put(stats models.AdminDatabaseStats) {
	a.slot.Put(struct{}{}, stats)
}
