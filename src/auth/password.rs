use std::sync::LazyLock;

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHasher, PasswordVerifier, phc::SaltString},
};

use crate::error::{AppError, AppResult};

/// Argon2 hasher for *new* hashes: [`Argon2::default`], or minimal cost under
/// `RDRS_FAST_HASH` (test/CI only). Safe because [`verify_password`] reads the
/// parameters from each stored hash.
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

/// Hash of a random per-process value, giving the "no such user" login path the
/// same cost as a real check. Uses the process's own cost parameters.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let filler = SaltString::generate();
    hash_password(&filler).expect("hashing with valid params cannot fail")
});

/// Shortest password accepted for a *new* credential (NIST SP800-63B: 15 without
/// a second factor; passkeys here replace passwords). Existing passwords are
/// not forced to rotate.
pub const PASSWORD_MIN_LENGTH: usize = 15;

/// Longest password accepted (OWASP: at least 64, bounded against long-password denial of service).
pub const PASSWORD_MAX_LENGTH: usize = 128;

/// Lowest zxcvbn score (0–4) for a new password; only rejects degenerate shapes.
const PASSWORD_MIN_SCORE: zxcvbn::Score = zxcvbn::Score::Three;

/// Check a proposed password: length (in characters, not bytes), then zxcvbn
/// guessability. Deliberately no composition rules, per OWASP.
///
/// zxcvbn rather than a breach list: the length minimum already excludes nearly
/// all breached passwords, while structured ones (`passwordpassword`) survive
/// it. `user_inputs` lets zxcvbn penalise passwords built from account data.
pub fn validate_password_strength(password: &str, user_inputs: &[&str]) -> AppResult<()> {
    let length = password.chars().count();

    if length < PASSWORD_MIN_LENGTH {
        return Err(AppError::Validation(format!(
            "Password must be at least {PASSWORD_MIN_LENGTH} characters"
        )));
    }
    if length > PASSWORD_MAX_LENGTH {
        // Rejected, never truncated; checked before the expensive estimator.
        return Err(AppError::Validation(format!(
            "Password must be at most {PASSWORD_MAX_LENGTH} characters"
        )));
    }

    let estimate = zxcvbn::zxcvbn(password, user_inputs);
    if estimate.score() < PASSWORD_MIN_SCORE {
        return Err(AppError::Validation(weakness_message(&estimate)));
    }

    Ok(())
}

/// Actionable rejection message: zxcvbn's warning (or a fallback) plus a suggestion.
fn weakness_message(estimate: &zxcvbn::Entropy) -> String {
    let feedback = estimate.feedback();

    let warning = feedback
        .and_then(zxcvbn::feedback::Feedback::warning)
        .map_or_else(
            || "That password is too easy to guess".to_string(),
            |w| w.to_string(),
        );

    let suggestion = feedback
        .and_then(|f| f.suggestions().first().map(ToString::to_string))
        .unwrap_or_else(|| "Try a longer phrase of unrelated words".to_string());

    format!("{}. {}", warning.trim_end_matches('.'), suggestion)
}

pub fn hash_password(password: &str) -> AppResult<String> {
    HASHER
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| AppError::Internal(format!("Password hashing failed: {e}")))
}

