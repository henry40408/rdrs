use std::sync::LazyLock;

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use crate::error::{AppError, AppResult};

/// Argon2 hasher used to derive *new* password hashes.
///
/// Production uses [`Argon2::default`] — OWASP-recommended, memory-hard,
/// hundreds of ms per hash in a debug build. `RDRS_FAST_HASH`, set only in
/// test/CI runs, swaps in minimal cost parameters. Safe because
/// [`verify_password`] reads the parameters out of each stored hash, so hashes
/// produced under either setting verify interchangeably.
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

/// A hash of a value nothing can supply, giving the "no such user" login path
/// the same cost as a real password check.
///
/// Produced by [`hash_password`], so it carries whatever cost parameters this
/// process runs with — including `RDRS_FAST_HASH`, which keeps the equalising
/// verify exactly as cheap as the real one it mirrors. Both the input and the
/// salt are freshly random per process: no string a caller could send verifies
/// against it, and the digest is not a constant to fingerprint.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let filler = SaltString::generate(&mut OsRng);
    hash_password(filler.as_str()).expect("hashing with valid params cannot fail")
});

/// Shortest password rdrs will accept for a *new* credential.
///
/// NIST SP800-63B, which OWASP follows, calls anything under 15 characters weak
/// when the account has no second factor. rdrs is in exactly that case: passkeys
/// here *replace* the password rather than supplement it.
///
/// Only new credentials are measured. Existing passwords keep working at
/// whatever length they were set — the same cheat sheet is explicit that
/// verifiers should not force rotation without reason to believe a credential is
/// compromised, and "we raised the minimum" is not one.
pub const PASSWORD_MIN_LENGTH: usize = 15;

/// Longest password rdrs will accept.
///
/// The cheat sheet asks for a documented maximum of at least 64 so passphrases
/// fit, and warns against long-password denial of service. Argon2's cost is
/// dominated by its memory parameters rather than input length, so this is a
/// generous bound — but better than the request-body limit deciding it.
pub const PASSWORD_MAX_LENGTH: usize = 128;

/// Lowest zxcvbn score a new password may have, on its 0–4 scale.
///
/// Three means "more than 10^10 guesses" by zxcvbn's reckoning — enough to rule
/// out the degenerate shapes below while leaving any ordinary passphrase
/// untouched. A non-degenerate password of [`PASSWORD_MIN_LENGTH`] characters
/// scores 4, so this gate almost never fires; that is the property worth having,
/// not a high bar.
const PASSWORD_MIN_SCORE: zxcvbn::Score = zxcvbn::Score::Three;

/// Check a proposed password against the policy: length, then guessability.
///
/// Deliberately the *whole* policy: no composition rules, no required character
/// classes, no rejected symbols. The cheat sheet is explicit that length and
/// blocklists are what help, and that composition rules push users toward
/// predictable substitutions. Unicode and whitespace are welcome.
///
/// Lengths are counted in characters, not bytes: a byte count would let a
/// 5-character CJK passphrase satisfy a 15-byte minimum while a 15-character
/// ASCII one barely passed.
///
/// # Why zxcvbn rather than a breached-password list
///
/// [`PASSWORD_MIN_LENGTH`] already does the blocklist's job: common-password
/// corpora are overwhelmingly short — in `SecLists`' 10k list exactly one entry
/// reaches 15 characters — so a blocklist consulted after the length check would
/// catch almost nothing, at the cost of embedding it in the binary.
///
/// What *does* survive a 15-character minimum is structure: `passwordpassword`,
/// `qwertyuiopasdfgh`, `aaaaaaaaaaaaaaaa`. None appear in a top-100k list and
/// all are trivially guessable, and scoring exactly those patterns is what
/// zxcvbn does.
///
/// `user_inputs` should carry whatever the account already reveals about its
/// owner: zxcvbn penalises a password built out of it, which no static list
/// could do.
///
/// The estimator's *score* gates, but its guess count is never shown — the cheat
/// sheet warns against advertising a bits-of-entropy figure as a guarantee, and
/// it would be one here too.
pub fn validate_password_strength(password: &str, user_inputs: &[&str]) -> AppResult<()> {
    let length = password.chars().count();

    if length < PASSWORD_MIN_LENGTH {
        return Err(AppError::Validation(format!(
            "Password must be at least {PASSWORD_MIN_LENGTH} characters"
        )));
    }
    if length > PASSWORD_MAX_LENGTH {
        // Rejected, never truncated: silently cutting a password would make the
        // stored credential differ from the one the user chose, and would quietly
        // weaken a long passphrase. Checked *before* the estimator, which is the
        // expensive step and has no business running on a refused input.
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

/// Turn a rejected estimate into something a user can act on.
///
/// zxcvbn's own strings are used verbatim where it has them — "Repeats like
/// 'aaa' are easy to guess" beats any generic message, because it names the
/// actual problem. The fallback matters though: `warning` is frequently `None`,
/// and a bare "Password is too weak" leaves the user guessing at what to change,
/// so a suggestion is appended whenever one exists.
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
    let salt = SaltString::generate(&mut OsRng);

    HASHER
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AppError::Internal(format!("Password hashing failed: {e}")))
}

