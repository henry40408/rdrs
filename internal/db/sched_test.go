package db

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestBackgroundWaitsForUserOperations(t *testing.T) {
	s := newSched()
	releaseFirst := s.enterUser()
	releaseSecond := s.enterUser()

	admitted := make(chan struct{})
	go func() {
		if err := s.waitIdle(context.Background()); err != nil {
			t.Error(err)
		}
		close(admitted)
	}()

	expectBlocked(t, admitted)
	releaseFirst()
	releaseFirst() // double release must not free the second operation's slot
	expectBlocked(t, admitted)
	releaseSecond()

	select {
	case <-admitted:
	case <-time.After(2 * time.Second):
		t.Fatal("background operation was not admitted after the last user operation ended")
	}
}

func expectBlocked(t *testing.T, ch <-chan struct{}) {
	t.Helper()
	select {
	case <-ch:
		t.Fatal("background operation was admitted while a user operation was in flight")
	case <-time.After(50 * time.Millisecond):
	}
}

func TestWaitIdleReturnsImmediatelyWhenIdle(t *testing.T) {
	if err := newSched().waitIdle(context.Background()); err != nil {
		t.Fatal(err)
	}
}

func TestWaitIdleHonorsContext(t *testing.T) {
	s := newSched()
	release := s.enterUser()
	defer release()

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if err := s.waitIdle(ctx); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("waitIdle = %v, want deadline exceeded", err)
	}
}

func TestIdleSignalIsRearmedAfterReuse(t *testing.T) {
	s := newSched()
	s.enterUser()()
	release := s.enterUser()

	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if err := s.waitIdle(ctx); err == nil {
		t.Fatal("waitIdle returned while a user operation was in flight")
	}
	release()
}
