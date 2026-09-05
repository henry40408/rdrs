//! Keyed derivation for everything rdrs signs.
//!
//! One process-wide root key — `RDRS_SECRET`, or a random one generated at boot
//! — backs every signature. Each use derives its own tag through a
//! domain-separation prefix, so a value minted for one purpose can never be
//! replayed as another:
//!
//! - [`DOMAIN_IMAGE`] signs image-proxy URLs, binding the upstream URL (and
//!   referrer, when present) so the proxy cannot become an open relay;
//! - [`DOMAIN_GREADER_TOKEN`] signs the Google Reader `T` post token;
//! - [`DOMAIN_SESSION`] signs the session cookie, so `<token>.<hmac>` is rejected
//!   before any database work and a leaked `session.session_token` is not usable
//!   on its own;
//! - [`DOMAIN_AUDIT`] derives the `sid` printed into audit log lines;
//! - [`DOMAIN_OFFLINE`] derives the opaque per-user name of the browser's
//!   offline cache, so the worker can tell one reader's stored articles from
//!   another's without ever being handed a user id;
//! - [`DOMAIN_PIXEL`] signs the open-tracking pixel URL, which is the endpoint's
//!   only authority — it is fetched by clients that carry no session.
//!
//! The prefixes are not decoration: two uses that MAC the same message under the
//! same key produce the same tag, and the CSRF token derives from the session
//! token — without domain separation, the token printed into every rendered form
//! *would be* the cookie's signature.
//!
//! Rotating the root key — including the implicit rotation of a restart with no
//! `RDRS_SECRET` set — invalidates every signature at once. Browser sessions end,
//! and image-proxy URLs already embedded in a client's cached entry HTML break
//! until it re-syncs. `GReader` `ClientLogin` tokens are unaffected: like session
//! tokens they are matched against the database rather than signed, and they live
//! in their own `api_token` table, so a leaked one is not a leaked web session.

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
/// Domain-separation prefix for account-invite tokens.
///
/// Used to *store* an invite rather than sign one: `user_invite.token_hash` holds
/// the tag, and redemption re-derives it from the token in the URL. That keeps a
/// database copy useless on its own for minting links, and lets the column be
/// compared with an ordinary indexed lookup rather than a row-by-row scan.
pub const DOMAIN_INVITE: &[u8] = b"invite:";
/// Domain-separation prefix for open-tracking pixel URLs.
pub const DOMAIN_PIXEL: &[u8] = b"pixel:";
/// Domain-separation prefix for the key that encrypts third-party service
/// credentials at rest — see [`seal`].
pub const DOMAIN_SERVICE_TOKENS: &[u8] = b"service-tokens:";
/// Domain-separation prefix for the flash-message cookie signature.
pub const DOMAIN_FLASH: &[u8] = b"flash:";

/// Separates the session token from its signature in the cookie value.
///
/// Session tokens come from `models::session::generate_token`, whose alphabet is
/// `A-Za-z0-9-_`, so `rsplit_once` on this can never cut into the token itself.
const SIG_SEPARATOR: char = '.';

/// Shortest `RDRS_SECRET` accepted; see `config::load_secret`. A guessable root
/// key lets an attacker mint session cookies and image-proxy URLs alike.
pub const MIN_SECRET_LEN: usize = 16;

/// Keyed MAC over `domain` followed by each part, ready to finalize or verify.
/// The parts are fed in order with no separator, so a caller passing more than
/// one must ensure the concatenation is unambiguous — either by fixing the length
/// of every part but the last, or by choosing parts that cannot straddle a
/// boundary.
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

/// Whether `candidate` is the tag for `parts` under `domain`. Comparison is
/// constant-time (`Mac::verify_slice`), and a wrong-length candidate is simply
/// rejected.
pub fn verify_tag(secret: &[u8], domain: &[u8], parts: &[&[u8]], candidate: &[u8]) -> bool {
    mac(secret, domain, parts).verify_slice(candidate).is_ok()
}

/// Marks a value written by [`seal`], and pins the construction it was written
/// with. A future change of cipher or key derivation gets `rdrs.v2.` and both
/// stay readable.
const SEALED_PREFIX: &str = "rdrs.v1.";

