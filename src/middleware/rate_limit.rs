//! Rate limiting for credential-accepting endpoints, in two dimensions.
//!
//! OWASP's Authentication Cheat Sheet and ASVS V2.2.1 both call for throttling
//! repeated authentication attempts. The limiter is deliberately simple —
//! in-memory fixed windows keyed by ([`Bucket`], subject) — because its only job
//! is to make credential stuffing expensive *before* the request reaches a
//! database query or an Argon2 `verify_password`. Argon2 is slow on purpose, and
//! a limiter running after hashing would let an attacker choose how much CPU the
//! server spends per guess.
//!
//! # Why two dimensions
//!
//! Throttling by client IP alone stops one host grinding through a dictionary
//! and nothing else: a spray from a thousand addresses gets the full per-IP
//! budget from each. So the password paths charge both a per-IP and a
//! per-account budget.
//!
//! The account budget is deliberately the wider of the two
//! ([`ACCOUNT_ATTEMPT_MULTIPLIER`]×) because it is the one an attacker can aim
//! at someone else: a tight per-account limit is a denial-of-service primitive,
//! letting anyone who knows a username keep its owner logged out. Both windows
//! are the same length, so a legitimate user caught behind an attack recovers as
//! soon as it stops.
//!
//! # Why not `check()` + `record()`
//!
//! Splitting the decision in two races: concurrent requests from one IP can both
//! `check` before either `record`s, both observe the pre-attack count and both
//! proceed — enforced in wall-clock time but not against concurrency, which is
//! exactly what a parallel stuffing tool exploits.
//! [`RateLimiter::try_acquire`] holds the lock across the whole
//! check-and-increment instead.
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// Default attempts allowed per client IP inside one window.
pub const LOGIN_MAX_ATTEMPTS: u32 = 5;
/// Default fixed-window length, in seconds.
pub const LOGIN_WINDOW_SECS: u64 = 60;

/// How much wider the per-account budget is than the per-IP one.
///
/// The per-account window exists to cap a spray arriving from many addresses at
/// once, not to police one person's typing. Four times the per-IP budget is far
/// beyond anything a user reaches by hand — a mistyped password is refunded on
/// the next success anyway — while still cutting a distributed attack to a fixed
/// rate per account. See the module docs on why this must not be tight.
pub const ACCOUNT_ATTEMPT_MULTIPLIER: u32 = 4;

/// Number of counter slots. Fixed at construction, so memory is constant however
/// wide an attack fans out and there is no capacity edge left to bypass: a
/// previous capped-`HashMap` design admitted an unrecorded, and therefore
/// unthrottled, request whenever a new IP arrived at capacity.
const SLOTS: usize = 16_384;

/// Which budget an attempt is charged against. Separate buckets stop abuse of
/// one endpoint from locking users out of another — in particular a registration
/// refused by configuration must never consume the password-login budget, which
/// would turn a misconfigured signup page into a denial of authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// Password and passkey *completion* — anything that verifies a credential.
    Login,
    /// The two anonymous paths that can create a usable credential: the
    /// first-run `/setup` form and redeeming an account invite. Both run the
    /// strength estimator and Argon2, so both are throttled before doing
    /// either.
    AccountSetup,
    /// Passkey ceremony *start*: cheap, unauthenticated, and enumerable, so it
    /// gets its own budget rather than spending the login one.
    PasskeyProbe,
    /// "Change password", which verifies the *current* password. The caller
    /// already holds a session, so this is no way in from outside — but an
    /// unthrottled Argon2 verify lets a hijacked session brute-force the
    /// original password and lets any logged-in user spend server CPU at will.
    PasswordChange,
}

/// The outcome of [`RateLimiter::try_acquire`].
///
/// Deliberately not a `bool`: the caller needs the remaining window length for
/// `Retry-After`, and computing it in a second call would take the lock twice
/// and race with a window that resets in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The attempt was reserved and may proceed.
    Allowed,
    /// The budget is spent. `retry_after_secs` is how long the current fixed
    /// window still has to run.
    Throttled { retry_after_secs: u64 },
}

impl Decision {
    /// How long to wait before retrying, or `None` when the attempt was
    /// allowed. Shaped as an `Option` so a caller can write
    /// `if let Some(secs) = ...try_acquire(...).retry_after_secs()`, which
    /// keeps the reject path and the number it needs in one place.
    #[must_use]
    pub fn retry_after_secs(self) -> Option<u64> {
        match self {
            Self::Allowed => None,
            Self::Throttled { retry_after_secs } => Some(retry_after_secs),
        }
    }
}

