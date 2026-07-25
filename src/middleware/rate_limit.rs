//! Per-client-IP rate limiting for credential-accepting endpoints.
//!
//! OWASP's Authentication Cheat Sheet (anti-automation controls) and ASVS
//! V2.2.1 both call for throttling repeated authentication attempts from a
//! single source. The limiter here is deliberately simple — an in-memory
//! sliding window keyed by [`IpAddr`] — because its only job is to make
//! credential stuffing and password spraying expensive *before* the request
//! reaches a database query or an Argon2 `verify_password` call. Argon2 is
//! tuned to be slow on purpose; a limiter that runs after hashing would still
//! let an attacker choose how much CPU the server spends per guess.
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
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// Default attempts allowed per client IP inside one window.
pub const LOGIN_MAX_ATTEMPTS: u32 = 5;
/// Default sliding-window length, in seconds.
pub const LOGIN_WINDOW_SECS: u64 = 60;
/// Hard cap on tracked IPs, so a spray from many source addresses cannot grow
/// the map without bound. At one `(IpAddr, (u32, Instant))` entry per
/// attacking address this is a modest, fixed amount of memory regardless of
/// how wide an attack fans out.
const MAX_ENTRIES: usize = 10_000;

/// A sliding-window rate limiter keyed by client IP.
///
/// Each entry records `(attempts_in_window, window_started_at)`. A window
/// resets lazily — on the next attempt after it has elapsed — rather than via
/// a background sweep, so the limiter needs no timer task of its own; the
/// only exception is [`RateLimiter::prune`], an optional helper a caller may
/// invoke periodically to reclaim memory held by IPs that attempted once and
/// never came back.
#[derive(Debug)]
pub struct RateLimiter {
    attempts: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    max_attempts: u32,
    window_secs: u64,
    max_entries: usize,
}

