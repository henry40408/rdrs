// Package secret derives everything rdrs signs from one root key.
//
// Each use MACs under its own domain prefix so a value minted for one purpose
// cannot be replayed as another. The constructions are byte-for-byte those of
// the Rust build: cookies, CSRF tokens, proxy URLs and sealed credentials
// written by one build must verify under the other.
package secret

import (
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"strings"
	"unicode"
	"unicode/utf8"

	"golang.org/x/crypto/chacha20poly1305"
)

// Domain-separation prefixes.
const (
	DomainImage         = "image:"
	DomainGReaderToken  = "greader-token:"
	DomainSession       = "session:"
	DomainCSRF          = "csrf:"
	DomainAudit         = "audit:"
	DomainOffline       = "offline:"
	DomainInvite        = "invite:"
	DomainPixel         = "pixel:"
	DomainServiceTokens = "service-tokens:"
	DomainFlash         = "flash:"
	DomainSetup         = "setup:"
)

// MinSecretLen is the shortest RDRS_SECRET accepted.
const MinSecretLen = 16

const (
	sessionSeparator = "."
	sealedPrefix     = "rdrs.v1."
	pixelSigBytes    = 16
	nonceBytes       = chacha20poly1305.NonceSizeX
	// setupTokenAlphabet is Crockford base32, which has no I, L, O or U.
	setupTokenAlphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
)

var urlSafeNoPad = base64.RawURLEncoding

// Tag is the 32-byte HMAC-SHA256 over domain followed by each part.
func Tag(secret []byte, domain string, parts ...[]byte) [32]byte {
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(domain))
	for _, part := range parts {
		mac.Write(part)
	}
	var out [32]byte
	copy(out[:], mac.Sum(nil))
	return out
}

// VerifyTag reports in constant time whether candidate is the tag for parts.
func VerifyTag(secret []byte, domain string, candidate []byte, parts ...[]byte) bool {
	want := Tag(secret, domain, parts...)
	return hmac.Equal(want[:], candidate)
}

// verifyTruncatedLeft accepts a candidate that is a prefix of the tag. Like the
// Rust verify_truncated_left it refuses prefixes shorter than 4 bytes.
func verifyTruncatedLeft(secret []byte, domain string, candidate []byte, parts ...[]byte) bool {
	if len(candidate) < 4 {
		return false
	}
	want := Tag(secret, domain, parts...)
	if len(candidate) > len(want) {
		return false
	}
	return hmac.Equal(want[:len(candidate)], candidate)
}

// SignSession returns the cookie value "<token>.<hex hmac>".
func SignSession(secret []byte, token string) string {
	sig := Tag(secret, DomainSession, []byte(token))
	return token + sessionSeparator + hex.EncodeToString(sig[:])
}

// VerifySession recovers the token from a cookie value, or reports false when
// the value is malformed or forged.
func VerifySession(secret []byte, cookieValue string) (string, bool) {
	cut := strings.LastIndex(cookieValue, sessionSeparator)
	if cut < 0 {
		return "", false
	}
	token, sigHex := cookieValue[:cut], cookieValue[cut+len(sessionSeparator):]
	sig, err := hex.DecodeString(sigHex)
	if err != nil || !VerifyTag(secret, DomainSession, sig, []byte(token)) {
		return "", false
	}
	return token, true
}

// DeriveCSRF derives the synchronizer token from a session token.
func DeriveCSRF(secret []byte, sessionToken string) string {
	t := Tag(secret, DomainCSRF, []byte(sessionToken))
	return hex.EncodeToString(t[:])
}

// VerifyCSRF checks a submitted CSRF token; malformed input is false.
func VerifyCSRF(secret []byte, sessionToken, submitted string) bool {
	candidate, err := hex.DecodeString(submitted)
	if err != nil {
		return false
	}
	return VerifyTag(secret, DomainCSRF, candidate, []byte(sessionToken))
}

// AuditID is a non-invertible session id for audit logs.
func AuditID(secret []byte, token string) string {
	t := Tag(secret, DomainAudit, []byte(token))
	return hex.EncodeToString(t[:8])
}

