//! Keyed derivation for everything rdrs signs.
//!
//! One root key (`RDRS_SECRET`, or random at boot) backs every signature; each
//! use MACs under its own `DOMAIN_*` prefix so a value minted for one purpose
//! can never be replayed as another. This is load-bearing: the CSRF token
//! derives from the session token, so without separation it would equal the
//! session cookie's signature.
//!
//! Rotating the key (including a restart without `RDRS_SECRET`) invalidates
//! every signature and ends all browser sessions. `GReader` `ClientLogin`
//! tokens are DB-matched, not signed, and survive rotation.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Domain-separation prefix for image-proxy URL signatures.
pub const DOMAIN_IMAGE: &[u8] = b"image:";
/// Domain-separation prefix for the Google Reader post token.
pub const DOMAIN_GREADER_TOKEN: &[u8] = b"greader-token:";
/// Domain-separation prefix for the session cookie signature.
pub const DOMAIN_SESSION: &[u8] = b"session:";
/// Domain-separation prefix for the CSRF synchronizer token.
pub const DOMAIN_CSRF: &[u8] = b"csrf:";
/// Domain-separation prefix for audit-log session identifiers.
pub const DOMAIN_AUDIT: &[u8] = b"audit:";
/// Domain-separation prefix for the per-user offline-cache namespace.
pub const DOMAIN_OFFLINE: &[u8] = b"offline:";
/// Domain-separation prefix for account-invite tokens. The tag is *stored* in
/// `user_invite.token_hash`, so a DB copy cannot mint links yet stays indexable.
pub const DOMAIN_INVITE: &[u8] = b"invite:";
/// Domain-separation prefix for open-tracking pixel URLs.
pub const DOMAIN_PIXEL: &[u8] = b"pixel:";
/// Domain-separation prefix for the at-rest credential key; see [`seal`].
pub const DOMAIN_SERVICE_TOKENS: &[u8] = b"service-tokens:";
/// Domain-separation prefix for the flash-message cookie signature.
pub const DOMAIN_FLASH: &[u8] = b"flash:";

/// Token/signature separator in the session cookie; absent from the token
/// alphabet (`A-Za-z0-9-_`), so `rsplit_once` cannot cut into the token.
const SIG_SEPARATOR: char = '.';

/// Shortest `RDRS_SECRET` accepted; a guessable root key lets anyone mint
/// session cookies and proxy URLs.
pub const MIN_SECRET_LEN: usize = 16;

/// Keyed MAC over `domain` then each part, with no separator: callers passing
/// several parts must keep the concatenation unambiguous (e.g. fixed lengths).
fn mac(secret: &[u8], domain: &[u8], parts: &[&[u8]]) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts a key of any length");
    mac.update(domain);
    for part in parts {
        mac.update(part);
    }
    mac
}

/// The full 32-byte tag for `parts` under `domain`.
pub fn tag(secret: &[u8], domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    mac(secret, domain, parts).finalize().into_bytes().into()
}

/// Constant-time check that `candidate` is the tag for `parts` under `domain`.
pub fn verify_tag(secret: &[u8], domain: &[u8], parts: &[&[u8]], candidate: &[u8]) -> bool {
    mac(secret, domain, parts).verify_slice(candidate).is_ok()
}

/// Marks and versions values written by [`seal`]; a new construction gets `v2`.
const SEALED_PREFIX: &str = "rdrs.v1.";

/// Encrypt a third-party credential for storage (`XChaCha20-Poly1305`, random
/// 24-byte nonce).
///
/// Protects only against the data leaking without the environment (dump,
/// backup, SQL injection); the key lives on the same host, so not against a
/// compromised server.
pub fn seal(secret: &[u8], plaintext: &str) -> String {
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::{KeyInit as AeadKeyInit, XChaCha20Poly1305, XNonce};
    use rand::Rng;

    let key = tag(secret, DOMAIN_SERVICE_TOKENS, &[]);
    let cipher = XChaCha20Poly1305::new((&key).into());

    let mut nonce_bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .expect("XChaCha20-Poly1305 encryption cannot fail for an in-memory plaintext");

    let mut payload = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
    payload.extend_from_slice(&nonce_bytes);
    payload.extend_from_slice(&ciphertext);

    format!("{SEALED_PREFIX}{}", URL_SAFE_NO_PAD.encode(payload))
}