/// Spend one password verification against [`DUMMY_HASH`], discarding the
/// (always negative) result.
///
/// Call this on the branch where the *username* did not resolve. Without it,
/// login answers "no such user" after a single indexed `SELECT` but "wrong
/// password" only after a deliberately slow Argon2 verify — a delta of tens of
/// milliseconds, trivially measurable over the network, turning the generic
/// `Invalid credentials` message into an account-existence oracle.
///
/// Returns nothing on purpose: the work *is* the return value, and a `bool`
/// would invite a caller to branch on a result false by construction.
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

    /// A deterministic, pattern-free password of `len` characters.
    ///
    /// Built from a small LCG rather than a literal so the length-boundary tests
    /// can ask for any length without smuggling in a pattern the estimator would
    /// quite correctly score as weak, turning a length test into a strength test.
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
        // No required character classes: lower-case words and spaces are a
        // fine password, and so is one made of nothing but CJK. The cheat
        // sheet asks for exactly this — length and guessability, not a
        // mixture of cases and symbols.
        assert!(validate_password_strength("correct horse battery staple", &[]).is_ok());
        assert!(validate_password_strength("vulture-mango-77-quilt", &[]).is_ok());
        assert!(validate_password_strength("heron lantern drift plume", &[]).is_ok());
        assert!(validate_password_strength("密碼很長也很難猜對不對真的很難猜", &[]).is_ok());
    }

    #[test]
    fn guessable_shapes_are_rejected_even_at_full_length() {
        // The whole reason the estimator is here. Every one of these clears
        // the 15-character minimum, none appears in a top-100k breach list,
        // and all of them are trivial to guess: doubled words, keyboard
        // walks, repeats, short cycles.
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
        // What no static blocklist can do. The password is a random string, so it
        // is strong in isolation — weak only *for this account*, which the
        // estimator can only know because the username is passed in. Both halves
        // are asserted, so dropping the plumbing fails too.
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
        // "Password is too weak" leaves the user guessing at what to change.
        // zxcvbn names the pattern it found, and the message must carry that
        // through rather than flattening it to something generic.
        let Err(AppError::Validation(msg)) = validate_password_strength("aaaaaaaaaaaaaaaa", &[])
        else {
            panic!("a repeat must be refused");
        };

        assert!(
            msg.to_lowercase().contains("repeat"),
            "the message should name the pattern, got {msg:?}"
        );
        // Two sentences: what is wrong, then what to do instead.
        assert!(msg.contains(". "), "expected a suggestion too, got {msg:?}");
    }

    #[test]
    fn password_length_is_counted_in_characters_not_bytes() {
        // 15 CJK characters are 45 bytes; 14 are 42 — comfortably over a
        // byte-based minimum despite being shorter than the policy allows.
        // Counting characters is what makes the rule mean the same thing in
        // every script.
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
        // Truncating would store a credential the user never chose, and would
        // silently discard the strength of a long passphrase.
        let long = strong_password(PASSWORD_MAX_LENGTH + 100);
        assert!(validate_password_strength(&long, &[]).is_err());

        // Nothing in the hashing path truncates either: two passphrases that
        // share their first PASSWORD_MAX_LENGTH characters must not verify
        // against each other's hash.
        let hash = hash_password(&long).unwrap();
        assert!(verify_password(&long, &hash));
        assert!(!verify_password(&long[..long.len() - 1], &hash));
    }

    #[test]
    fn dummy_verify_costs_the_same_as_a_real_one() {
        // The equalising verify is only worth anything if it does the same work as
        // the check it stands in for. If the dummy hash's cost parameters ever
        // diverge from a freshly minted one's, the "no such user" path becomes
        // distinguishable by timing again.
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