// OfflineID is the opaque per-user offline-cache name.
func OfflineID(secret []byte, userID int64) string {
	t := Tag(secret, DomainOffline, int64LE(userID))
	return hex.EncodeToString(t[:8])
}

// PixelSig signs one (user, entry) pair for the open-tracking pixel.
func PixelSig(secret []byte, userID, entryID int64) string {
	t := Tag(secret, DomainPixel, int64LE(userID), int64LE(entryID))
	return hex.EncodeToString(t[:pixelSigBytes])
}

// VerifyPixelSig checks a pixel signature. The length check matters: the
// truncated comparison would accept much shorter prefixes.
func VerifyPixelSig(secret []byte, userID, entryID int64, candidate string) bool {
	raw, err := hex.DecodeString(candidate)
	if err != nil || len(raw) != pixelSigBytes {
		return false
	}
	return verifyTruncatedLeft(secret, DomainPixel, raw, int64LE(userID), int64LE(entryID))
}

func int64LE(n int64) []byte {
	buf := make([]byte, 8)
	binary.LittleEndian.PutUint64(buf, uint64(n))
	return buf
}

// GenerateSetupToken returns a fresh first-run token, "XXXXX-XXXXX".
func GenerateSetupToken() (string, error) {
	raw := make([]byte, 10)
	if _, err := rand.Read(raw); err != nil {
		return "", err
	}
	letters := make([]byte, len(raw))
	for i, b := range raw {
		// 256 is a multiple of 32, so the modulo is unbiased.
		letters[i] = setupTokenAlphabet[int(b)%len(setupTokenAlphabet)]
	}
	return string(letters[:5]) + "-" + string(letters[5:]), nil
}

func normalizeSetupToken(token string) string {
	var out strings.Builder
	for _, r := range token {
		if r == '-' || unicode.IsSpace(r) {
			continue
		}
		out.WriteString(strings.ToUpper(string(r)))
	}
	return out.String()
}

// VerifySetupToken compares MACs of both sides so neither the length nor the
// matching prefix of the token leaks through timing.
func VerifySetupToken(secret []byte, expected, submitted string) bool {
	want := Tag(secret, DomainSetup, []byte(normalizeSetupToken(expected)))
	return VerifyTag(secret, DomainSetup, want[:], []byte(normalizeSetupToken(submitted)))
}

func sealKey(secret []byte) []byte {
	key := Tag(secret, DomainServiceTokens)
	return key[:]
}

// Seal encrypts a third-party credential with XChaCha20-Poly1305.
func Seal(secret []byte, plaintext string) (string, error) {
	aead, err := chacha20poly1305.NewX(sealKey(secret))
	if err != nil {
		return "", err
	}
	nonce := make([]byte, nonceBytes)
	if _, err := rand.Read(nonce); err != nil {
		return "", err
	}
	payload := aead.Seal(nonce, nonce, []byte(plaintext), nil)
	return sealedPrefix + urlSafeNoPad.EncodeToString(payload), nil
}

// IsSealed reports whether stored was written by Seal rather than being
// legacy plaintext.
func IsSealed(stored string) bool {
	return strings.HasPrefix(stored, sealedPrefix)
}

// Open decrypts a value written by Seal. False means unreadable with this key;
// callers must not treat that as "not configured" and overwrite the value.
func Open(secret []byte, stored string) (string, bool) {
	encoded, ok := strings.CutPrefix(stored, sealedPrefix)
	if !ok {
		return "", false
	}
	payload, err := urlSafeNoPad.DecodeString(encoded)
	if err != nil || len(payload) < nonceBytes {
		return "", false
	}
	aead, err := chacha20poly1305.NewX(sealKey(secret))
	if err != nil {
		return "", false
	}
	plain, err := aead.Open(nil, payload[:nonceBytes], payload[nonceBytes:], nil)
	if err != nil || !utf8.Valid(plain) {
		return "", false
	}
	return string(plain), true
}