/// Whether `stored` was written by [`seal`] rather than being legacy plaintext.
pub fn is_sealed(stored: &str) -> bool {
    stored.starts_with(SEALED_PREFIX)
}

/// Decrypt a value written by [`seal`].
///
/// `None` means unreadable with this key (rotated secret, truncation). Callers
/// must not treat it as "not configured" and overwrite, or data is lost.
pub fn open(secret: &[u8], stored: &str) -> Option<String> {
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::{KeyInit as AeadKeyInit, XChaCha20Poly1305, XNonce};

    let payload = URL_SAFE_NO_PAD
        .decode(stored.strip_prefix(SEALED_PREFIX)?)
        .ok()?;
    let (nonce_bytes, ciphertext) = payload.split_at_checked(24)?;

    let key = tag(secret, DOMAIN_SERVICE_TOKENS, &[]);
    let cipher = XChaCha20Poly1305::new((&key).into());

    let plaintext = cipher
        .decrypt(&XNonce::try_from(nonce_bytes).ok()?, ciphertext)
        .ok()?;
    String::from_utf8(plaintext).ok()
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Decode hex of either case; `None` for odd length or a non-hex byte.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let hi = char::from(pair[0]).to_digit(16)?;
        let lo = char::from(pair[1]).to_digit(16)?;
        out.push(u8::try_from(hi * 16 + lo).expect("two hex digits fit in a byte"));
    }
    Some(out)
}

/// Session cookie value: `<token>.<hex hmac>`.
pub fn sign_session(secret: &[u8], token: &str) -> String {
    let sig = hex_encode(&tag(secret, DOMAIN_SESSION, &[token.as_bytes()]));
    format!("{token}{SIG_SEPARATOR}{sig}")
}

/// Recover the token from a cookie value, or `None` if malformed or forged.
/// Runs before any DB work, so a forged cookie costs one HMAC, not a query.
pub fn verify_session(secret: &[u8], cookie_value: &str) -> Option<String> {
    let (token, sig) = cookie_value.rsplit_once(SIG_SEPARATOR)?;
    let sig = hex_decode(sig)?;
    verify_tag(secret, DOMAIN_SESSION, &[token.as_bytes()], &sig).then(|| token.to_string())
}

/// CSRF synchronizer token (`_csrf` field / `X-CSRF-Token` header), derived
/// from the session token under [`DOMAIN_CSRF`] so it needs no storage. Works
/// for a row-less session token too, so pre-auth pages can carry one.
pub fn derive_csrf(secret: &[u8], session_token: &str) -> String {
    hex_encode(&tag(secret, DOMAIN_CSRF, &[session_token.as_bytes()]))
}

/// Constant-time check of a submitted CSRF token; `false` if malformed.
pub fn verify_csrf(secret: &[u8], session_token: &str, submitted: &str) -> bool {
    let Some(bytes) = hex_decode(submitted) else {
        return false;
    };
    verify_tag(secret, DOMAIN_CSRF, &[session_token.as_bytes()], &bytes)
}

/// Non-invertible session id for audit logs (8-byte HMAC, hex). The root key
/// is the salt, satisfying OWASP's "hashed with a salt"; rotation breaks
/// correlation with older lines.
pub fn audit_id(secret: &[u8], token: &str) -> String {
    hex_encode(&tag(secret, DOMAIN_AUDIT, &[token.as_bytes()])[..8])
}

/// Opaque per-user offline-cache name; the service worker wipes caches that
/// don't match, isolating readers on a shared device. Exposed to JS, so it must
/// not be the user id. Not a credential, so truncation is fine.
pub fn offline_id(secret: &[u8], user_id: i64) -> String {
    hex_encode(&tag(secret, DOMAIN_OFFLINE, &[&user_id.to_le_bytes()])[..8])
}

/// Tag bytes kept in a pixel token; a forged one writes undetectable bogus opens.
const PIXEL_SIG_BYTES: usize = 16;

/// Pixel signature for (`user_id`, `entry_id`), both fixed 8 bytes. It is the
/// endpoint's *only* authority, since clients fetch it without a session.
pub fn pixel_sig(secret: &[u8], user_id: i64, entry_id: i64) -> String {
    let t = tag(
        secret,
        DOMAIN_PIXEL,
        &[&user_id.to_le_bytes(), &entry_id.to_le_bytes()],
    );
    hex_encode(&t[..PIXEL_SIG_BYTES])
}

