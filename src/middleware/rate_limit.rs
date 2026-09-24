//! Fixed-window rate limiting for credential endpoints (OWASP ASVS V2.2.1),
//! keyed by ([`Bucket`], subject). It must run *before* any DB query or Argon2
//! verify, or an attacker chooses how much CPU each guess costs.
//!
//! Password paths charge both a per-IP budget (stops one host) and a per-account
//! one (stops a distributed spray). The account budget is
//! [`ACCOUNT_ATTEMPT_MULTIPLIER`]× wider because anyone can aim it at a victim:
//! a tight one is a lockout denial of service.
//!
//! [`RateLimiter::try_acquire`] checks and increments under one lock; a split
//! `check()` + `record()` would let concurrent requests all pass the check.
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// Default attempts allowed per client IP inside one window.
pub const LOGIN_MAX_ATTEMPTS: u32 = 5;
/// Default fixed-window length, in seconds.
pub const LOGIN_WINDOW_SECS: u64 = 60;

/// How much wider the per-account budget is than the per-IP one. Must not be
/// tight: see the module docs.
pub const ACCOUNT_ATTEMPT_MULTIPLIER: u32 = 4;

/// Number of counter slots. Fixed, so memory is constant and there is no
/// capacity edge an attacker can use to get an unrecorded attempt.
const SLOTS: usize = 16_384;

/// Which budget an attempt is charged against, so abuse of one endpoint cannot
/// lock users out of another (e.g. a refused signup must not spend the login
/// budget).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// Password and passkey *completion* — anything that verifies a credential.
    Login,
    /// First-run `/setup` and invite redemption; both run the strength
    /// estimator and Argon2.
    AccountSetup,
    /// Passkey ceremony *start*: cheap, unauthenticated and enumerable.
    PasskeyProbe,
    /// Change password: stops a hijacked session brute-forcing the current
    /// password or burning CPU with Argon2.
    PasswordChange,
}

/// The outcome of [`RateLimiter::try_acquire`]. Carries `Retry-After` so it is
/// computed under the same lock as the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The attempt was reserved and may proceed.
    Allowed,
    /// The budget is spent; `retry_after_secs` is what remains of the window.
    Throttled { retry_after_secs: u64 },
}

impl Decision {
    /// How long to wait before retrying, or `None` when allowed.
    #[must_use]
    pub fn retry_after_secs(self) -> Option<u64> {
        match self {
            Self::Allowed => None,
            Self::Throttled { retry_after_secs } => Some(retry_after_secs),
        }
    }
}

/// Fixed-window counters of `(attempts, window_start)`, reset lazily on the next
/// attempt after the window elapses.
///
/// Keys hash into slots with a per-process [`RandomState`], so collisions only
/// over-throttle and cannot be aimed at a victim; a colliding
/// [`Window::release`] is a mild under-throttle, not a bypass. Being fixed, not
/// sliding, a client can get `2 × max_attempts` across a boundary — acceptable.
#[derive(Debug)]
struct Window {
    slots: Mutex<Box<[(u32, Instant)]>>,
    hasher: RandomState,
    max_attempts: u32,
    window_secs: u64,
}

impl Window {
    fn new(max_attempts: u32, window_secs: u64) -> Self {
        let slots = vec![(0u32, Instant::now()); SLOTS].into_boxed_slice();
        Self {
            slots: Mutex::new(slots),
            hasher: RandomState::new(),
            max_attempts,
            window_secs,
        }
    }

