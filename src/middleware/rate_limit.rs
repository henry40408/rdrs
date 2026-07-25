//! Per-client-IP rate limiting for credential-accepting endpoints.
//!
//! OWASP's Authentication Cheat Sheet (anti-automation controls) and ASVS
//! V2.2.1 both call for throttling repeated authentication attempts from a
//! single source. The limiter here is deliberately simple — an in-memory
//! fixed window keyed by ([`Bucket`], [`IpAddr`]) — because its only job is to
//! make credential stuffing and password spraying expensive *before* the
//! request reaches a database query or an Argon2 `verify_password` call.
//! Argon2 is tuned to be slow on purpose; a limiter that runs after hashing
//! would still let an attacker choose how much CPU the server spends per
//! guess.
//!
//! # Why not `check()` + `record()`
//!
//! An earlier design split the decision into two calls: `check(ip) -> bool`
//! to ask "is this IP still under budget?", then, once the handler decided to
//! proceed, `record(ip)` to count the attempt. That shape has a race: two
//! concurrent requests from the same IP can both call `check` before either
//! calls `record`, both observe the pre-attack count, and both proceed — the
//! limit is enforced in wall-clock time but not against concurrency. Under a
//! real credential-stuffing tool, which fires requests in parallel precisely
//! to exploit this kind of gap, the limiter would do nothing.
//! [`RateLimiter::try_acquire`] holds the lock for the entire
//! check-and-increment, so concurrent callers are serialized against the same
//! counter and cannot all observe the same pre-attempt state.
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// Default attempts allowed per client IP inside one window.
pub const LOGIN_MAX_ATTEMPTS: u32 = 5;
/// Default fixed-window length, in seconds.
pub const LOGIN_WINDOW_SECS: u64 = 60;

/// Number of counter slots. Fixed at construction, so the limiter's memory is
/// constant regardless of how wide an attack fans out — there is no capacity
/// edge case left to bypass (a previous capped-`HashMap` design admitted an
/// unrecorded, and therefore unthrottled, request whenever a new IP arrived
/// at capacity).
const SLOTS: usize = 16_384;

/// Which budget an attempt is charged against.
///
/// Separate buckets stop abuse of one endpoint from locking users out of
/// another — in particular a registration refused by configuration must never
/// consume the password-login budget, which would turn a misconfigured signup
/// page into a denial of authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// Password and passkey *completion* — anything that verifies a credential.
    Login,
    /// Account creation.
    Register,
    /// Passkey ceremony *start*: cheap, unauthenticated, and enumerable, so it
    /// gets its own budget rather than spending the login one.
    PasskeyProbe,
}

/// A fixed-window rate limiter keyed by `(Bucket, IpAddr)`.
///
/// Each slot records `(attempts_in_window, window_started_at)`. A window
/// resets lazily — on the next attempt after it has elapsed — rather than via
/// a background sweep, so the limiter needs no timer task of its own.
///
/// # Collisions
///
/// The slot array is addressed by a process-random keyed hash of
/// `(bucket, ip)`, not by an exact-match table, so two distinct keys may land
/// on the same slot. The effect is bounded and asymmetric: colliding keys can
/// only *over*-throttle each other (they share one counter), and because
/// [`RandomState`] is seeded per process an attacker cannot predict or
/// deliberately provoke a collision with a chosen victim. A [`RateLimiter::release`]
/// from one colliding key decrements the other's count too — a mild, bounded
/// under-throttle, not a bypass.
///
/// # Fixed, not sliding, window
///
/// The window resets lazily on the first attempt after it elapses, so a
/// client can spend its full budget at the end of one window and again at the
/// start of the next: up to `2 × max_attempts` in a short span across the
/// boundary. That is acceptable for an anti-automation control whose job is
/// to make bulk guessing expensive, and it keeps the limiter allocation-free
/// after construction.
#[derive(Debug)]
pub struct RateLimiter {
    slots: Mutex<Box<[(u32, Instant)]>>,
    hasher: RandomState,
    max_attempts: u32,
    window_secs: u64,
}