/// Constant-time pixel signature check. The length check is load-bearing:
/// `verify_truncated_left` accepts prefixes down to 4 bytes.
pub fn verify_pixel_sig(secret: &[u8], user_id: i64, entry_id: i64, candidate: &str) -> bool {
    let Some(bytes) = hex_decode(candidate) else {
        return false;
    };
    if bytes.len() != PIXEL_SIG_BYTES {
        return false;
    }
    mac(
        secret,
        DOMAIN_PIXEL,
        &[&user_id.to_le_bytes(), &entry_id.to_le_bytes()],
    )
    .verify_truncated_left(&bytes)
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn sealed_values_round_trip() {
        let sealed = seal(SECRET, "SUPERSECRETTOKEN123");
        assert_eq!(
            open(SECRET, &sealed).as_deref(),
            Some("SUPERSECRETTOKEN123")
        );
    }

    /// The point of the exercise: a database dump must not contain the token.
    #[test]
    fn a_sealed_value_does_not_contain_its_plaintext() {
        let sealed = seal(SECRET, "SUPERSECRETTOKEN123");
        assert!(!sealed.contains("SUPERSECRETTOKEN123"));
        assert!(is_sealed(&sealed));
    }

    #[test]
    fn each_seal_uses_a_fresh_nonce() {
        // Equal ciphertexts would reveal accounts sharing a token.
        let a = seal(SECRET, "same");
        let b = seal(SECRET, "same");
        assert_ne!(a, b);
        assert_eq!(open(SECRET, &a).as_deref(), Some("same"));
        assert_eq!(open(SECRET, &b).as_deref(), Some("same"));
    }

    #[test]
    fn another_key_cannot_open_it() {
        let sealed = seal(SECRET, "token");
        assert_eq!(open(b"another key that is long enough", &sealed), None);
    }

    #[test]
    fn tampering_is_detected() {
        let sealed = seal(SECRET, "token");
        let mut chars: Vec<char> = sealed.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert_eq!(open(SECRET, &tampered), None);
    }

    #[test]
    fn plaintext_is_not_mistaken_for_a_sealed_value() {
        let legacy = r#"{"linkding":{"api_token":"plain"}}"#;
        assert!(!is_sealed(legacy));
        assert_eq!(open(SECRET, legacy), None);
    }

    #[test]
    fn a_truncated_payload_is_rejected_rather_than_panicking() {
        // Shorter than the 24-byte nonce.
        assert_eq!(open(SECRET, "rdrs.v1.AAAA"), None);
        assert_eq!(open(SECRET, "rdrs.v1."), None);
        assert_eq!(open(SECRET, "rdrs.v1.!!!not-base64!!!"), None);
    }

    #[test]
    fn session_signature_round_trips() {
        let signed = sign_session(SECRET, "abc123");
        assert!(signed.starts_with("abc123."));
        assert_eq!(verify_session(SECRET, &signed).as_deref(), Some("abc123"));
    }

    #[test]
    fn session_signature_rejects_tampering() {
        let signed = sign_session(SECRET, "abc123");

        assert!(verify_session(b"another key that is long enough", &signed).is_none());

        // Token swapped, signature kept.
        let sig = signed.split_once('.').unwrap().1;
        assert!(verify_session(SECRET, &format!("abc124.{sig}")).is_none());

        // Signature corrupted, truncated, non-hex, or absent entirely.
        assert!(verify_session(SECRET, &signed.replace("abc123.", "abc123.0")).is_none());
        assert!(verify_session(SECRET, &signed[..signed.len() - 2]).is_none());
        assert!(verify_session(SECRET, "abc123.zzzz").is_none());
        assert!(verify_session(SECRET, "abc123").is_none());
        assert!(verify_session(SECRET, "").is_none());
    }

    #[test]
    fn domains_separate_identical_messages() {
        let a = tag(SECRET, DOMAIN_SESSION, &[b"same"]);
        let b = tag(SECRET, DOMAIN_IMAGE, &[b"same"]);
        let c = tag(SECRET, DOMAIN_GREADER_TOKEN, &[b"same"]);
        let d = tag(SECRET, DOMAIN_CSRF, &[b"same"]);
        let e = tag(SECRET, DOMAIN_AUDIT, &[b"same"]);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert_ne!(a, e);
        assert_ne!(b, c);
        assert_ne!(b, d);
        assert_ne!(b, e);
        assert_ne!(c, d);
        assert_ne!(c, e);
        assert_ne!(d, e);
    }

    #[test]
    fn audit_id_is_stable_and_16_hex_chars() {
        let id = audit_id(SECRET, "tok-1");
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id, audit_id(SECRET, "tok-1"));
    }

    #[test]
    fn audit_id_differs_per_token_and_per_key() {
        assert_ne!(audit_id(SECRET, "tok-1"), audit_id(SECRET, "tok-2"));
        assert_ne!(
            audit_id(SECRET, "tok-1"),
            audit_id(b"another key that is long enough", "tok-1")
        );
    }

    #[test]
    fn audit_id_never_contains_the_token() {
        assert!(!audit_id(SECRET, "abc123").contains("abc123"));
    }

    #[test]
    fn csrf_token_verifies_and_is_session_scoped() {
        let token = derive_csrf(SECRET, "sess-abc");
        assert!(verify_csrf(SECRET, "sess-abc", &token));
        assert!(!verify_csrf(SECRET, "sess-xyz", &token));
        assert!(!verify_csrf(
            b"another key that is long enough",
            "sess-abc",
            &token
        ));
        assert!(!verify_csrf(SECRET, "sess-abc", "not-hex"));
        assert!(!verify_csrf(SECRET, "sess-abc", ""));
    }

    #[test]
    fn csrf_token_differs_from_the_session_signature() {
        // Same key and token; only the domain separates them.
        let signed = sign_session(SECRET, "tok");
        let sig = signed.split_once('.').unwrap().1;
        assert_ne!(sig, derive_csrf(SECRET, "tok"));
    }

    #[test]
    fn verify_tag_matches_tag() {
        let t = tag(SECRET, DOMAIN_IMAGE, &[b"https://example.com/a.png"]);
        assert!(verify_tag(
            SECRET,
            DOMAIN_IMAGE,
            &[b"https://example.com/a.png"],
            &t
        ));
        assert!(!verify_tag(
            SECRET,
            DOMAIN_IMAGE,
            &[b"https://example.com/b.png"],
            &t
        ));
        assert!(!verify_tag(
            SECRET,
            DOMAIN_SESSION,
            &[b"https://example.com/a.png"],
            &t
        ));
        assert!(!verify_tag(
            SECRET,
            DOMAIN_IMAGE,
            &[b"https://example.com/a.png"],
            &t[..8]
        ));
    }

    #[test]
    fn pixel_sig_round_trips() {
        let sig = pixel_sig(SECRET, 7, 42);
        assert_eq!(sig.len(), PIXEL_SIG_BYTES * 2);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(verify_pixel_sig(SECRET, 7, 42, &sig));
    }

    #[test]
    fn pixel_sig_binds_both_ids_and_the_key() {
        let sig = pixel_sig(SECRET, 7, 42);
        assert!(!verify_pixel_sig(SECRET, 8, 42, &sig));
        assert!(!verify_pixel_sig(SECRET, 7, 43, &sig));
        assert!(!verify_pixel_sig(SECRET, 42, 7, &sig));
        assert!(!verify_pixel_sig(
            b"another key that is long enough",
            7,
            42,
            &sig
        ));
    }

    #[test]
    fn pixel_sig_rejects_malformed_and_truncated_candidates() {
        let sig = pixel_sig(SECRET, 7, 42);
        // Would pass `verify_truncated_left` without the length check.
        assert!(!verify_pixel_sig(SECRET, 7, 42, &sig[..8]));
        assert!(!verify_pixel_sig(SECRET, 7, 42, &sig[..sig.len() - 2]));
        assert!(!verify_pixel_sig(SECRET, 7, 42, &format!("{sig}00")));
        assert!(!verify_pixel_sig(SECRET, 7, 42, "zzzz"));
        assert!(!verify_pixel_sig(SECRET, 7, 42, ""));
    }

    #[test]
    fn pixel_domain_separates_from_the_image_proxy() {
        let msg: &[&[u8]] = &[&7i64.to_le_bytes(), &42i64.to_le_bytes()];
        assert_ne!(
            tag(SECRET, DOMAIN_PIXEL, msg),
            tag(SECRET, DOMAIN_IMAGE, msg)
        );
    }
}