/// Spend one verification against `DUMMY_HASH` when the username did not
/// resolve, so login timing is not an account-existence oracle. Returns nothing
/// so no caller can branch on it.
pub fn verify_dummy_password(password: &str) {
    // `black_box` keeps the optimiser from eliding the unused hash.
    std::hint::black_box(verify_password(password, &DUMMY_HASH));
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use argon2::password_hash::phc::PasswordHash;

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

    /// A deterministic, pattern-free (LCG) password of `len` characters.
    fn strong_password(len: usize) -> String {
        const ALPHABET: &[u8] =
            b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*";
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ALPHABET[usize::try_from(state >> 33).unwrap() % ALPHABET.len()] as char
            })
            .collect()
    }

    #[test]
    fn password_policy_enforces_the_length_bounds() {
        // Strong at every length, so only the bound under test can fail.
        assert!(validate_password_strength(&strong_password(PASSWORD_MIN_LENGTH), &[]).is_ok());
        assert!(
            validate_password_strength(&strong_password(PASSWORD_MIN_LENGTH - 1), &[]).is_err()
        );
        assert!(validate_password_strength(&strong_password(PASSWORD_MAX_LENGTH), &[]).is_ok());
        assert!(
            validate_password_strength(&strong_password(PASSWORD_MAX_LENGTH + 1), &[]).is_err()
        );
    }

    #[test]
    fn password_policy_has_no_composition_rules() {
        assert!(validate_password_strength("correct horse battery staple", &[]).is_ok());
        assert!(validate_password_strength("vulture-mango-77-quilt", &[]).is_ok());
        assert!(validate_password_strength("heron lantern drift plume", &[]).is_ok());
        assert!(validate_password_strength("密碼很長也很難猜對不對真的很難猜", &[]).is_ok());
    }

    #[test]
    fn guessable_shapes_are_rejected_even_at_full_length() {
        for weak in [
            "passwordpassword",
            "qwertyuiopasdfgh",
            "aaaaaaaaaaaaaaaa",
            "abcabcabcabcabcabc",
            "iloveyouiloveyou",
            "1234567890123456",
            "letmeinletmein12",
        ] {
            assert!(
                weak.chars().count() >= PASSWORD_MIN_LENGTH,
                "{weak} must clear the length gate for this test to mean anything"
            );
            assert!(
                validate_password_strength(weak, &[]).is_err(),
                "{weak} must be refused as guessable"
            );
        }
    }

    #[test]
    fn a_password_built_from_the_account_it_protects_is_rejected() {
        // Strong in isolation, weak only given the username.
        let username = strong_password(20);
        let password = format!("{username}42");

        assert!(
            validate_password_strength(&password, &[]).is_ok(),
            "premise: the password is strong when the username is unknown"
        );
        assert!(
            validate_password_strength(&password, &[&username]).is_err(),
            "a password that is just the username must be refused"
        );
    }

    #[test]
    fn a_rejection_says_what_to_do_about_it() {
        let Err(AppError::Validation(msg)) = validate_password_strength("aaaaaaaaaaaaaaaa", &[])
        else {
            panic!("a repeat must be refused");
        };

        assert!(
            msg.to_lowercase().contains("repeat"),
            "the message should name the pattern, got {msg:?}"
        );
        assert!(msg.contains(". "), "expected a suggestion too, got {msg:?}");
    }

    #[test]
    fn password_length_is_counted_in_characters_not_bytes() {
        let fourteen = "密碼很長也很難猜對不對真的難".to_string();
        assert_eq!(fourteen.chars().count(), PASSWORD_MIN_LENGTH - 1);
        assert!(
            fourteen.len() > PASSWORD_MIN_LENGTH,
            "premise: bytes exceed"
        );
        assert!(validate_password_strength(&fourteen, &[]).is_err());
    }

    #[test]
    fn an_over_long_password_is_rejected_not_truncated() {
        let long = strong_password(PASSWORD_MAX_LENGTH + 100);
        assert!(validate_password_strength(&long, &[]).is_err());

        // The hashing path must not truncate either.
        let hash = hash_password(&long).unwrap();
        assert!(verify_password(&long, &hash));
        assert!(!verify_password(&long[..long.len() - 1], &hash));
    }

    #[test]
    fn dummy_verify_costs_the_same_as_a_real_one() {
        // Diverging cost parameters would reopen the timing oracle.
        let real = hash_password("whatever").unwrap();
        let real = PasswordHash::new(&real).unwrap();
        let dummy = PasswordHash::new(&DUMMY_HASH).unwrap();

        assert_eq!(dummy.algorithm, real.algorithm);
        assert_eq!(dummy.params, real.params);
        assert_ne!(dummy.salt, real.salt);
    }

    #[test]
    fn dummy_verify_accepts_any_input_and_returns_nothing() {
        // Attacker-controlled input must never panic.
        verify_dummy_password("");
        verify_dummy_password("password123");
        verify_dummy_password(&"x".repeat(4096));
    }

    #[test]
    fn test_verify_is_independent_of_configured_params() {
        // Guards RDRS_FAST_HASH: verify reads params from the stored hash.
        let strong = Argon2::default().hash_password(b"pw").unwrap().to_string();
        let weak_params = Params::new(
            Params::MIN_M_COST,
            Params::MIN_T_COST,
            Params::MIN_P_COST,
            None,
        )
        .unwrap();
        let weak = Argon2::new(Algorithm::Argon2id, Version::V0x13, weak_params)
            .hash_password(b"pw")
            .unwrap()
            .to_string();

        assert!(verify_password("pw", &strong));
        assert!(verify_password("pw", &weak));
        assert!(!verify_password("nope", &strong));
        assert!(!verify_password("nope", &weak));
    }
}
