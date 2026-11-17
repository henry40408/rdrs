// Package imageproxy signs and verifies the URLs that route feed images
// through the server, so readers never contact the feed author's host.
package imageproxy

import (
	"crypto/subtle"
	"encoding/base64"
	"strings"

	"github.com/henry40408/rdrs/internal/secret"
)

// sigBytes keeps URLs short: 64 bits of a keyed MAC is a 2^64 forgery search.
const sigBytes = 8

var b64 = base64.RawURLEncoding

// SignURL signs url under the image domain.
func SignURL(rootSecret []byte, url string) string {
	return signMessage(rootSecret, url)
}

// SignURLWithReferrer signs url and referrer together so a signature cannot be
// replayed with another referrer.
func SignURLWithReferrer(rootSecret []byte, url, referrer string) string {
	return signMessage(rootSecret, url+"|"+referrer)
}

func signMessage(rootSecret []byte, message string) string {
	t := secret.Tag(rootSecret, secret.DomainImage, []byte(message))
	return b64.EncodeToString(t[:sigBytes])
}

// VerifySignature compares in constant time.
func VerifySignature(rootSecret []byte, url, signature string) bool {
	return constantTimeEqual(SignURL(rootSecret, url), signature)
}

// VerifySignatureWithReferrer compares in constant time.
func VerifySignatureWithReferrer(rootSecret []byte, url, referrer, signature string) bool {
	return constantTimeEqual(SignURLWithReferrer(rootSecret, url, referrer), signature)
}

func constantTimeEqual(a, b string) bool {
	return subtle.ConstantTimeCompare([]byte(a), []byte(b)) == 1
}

// CreateProxyURL is absolute when baseURL is non-empty (for external
// readers) and root-relative otherwise.
func CreateProxyURL(rootSecret []byte, originalURL, baseURL string) string {
	path := "/api/proxy/image?url=" + b64.EncodeToString([]byte(originalURL)) +
		"&s=" + SignURL(rootSecret, originalURL)
	return joinBase(baseURL, path)
}

// CreateProxyURLWithReferrer adds the referrer hotlink-protected images need.
func CreateProxyURLWithReferrer(rootSecret []byte, originalURL, referrer, baseURL string) string {
	path := "/api/proxy/image?url=" + b64.EncodeToString([]byte(originalURL)) +
		"&s=" + SignURLWithReferrer(rootSecret, originalURL, referrer) +
		"&r=" + b64.EncodeToString([]byte(referrer))
	return joinBase(baseURL, path)
}

func joinBase(baseURL, path string) string {
	if baseURL == "" {
		return path
	}
	return strings.TrimRight(baseURL, "/") + path
}
