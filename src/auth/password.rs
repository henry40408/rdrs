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

pub fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);

    HASHER
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AppError::Internal(format!("Password hashing failed: {}", e)))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
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
