// Package events fans per-user notifications out to open browser tabs.
package events

import (
	"context"
	"encoding/json"
	"errors"
	"sync"
	"sync/atomic"

	"github.com/henry40408/rdrs/internal/models"
)

// Capacity is how many events a subscriber may fall behind before it is
// marked lagged.
const Capacity = 256

// ErrClosed is returned by Next once the bus is closed.
var ErrClosed = errors.New("event bus closed")

// Kind says what changed.
type Kind int

// The event kinds.
const (
	// Sidebar means counts changed; the client refetches /api/sidebar.
	Sidebar Kind = iota
	// Summary means an entry's summary status changed.
	Summary
)

// Event is one notification for one user.
type Event struct {
	UserID  int64
	Kind    Kind
	EntryID int64
	// Status is nil when the summary was cleared.
	Status *models.SummaryStatus
}

// Wire returns the SSE event name and data line of the event.
func (e Event) Wire() (name, data string) {
	if e.Kind == Sidebar {
		return "sidebar", "1"
	}
	payload := struct {
		EntryID int64   `json:"entry_id"`
		Status  *string `json:"status"`
	}{EntryID: e.EntryID}
	if e.Status != nil {
		status := string(*e.Status)
		payload.Status = &status
	}
	encoded, _ := json.Marshal(payload) // a fixed shape cannot fail
	return "summary", string(encoded)
}

// Bus delivers events to every subscriber without blocking the sender.
type Bus struct {
	mu          sync.Mutex
	subscribers map[*Subscription]struct{}
	closed      bool
}

// NewBus makes an empty bus.
func NewBus() *Bus {
	return &Bus{subscribers: make(map[*Subscription]struct{})}
}

// Subscription is one tab's view of the bus.
type Subscription struct {
	bus    *Bus
	events chan Event
	lagged atomic.Bool
}

// Subscribe registers a subscriber. Call Close when done with it.
func (b *Bus) Subscribe() *Subscription {
	sub := &Subscription{bus: b, events: make(chan Event, Capacity)}
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.closed {
		close(sub.events)
		return sub
	}
	b.subscribers[sub] = struct{}{}
	return sub
}

// Emit delivers the event to every subscriber. A subscriber whose buffer is
// full misses it and is marked lagged, so its next read is a resync nudge. With
// no subscribers (no open tabs) it does nothing.
func (b *Bus) Emit(event Event) {
	b.mu.Lock()
	defer b.mu.Unlock()
	for sub := range b.subscribers {
		select {
		case sub.events <- event:
		default:
			sub.lagged.Store(true)
		}
	}
}

// EmitSidebar tells the user's tabs to refetch the sidebar.
func (b *Bus) EmitSidebar(userID int64) {
	b.Emit(Event{UserID: userID, Kind: Sidebar})
}

// EmitSummary tells the user's tabs an entry's summary status changed; a nil
// status means it was cleared.
func (b *Bus) EmitSummary(userID, entryID int64, status *models.SummaryStatus) {
	b.Emit(Event{UserID: userID, Kind: Summary, EntryID: entryID, Status: status})
}

// Close ends every subscription once its buffered events are read.
func (b *Bus) Close() {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.closed {
		return
	}
	b.closed = true
	for sub := range b.subscribers {
		close(sub.events)
	}
	b.subscribers = nil
}

// Next waits for the next event. A lagged subscriber first gets a Sidebar event
// for its own user ID of zero, which means "refetch": the caller must send it
// to this tab whatever the user. It returns ErrClosed when the bus closes and
// the context's error when ctx ends.
func (s *Subscription) Next(ctx context.Context) (Event, error) {
	if s.lagged.Swap(false) {
		return Event{Kind: Sidebar}, nil
	}
	select {
	case event, open := <-s.events:
		if !open {
			return Event{}, ErrClosed
		}
		return event, nil
	case <-ctx.Done():
		return Event{}, ctx.Err()
	}
}

// IsResync reports whether the event is the nudge Next sends to a lagged
// subscriber, as opposed to a sidebar event for a user.
func (e Event) IsResync() bool {
	return e.Kind == Sidebar && e.UserID == 0
}

// Close unsubscribes.
func (s *Subscription) Close() {
	s.bus.mu.Lock()
	defer s.bus.mu.Unlock()
	if _, subscribed := s.bus.subscribers[s]; !subscribed {
		return
	}
	delete(s.bus.subscribers, s)
	close(s.events)
}
