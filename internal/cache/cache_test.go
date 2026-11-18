package cache

import (
	"sync"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/timezone"
)

type fakeClock struct{ now time.Time }

func (f *fakeClock) advance(d time.Duration) { f.now = f.now.Add(d) }

func ttlWithClock[K comparable, V any](capacity int, ttl time.Duration) (*TTL[K, V], *fakeClock) {
	clock := &fakeClock{now: time.Unix(1_000_000, 0)}
	c := NewTTL[K, V](capacity, ttl)
	c.now = func() time.Time { return clock.now }
	return c, clock
}

func TestTTLExpiresFromInsertionNotFromRead(t *testing.T) {
	c, clock := ttlWithClock[string, int](10, time.Minute)
	c.Put("a", 1)
	clock.advance(59 * time.Second)
	if v, ok := c.Get("a"); !ok || v != 1 {
		t.Fatalf("Get = %d, %v", v, ok)
	}
	clock.advance(time.Second)
	if _, ok := c.Get("a"); ok {
		t.Fatal("entry should have expired exactly at its TTL")
	}
}

func TestTTLPutRestartsTheClock(t *testing.T) {
	c, clock := ttlWithClock[string, int](10, time.Minute)
	c.Put("a", 1)
	clock.advance(50 * time.Second)
	c.Put("a", 2)
	clock.advance(50 * time.Second)
	if v, ok := c.Get("a"); !ok || v != 2 {
		t.Fatalf("Get = %d, %v", v, ok)
	}
}

func TestTTLEvictsTheOldestInsertionWhenFull(t *testing.T) {
	c, _ := ttlWithClock[int, int](2, time.Hour)
	c.Put(1, 1)
	c.Put(2, 2)
	c.Get(1) // a read does not refresh the position
	c.Put(3, 3)
	if _, ok := c.Get(1); ok {
		t.Error("1 is the oldest insertion and should be gone")
	}
	if _, ok := c.Get(2); !ok {
		t.Error("2 should remain")
	}
	if _, ok := c.Get(3); !ok {
		t.Error("3 should remain")
	}
	c.Put(2, 22) // replacing does not grow the cache
	if _, ok := c.Get(3); !ok {
		t.Error("replacing a key must not evict another")
	}
}

func TestTTLZeroCapacityStoresNothing(t *testing.T) {
	c := NewTTL[int, int](0, time.Hour)
	c.Put(1, 1)
	if _, ok := c.Get(1); ok {
		t.Fatal("stored despite zero capacity")
	}
}

func TestTTLDelete(t *testing.T) {
	c, _ := ttlWithClock[int, int](2, time.Hour)
	c.Put(1, 1)
	c.Delete(1)
	c.Delete(9)
	if _, ok := c.Get(1); ok {
		t.Fatal("still present")
	}
}

func chromeFor(unread int64) Chrome {
	return Chrome{TotalUnread: unread, Timezone: time.UTC, Today: timezone.TodayIn(time.UTC)}
}

func TestSidebarMissReturnsNil(t *testing.T) {
	if NewSidebar(100, time.Minute).Get(1) != nil {
		t.Fatal("miss should be nil")
	}
}

func publish(s *Sidebar, userID int64, chrome Chrome) {
	s.InsertIfCurrent(userID, s.BeginRead(userID), chrome)
}

func TestSidebarInsertGetAndBust(t *testing.T) {
	s := NewSidebar(100, time.Minute)
	publish(s, 1, chromeFor(3))
	if got := s.Get(1); got == nil || got.TotalUnread != 3 {
		t.Fatalf("Get = %+v", got)
	}
	if s.Get(2) != nil {
		t.Fatal("another user's slot must be empty")
	}
	s.Bust(1)
	if s.Get(1) != nil {
		t.Fatal("Bust must clear the chrome")
	}
	publish(s, 1, chromeFor(4))
	if got := s.Get(1); got == nil || got.TotalUnread != 4 {
		t.Fatalf("Get after refill = %+v", got)
	}
}

func TestSidebarRacingFillIsDropped(t *testing.T) {
	s := NewSidebar(100, time.Minute)
	since := s.BeginRead(1)
	s.Bust(1) // a write lands while the reader is querying
	s.InsertIfCurrent(1, since, chromeFor(9))
	if s.Get(1) != nil {
		t.Fatal("a fill that started before the bust must not be published")
	}
	publish(s, 1, chromeFor(1))
	if s.Get(1) == nil {
		t.Fatal("a fill that started after the bust is published")
	}
}