/// One fixed-window counter array, addressed by an arbitrary hashable key.
///
/// Each slot records `(attempts_in_window, window_started_at)`. A window resets
/// lazily on the next attempt after it elapses, so the limiter needs no timer.
///
/// # Collisions
///
/// The slot array is addressed by a process-random keyed hash, so two distinct
/// keys may share a slot. The effect is bounded and asymmetric: colliding keys
/// can only *over*-throttle each other, and because [`RandomState`] is seeded
/// per process an attacker cannot provoke a collision with a chosen victim. A
/// [`Window::release`] from one colliding key decrements the other's count too —
/// a mild under-throttle, not a bypass.
///
/// # Fixed, not sliding, window
///
/// A client can spend its full budget at the end of one window and again at the
/// start of the next: up to `2 × max_attempts` across the boundary. Acceptable
/// for an anti-automation control whose job is to make bulk guessing expensive,
/// and it keeps the limiter allocation-free after construction.
#[derive(Debug)]
struct Window {
    slots: Mutex<Box<[(u32, Instant)]>>,
    hasher: RandomState,
    max_attempts: u32,
    window_secs: u64,
}

impl Window {
    fn new(max_attempts: u32, window_secs: u64) -> Self {
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
    /// A panic while holding the lock must not take the login path down with it:
    /// the counters are advisory, so recovering the inner slice beats poisoning
    /// every subsequent attempt and locking every client out of authentication
    /// because of an unrelated bug.
    fn guard(&self) -> MutexGuard<'_, Box<[(u32, Instant)]>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hash a key down to a slot index. Process-random (see the collision
    /// note on the struct docs), so an attacker cannot compute in advance
    /// which other key shares their slot.
    fn slot_index<K: Hash>(&self, key: K) -> usize {
        let hash = self.hasher.hash_one(key);
        // SLOTS fits comfortably in a u64, and the modulo result is always
        // < SLOTS, so this always fits back into a usize.
        usize::try_from(hash % SLOTS as u64).expect("modulo SLOTS fits usize")
    }

    fn try_acquire<K: Hash>(&self, key: K) -> Decision {
        // Disabled: every request proceeds. Without this early return the
        // generic `slot.0 >= self.max_attempts` comparison below would block
        // every single request once `max_attempts` is 0, the opposite of
        // "disabled".
        if self.max_attempts == 0 {
            return Decision::Allowed;
        }

        let idx = self.slot_index(key);

        // The entire check-and-increment happens under one lock acquisition.
        // Splitting this into a separate "peek" and "increment" would reopen
        // the race described in the module docs: two concurrent requests
        // could both see room in the window and both be admitted.
        let mut slots = self.guard();
        let slot = &mut slots[idx];

        let elapsed = slot.1.elapsed().as_secs();
        if elapsed >= self.window_secs {
            // The window has elapsed: start a fresh one with this attempt as
            // the first count in it, regardless of what the stale count was.
            *slot = (1, Instant::now());
            return Decision::Allowed;
        }

        if slot.0 >= self.max_attempts {
            // What is left of the current fixed window. Floored at 1: a
            // `Retry-After: 0` invites an immediate retry that is certain to
            // be rejected again, which is worse than no header at all.
            return Decision::Throttled {
                retry_after_secs: self.window_secs.saturating_sub(elapsed).max(1),
            };
        }

        slot.0 += 1;
        Decision::Allowed
    }

    fn release<K: Hash>(&self, key: K) {
        let idx = self.slot_index(key);
        let mut slots = self.guard();
        slots[idx].0 = slots[idx].0.saturating_sub(1);
    }
}

/// A fixed-window rate limiter over two independent subject spaces: the client
/// IP a request arrived from, and the account name it names.
///
/// Each gets its own slot array rather than sharing one keyed by an either-or
/// enum, so an address can never collide with an account and throttle it by
/// accident. The spaces are populated by different parties, and keeping them
/// apart means a busy proxy address cannot degrade anyone's ability to log in.
#[derive(Debug)]
pub struct RateLimiter {
    per_ip: Window,
    per_account: Window,
}

impl RateLimiter {
    /// Build a limiter allowing `max_attempts` per `window_secs`-second fixed
    /// window per `(bucket, ip)`, and [`ACCOUNT_ATTEMPT_MULTIPLIER`] times that
    /// per `(bucket, account)` over the same window. `max_attempts == 0` disables
    /// both dimensions — the documented escape hatch for deployments behind an
    /// already-authenticating reverse proxy.
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            per_ip: Window::new(max_attempts, window_secs),
            // `saturating_mul` so an operator who configures an absurd budget
            // gets a saturated one rather than a wrapped (and tiny) account
            // limit — the failure mode there would be locking everyone out.
            per_account: Window::new(
                max_attempts.saturating_mul(ACCOUNT_ATTEMPT_MULTIPLIER),
                window_secs,
            ),
        }
    }

    /// Reserve an attempt for `(bucket, ip)`, returning whether it may proceed.
    ///
    /// Call this *before* any database query or password verification; that
    /// ordering is the entire point of the limiter. On [`Decision::Allowed`] the
    /// caller has spent one of this window's attempts — if the credential check
    /// goes on to succeed, call [`RateLimiter::release`] to hand it back.
    pub fn try_acquire(&self, bucket: Bucket, ip: IpAddr) -> Decision {
        self.per_ip.try_acquire((bucket, ip))
    }

    /// Hand back the attempt reserved by [`RateLimiter::try_acquire`], after a
    /// *successful* credential check for `(bucket, ip)`.
    ///
    /// A legitimate user who signs in repeatedly — a new device, a cleared cookie
    /// jar, a test looping over login — must never be locked out by their own
    /// successes. An attacker, whose every attempt fails by definition, never
    /// calls this. An expired or never-incremented slot is a harmless no-op:
    /// `saturating_sub` floors at zero rather than wrapping.
    pub fn release(&self, bucket: Bucket, ip: IpAddr) {
        self.per_ip.release((bucket, ip));
    }

    /// Reserve an attempt against the *account* named by the request, independent
    /// of where it came from.
    ///
    /// This is the dimension that survives a distributed attack: an attacker
    /// rotating addresses gets a fresh per-IP budget each time but keeps drawing
    /// on the same account budget. Charge it alongside
    /// [`RateLimiter::try_acquire`], before any lookup, and pass the username
    /// exactly as it will be looked up — the key is case-sensitive.
    ///
    /// The account name is only hashed into a slot index, never retained, so this
    /// adds no store of who has been trying to log in.
    pub fn try_acquire_account(&self, bucket: Bucket, username: &str) -> Decision {
        self.per_account.try_acquire((bucket, username))
    }

    /// Hand back the attempt reserved by [`RateLimiter::try_acquire_account`]
    /// after a successful credential check, for the same reason
    /// [`RateLimiter::release`] exists: a user who really can log in must
    /// never spend their own account's budget doing it.
    pub fn release_account(&self, bucket: Bucket, username: &str) {
        self.per_account.release((bucket, username));
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

    /// Most of these tests only care whether the attempt got through, not how
    /// long the caller was told to wait; the `Retry-After` value itself is
    /// asserted separately in `throttling_reports_the_remaining_window`.
    fn allowed(decision: Decision) -> bool {
        decision == Decision::Allowed
    }

    #[test]
    fn allows_up_to_max_then_blocks() {
        let limiter = RateLimiter::new(5, 60);
        let ip = ipv4(1);
        for _ in 0..5 {
            assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
        }
        assert!(!allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn window_expiry_resets_the_counter() {
        // A zero-second window is elapsed the instant it is recorded, so even a
        // limit of 1 never throttles. `Config` rejects a zero
        // `RDRS_LOGIN_RATE_LIMIT_WINDOW_SECS` at startup, so this shape is only
        // reachable by driving the limiter directly.
        let limiter = RateLimiter::new(1, 0);
        let ip = ipv4(2);
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn distinct_ips_have_independent_buckets() {
        let limiter = RateLimiter::new(1, 60);
        let v4: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 3));
        let v6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 3));

        assert!(allowed(limiter.try_acquire(Bucket::Login, v4)));
        assert!(!allowed(limiter.try_acquire(Bucket::Login, v4)));

        // A different address (even a different family) gets its own budget.
        assert!(allowed(limiter.try_acquire(Bucket::Login, v6)));
        assert!(!allowed(limiter.try_acquire(Bucket::Login, v6)));
    }

    #[test]
    fn release_returns_the_reserved_attempt() {
        let limiter = RateLimiter::new(2, 60);
        let ip = ipv4(4);
        for _ in 0..10 {
            assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
            limiter.release(Bucket::Login, ip);
        }
        // Every reservation was released, so ten cycles through a budget of 2
        // never exhaust it — a further attempt must still be admitted.
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn zero_max_attempts_disables_the_limiter() {
        let limiter = RateLimiter::new(0, 60);
        let ip = ipv4(5);
        for _ in 0..1000 {
            assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
        }
    }

    #[test]
    fn spray_does_not_unthrottle_anyone() {
        // Regression for the old capped-HashMap design: at capacity a brand new
        // IP was admitted *without being recorded*, leaving it unthrottled
        // forever. The fixed-size slot array has no such edge, but this proves it
        // end to end — a throttled victim stays throttled through a wide spray,
        // and a fresh IP arriving after it still gets throttled.
        let limiter = RateLimiter::new(2, 60);

        let victim = ipv4(200);
        assert!(allowed(limiter.try_acquire(Bucket::Login, victim)));
        assert!(allowed(limiter.try_acquire(Bucket::Login, victim)));
        assert!(
            !allowed(limiter.try_acquire(Bucket::Login, victim)),
            "victim should be throttled"
        );

        // Spray 50,000 distinct IPv4 and IPv6 addresses — far more than
        // `SLOTS` — so slot collisions are guaranteed.
        for i in 0..25_000u32 {
            let v4 = IpAddr::V4(std::net::Ipv4Addr::from(i.to_be_bytes()));
            let v6 = IpAddr::V6(std::net::Ipv6Addr::from(u128::from(i)));
            allowed(limiter.try_acquire(Bucket::Login, v4));
            allowed(limiter.try_acquire(Bucket::Login, v6));
        }

        assert!(
            !allowed(limiter.try_acquire(Bucket::Login, victim)),
            "victim must remain throttled after an unrelated spray"
        );

        // A fresh IP that never appeared in the spray must still be throttled;
        // the old failure mode was this address getting unlimited attempts
        // forever. It may share a slot with a sprayed address (50,000 keys into
        // 16,384 slots guarantees collisions), so it can be throttled sooner than
        // its own budget — all this asserts is that it is throttled at all.
        let fresh = ipv4(201);
        let throttled = (0..5).any(|_| !allowed(limiter.try_acquire(Bucket::Login, fresh)));
        assert!(
            throttled,
            "a fresh IP arriving after a wide spray must still be throttled eventually, \
             not admitted forever the way the old capped-HashMap design admitted it"
        );
    }

    #[test]
    fn throttling_reports_the_remaining_window() {
        let limiter = RateLimiter::new(1, 60);
        let ip = ipv4(10);

        assert_eq!(limiter.try_acquire(Bucket::Login, ip), Decision::Allowed);

        // The window opened moments ago, so essentially all 60s remain. The
        // exact value depends on how long this test took to reach here, hence
        // a range rather than an equality.
        let secs = limiter
            .try_acquire(Bucket::Login, ip)
            .retry_after_secs()
            .expect("a throttled attempt must carry a retry-after");
        assert!(
            (1..=60).contains(&secs),
            "retry-after must be within the window, got {secs}"
        );
    }

    #[test]
    fn retry_after_is_never_zero() {
        // A 1-second window observed just before it elapses would otherwise
        // compute `1 - 1 = 0`, telling the client to retry immediately into a
        // rejection. The floor keeps the advice honest.
        let limiter = RateLimiter::new(1, 1);
        let ip = ipv4(11);
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));

        if let Some(secs) = limiter.try_acquire(Bucket::Login, ip).retry_after_secs() {
            assert!(secs >= 1, "retry-after must never be zero, got {secs}");
        }
        // If the window already elapsed the attempt is allowed instead, which
        // is equally correct — there is nothing to assert in that case.
    }

    #[test]
    fn allowed_attempts_carry_no_retry_after() {
        let limiter = RateLimiter::new(5, 60);
        assert_eq!(
            limiter
                .try_acquire(Bucket::Login, ipv4(12))
                .retry_after_secs(),
            None
        );
    }

    #[test]
    fn password_change_has_its_own_budget() {
        // Exhausting the change-password budget must not stop the same client
        // from logging in, and vice versa — the whole point of a separate
        // bucket.
        let limiter = RateLimiter::new(1, 60);
        let ip = ipv4(13);

        assert!(allowed(limiter.try_acquire(Bucket::PasswordChange, ip)));
        assert!(!allowed(limiter.try_acquire(Bucket::PasswordChange, ip)));

        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn separate_buckets_do_not_share_budget() {
        // The critical regression: a registration refused by configuration
        // must never spend the login budget for the same IP.
        let limiter = RateLimiter::new(5, 60);
        let ip = ipv4(6);

        for _ in 0..5 {
            assert!(allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));
        }
        assert!(!allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));

        // Login for the same IP is untouched.
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn release_only_refunds_its_own_bucket() {
        let limiter = RateLimiter::new(1, 60);
        let ip = ipv4(7);

        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
        limiter.release(Bucket::Login, ip);
        // Login is refunded and admits another attempt...
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));

        // ...but Register for the same IP was never touched, so it is
        // exhausted by its own single attempt, independent of the Login
        // release above.
        assert!(allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));
        assert!(!allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));
    }

    #[test]
    fn a_spray_from_many_ips_is_capped_by_the_account_budget() {
        // The reason the account dimension exists. Every request comes from a
        // fresh address, so the per-IP budget is never even dented; only the
        // per-account counter can stop this.
        let limiter = RateLimiter::new(5, 60);
        let account = "victim";

        // Ten times the account budget, but few enough addresses that a slot
        // collision cannot plausibly stack up to the per-IP limit of 5 (see
        // the collision note on `Window`).
        let mut admitted = 0;
        for i in 0..200u32 {
            let ip = IpAddr::V4(Ipv4Addr::from(i.to_be_bytes()));
            // Each address is new, so its own budget always has room...
            assert!(
                allowed(limiter.try_acquire(Bucket::Login, ip)),
                "a fresh IP must never be throttled on its first attempt"
            );
            // ...and the account counter is the only thing keeping score.
            if allowed(limiter.try_acquire_account(Bucket::Login, account)) {
                admitted += 1;
            }
        }

        assert_eq!(
            admitted,
            5 * ACCOUNT_ATTEMPT_MULTIPLIER,
            "a distributed spray must be capped by the per-account budget"
        );
    }

    #[test]
    fn the_account_budget_is_wider_than_the_per_ip_one() {
        // A user typing badly must hit their own address's limit long before
        // they reach the account limit — otherwise the account dimension,
        // which anyone can aim at anyone, becomes the effective lockout.
        let limiter = RateLimiter::new(5, 60);
        let ip = ipv4(20);
        let account = "admin";

        for _ in 0..5 {
            assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, account)));
        }
        assert!(
            !allowed(limiter.try_acquire(Bucket::Login, ip)),
            "the per-IP budget must run out first"
        );
        assert!(
            allowed(limiter.try_acquire_account(Bucket::Login, account)),
            "the account budget must still have room when one address is done"
        );
    }

    #[test]
    fn accounts_and_addresses_do_not_share_a_budget() {
        // Exhausting one account must leave every address — and every other
        // account — untouched.
        let limiter = RateLimiter::new(1, 60);
        let ip = ipv4(21);

        for _ in 0..ACCOUNT_ATTEMPT_MULTIPLIER {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "alice")));
        }
        assert!(!allowed(
            limiter.try_acquire_account(Bucket::Login, "alice")
        ));

        assert!(allowed(limiter.try_acquire_account(Bucket::Login, "bob")));
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn account_keys_are_case_sensitive_like_the_lookup() {
        // The key must match how `user::find_by_username` resolves an
        // account, or the budget would be charged to a name that cannot log
        // in — a free bypass for anyone who varies the casing.
        let limiter = RateLimiter::new(1, 60);

        for _ in 0..ACCOUNT_ATTEMPT_MULTIPLIER {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "admin")));
        }
        assert!(!allowed(
            limiter.try_acquire_account(Bucket::Login, "admin")
        ));
        assert!(allowed(limiter.try_acquire_account(Bucket::Login, "Admin")));
    }

    #[test]
    fn release_account_returns_the_reserved_attempt() {
        let limiter = RateLimiter::new(1, 60);
        for _ in 0..10 {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "admin")));
            limiter.release_account(Bucket::Login, "admin");
        }
        assert!(allowed(limiter.try_acquire_account(Bucket::Login, "admin")));
    }

    #[test]
    fn account_buckets_are_independent_of_each_other() {
        let limiter = RateLimiter::new(1, 60);

        for _ in 0..ACCOUNT_ATTEMPT_MULTIPLIER {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "admin")));
        }
        assert!(!allowed(
            limiter.try_acquire_account(Bucket::Login, "admin")
        ));

        // A different bucket for the same account keeps its own budget, so a
        // login spray cannot deny that user their change-password path.
        assert!(allowed(
            limiter.try_acquire_account(Bucket::PasswordChange, "admin")
        ));
    }

    #[test]
    fn zero_max_attempts_disables_the_account_dimension_too() {
        // `0` means "no rate limiting" everywhere, not "no rate limiting by
        // IP but a limit of 0 per account", which would refuse every login.
        let limiter = RateLimiter::new(0, 60);
        for _ in 0..1000 {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "admin")));
        }
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
                    allowed(limiter.try_acquire(Bucket::Login, ip))
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
