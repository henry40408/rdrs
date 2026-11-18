package auth

import (
	"net/netip"
	"testing"
	"time"
)

type fakeClock struct{ now time.Time }

func (c *fakeClock) Now() time.Time          { return c.now }
func (c *fakeClock) Advance(d time.Duration) { c.now = c.now.Add(d) }

var testIP = netip.MustParseAddr("203.0.113.7")

func TestRateLimiterBlocksAfterBudgetAndReportsRetryAfter(t *testing.T) {
	clock := &fakeClock{now: time.Unix(1000, 0)}
	limiter := newRateLimiter(3, 60, clock.Now)
	for i := 0; i < 3; i++ {
		if !limiter.TryAcquire(BucketLogin, testIP).Allowed {
			t.Fatalf("attempt %d refused", i+1)
		}
	}
	clock.Advance(10*time.Second + 500*time.Millisecond)
	decision := limiter.TryAcquire(BucketLogin, testIP)
	if decision.Allowed {
		t.Fatal("fourth attempt allowed")
	}
	// Whole elapsed seconds, as the Rust limiter counted: 60 - 10.
	if decision.RetryAfterSecs != 50 {
		t.Errorf("RetryAfterSecs = %d, want 50", decision.RetryAfterSecs)
	}
}

func TestRateLimiterRetryAfterIsAtLeastOne(t *testing.T) {
	clock := &fakeClock{now: time.Unix(1000, 0)}
	limiter := newRateLimiter(1, 60, clock.Now)
	limiter.TryAcquire(BucketLogin, testIP)
	clock.Advance(59*time.Second + 900*time.Millisecond)
	if got := limiter.TryAcquire(BucketLogin, testIP).RetryAfterSecs; got != 1 {
		t.Errorf("RetryAfterSecs = %d, want 1", got)
	}
}

func TestRateLimiterWindowResetsExactlyAtItsLength(t *testing.T) {
	clock := &fakeClock{now: time.Unix(1000, 0)}
	limiter := newRateLimiter(1, 60, clock.Now)
	limiter.TryAcquire(BucketLogin, testIP)
	clock.Advance(60*time.Second - time.Nanosecond)
	if limiter.TryAcquire(BucketLogin, testIP).Allowed {
		t.Fatal("allowed just before the window ends")
	}
	clock.Advance(time.Nanosecond)
	if !limiter.TryAcquire(BucketLogin, testIP).Allowed {
		t.Fatal("refused once the window elapsed")
	}
}

func TestRateLimiterReleaseRefundsOneAttempt(t *testing.T) {
	limiter := NewRateLimiter(1, 60)
	limiter.TryAcquire(BucketLogin, testIP)
	limiter.Release(BucketLogin, testIP)
	if !limiter.TryAcquire(BucketLogin, testIP).Allowed {
		t.Error("refund did not restore the attempt")
	}
	// Releasing an empty slot must not underflow into a huge budget.
	other := netip.MustParseAddr("198.51.100.1")
	limiter.Release(BucketLogin, other)
	limiter.TryAcquire(BucketLogin, other)
	if limiter.TryAcquire(BucketLogin, other).Allowed {
		t.Error("release on an empty slot granted extra attempts")
	}
}

func TestRateLimiterZeroDisables(t *testing.T) {
	limiter := NewRateLimiter(0, 60)
	for i := 0; i < 100; i++ {
		if !limiter.TryAcquire(BucketLogin, testIP).Allowed || !limiter.TryAcquireAccount(BucketLogin, "alice").Allowed {
			t.Fatal("a disabled limiter refused")
		}
	}
}

func TestRateLimiterAccountBudgetIsFourTimesWider(t *testing.T) {
	limiter := NewRateLimiter(2, 60)
	for i := 0; i < 2*AccountAttemptMultiplier; i++ {
		if !limiter.TryAcquireAccount(BucketLogin, "alice").Allowed {
			t.Fatalf("account attempt %d refused", i+1)
		}
	}
	if limiter.TryAcquireAccount(BucketLogin, "alice").Allowed {
		t.Error("account budget not enforced")
	}
	limiter.ReleaseAccount(BucketLogin, "alice")
	if !limiter.TryAcquireAccount(BucketLogin, "alice").Allowed {
		t.Error("account refund ignored")
	}
}

func TestRateLimiterAccountBudgetSaturates(t *testing.T) {
	limiter := NewRateLimiter(^uint32(0), 60)
	if !limiter.TryAcquireAccount(BucketLogin, "alice").Allowed {
		t.Error("a wrapped account limit locked the account out")
	}
}

func TestRateLimiterBucketsAndSubjectsAreIndependent(t *testing.T) {
	limiter := NewRateLimiter(1, 60)
	limiter.TryAcquire(BucketLogin, testIP)
	// Slots collide with probability 1/16384 by design (an over-throttle only),
	// so try several subjects rather than asserting on one.
	free := 0
	for _, bucket := range []Bucket{BucketAccountSetup, BucketPasskeyProbe, BucketPasswordChange} {
		if limiter.TryAcquire(bucket, testIP).Allowed {
			free++
		}
	}
	if free < 2 {
		t.Errorf("only %d of 3 other buckets were independent of Login", free)
	}
	// An address and an account name never share a window.
	if !limiter.TryAcquireAccount(BucketLogin, testIP.String()).Allowed {
		t.Error("account space shares slots with the IP space")
	}
}