func TestSidebarGenerationSurvivesRepeatedBusts(t *testing.T) {
	s := NewSidebar(100, time.Minute)
	first := s.BeginRead(1)
	s.Bust(1)
	second := s.BeginRead(1)
	s.Bust(1)
	s.InsertIfCurrent(1, first, chromeFor(1))
	s.InsertIfCurrent(1, second, chromeFor(2))
	if s.Get(1) != nil {
		t.Fatal("both fills are stale")
	}
}

func TestSidebarDisabledStoresNothing(t *testing.T) {
	s := NewDisabledSidebar()
	publish(s, 1, chromeFor(1))
	s.Bust(1)
	if s.Get(1) != nil {
		t.Fatal("disabled cache stored a value")
	}
}

func TestDefaultSidebarHonoursTheDisableVariable(t *testing.T) {
	t.Setenv("RDRS_DISABLE_SIDEBAR_CACHE", "")
	publish(DefaultSidebar(), 1, chromeFor(1))
	if DefaultSidebar().Get(1) != nil {
		t.Fatal("variable set, even empty, must disable the cache")
	}
	// os.Unsetenv is restored by t.Setenv's cleanup.
	unset(t, "RDRS_DISABLE_SIDEBAR_CACHE")
	s := DefaultSidebar()
	publish(s, 1, chromeFor(1))
	if s.Get(1) == nil {
		t.Fatal("cache should be on by default")
	}
}

func TestSidebarConcurrentFillsAndBusts(t *testing.T) {
	s := NewSidebar(100, time.Minute)
	var wg sync.WaitGroup
	for i := range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for range 200 {
				if i%2 == 0 {
					s.Bust(1)
				} else {
					publish(s, 1, chromeFor(int64(i)))
				}
			}
		}()
	}
	wg.Wait()
}

func TestChromeIsCurrentOnlyForToday(t *testing.T) {
	tz, err := time.LoadLocation("Asia/Taipei")
	if err != nil {
		t.Skip("no tz database")
	}
	today := timezone.TodayIn(tz)
	if !(Chrome{Timezone: tz, Today: today}).IsCurrent() {
		t.Error("today's chrome should be current")
	}
	if (Chrome{Timezone: tz, Today: today.AddDays(-1)}).IsCurrent() {
		t.Error("yesterday's chrome should not be current")
	}
}

func TestSummaryLifecycle(t *testing.T) {
	s := NewSummary(100, time.Hour)
	if _, ok := s.Get(1, 100); ok {
		t.Fatal("empty cache returned an entry")
	}
	s.SetPending(1, 100)
	if status, _ := s.GetStatus(1, 100); status != models.SummaryPending {
		t.Fatalf("status = %s", status)
	}
	s.SetProcessing(1, 100)
	if status, _ := s.GetStatus(1, 100); status != models.SummaryProcessing {
		t.Fatalf("status = %s", status)
	}
	s.SetCompleted(1, 100, "text")
	if e, _ := s.Get(1, 100); e.Status != models.SummaryCompleted || *e.SummaryText != "text" || e.ErrorMessage != nil || e.CreatedAt.IsZero() {
		t.Fatalf("entry = %+v", e)
	}
	s.SetFailed(1, 100, "boom")
	if e, _ := s.Get(1, 100); e.Status != models.SummaryFailed || *e.ErrorMessage != "boom" || e.SummaryText != nil {
		t.Fatalf("entry = %+v", e)
	}
	s.Remove(1, 100)
	if _, ok := s.GetStatus(1, 100); ok {
		t.Fatal("removed entry still cached")
	}
}

func TestSummaryKeysAreScopedToTheUser(t *testing.T) {
	s := NewSummary(100, time.Hour)
	s.SetPending(1, 100)
	if _, ok := s.Get(2, 100); ok {
		t.Fatal("user 2 must not see user 1's entry")
	}
	if _, ok := s.Get(1, 101); ok {
		t.Fatal("another entry must not match")
	}
}

func TestAdminDBStats(t *testing.T) {
	a := NewAdminDBStats()
	if _, ok := a.Get(); ok {
		t.Fatal("empty")
	}
	a.Put(models.AdminDatabaseStats{TotalEntries: 7})
	if got, ok := a.Get(); !ok || got.TotalEntries != 7 {
		t.Fatalf("Get = %+v, %v", got, ok)
	}
	if AdminDBStatsTTL != 60*time.Second {
		t.Fatalf("ttl = %v", AdminDBStatsTTL)
	}
}
