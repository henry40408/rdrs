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

/// Shortest password rdrs will accept for a *new* credential.
///
/// NIST SP800-63B, which OWASP's Authentication Cheat Sheet follows, calls
/// anything under 15 characters weak when the account has no second factor.
/// rdrs is in exactly that case: passkeys here *replace* the password rather
/// than supplement it, so an account protected by a password is protected by
/// the password alone.
///
/// Only new credentials are measured. Existing passwords keep working at
/// whatever length they were set — the same cheat sheet is explicit that
/// verifiers should not force rotation without a reason to believe a
/// credential is compromised, and "we raised the minimum" is not one.
pub const PASSWORD_MIN_LENGTH: usize = 15;

/// Longest password rdrs will accept.
///
/// The cheat sheet asks for a documented maximum of at least 64 so passphrases
/// fit, and warns against long-password denial of service. Argon2's cost is
/// dominated by its memory parameters rather than input length, so this is a
/// generous bound rather than a tight one — but an explicit limit is still
/// better than the request-body limit deciding it by accident.
pub const PASSWORD_MAX_LENGTH: usize = 128;

/// Check a proposed password against the length policy.
///
/// Deliberately the *whole* policy: no composition rules, no required
/// character classes, no rejected symbols. The cheat sheet is explicit that
/// length and blocklists are what help, and that composition rules mostly
/// push users toward predictable substitutions. Unicode and whitespace are
/// welcome.
///
/// Lengths are counted in characters, not bytes. A byte count would let a
/// 5-character CJK passphrase satisfy a 15-byte minimum while a 15-character
/// ASCII one barely passed — the same rule producing very different strength
/// depending on the writing system.
pub fn validate_password_strength(password: &str) -> AppResult<()> {
    let length = password.chars().count();

    if length < PASSWORD_MIN_LENGTH {
        return Err(AppError::Validation(format!(
            "Password must be at least {PASSWORD_MIN_LENGTH} characters"
        )));
    }
    if length > PASSWORD_MAX_LENGTH {
        // Rejected, never truncated: silently cutting a password would make
        // the stored credential differ from the one the user believes they
        // chose, and would quietly weaken a long passphrase.
        return Err(AppError::Validation(format!(
            "Password must be at most {PASSWORD_MAX_LENGTH} characters"
        )));
    }

    Ok(())
}

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
    fn password_policy_measures_length_and_nothing_else() {
        assert!(validate_password_strength(&"a".repeat(PASSWORD_MIN_LENGTH)).is_ok());
        assert!(validate_password_strength(&"a".repeat(PASSWORD_MIN_LENGTH - 1)).is_err());
        assert!(validate_password_strength(&"a".repeat(PASSWORD_MAX_LENGTH)).is_ok());
        assert!(validate_password_strength(&"a".repeat(PASSWORD_MAX_LENGTH + 1)).is_err());

        // No composition rules: a long run of one character, spaces, symbols
        // and emoji are all acceptable. The cheat sheet asks for exactly this
        // — length and blocklists, not character-class requirements.
        assert!(validate_password_strength("correct horse battery staple").is_ok());
        assert!(validate_password_strength("            ␣␣␣ tabs and spaces").is_ok());
        assert!(validate_password_strength("🔐🔐🔐🔐🔐🔐🔐🔐🔐🔐🔐🔐🔐🔐🔐").is_ok());
    }

    #[test]
    fn password_length_is_counted_in_characters_not_bytes() {
        // 15 CJK characters are 45 bytes; 14 are 42 — comfortably over a
        // byte-based minimum despite being shorter than the policy allows.
        // Counting characters is what makes the rule mean the same thing in
        // every script.
        let fourteen = "密".repeat(PASSWORD_MIN_LENGTH - 1);
        assert!(
            fourteen.len() > PASSWORD_MIN_LENGTH,
            "premise: bytes exceed"
        );
        assert!(validate_password_strength(&fourteen).is_err());

        assert!(validate_password_strength(&"密".repeat(PASSWORD_MIN_LENGTH)).is_ok());
        // ...and the maximum must not punish them for the same reason.
        assert!(validate_password_strength(&"密".repeat(PASSWORD_MAX_LENGTH)).is_ok());
    }

    #[test]
    fn an_over_long_password_is_rejected_not_truncated() {
        // Truncating would store a credential the user never chose, and would
        // silently discard the strength of a long passphrase.
        let long = "a".repeat(PASSWORD_MAX_LENGTH + 100);
        assert!(validate_password_strength(&long).is_err());

        // Nothing in the hashing path truncates either: two passphrases that
        // share their first PASSWORD_MAX_LENGTH characters must not verify
        // against each other's hash.
        let hash = hash_password(&long).unwrap();
        assert!(verify_password(&long, &hash));
        assert!(!verify_password(
            &"a".repeat(PASSWORD_MAX_LENGTH + 99),
            &hash
        ));
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
