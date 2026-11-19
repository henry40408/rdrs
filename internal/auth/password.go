package auth

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
	"unicode/utf8"

	"golang.org/x/crypto/argon2"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/zxcvbn"
)

// PasswordMinLength is the shortest password accepted for a new credential
// (NIST SP800-63B: 15 without a second factor). Existing passwords are not
// forced to rotate.
const PasswordMinLength = 15

// PasswordMaxLength bounds long-password denial of service (OWASP: at least 64).
const PasswordMaxLength = 128

// passwordMinScore only rejects degenerate shapes.
const passwordMinScore = 3

// Argon2 parameters of a new hash: the argon2 0.6 defaults, or the minimum
// under RDRS_FAST_HASH (tests and CI only). Verification reads the parameters
// from the stored hash, so the two kinds coexist.
const (
	defaultMemoryKiB = 19456
	defaultTime      = 2
	fastMemoryKiB    = 8
	fastTime         = 1
	argonThreads     = 1
	argonSaltLen     = 16
	argonKeyLen      = 32
	argonVersion     = 19
)

// maxVerifyMemoryKiB refuses a stored hash whose cost would exhaust memory.
const maxVerifyMemoryKiB = 1 << 21

var phcEncoding = base64.RawStdEncoding

// ValidatePasswordStrength checks length (in characters, not bytes), then
// zxcvbn guessability, with no composition rules (OWASP). userInputs, such as
// the username, count as known to the attacker.
func ValidatePasswordStrength(password string, userInputs []string) error {
	length := utf8.RuneCountInString(password)
	if length < PasswordMinLength {
		return &models.ValidationError{Message: fmt.Sprintf("Password must be at least %d characters", PasswordMinLength)}
	}
	if length > PasswordMaxLength {
		return &models.ValidationError{Message: fmt.Sprintf("Password must be at most %d characters", PasswordMaxLength)}
	}
	result := zxcvbn.PasswordStrength(password, userInputs)
	if result.Score >= passwordMinScore {
		return nil
	}
	return &models.ValidationError{Message: weaknessMessage(result)}
}

func weaknessMessage(result zxcvbn.Result) string {
	feedback, _ := zxcvbn.FeedbackFor(result)
	warning := feedback.Warning
	if warning == "" {
		warning = "That password is too easy to guess"
	}
	suggestion := "Try a longer phrase of unrelated words"
	if len(feedback.Suggestions) > 0 {
		suggestion = feedback.Suggestions[0]
	}
	return strings.TrimRight(warning, ".") + ". " + suggestion
}

// HashPassword returns an Argon2id PHC string:
// $argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>.
func HashPassword(password string) (string, error) {
	memory, passes := hashCost()
	salt := make([]byte, argonSaltLen)
	if _, err := rand.Read(salt); err != nil {
		return "", fmt.Errorf("Password hashing failed: %w", err) //nolint:staticcheck // ST1005: user-facing text kept verbatim from the Rust build
	}
	key := argon2.IDKey([]byte(password), salt, passes, memory, argonThreads, argonKeyLen)
	return fmt.Sprintf("$argon2id$v=%d$m=%d,t=%d,p=%d$%s$%s", argonVersion, memory, passes, argonThreads,
		phcEncoding.EncodeToString(salt), phcEncoding.EncodeToString(key)), nil
}

func hashCost() (memory, passes uint32) {
	if _, fast := os.LookupEnv("RDRS_FAST_HASH"); fast {
		return fastMemoryKiB, fastTime
	}
	return defaultMemoryKiB, defaultTime
}

// VerifyPassword reports whether password matches hash. A malformed hash, the
// "!" sentinel and any unsupported algorithm never verify.
func VerifyPassword(password, hash string) bool {
	parsed, ok := parsePHC(hash)
	if !ok {
		return false
	}
	derive := argon2.IDKey
	if parsed.algorithm == "argon2i" {
		derive = argon2.Key
	}
	key := derive([]byte(password), parsed.salt, parsed.passes, parsed.memory, parsed.threads, uint32(len(parsed.key)))
	return subtle.ConstantTimeCompare(key, parsed.key) == 1
}

type phcHash struct {
	algorithm string
	memory    uint32
	passes    uint32
	threads   uint8
	salt      []byte
	key       []byte
}

func parsePHC(hash string) (phcHash, bool) {
	fields := strings.Split(hash, "$")
	if len(fields) != 6 || fields[0] != "" {
		return phcHash{}, false
	}
	algorithm := fields[1]
	if algorithm != "argon2id" && algorithm != "argon2i" {
		return phcHash{}, false
	}
	if fields[2] != "v="+strconv.Itoa(argonVersion) {
		return phcHash{}, false
	}
	memory, passes, threads, ok := parseCostParams(fields[3])
	if !ok {
		return phcHash{}, false
	}
	salt, saltErr := phcEncoding.DecodeString(fields[4])
	key, keyErr := phcEncoding.DecodeString(fields[5])
	if saltErr != nil || keyErr != nil || len(salt) == 0 || len(key) < 4 {
		return phcHash{}, false
	}
	return phcHash{algorithm: algorithm, memory: memory, passes: passes, threads: threads, salt: salt, key: key}, true
}

// parseCostParams reads "m=..,t=..,p=.." in exactly that order, the order the
// PHC string format and every writer of these hashes use.
func parseCostParams(field string) (memory, passes uint32, threads uint8, ok bool) {
	parts := strings.Split(field, ",")
	if len(parts) != 3 {
		return 0, 0, 0, false
	}
	m, mErr := parseParam(parts[0], "m=", 32)
	t, tErr := parseParam(parts[1], "t=", 32)
	p, pErr := parseParam(parts[2], "p=", 8)
	if mErr != nil || tErr != nil || pErr != nil {
		return 0, 0, 0, false
	}
	if t == 0 || p == 0 || m < 8*p || m > maxVerifyMemoryKiB {
		return 0, 0, 0, false
	}
	return uint32(m), uint32(t), uint8(p), true
}

func parseParam(part, prefix string, bits int) (uint64, error) {
	value, found := strings.CutPrefix(part, prefix)
	if !found {
		return 0, fmt.Errorf("missing %s", prefix)
	}
	return strconv.ParseUint(value, 10, bits)
}

var dummyHash = sync.OnceValue(func() string {
	filler := make([]byte, argonSaltLen)
	// A failed read leaves zeros: the dummy only has to cost the same.
	_, _ = rand.Read(filler)
	hash, err := HashPassword(base64.RawStdEncoding.EncodeToString(filler))
	if err != nil {
		return ""
	}
	return hash
})

// VerifyDummyPassword spends one verification against a hash of the process's
// own cost when the username did not resolve, so login timing is not an
// account-existence oracle. It returns nothing so no caller can branch on it.
func VerifyDummyPassword(password string) {
	_ = VerifyPassword(password, dummyHash())
}
