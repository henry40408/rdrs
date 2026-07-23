//! Keyed derivation for everything rdrs signs.
//!
//! One process-wide root key — `RDRS_SECRET`, or a random one generated at boot
//! — backs every signature rdrs produces. Each use derives its own tag through a
//! domain-separation prefix, so a value minted for one purpose can never be
//! replayed as another:
//!
//! - [`DOMAIN_IMAGE`] signs image-proxy URLs, binding the upstream URL (and
//!   referrer, when present) so the proxy cannot be turned into an open relay;
//! - [`DOMAIN_GREADER_TOKEN`] signs the Google Reader `T` post token;
//! - [`DOMAIN_SESSION`] signs the session cookie, so `<token>.<hmac>` is
//!   rejected before any database work and a leaked `session.session_token` is
//!   not usable on its own.
//!
//! The prefixes are not decoration. Two uses that MAC the same message under
//! the same key produce the same tag, and the CSRF token that will join this
//! module derives from the session token too — without domain separation, the
//! token printed into every rendered form *would be* the cookie's signature.
//!
//! Rotating the root key — including the implicit rotation of a restart with no
//! `RDRS_SECRET` set — invalidates every signature at once. Browser sessions
//! end, and image-proxy URLs already embedded in a Google Reader client's cached
//! entry HTML break until that client re-syncs. Native `GReader` `ClientLogin`
//! tokens are unaffected: they are the raw `session.session_token`, matched
//! against the database rather than signed.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Domain-separation prefix for image-proxy URL signatures.
pub const DOMAIN_IMAGE: &[u8] = b"image:";
/// Domain-separation prefix for the Google Reader post token.
pub const DOMAIN_GREADER_TOKEN: &[u8] = b"greader-token:";
/// Domain-separation prefix for the session cookie signature.
pub const DOMAIN_SESSION: &[u8] = b"session:";

/// Separates the session token from its signature in the cookie value.
///
/// Session tokens come from `models::session::generate_token`, whose alphabet is
/// `A-Za-z0-9-_`, so `rsplit_once` on this can never cut into the token itself.
const SIG_SEPARATOR: char = '.';

/// Shortest `RDRS_SECRET` accepted; see `config::load_secret`. A guessable root
/// key lets an attacker mint session cookies and image-proxy URLs alike.
pub const MIN_SECRET_LEN: usize = 16;

/// Keyed MAC over `domain` followed by each part, ready to finalize or verify.
///
/// The parts are fed in order with no separator, so a caller passing more than
/// one must ensure the concatenation is unambiguous — either by fixing the
/// length of every part but the last, or by choosing parts that cannot straddle
/// a boundary.
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
    for pair in bytes.chunks_exact(2) {
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
/// malformed or its signature does not verify.
///
/// This runs before the database is touched, so a forged or truncated cookie
/// costs one HMAC rather than a query.
pub fn verify_session(secret: &[u8], cookie_value: &str) -> Option<String> {
    let (token, sig) = cookie_value.rsplit_once(SIG_SEPARATOR)?;
    let sig = hex_decode(sig)?;
    verify_tag(secret, DOMAIN_SESSION, &[token.as_bytes()], &sig).then(|| token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

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
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
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
}
