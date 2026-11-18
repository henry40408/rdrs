package greader

import (
	"encoding/hex"
	"errors"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/secret"
)

// PostTokenValidity is how long a POST token stays valid.
const PostTokenValidity = 30 * time.Minute

// ErrBadPostToken is any reason a POST token is not accepted.
var ErrBadPostToken = errors.New("invalid or expired POST token")

// postTokenParts is the MAC input "<subject>/<timestamp>"; subjects never
// contain "/", so the concatenation is unambiguous.
func postTokenParts(subject, timestamp string) [][]byte {
	return [][]byte{[]byte(subject), []byte("/"), []byte(timestamp)}
}

// GeneratePostToken is "<unix seconds>/<hmac hex>", keyed off the root secret
// and bound to the credential (subject) it was issued for.
func GeneratePostToken(rootSecret []byte, subject string, now time.Time) string {
	timestamp := strconv.FormatInt(now.Unix(), 10)
	tag := secret.Tag(rootSecret, secret.DomainGReaderToken, postTokenParts(subject, timestamp)...)
	return timestamp + "/" + hex.EncodeToString(tag[:])
}

// VerifyPostToken reports nil for a token generated for subject within
// PostTokenValidity of now.
func VerifyPostToken(rootSecret []byte, subject, token string, now time.Time) error {
	timestampText, signatureHex, found := strings.Cut(token, "/")
	if !found {
		return ErrBadPostToken
	}
	timestamp, err := strconv.ParseInt(timestampText, 10, 64)
	if err != nil {
		return ErrBadPostToken
	}
	if now.Unix()-timestamp > int64(PostTokenValidity/time.Second) {
		return ErrBadPostToken
	}
	signature, err := hex.DecodeString(signatureHex)
	if err != nil {
		return ErrBadPostToken
	}
	if !secret.VerifyTag(rootSecret, secret.DomainGReaderToken, signature, postTokenParts(subject, timestampText)...) {
		return ErrBadPostToken
	}
	return nil
}