impl RateLimiter {
    /// Build a limiter allowing `max_attempts` per `window_secs`-second fixed
    /// window per `(bucket, ip)`. `max_attempts == 0` disables the limiter
    /// entirely (see [`RateLimiter::try_acquire`]) — the documented escape
    /// hatch for internal deployments (e.g. behind an already-authenticating
    /// reverse proxy) where this protection would only get in the way.
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        // A zero count makes the initial `Instant` irrelevant: the first
        // `try_acquire` for any slot either finds the window elapsed (and
        // resets to `(1, now)`) or increments from a genuine zero.
        let slots = vec![(0u32, Instant::now()); SLOTS].into_boxed_slice();
        Self {
            slots: Mutex::new(slots),
            hasher: RandomState::new(),
            max_attempts,
            window_secs,
        }
    }

    /// Lock the slot array, tolerating poison.
    ///
    /// A panic while holding the lock must not take the login path down with
    /// it: the counters are advisory (losing one is a minor availability
    /// blip, not a security hole), so recovering the inner slice is strictly
    /// better than poisoning every subsequent attempt and locking every
    /// client out of authentication because of an unrelated bug.
    fn guard(&self) -> MutexGuard<'_, Box<[(u32, Instant)]>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hash `(bucket, ip)` down to a slot index. Process-random (see the
    /// collision note on the struct docs), so an attacker cannot compute in
    /// advance which other key shares their slot.
    fn slot_index(&self, bucket: Bucket, ip: IpAddr) -> usize {
        let hash = self.hasher.hash_one((bucket, ip));
        // SLOTS fits comfortably in a u64, and the modulo result is always
        // < SLOTS, so this always fits back into a usize.
        usize::try_from(hash % SLOTS as u64).expect("modulo SLOTS fits usize")
    }

    /// Reserve an attempt for `(bucket, ip)`, returning whether it may
    /// proceed.
    ///
    /// Call this *before* any database query or password verification for
    /// the request; that ordering is the entire point of the limiter. On
    /// `true`, the caller has spent one of `(bucket, ip)`'s attempts for this
    /// window — if the credential check goes on to succeed, call
    /// [`RateLimiter::release`] to hand it back.
    pub fn try_acquire(&self, bucket: Bucket, ip: IpAddr) -> bool {
        // Disabled: every request proceeds. Without this early return the
        // generic `slot.0 >= self.max_attempts` comparison below would block
        // every single request once `max_attempts` is 0, the opposite of
        // "disabled".
        if self.max_attempts == 0 {
            return true;
        }

        let idx = self.slot_index(bucket, ip);

        // The entire check-and-increment happens under one lock acquisition.
        // Splitting this into a separate "peek" and "increment" would reopen
        // the race described in the module docs: two concurrent requests
        // could both see room in the window and both be admitted.
        let mut slots = self.guard();
        let slot = &mut slots[idx];

        if slot.1.elapsed().as_secs() >= self.window_secs {
            // The window has elapsed: start a fresh one with this attempt as
            // the first count in it, regardless of what the stale count was.
            *slot = (1, Instant::now());
            return true;
        }

        if slot.0 >= self.max_attempts {
            return false;
        }

        slot.0 += 1;
        true
    }

    /// Hand back the attempt reserved by [`RateLimiter::try_acquire`], after
    /// a *successful* credential check for `(bucket, ip)`.
    ///
    /// A legitimate user who signs in repeatedly — a new device, a cleared
    /// cookie jar, an integration test exercising login in a loop — must
    /// never be locked out by their own successful attempts. An attacker,
    /// whose every attempt fails by definition, never calls this and so gets
    /// no refund. If the slot's window has already expired or was never
    /// incremented (e.g. the limiter is disabled), this is a harmless no-op:
    /// `saturating_sub` floors the count at zero rather than wrapping.
    pub fn release(&self, bucket: Bucket, ip: IpAddr) {
        let idx = self.slot_index(bucket, ip);
        let mut slots = self.guard();
        slots[idx].0 = slots[idx].0.saturating_sub(1);
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(LOGIN_MAX_ATTEMPTS, LOGIN_WINDOW_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn ipv4(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, last))
    }

    #[test]
    fn allows_up_to_max_then_blocks() {
        let limiter = RateLimiter::new(5, 60);
        let ip = ipv4(1);
        for _ in 0..5 {
            assert!(limiter.try_acquire(Bucket::Login, ip));
        }
        assert!(!limiter.try_acquire(Bucket::Login, ip));
    }

    #[test]
    fn window_expiry_resets_the_counter() {
        // A zero-second window is elapsed the instant it is recorded, so even
        // a limit of 1 never actually throttles anything. `Config` rejects a
        // zero `RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS` at startup (see
        // `parse_login_rate_limit_window_secs`), so this shape is only
        // reachable here, exercising the limiter directly.
        let limiter = RateLimiter::new(1, 0);
        let ip = ipv4(2);
        assert!(limiter.try_acquire(Bucket::Login, ip));
        assert!(limiter.try_acquire(Bucket::Login, ip));
    }

    #[test]
    fn distinct_ips_have_independent_buckets() {
        let limiter = RateLimiter::new(1, 60);
        let v4: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 3));
        let v6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 3));

        assert!(limiter.try_acquire(Bucket::Login, v4));
        assert!(!limiter.try_acquire(Bucket::Login, v4));

        // A different address (even a different family) gets its own budget.
        assert!(limiter.try_acquire(Bucket::Login, v6));
        assert!(!limiter.try_acquire(Bucket::Login, v6));
    }

    #[test]
    fn release_returns_the_reserved_attempt() {
        let limiter = RateLimiter::new(2, 60);
        let ip = ipv4(4);
        for _ in 0..10 {
            assert!(limiter.try_acquire(Bucket::Login, ip));
            limiter.release(Bucket::Login, ip);
        }
        // Every reservation was released, so ten cycles through a budget of 2
        // never exhaust it — a further attempt must still be admitted.
        assert!(limiter.try_acquire(Bucket::Login, ip));
    }

    #[test]
    fn zero_max_attempts_disables_the_limiter() {
        let limiter = RateLimiter::new(0, 60);
        let ip = ipv4(5);
        for _ in 0..1000 {
            assert!(limiter.try_acquire(Bucket::Login, ip));
        }
    }

    #[test]
    fn spray_does_not_unthrottle_anyone() {
        // Regression for the old capped-HashMap design: at capacity, a brand
        // new IP was admitted *without being recorded*, leaving it
        // unthrottled forever rather than just for one request. The
        // fixed-size slot array has no such capacity edge, but this proves it
        // end to end: a throttled victim stays throttled through a wide
        // spray, and a fresh IP arriving after the spray still gets throttled
        // once it exhausts its own budget.
        let limiter = RateLimiter::new(2, 60);

        let victim = ipv4(200);
        assert!(limiter.try_acquire(Bucket::Login, victim));
        assert!(limiter.try_acquire(Bucket::Login, victim));
        assert!(
            !limiter.try_acquire(Bucket::Login, victim),
            "victim should be throttled"
        );

        // Spray 50,000 distinct IPv4 and IPv6 addresses — far more than
        // `SLOTS` — so slot collisions are guaranteed.
        for i in 0..25_000u32 {
            let v4 = IpAddr::V4(std::net::Ipv4Addr::from(i.to_be_bytes()));
            let v6 = IpAddr::V6(std::net::Ipv6Addr::from(u128::from(i)));
            limiter.try_acquire(Bucket::Login, v4);
            limiter.try_acquire(Bucket::Login, v6);
        }

        assert!(
            !limiter.try_acquire(Bucket::Login, victim),
            "victim must remain throttled after an unrelated spray"
        );

        // A fresh IP that never appeared in the spray must still be
        // throttled — the defect-2 failure mode was this address getting
        // unlimited attempts forever. It may share a slot with a sprayed
        // address (a 50,000-key spray into 16,384 slots guarantees
        // collisions — see the collision note on the struct docs), so it can
        // be throttled sooner than its own two-attempt budget; the only
        // thing this asserts is that it is throttled at all within a few
        // attempts, never indefinitely admitted.
        let fresh = ipv4(201);
        let throttled = (0..5).any(|_| !limiter.try_acquire(Bucket::Login, fresh));
        assert!(
            throttled,
            "a fresh IP arriving after a wide spray must still be throttled eventually, \
             not admitted forever the way the old capped-HashMap design admitted it"
        );
    }

    #[test]
    fn separate_buckets_do_not_share_budget() {
        // The critical regression: a registration refused by configuration
        // must never spend the login budget for the same IP.
        let limiter = RateLimiter::new(5, 60);
        let ip = ipv4(6);

        for _ in 0..5 {
            assert!(limiter.try_acquire(Bucket::Register, ip));
        }
        assert!(!limiter.try_acquire(Bucket::Register, ip));

        // Login for the same IP is untouched.
        assert!(limiter.try_acquire(Bucket::Login, ip));
    }

    #[test]
    fn release_only_refunds_its_own_bucket() {
        let limiter = RateLimiter::new(1, 60);
        let ip = ipv4(7);

        assert!(limiter.try_acquire(Bucket::Login, ip));
        limiter.release(Bucket::Login, ip);
        // Login is refunded and admits another attempt...
        assert!(limiter.try_acquire(Bucket::Login, ip));

        // ...but Register for the same IP was never touched, so it is
        // exhausted by its own single attempt, independent of the Login
        // release above.
        assert!(limiter.try_acquire(Bucket::Register, ip));
        assert!(!limiter.try_acquire(Bucket::Register, ip));
    }

    #[test]
    fn concurrent_attempts_cannot_exceed_the_limit() {
        // Regression for the check/record split: 64 threads race on a single
        // IP with a limit of 5. Exactly 5 must be admitted no matter how the
        // threads interleave.
        let limiter = Arc::new(RateLimiter::new(5, 60));
        let ip = ipv4(9);
        let n = 64;
        let barrier = Arc::new(Barrier::new(n));

        let handles: Vec<_> = (0..n)
            .map(|_| {
                let limiter = Arc::clone(&limiter);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    limiter.try_acquire(Bucket::Login, ip)
                })
            })
            .collect();

        let admitted = handles
            .into_iter()
            .map(|h| h.join().expect("worker thread must not panic"))
            .filter(|&acquired| acquired)
            .count();

        assert_eq!(admitted, 5);
    }
}
