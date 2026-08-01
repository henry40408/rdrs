use std::sync::LazyLock;

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::error::{AppError, AppResult};

/// Argon2 hasher used to derive *new* password hashes.
///
/// Production uses [`Argon2::default`] (OWASP-recommended, deliberately
/// memory-hard, ~hundreds of ms per hash in a debug build). When the
/// `RDRS_FAST_HASH` environment variable is set — only ever in test/CI runs —
/// minimal cost parameters are used instead, cutting each hash to microseconds.
/// This is safe: production never sets the flag, and [`verify_password`] reads
/// the cost parameters out of each stored hash, so hashes produced under either
/// setting verify interchangeably.
static HASHER: LazyLock<Argon2<'static>> = LazyLock::new(|| {
    if std::env::var_os("RDRS_FAST_HASH").is_some() {
        let params = Params::new(
            Params::MIN_M_COST,
            Params::MIN_T_COST,
            Params::MIN_P_COST,
            None,
        )
        .expect("minimal argon2 params are valid");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
    } else {
        Argon2::default()
    }
});

/// A hash of a value nothing can supply, used to give the "no such user"
/// login path the same cost as a real password check.
///
/// Produced by [`hash_password`], so it carries whatever cost parameters this
/// process runs with — including the `RDRS_FAST_HASH` test setting, which
/// keeps the equalising verify exactly as cheap as the real one it mirrors.
///
/// Both the input and the salt are freshly random per process: no string a
/// caller could send verifies against it, and the digest is not a constant an
/// attacker could fingerprint across deployments.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let filler = SaltString::generate(&mut OsRng);
    hash_password(filler.as_str()).expect("hashing with valid params cannot fail")
});

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);

    HASHER
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AppError::Internal(format!("Password hashing failed: {e}")))
}

/// Spend one password verification against [`DUMMY_HASH`], discarding the
/// (always negative) result.
///
/// Call this on the branch where the *username* did not resolve to an account.
/// Without it, login answers "no such user" after a single indexed `SELECT`
/// but answers "wrong password" only after a deliberately slow Argon2 verify —
/// a delta of tens of milliseconds that is trivially measurable over the
/// network and turns the deliberately generic `Invalid credentials` message
/// into an account-existence oracle. This is the "quick exit" reject pattern
/// OWASP's Authentication Cheat Sheet names under *Authentication and Error
/// Messages*; running the hash on both paths is its remedy.
///
/// Returns nothing on purpose: the work *is* the return value, and a `bool`
/// would invite a caller to branch on a result that is false by construction.
pub fn verify_dummy_password(password: &str) {
    // `black_box` stops the optimiser from observing that the result is unused
    // and eliding the hash — which would silently restore the timing gap this
    // function exists to close.
    std::hint::black_box(verify_password(password, &DUMMY_HASH));
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed_hash) = PasswordHash::new(hash) else {
        return false;
    };

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify() {
        let password = "secure_password_123";
        let hash = hash_password(password).unwrap();

        assert!(verify_password(password, &hash));
        assert!(!verify_password("wrong_password", &hash));
    }

    #[test]
    fn test_different_hashes() {
        let password = "same_password";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();

        assert_ne!(hash1, hash2);
        assert!(verify_password(password, &hash1));
        assert!(verify_password(password, &hash2));
    }

    #[test]
    fn test_invalid_hash() {
        assert!(!verify_password("password", "invalid_hash"));
    }

    #[test]
    fn dummy_verify_costs_the_same_as_a_real_one() {
        // The equalising verify is only worth anything if it does the same
        // work as the check it stands in for. Compare the cost parameters
        // encoded in the dummy hash against those of a freshly minted one:
        // if they ever diverge (e.g. someone builds the dummy with
        // `Argon2::default()` while `HASHER` is running fast-hash params),
        // the "no such user" path becomes distinguishable again by timing.
        let real = hash_password("whatever").unwrap();
        let real = PasswordHash::new(&real).unwrap();
        let dummy = PasswordHash::new(&DUMMY_HASH).unwrap();

        assert_eq!(dummy.algorithm, real.algorithm);
        assert_eq!(dummy.params, real.params);
        // A per-process salt, not a constant an attacker could fingerprint.
        assert_ne!(dummy.salt, real.salt);
    }

    #[test]
    fn dummy_verify_accepts_any_input_and_returns_nothing() {
        // Callers pass attacker-controlled bytes straight in, including the
        // degenerate ones. Nothing here may panic, and there is no result to
        // branch on — the guarantee is that it only ever burns time.
        verify_dummy_password("");
        verify_dummy_password("password123");
        verify_dummy_password(&"x".repeat(4096));
    }

    #[test]
    fn test_verify_is_independent_of_configured_params() {
        // Guards the RDRS_FAST_HASH optimisation: verification reads the cost
        // parameters from the stored hash, so a hash produced with strong
        // (default) params and one produced with minimal params must both
        // verify. This is what makes weakening hash params in test/CI safe.
        let strong = Argon2::default()
            .hash_password(b"pw", &SaltString::generate(&mut OsRng))
            .unwrap()
            .to_string();
        let weak_params = Params::new(
            Params::MIN_M_COST,
            Params::MIN_T_COST,
            Params::MIN_P_COST,
            None,
        )
        .unwrap();
        let weak = Argon2::new(Algorithm::Argon2id, Version::V0x13, weak_params)
            .hash_password(b"pw", &SaltString::generate(&mut OsRng))
            .unwrap()
            .to_string();

        assert!(verify_password("pw", &strong));
        assert!(verify_password("pw", &weak));
        assert!(!verify_password("nope", &strong));
        assert!(!verify_password("nope", &weak));
    }
}