    /// Lock the slot array, tolerating poison: the counters are advisory, and a
    /// poisoned lock must not lock everyone out of login.
    fn guard(&self) -> MutexGuard<'_, Box<[(u32, Instant)]>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn slot_index<K: Hash>(&self, key: K) -> usize {
        let hash = self.hasher.hash_one(key);
        usize::try_from(hash % SLOTS as u64).expect("modulo SLOTS fits usize")
    }

    fn try_acquire<K: Hash>(&self, key: K) -> Decision {
        // 0 means disabled; the comparison below would otherwise block everything.
        if self.max_attempts == 0 {
            return Decision::Allowed;
        }

        let idx = self.slot_index(key);

        // Check-and-increment under one lock, or concurrent requests race past.
        let mut slots = self.guard();
        let slot = &mut slots[idx];

        let elapsed = slot.1.elapsed().as_secs();
        if elapsed >= self.window_secs {
            *slot = (1, Instant::now());
            return Decision::Allowed;
        }

        if slot.0 >= self.max_attempts {
            // Floored at 1: `Retry-After: 0` invites a retry certain to fail.
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

/// Rate limiter over two independent subject spaces, client IP and account
/// name, each with its own slot array so an address never collides with an
/// account.
#[derive(Debug)]
pub struct RateLimiter {
    per_ip: Window,
    per_account: Window,
}

impl RateLimiter {
    /// Allow `max_attempts` per `window_secs` per `(bucket, ip)`, and
    /// [`ACCOUNT_ATTEMPT_MULTIPLIER`]× that per `(bucket, account)`.
    /// `max_attempts == 0` disables both.
    pub fn new(max_attempts: u32, window_secs: u64) -> Self {
        Self {
            per_ip: Window::new(max_attempts, window_secs),
            // Saturate: a wrapped (tiny) account limit would lock everyone out.
            per_account: Window::new(
                max_attempts.saturating_mul(ACCOUNT_ATTEMPT_MULTIPLIER),
                window_secs,
            ),
        }
    }

    /// Reserve an attempt for `(bucket, ip)`. Call *before* any DB query or
    /// password verify; on success call [`RateLimiter::release`].
    pub fn try_acquire(&self, bucket: Bucket, ip: IpAddr) -> Decision {
        self.per_ip.try_acquire((bucket, ip))
    }

    /// Refund the attempt after a *successful* credential check, so users are
    /// never locked out by their own logins. Harmless on an expired slot.
    pub fn release(&self, bucket: Bucket, ip: IpAddr) {
        self.per_ip.release((bucket, ip));
    }

    /// Reserve an attempt against the named account, alongside
    /// [`RateLimiter::try_acquire`] and before any lookup. Pass the username
    /// exactly as it will be looked up (case-sensitive); it is only hashed,
    /// never stored.
    pub fn try_acquire_account(&self, bucket: Bucket, username: &str) -> Decision {
        self.per_account.try_acquire((bucket, username))
    }

    /// Refund [`RateLimiter::try_acquire_account`] after a successful check.
    pub fn release_account(&self, bucket: Bucket, username: &str) {
        self.per_account.release((bucket, username));
    }

    /// Test support: whether every [`Bucket`] of `ip`, and of `account`, has a
    /// slot to itself. Keys share a slot with probability 1/`SLOTS` by design
    /// (only an over-throttle), which would flake a test asserting that one
    /// bucket's exhaustion leaves another untouched.
    #[doc(hidden)]
    pub fn separates_buckets(&self, ip: IpAddr, account: &str) -> bool {
        const ALL: [Bucket; 4] = [
            Bucket::Login,
            Bucket::AccountSetup,
            Bucket::PasskeyProbe,
            Bucket::PasswordChange,
        ];
        let distinct = |slots: [usize; 4]| {
            let mut slots = slots;
            slots.sort_unstable();
            slots.windows(2).all(|pair| pair[0] != pair[1])
        };
        distinct(ALL.map(|bucket| self.per_ip.slot_index((bucket, ip))))
            && distinct(ALL.map(|bucket| self.per_account.slot_index((bucket, account))))
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

    fn allowed(decision: Decision) -> bool {
        decision == Decision::Allowed
    }

    fn distinct_slots<K: Hash + Copy>(window: &Window, keys: &[K]) -> bool {
        let mut slots: Vec<usize> = keys.iter().map(|k| window.slot_index(*k)).collect();
        slots.sort_unstable();
        slots.dedup();
        slots.len() == keys.len()
    }

    /// A limiter whose keys under test land in distinct slots. Slots come from a
    /// per-process random hash, so any two keys collide with probability
    /// 1/`SLOTS` — by design only an over-throttle, but enough to flake a test
    /// asserting that a second key is still free. Each `new` draws fresh hash
    /// keys, so this almost always returns the first limiter.
    fn separated(
        max_attempts: u32,
        window_secs: u64,
        ips: &[(Bucket, IpAddr)],
        accounts: &[(Bucket, &str)],
    ) -> RateLimiter {
        loop {
            let limiter = RateLimiter::new(max_attempts, window_secs);
            if distinct_slots(&limiter.per_ip, ips)
                && distinct_slots(&limiter.per_account, accounts)
            {
                return limiter;
            }
        }
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
        // A zero-second window elapses instantly (`Config` rejects it at startup).
        let limiter = RateLimiter::new(1, 0);
        let ip = ipv4(2);
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn distinct_ips_have_independent_buckets() {
        let v4: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 3));
        let v6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 3));
        let limiter = separated(1, 60, &[(Bucket::Login, v4), (Bucket::Login, v6)], &[]);

        assert!(allowed(limiter.try_acquire(Bucket::Login, v4)));
        assert!(!allowed(limiter.try_acquire(Bucket::Login, v4)));

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
        // A throttled victim stays throttled through a wide spray, and a fresh
        // IP afterwards is still throttled.
        let limiter = RateLimiter::new(2, 60);

        let victim = ipv4(200);
        assert!(allowed(limiter.try_acquire(Bucket::Login, victim)));
        assert!(allowed(limiter.try_acquire(Bucket::Login, victim)));
        assert!(
            !allowed(limiter.try_acquire(Bucket::Login, victim)),
            "victim should be throttled"
        );

        // 50,000 addresses, far more than `SLOTS`.
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

        // It may share a slot with a sprayed address, so only assert that it is
        // throttled at all.
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

        // A range: the exact value depends on elapsed test time.
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
        // Just before a 1s window elapses, `1 - 1 = 0` without the floor.
        let limiter = RateLimiter::new(1, 1);
        let ip = ipv4(11);
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));

        if let Some(secs) = limiter.try_acquire(Bucket::Login, ip).retry_after_secs() {
            assert!(secs >= 1, "retry-after must never be zero, got {secs}");
        }
        // If the window already elapsed, being allowed is also correct.
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
        let ip = ipv4(13);
        let limiter = separated(
            1,
            60,
            &[(Bucket::PasswordChange, ip), (Bucket::Login, ip)],
            &[],
        );

        assert!(allowed(limiter.try_acquire(Bucket::PasswordChange, ip)));
        assert!(!allowed(limiter.try_acquire(Bucket::PasswordChange, ip)));

        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn separate_buckets_do_not_share_budget() {
        // A refused registration must never spend the login budget.
        let ip = ipv4(6);
        let limiter = separated(
            5,
            60,
            &[(Bucket::AccountSetup, ip), (Bucket::Login, ip)],
            &[],
        );

        for _ in 0..5 {
            assert!(allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));
        }
        assert!(!allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));

        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
    }

    #[test]
    fn release_only_refunds_its_own_bucket() {
        let ip = ipv4(7);
        let limiter = separated(
            1,
            60,
            &[(Bucket::Login, ip), (Bucket::AccountSetup, ip)],
            &[],
        );

        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));
        limiter.release(Bucket::Login, ip);
        assert!(allowed(limiter.try_acquire(Bucket::Login, ip)));

        // The Login release did not refund AccountSetup.
        assert!(allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));
        assert!(!allowed(limiter.try_acquire(Bucket::AccountSetup, ip)));
    }

    #[test]
    fn a_spray_from_many_ips_is_capped_by_the_account_budget() {
        // Every request is from a fresh address; only the account budget binds.
        // 200 addresses is few enough that collisions cannot reach the per-IP 5.
        let limiter = RateLimiter::new(5, 60);
        let account = "victim";

        let mut admitted = 0;
        for i in 0..200u32 {
            let ip = IpAddr::V4(Ipv4Addr::from(i.to_be_bytes()));
            assert!(
                allowed(limiter.try_acquire(Bucket::Login, ip)),
                "a fresh IP must never be throttled on its first attempt"
            );
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
        // Otherwise the account budget, which anyone can aim, is the lockout.
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
        let ip = ipv4(21);
        let limiter = separated(
            1,
            60,
            &[],
            &[(Bucket::Login, "alice"), (Bucket::Login, "bob")],
        );

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
        // Must match `user::find_by_username`, or varying case would bypass it.
        let limiter = separated(
            1,
            60,
            &[],
            &[(Bucket::Login, "admin"), (Bucket::Login, "Admin")],
        );

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
        let limiter = separated(
            1,
            60,
            &[],
            &[(Bucket::Login, "admin"), (Bucket::PasswordChange, "admin")],
        );

        for _ in 0..ACCOUNT_ATTEMPT_MULTIPLIER {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "admin")));
        }
        assert!(!allowed(
            limiter.try_acquire_account(Bucket::Login, "admin")
        ));

        // A login spray must not deny that user change-password.
        assert!(allowed(
            limiter.try_acquire_account(Bucket::PasswordChange, "admin")
        ));
    }

    /// What [`separated`] steers around: keys sharing a slot share a budget.
    /// That may over-throttle the innocent key, never under-throttle either.
    #[test]
    fn colliding_keys_only_over_throttle() {
        let limiter = RateLimiter::new(1, 60);
        let alice = limiter.per_account.slot_index((Bucket::Login, "alice"));
        // 1M names miss a 1/16384 slot with probability ~e^-61.
        let twin = (0u32..1_000_000)
            .map(|i| format!("user{i}"))
            .find(|name| {
                limiter
                    .per_account
                    .slot_index((Bucket::Login, name.as_str()))
                    == alice
            })
            .expect("some name shares alice's slot");

        for _ in 0..ACCOUNT_ATTEMPT_MULTIPLIER {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "alice")));
        }
        assert!(!allowed(
            limiter.try_acquire_account(Bucket::Login, "alice")
        ));
        assert!(
            !allowed(limiter.try_acquire_account(Bucket::Login, &twin)),
            "a colliding key is throttled along with alice"
        );
    }

    #[test]
    fn zero_max_attempts_disables_the_account_dimension_too() {
        // Not "a limit of 0 per account", which would refuse every login.
        let limiter = RateLimiter::new(0, 60);
        for _ in 0..1000 {
            assert!(allowed(limiter.try_acquire_account(Bucket::Login, "admin")));
        }
    }

    #[test]
    fn concurrent_attempts_cannot_exceed_the_limit() {
        // 64 threads race one IP with a limit of 5; exactly 5 must pass.
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