impl RateLimiter {
    /// Build a limiter allowing `max_attempts` per `window_secs`-second
    /// sliding window per IP. `max_attempts == 0` disables the limiter
    /// entirely (see [`RateLimiter::try_acquire`]) — the documented escape
    /// hatch for internal deployments (e.g. behind an already-authenticating
    /// reverse proxy) where this protection would only get in the way.
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts,
            window_secs,
            max_entries: MAX_ENTRIES,
        }
    }

    /// Lock the attempt map, tolerating poison.
    ///
    /// A panic while holding the lock must not take the login path down with
    /// it: the counters are advisory (losing one is a minor availability
    /// blip, not a security hole), so recovering the inner map is strictly
    /// better than poisoning every subsequent attempt and locking every
    /// client out of authentication because of an unrelated bug.
    fn guard(&self) -> MutexGuard<'_, HashMap<IpAddr, (u32, Instant)>> {
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reserve an attempt for `ip`, returning whether it may proceed.
    ///
    /// Call this *before* any database query or password verification for
    /// the request; that ordering is the entire point of the limiter. On
    /// `true`, the caller has spent one of `ip`'s attempts for this window —
    /// if the credential check goes on to succeed, call
    /// [`RateLimiter::release`] to hand it back.
    pub fn try_acquire(&self, ip: IpAddr) -> bool {
        // Disabled: every request proceeds. Without this early return the
        // generic `entry.0 >= self.max_attempts` comparison below would block
        // every single request once `max_attempts` is 0, the opposite of
        // "disabled".
        if self.max_attempts == 0 {
            return true;
        }

        // The entire check-and-increment happens under one lock acquisition.
        // Splitting this into a separate "peek" and "increment" would reopen
        // the race described in the module docs: two concurrent requests
        // could both see room in the window and both be admitted.
        let mut map = self.guard();

        if map.len() >= self.max_entries && !map.contains_key(&ip) {
            // At capacity and this is a new IP. First try to make room by
            // dropping windows that have already elapsed — the common case
            // for a map that grew from a burst of now-stale one-off callers.
            map.retain(|_, (_, started)| started.elapsed().as_secs() < self.window_secs);

            if map.len() >= self.max_entries {
                // Still full of *live* windows: every tracked IP is inside an
                // active window, meaning the map is legitimately busy right
                // now. Two options were rejected here:
                //   - `map.clear()` would hand every currently-throttled IP a
                //     fresh budget merely because *other* addresses showed up.
                //     An attacker could spray attempts from thousands of
                //     source IPs specifically to trigger this and reset their
                //     own counter — turning the cap into a bypass.
                //   - Refusing (returning `false`) would turn a wide spray
                //     into a global login lockout for every legitimate user,
                //     since a new IP could never get a bucket of its own.
                // Allowing the request through without a bucket is the least
                // bad option: this specific unseen IP is not rate-limited for
                // this one request, but no existing counter is disturbed.
                return true;
            }
        }

        let entry = map.entry(ip).or_insert((0, Instant::now()));

        if entry.1.elapsed().as_secs() >= self.window_secs {
            // The window has elapsed: start a fresh one with this attempt as
            // the first count in it, regardless of what the stale count was.
            *entry = (1, Instant::now());
            return true;
        }

        if entry.0 >= self.max_attempts {
            return false;
        }

        entry.0 += 1;
        true
    }

    /// Hand back the attempt reserved by [`RateLimiter::try_acquire`], after
    /// a *successful* credential check for `ip`.
    ///
    /// A legitimate user who signs in repeatedly — a new device, a cleared
    /// cookie jar, an integration test exercising login in a loop — must
    /// never be locked out by their own successful attempts. An attacker,
    /// whose every attempt fails by definition, never calls this and so gets
    /// no refund. If the entry has already expired or was never created
    /// (e.g. the limiter is disabled), this is a harmless no-op.
    pub fn release(&self, ip: IpAddr) {
        let mut map = self.guard();
        let Some(entry) = map.get_mut(&ip) else {
            return;
        };
        entry.0 = entry.0.saturating_sub(1);
        if entry.0 == 0 {
            map.remove(&ip);
        }
    }

    /// Drop entries whose window has already elapsed. Returns how many were
    /// removed.
    ///
    /// This is not required for correctness — an expired window is detected
    /// and reset lazily the next time that IP calls `try_acquire` — but a
    /// caller may still want to invoke it periodically (e.g. from a
    /// background task) to release memory held by one-off callers that never
    /// come back.
    pub fn prune(&self) -> usize {
        let mut map = self.guard();
        let before = map.len();
        map.retain(|_, (_, started)| started.elapsed().as_secs() < self.window_secs);
        before - map.len()
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
            assert!(limiter.try_acquire(ip));
        }
        assert!(!limiter.try_acquire(ip));
    }

    #[test]
    fn window_expiry_resets_the_counter() {
        // A zero-second window is elapsed the instant it is recorded, so even
        // a limit of 1 never actually throttles anything.
        let limiter = RateLimiter::new(1, 0);
        let ip = ipv4(2);
        assert!(limiter.try_acquire(ip));
        assert!(limiter.try_acquire(ip));
    }

    #[test]
    fn distinct_ips_have_independent_buckets() {
        let limiter = RateLimiter::new(1, 60);
        let v4: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 3));
        let v6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 3));

        assert!(limiter.try_acquire(v4));
        assert!(!limiter.try_acquire(v4));

        // A different address (even a different family) gets its own budget.
        assert!(limiter.try_acquire(v6));
        assert!(!limiter.try_acquire(v6));
    }

    #[test]
    fn release_returns_the_reserved_attempt() {
        let limiter = RateLimiter::new(2, 60);
        let ip = ipv4(4);
        for _ in 0..10 {
            assert!(limiter.try_acquire(ip));
            limiter.release(ip);
        }
        // Every reservation was released, so the entry should not linger.
        assert_eq!(limiter.guard().len(), 0);
    }

    #[test]
    fn zero_max_attempts_disables_the_limiter() {
        let limiter = RateLimiter::new(0, 60);
        let ip = ipv4(5);
        for _ in 0..1000 {
            assert!(limiter.try_acquire(ip));
        }
    }

    #[test]
    fn map_is_pruned_at_capacity() {
        let limiter = RateLimiter {
            max_entries: 4,
            ..RateLimiter::new(5, 60)
        };
        for i in 0..50u8 {
            limiter.try_acquire(ipv4(i));
        }
        // Every entry above was recorded with a live (non-expired) window, so
        // `retain` inside `try_acquire` had nothing to prune: the map fills
        // up to `max_entries` and then simply stops admitting new keys,
        // rather than growing to track all 50 sprayed addresses.
        assert_eq!(limiter.guard().len(), 4);
    }

    #[test]
    fn capacity_spray_does_not_reset_an_existing_counter() {
        // Regression: a throttled IP must stay throttled even when the map is
        // subsequently sprayed with fresh keys that push it to capacity.
        let limiter = RateLimiter {
            max_entries: 4,
            ..RateLimiter::new(2, 60)
        };
        let victim = ipv4(200);
        assert!(limiter.try_acquire(victim));
        assert!(limiter.try_acquire(victim));
        assert!(!limiter.try_acquire(victim), "victim should be throttled");

        // Fill the map to (and past) capacity with unrelated IPs.
        for i in 0..10u8 {
            limiter.try_acquire(ipv4(i));
        }

        // The victim must still be throttled — a spray of fresh keys must not
        // hand it (or anyone) a way to reset their own budget, nor must it
        // clear other callers' legitimate state.
        assert!(
            !limiter.try_acquire(victim),
            "victim must remain throttled after an unrelated spray"
        );
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
                    limiter.try_acquire(ip)
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

    #[test]
    fn prune_removes_expired_entries() {
        let limiter = RateLimiter::new(5, 0);
        for i in 0..5u8 {
            limiter.try_acquire(ipv4(i));
        }
        // Window length is 0, so every entry above is already "expired" by
        // the time prune runs.
        assert_eq!(limiter.prune(), 5);
        assert_eq!(limiter.guard().len(), 0);
    }
}
