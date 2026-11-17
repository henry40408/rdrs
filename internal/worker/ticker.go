// Package worker holds the background loops: feed sync, read-entry retention,
// expired-row cleanup and the one-shot content_text backfill. Each Run blocks
// until its context is cancelled, which is only noticed between ticks.
package worker

import (
	"context"
	"time"
)

// everyInterval calls tick at once and then every interval until ctx ends.
// When a tick overruns, the missed ticks run back to back instead of being
// dropped, as tokio's default interval does; time.Ticker would drop them.
func everyInterval(ctx context.Context, interval time.Duration, tick func()) {
	next := time.Now()
	for {
		if !waitUntil(ctx, next) {
			return
		}
		tick()
		next = next.Add(interval)
	}
}

// waitUntil reports false when ctx ended first. A deadline already in the
// past returns at once, unless ctx has ended.
func waitUntil(ctx context.Context, deadline time.Time) bool {
	if ctx.Err() != nil {
		return false
	}
	wait := time.Until(deadline)
	if wait <= 0 {
		return true
	}
	timer := time.NewTimer(wait)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return true
	}
}
