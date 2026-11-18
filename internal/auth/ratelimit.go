package auth

import (
	"hash/maphash"
	"net/netip"
	"sync"
	"time"
)

// Defaults for attempts per client IP inside one window, and the window.
const (
	LoginMaxAttempts = 5
	LoginWindowSecs  = 60
)

// AccountAttemptMultiplier is how much wider the per-account budget is than the
// per-IP one. It must stay wide: anyone can aim an account budget at a victim,
// so a tight one is a lockout denial of service.
const AccountAttemptMultiplier = 4

// rateSlots is the number of counter slots. It is fixed so memory is constant
// and no capacity edge gives an attacker an unrecorded attempt.
const rateSlots = 16_384

// Bucket names the budget an attempt is charged against, so abuse of one
// endpoint cannot lock users out of another.
type Bucket uint8

const (
	// BucketLogin covers password and passkey completion: anything that
	// verifies a credential.
	BucketLogin Bucket = iota
	// BucketAccountSetup covers first-run setup and invite redemption.
	BucketAccountSetup
	// BucketPasskeyProbe covers the passkey ceremony start: cheap,
	// unauthenticated and enumerable.
	BucketPasskeyProbe
	// BucketPasswordChange stops a hijacked session guessing the current
	// password or burning CPU on Argon2.
	BucketPasswordChange
)

// Decision is the outcome of an acquire. RetryAfterSecs is computed under the
// same lock as the decision.
type Decision struct {
	Allowed        bool
	RetryAfterSecs int
}

var allowed = Decision{Allowed: true}

type slot struct {
	attempts uint32
	start    time.Time
}

// window holds fixed-window counters of (attempts, window start), reset lazily
// on the first attempt after the window elapsed. Keys hash into slots with a
// per-process seed, so a collision only over-throttles and cannot be aimed at
// a victim.
type window struct {
	mu          sync.Mutex
	slots       []slot
	seed        maphash.Seed
	maxAttempts uint32
	window      time.Duration
	now         func() time.Time
}

func newWindow(maxAttempts uint32, windowSecs uint64, now func() time.Time) *window {
	start := now()
	slots := make([]slot, rateSlots)
	for i := range slots {
		slots[i].start = start
	}
	return &window{
		slots:       slots,
		seed:        maphash.MakeSeed(),
		maxAttempts: maxAttempts,
		window:      time.Duration(windowSecs) * time.Second,
		now:         now,
	}
}

type ipKey struct {
	bucket Bucket
	ip     netip.Addr
}

type accountKey struct {
	bucket  Bucket
	account string
}

func slotIndex[K comparable](w *window, key K) int {
	return int(maphash.Comparable(w.seed, key) % rateSlots)
}

// tryAcquire checks and increments under one lock; split into check and
// record, concurrent requests would all pass the check.
func tryAcquire[K comparable](w *window, key K) Decision {
	if w.maxAttempts == 0 {
		return allowed
	}
	w.mu.Lock()
	defer w.mu.Unlock()

	s := &w.slots[slotIndex(w, key)]
	now := w.now()
	elapsed := now.Sub(s.start)
	if elapsed >= w.window {
		*s = slot{attempts: 1, start: now}
		return allowed
	}
	if s.attempts >= w.maxAttempts {
		// Floored at 1: "Retry-After: 0" invites a retry certain to fail.
		// Whole elapsed seconds, as the Rust limiter counted them.
		remaining := int((w.window - elapsed.Truncate(time.Second)) / time.Second)
		return Decision{RetryAfterSecs: max(remaining, 1)}
	}
	s.attempts++
	return allowed
}

func release[K comparable](w *window, key K) {
	w.mu.Lock()
	defer w.mu.Unlock()
	s := &w.slots[slotIndex(w, key)]
	if s.attempts > 0 {
		s.attempts--
	}
}

// RateLimiter limits credential endpoints (OWASP ASVS V2.2.1) over two
// independent subject spaces, client IP and account name. It must run before
// any query or Argon2 verify, or an attacker picks the CPU cost of each guess.
type RateLimiter struct {
	perIP      *window
	perAccount *window
}

// NewRateLimiter allows maxAttempts per window per (bucket, ip) and
// AccountAttemptMultiplier times that per (bucket, account). Zero attempts
// disables both.
func NewRateLimiter(maxAttempts uint32, windowSecs uint64) *RateLimiter {
	return newRateLimiter(maxAttempts, windowSecs, time.Now)
}

func newRateLimiter(maxAttempts uint32, windowSecs uint64, now func() time.Time) *RateLimiter {
	accountAttempts := uint64(maxAttempts) * AccountAttemptMultiplier
	// Saturate: a wrapped (tiny) account limit would lock everyone out.
	accountAttempts = min(accountAttempts, uint64(^uint32(0)))
	return &RateLimiter{
		perIP:      newWindow(maxAttempts, windowSecs, now),
		perAccount: newWindow(uint32(accountAttempts), windowSecs, now),
	}
}

// TryAcquire reserves an attempt for (bucket, ip); call Release after a
// successful credential check.
func (l *RateLimiter) TryAcquire(bucket Bucket, ip netip.Addr) Decision {
	return tryAcquire(l.perIP, ipKey{bucket, ip})
}

// Release refunds the attempt after a successful credential check, so users
// are never locked out by their own logins.
func (l *RateLimiter) Release(bucket Bucket, ip netip.Addr) {
	release(l.perIP, ipKey{bucket, ip})
}

// TryAcquireAccount reserves an attempt against the named account, before any
// lookup. Pass the username exactly as it will be looked up; it is only
// hashed, never stored.
func (l *RateLimiter) TryAcquireAccount(bucket Bucket, username string) Decision {
	return tryAcquire(l.perAccount, accountKey{bucket, username})
}

// ReleaseAccount refunds TryAcquireAccount after a successful check.
func (l *RateLimiter) ReleaseAccount(bucket Bucket, username string) {
	release(l.perAccount, accountKey{bucket, username})
}