/// Encrypt a third-party credential for storage.
///
/// What this defends against is narrow and worth stating: the key is derived
/// from `RDRS_SECRET`, which lives on the same host as the database, so this
/// does nothing against a compromised server. It covers the case where the
/// *data* leaves without the environment — a database dump, a backup archive, a
/// SQL injection that can read rows, someone opening the SQLite file to look
/// around. That is the common way these leak, which is why it is worth doing at
/// all.
///
/// `XChaCha20-Poly1305` for its 24-byte nonce: random nonces are safe at any
/// volume this will see, with no counter to keep or reuse to get wrong.
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

/// Whether `stored` was written by [`seal`], as opposed to being a plaintext
/// value from before encryption existed.
pub fn is_sealed(stored: &str) -> bool {
    stored.starts_with(SEALED_PREFIX)
}

/// Decrypt a value written by [`seal`].
///
/// `None` means the value cannot be read with this key — a rotated
/// `RDRS_SECRET`, or a truncated column. Callers must not treat that as "not
/// configured": overwriting on that assumption is how a recoverable key mistake
/// becomes lost data.
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

/// Decode lowercase-or-uppercase hex. `None` for an odd length or a non-hex
/// byte, which is how a malformed signature is rejected before any comparison.
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

/// Build the session cookie value for `token`: the token plus its signature.
pub fn sign_session(secret: &[u8], token: &str) -> String {
    let sig = hex_encode(&tag(secret, DOMAIN_SESSION, &[token.as_bytes()]));
    format!("{token}{SIG_SEPARATOR}{sig}")
}

/// Recover the session token from a cookie value, or `None` when the value is
/// malformed or its signature does not verify. Runs before the database is
/// touched, so a forged or truncated cookie costs one HMAC rather than a query.
pub fn verify_session(secret: &[u8], cookie_value: &str) -> Option<String> {
    let (token, sig) = cookie_value.rsplit_once(SIG_SEPARATOR)?;
    let sig = hex_decode(sig)?;
    verify_tag(secret, DOMAIN_SESSION, &[token.as_bytes()], &sig).then(|| token.to_string())
}

/// The CSRF synchronizer token for a session, to embed in rendered forms as a
/// hidden `_csrf` field and accept back as the `X-CSRF-Token` header.
///
/// Derived from the session token under [`DOMAIN_CSRF`], so it needs no column
/// and no database round trip — and cannot equal the session cookie's own
/// signature, which shares the key but not the domain. A session token with no
/// row behind it still yields a valid token, which is what lets a pre-auth page
/// carry one.
pub fn derive_csrf(secret: &[u8], session_token: &str) -> String {
    hex_encode(&tag(secret, DOMAIN_CSRF, &[session_token.as_bytes()]))
}

/// Constant-time check of a submitted CSRF token against the one derived for
/// `session_token`. `false` for a malformed (non-hex) or mismatched token.
pub fn verify_csrf(secret: &[u8], session_token: &str, submitted: &str) -> bool {
    let Some(bytes) = hex_decode(submitted) else {
        return false;
    };
    verify_tag(secret, DOMAIN_CSRF, &[session_token.as_bytes()], &bytes)
}

/// A session/token identifier for audit logs: HMAC-SHA256 salted with the root
/// key, truncated to 8 bytes and hex-encoded. Enough to correlate events
/// belonging to one session, impossible to invert, and not comparable across
/// deployments.
///
/// The root key *is* the salt, which satisfies OWASP's "hashed with a salt"
/// requirement without another setting. The cost is that rotating `RDRS_SECRET`
/// breaks correlation with older lines — consistent with it already ending every
/// session.
pub fn audit_id(secret: &[u8], token: &str) -> String {
    hex_encode(&tag(secret, DOMAIN_AUDIT, &[token.as_bytes()])[..8])
}

/// Opaque namespace for one user's offline cache, as the browser sees it.
///
/// The service worker names its caches after this and wipes any that do not
/// match the value the current page reports, which is what keeps one reader's
/// articles from surviving into another's session on a shared device. It is
/// handed to client JavaScript, so it must not be the user id: that leaks the
/// account's position in the table and is guessable across deployments. The
/// tag is not a credential — nothing is authorised by it — so a truncated one
/// is enough to be collision-free in practice.
pub fn offline_id(secret: &[u8], user_id: i64) -> String {
    hex_encode(&tag(secret, DOMAIN_OFFLINE, &[&user_id.to_le_bytes()])[..8])
}

/// Bytes of the derived tag kept in a pixel token. Wider than the image proxy's
/// 8 because nothing here pays for the length — the URL carries only two ids,
/// where a proxy URL already carries a base64 upstream URL — and a forged token
/// writes a bogus open into the reader's own statistics, which no later check
/// can tell apart from a real one.
const PIXEL_SIG_BYTES: usize = 16;

