package events

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
)

func next(t *testing.T, sub *Subscription) Event {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	event, err := sub.Next(ctx)
	if err != nil {
		t.Fatalf("Next: %v", err)
	}
	return event
}

func TestEmitReachesEverySubscriber(t *testing.T) {
	bus := NewBus()
	a, b := bus.Subscribe(), bus.Subscribe()
	bus.EmitSidebar(7)
	for _, sub := range []*Subscription{a, b} {
		if e := next(t, sub); e.UserID != 7 || e.Kind != Sidebar || e.IsResync() {
			t.Fatalf("event = %+v", e)
		}
	}
}

func TestEmitWithoutSubscribersIsHarmless(t *testing.T) {
	NewBus().EmitSidebar(1)
}

func TestSummaryEventCarriesStatus(t *testing.T) {
	bus := NewBus()
	sub := bus.Subscribe()
	done := models.SummaryCompleted
	bus.EmitSummary(1, 50, &done)
	bus.EmitSummary(1, 51, nil)
	first, second := next(t, sub), next(t, sub)
	if first.EntryID != 50 || *first.Status != models.SummaryCompleted {
		t.Fatalf("first = %+v", first)
	}
	if second.EntryID != 51 || second.Status != nil {
		t.Fatalf("second = %+v", second)
	}
}

func TestWire(t *testing.T) {
	done := models.SummaryCompleted
	cases := []struct {
		event      Event
		name, data string
	}{
		{Event{Kind: Sidebar}, "sidebar", "1"},
		{Event{Kind: Summary, EntryID: 5, Status: &done}, "summary", `{"entry_id":5,"status":"completed"}`},
		{Event{Kind: Summary, EntryID: 6}, "summary", `{"entry_id":6,"status":null}`},
	}
	for _, c := range cases {
		if name, data := c.event.Wire(); name != c.name || data != c.data {
			t.Errorf("Wire(%+v) = %q, %q", c.event, name, data)
		}
	}
}

func TestSlowSubscriberIsMarkedLaggedAndResyncs(t *testing.T) {
	bus := NewBus()
	slow := bus.Subscribe()
	for range Capacity + 1 {
		bus.EmitSidebar(1)
	}
	if e := next(t, slow); !e.IsResync() {
		t.Fatalf("first read after lag = %+v, want resync", e)
	}
	if e := next(t, slow); e.IsResync() || e.UserID != 1 {
		t.Fatalf("buffered event = %+v", e)
	}
}

func TestExactlyCapacityDoesNotLag(t *testing.T) {
	bus := NewBus()
	sub := bus.Subscribe()
	for range Capacity {
		bus.EmitSidebar(1)
	}
	if e := next(t, sub); e.IsResync() {
		t.Fatal("a full buffer without overflow is not lag")
	}
}

func TestNextHonoursContext(t *testing.T) {
	sub := NewBus().Subscribe()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := sub.Next(ctx); !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v", err)
	}
}

func TestCloseEndsSubscriptionsAfterBufferedEvents(t *testing.T) {
	bus := NewBus()
	sub := bus.Subscribe()
	bus.EmitSidebar(1)
	bus.Close()
	bus.Close()
	if e := next(t, sub); e.UserID != 1 {
		t.Fatalf("event = %+v", e)
	}
	if _, err := sub.Next(context.Background()); !errors.Is(err, ErrClosed) {
		t.Fatalf("err = %v", err)
	}
	late := bus.Subscribe()
	if _, err := late.Next(context.Background()); !errors.Is(err, ErrClosed) {
		t.Fatalf("late subscriber err = %v", err)
	}
	bus.EmitSidebar(1)
	late.Close()
}

func TestUnsubscribeStopsDelivery(t *testing.T) {
	bus := NewBus()
	sub := bus.Subscribe()
	sub.Close()
	sub.Close()
	bus.EmitSidebar(1)
	if _, err := sub.Next(context.Background()); !errors.Is(err, ErrClosed) {
		t.Fatalf("err = %v", err)
	}
}
