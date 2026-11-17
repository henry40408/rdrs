package db

import (
	"context"
	"sync"
)

// Priority is the scheduling class of a DB handle.
type Priority int

// User operations are interactive; Background covers feed sync, the summary
// worker, retention and backfill.
const (
	User Priority = iota
	Background
)

// sched is the SQLite write-priority admission gate. It is not a queue: User
// operations count as in flight, Background operations wait for zero, so a
// background batch never makes an interactive click wait on the single writer.
type sched struct {
	mu       sync.Mutex
	inflight int
	// idle is closed while inflight is zero.
	idle chan struct{}
}

func newSched() *sched {
	idle := make(chan struct{})
	close(idle)
	return &sched{idle: idle}
}

// enterUser registers a User operation; the returned func unregisters it and is
// safe to call more than once.
func (s *sched) enterUser() (release func()) {
	s.mu.Lock()
	if s.inflight == 0 {
		s.idle = make(chan struct{})
	}
	s.inflight++
	s.mu.Unlock()

	var once sync.Once
	return func() { once.Do(s.leaveUser) }
}

func (s *sched) leaveUser() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.inflight--
	if s.inflight == 0 {
		close(s.idle)
	}
}

// waitIdle blocks until no User operation is in flight or ctx ends.
func (s *sched) waitIdle(ctx context.Context) error {
	for {
		s.mu.Lock()
		if s.inflight == 0 {
			s.mu.Unlock()
			return nil
		}
		idle := s.idle
		s.mu.Unlock()

		select {
		case <-idle:
		case <-ctx.Done():
			return ctx.Err()
		}
	}
}

func noRelease() {}