/// Signature for the open-tracking pixel of `entry_id` as seen by `user_id`.
///
/// The two ids are the whole message, each a fixed 8 bytes, so the pair cannot
/// be re-cut into a different pair that MACs the same. This is the pixel
/// endpoint's *only* authority: it is fetched by external readers that carry no
/// session cookie, so an unsigned or guessable URL would let anyone write into
/// another account's open counts.
pub fn pixel_sig(secret: &[u8], user_id: i64, entry_id: i64) -> String {
    let t = tag(
        secret,
        DOMAIN_PIXEL,
        &[&user_id.to_le_bytes(), &entry_id.to_le_bytes()],
    );
    hex_encode(&t[..PIXEL_SIG_BYTES])
}

/// Constant-time check of a submitted pixel signature. `false` for a malformed
/// (non-hex) or mismatched token.
///
/// The explicit length check is load-bearing: `verify_truncated_left` accepts
/// *any* prefix down to 4 bytes, so without it a 4-byte guess would verify
/// against a 16-byte signature and reduce the search to 2^32.
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
        // Equal plaintexts must not produce equal ciphertexts, or a dump would
        // show which accounts share a token.
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
        // Flip the last payload character; Poly1305 must reject the result
        // rather than yielding garbage plaintext.
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
        // Shorter than the 24-byte nonce: `split_at` would panic, so `open`
        // uses the checked form.
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

        // A different key was used to sign it.
        assert!(verify_session(b"another key that is long enough", &signed).is_none());

        // The token was swapped while the signature was kept — the case that
        // matters, since `session.session_token` values are what an attacker
        // would guess at.
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
        // The whole point of the prefixes: the same message under the same key
        // must not produce the same tag in two different roles.
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
        // Same secret, same token -> same id every time, which is what makes
        // it useful for correlating log lines belonging to one session.
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
        // Pins the "never log the token" rule: even a short, easily-embedded
        // token must not surface verbatim inside its own audit id.
        assert!(!audit_id(SECRET, "abc123").contains("abc123"));
    }

    #[test]
    fn csrf_token_verifies_and_is_session_scoped() {
        let token = derive_csrf(SECRET, "sess-abc");
        assert!(verify_csrf(SECRET, "sess-abc", &token));
        // Bound to the session token: a token minted for one session must not
        // pass for another, nor under a different root key.
        assert!(!verify_csrf(SECRET, "sess-xyz", &token));
        assert!(!verify_csrf(
            b"another key that is long enough",
            "sess-abc",
            &token
        ));
        // Malformed submissions are rejected, not panicked on.
        assert!(!verify_csrf(SECRET, "sess-abc", "not-hex"));
        assert!(!verify_csrf(SECRET, "sess-abc", ""));
    }

    #[test]
    fn csrf_token_differs_from_the_session_signature() {
        // Both derive from the same key and the same token; only the domain
        // separates them. Without that, the `_csrf` printed into every form
        // would be the session cookie's signature.
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
        // Wrong message, wrong domain, and a wrong-length candidate all fail
        // rather than panicking.
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
        // Swapping either id, or the pair as a whole, must not verify — one
        // reader's token cannot record an open for another's entry.
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
        // A prefix of a valid signature is the one that would otherwise slip
        // through: `verify_truncated_left` accepts short tags by design.
        assert!(!verify_pixel_sig(SECRET, 7, 42, &sig[..8]));
        assert!(!verify_pixel_sig(SECRET, 7, 42, &sig[..sig.len() - 2]));
        assert!(!verify_pixel_sig(SECRET, 7, 42, &format!("{sig}00")));
        assert!(!verify_pixel_sig(SECRET, 7, 42, "zzzz"));
        assert!(!verify_pixel_sig(SECRET, 7, 42, ""));
    }

    #[test]
    fn pixel_domain_separates_from_the_image_proxy() {
        // Both sign under the same root key; only the domain keeps a pixel
        // token from being mintable through the proxy's signer, and vice versa.
        let msg: &[&[u8]] = &[&7i64.to_le_bytes(), &42i64.to_le_bytes()];
        assert_ne!(
            tag(SECRET, DOMAIN_PIXEL, msg),
            tag(SECRET, DOMAIN_IMAGE, msg)
        );
    }
}
