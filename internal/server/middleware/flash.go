package middleware

import (
	"encoding/base64"
	"net/http"
	"strings"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/secret"
)

// flashSignatureSeparator splits payload and signature: outside base64url
// and, unlike "~", never percent-encoded by clients.
const flashSignatureSeparator = "."

var flashPrefix = flash.CookieName + "="

var base64url = base64.RawURLEncoding

// SignFlashCookies signs the flash cookie on the way out and verifies it on
// the way in, so anything else able to set a cookie for this host (a sibling
// port or subdomain) cannot put spoofed text in a server banner. It is a
// layer because the many handlers that build flashes do not hold the secret.
func SignFlashCookies(rootSecret []byte) Middleware {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			verifyCookieHeaders(r.Header, rootSecret)
			withHead(next, func(header http.Header, _ int) {
				signSetCookieHeaders(header, rootSecret)
			}).ServeHTTP(w, r)
		})
	}
}

// verifyCookieHeaders rewrites every Cookie header in place: the signature is
// stripped from a valid flash cookie and an invalid one is dropped. Every
// header is rebuilt, since cookies may be split across several.
func verifyCookieHeaders(header http.Header, rootSecret []byte) {
	existing := header.Values("Cookie")
	if !anyContains(existing, flash.CookieName) {
		return
	}
	header.Del("Cookie")
	for _, line := range existing {
		rewritten, hasFlash := verifyCookieLine(line, rootSecret)
		switch {
		case !hasFlash:
			header.Add("Cookie", line)
		case rewritten != "":
			// An emptied header is dropped, not sent empty.
			header.Add("Cookie", rewritten)
		}
	}
}

func anyContains(lines []string, needle string) bool {
	for _, line := range lines {
		if strings.Contains(line, needle) {
			return true
		}
	}
	return false
}

// verifyCookieLine rewrites one Cookie header; hasFlash is false when it
// carries no flash cookie.
func verifyCookieLine(line string, rootSecret []byte) (rewritten string, hasFlash bool) {
	if !strings.Contains(line, flashPrefix) {
		return "", false
	}
	var kept []string
	for _, pair := range strings.Split(line, ";") {
		pair = strings.TrimSpace(pair)
		value, isFlash := strings.CutPrefix(pair, flashPrefix)
		if !isFlash {
			kept = append(kept, pair)
			continue
		}
		// An unsigned or wrongly signed value is dropped, so the reader sees
		// no flash.
		if payload, ok := verifyFlashValue(value, rootSecret); ok {
			kept = append(kept, flashPrefix+payload)
		}
	}
	return strings.Join(kept, "; "), true
}

func flashSignature(rootSecret []byte, encoded string) string {
	tag := secret.Tag(rootSecret, secret.DomainFlash, []byte(encoded))
	return base64url.EncodeToString(tag[:])
}

// verifyFlashValue checks "payload.signature" and decodes the payload. The
// payload is base64url because clients may percent-encode raw JSON's quotes
// and commas, breaking the signature.
func verifyFlashValue(value string, rootSecret []byte) (string, bool) {
	cut := strings.LastIndex(value, flashSignatureSeparator)
	if cut < 0 {
		return "", false
	}
	encoded, signature := value[:cut], value[cut+1:]
	if signature != flashSignature(rootSecret, encoded) {
		return "", false
	}
	payload, err := base64url.Strict().DecodeString(encoded)
	if err != nil || !utf8.Valid(payload) {
		return "", false
	}
	return string(payload), true
}

// signSetCookieHeaders signs every outgoing flash cookie.
func signSetCookieHeaders(header http.Header, rootSecret []byte) {
	existing := header.Values("Set-Cookie")
	signedAny := false
	for _, line := range existing {
		if strings.HasPrefix(line, flashPrefix) {
			signedAny = true
		}
	}
	if !signedAny {
		return
	}
	header.Del("Set-Cookie")
	for _, line := range existing {
		if signed, ok := signSetCookie(line, rootSecret); ok {
			line = signed
		}
		header.Add("Set-Cookie", line)
	}
}

// signSetCookie signs a flash Set-Cookie line. It leaves other cookies and the
// empty removal cookie alone: signing the latter would leave the flash
// undeletable.
func signSetCookie(line string, rootSecret []byte) (string, bool) {
	rest, isFlash := strings.CutPrefix(line, flashPrefix)
	if !isFlash {
		return "", false
	}
	payload, attributes, hasAttributes := strings.Cut(rest, ";")
	if payload == "" {
		return "", false
	}
	// Encoded, not raw: see verifyFlashValue.
	encoded := base64url.EncodeToString([]byte(payload))
	signed := flashPrefix + encoded + flashSignatureSeparator + flashSignature(rootSecret, encoded)
	if hasAttributes {
		signed += ";" + attributes
	}
	return signed, true
}
