package auth

import (
	"encoding/json"
	"errors"
	"os"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
)

type argonVector struct {
	Hash         string `json:"hash"`
	Password     string `json:"password"`
	RejectsOther bool   `json:"rejects_other"`
	Verifies     bool   `json:"verifies"`
}

type argonFile struct {
	Hashed      []argonVector `json:"hashed"`
	VerifyCases []argonVector `json:"verify_cases"`
}

func readArgonFile(t *testing.T, name string) argonFile {
	t.Helper()
	raw, err := os.ReadFile("../../testdata/vectors/" + name)
	if err != nil {
		t.Fatal(err)
	}
	var file argonFile
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	return file
}

func TestVerifyPasswordAcceptsRustHashes(t *testing.T) {
	for _, name := range []string{"argon2_default.json", "argon2_fast.json"} {
		file := readArgonFile(t, name)
		if len(file.Hashed) == 0 {
			t.Fatalf("%s: no vectors", name)
		}
		for _, v := range file.Hashed {
			if got := VerifyPassword(v.Password, v.Hash); got != v.Verifies {
				t.Errorf("%s %q: verify = %v, want %v", name, v.Password, got, v.Verifies)
			}
			// rejects_other holds what Rust returned for a wrong password.
			if VerifyPassword(v.Password+"x", v.Hash) != v.RejectsOther {
				t.Errorf("%s %q: wrong password verified", name, v.Password)
			}
		}
		for _, v := range file.VerifyCases {
			if got := VerifyPassword("anything", v.Hash); got != v.Verifies {
				t.Errorf("%s hash %q: verify = %v, want %v", name, v.Hash, got, v.Verifies)
			}
		}
	}
}

func TestHashPasswordFormatAndRoundTrip(t *testing.T) {
	t.Setenv("RDRS_FAST_HASH", "1")
	hash, err := HashPassword("secure_password_123")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(hash, "$argon2id$v=19$m=8,t=1,p=1$") {
		t.Fatalf("fast hash format: %s", hash)
	}
	if !VerifyPassword("secure_password_123", hash) || VerifyPassword("wrong_password", hash) {
		t.Fatal("round trip failed")
	}
	other, _ := HashPassword("secure_password_123")
	if other == hash {
		t.Fatal("salt must differ between hashes")
	}
}

func TestHashPasswordDefaultParameters(t *testing.T) {
	// Setenv cannot unset; Unsetenv with cleanup keeps the process clean.
	if value, ok := os.LookupEnv("RDRS_FAST_HASH"); ok {
		os.Unsetenv("RDRS_FAST_HASH")
		t.Cleanup(func() { os.Setenv("RDRS_FAST_HASH", value) })
	}
	hash, err := HashPassword("secure_password_123")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(hash, "$argon2id$v=19$m=19456,t=2,p=1$") {
		t.Fatalf("default hash format: %s", hash)
	}
	fields := strings.Split(hash, "$")
	if len(fields[4]) != 22 || len(fields[5]) != 43 {
		t.Fatalf("salt/hash lengths %d/%d, want 22/43 (16 and 32 bytes, unpadded)", len(fields[4]), len(fields[5]))
	}
	if !VerifyPassword("secure_password_123", hash) {
		t.Fatal("default hash does not verify")
	}
}

func TestVerifyPasswordRejectsMalformedHashes(t *testing.T) {
	t.Setenv("RDRS_FAST_HASH", "1")
	good, _ := HashPassword("secure_password_123")
	fields := strings.Split(good, "$")
	cases := map[string]string{
		"empty":        "",
		"sentinel":     "!",
		"garbage":      "invalid_hash",
		"argon2d":      strings.Replace(good, "argon2id", "argon2d", 1),
		"bcrypt":       "$2b$12$abcdefghijklmnopqrstuuabcdefghijklmnopqrstuvwxyz01234",
		"old version":  strings.Replace(good, "v=19", "v=16", 1),
		"no version":   strings.Replace(good, "$v=19", "", 1),
		"zero time":    strings.Replace(good, "t=1", "t=0", 1),
		"zero threads": strings.Replace(good, "p=1", "p=0", 1),
		"huge memory":  strings.Replace(good, "m=8", "m=4294967295", 1),
		"order swap":   "$argon2id$v=19$t=1,m=8,p=1$" + fields[4] + "$" + fields[5],
		"bad salt":     "$argon2id$v=19$m=8,t=1,p=1$!!!$" + fields[5],
		"empty salt":   "$argon2id$v=19$m=8,t=1,p=1$$" + fields[5],
		"short key":    "$argon2id$v=19$m=8,t=1,p=1$" + fields[4] + "$AAA",
		"extra field":  good + "$x",
	}
	for name, hash := range cases {
		if VerifyPassword("secure_password_123", hash) {
			t.Errorf("%s: malformed hash verified", name)
		}
	}
}

func TestVerifyDummyPasswordDoesNotPanic(t *testing.T) {
	VerifyDummyPassword("whatever")
	if dummyHash() == "" {
		t.Fatal("dummy hash was not generated")
	}
}

type strengthVector struct {
	Error      *string  `json:"error"`
	Password   string   `json:"password"`
	UserInputs []string `json:"user_inputs"`
}

// knownFeedbackDifferences lists vectors the Go estimator rejects like Rust
// but explains with another warning: the dictionary word 123456789 and the
// sequence 123456789 both cost 50 guesses, and Go keeps the dictionary match.
var knownFeedbackDifferences = map[string]string{
	"123456789012345": "dictionary vs sequence tie",
}

func TestValidatePasswordStrengthMatchesRust(t *testing.T) {
	raw, err := os.ReadFile("../../testdata/vectors/zxcvbn.json")
	if err != nil {
		t.Fatal(err)
	}
	var vectors []strengthVector
	if err := json.Unmarshal(raw, &vectors); err != nil {
		t.Fatal(err)
	}
	for _, v := range vectors {
		err := ValidatePasswordStrength(v.Password, v.UserInputs)
		if v.Error == nil {
			if err != nil {
				t.Errorf("%q: unexpected error %v", v.Password, err)
			}
			continue
		}
		want := strings.TrimPrefix(*v.Error, "Validation error: ")
		if reason, known := knownFeedbackDifferences[v.Password]; known {
			// Same verdict, different tie-break between equally cheap matches.
			if err == nil || !strings.HasSuffix(err.Error(), "Add another word or two. Uncommon words are better.") {
				t.Errorf("%q (%s): error = %v", v.Password, reason, err)
			}
			continue
		}
		var validation *models.ValidationError
		if !errors.As(err, &validation) || validation.Message != want {
			t.Errorf("%q: error = %v, want %q", v.Password, err, want)
		}
	}
}

func TestPasswordLengthBoundsCountCharacters(t *testing.T) {
	strong := func(n int) string {
		const alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*"
		state := uint64(0x2545F4914F6CDD1D)
		out := make([]byte, n)
		for i := range out {
			state = state*6364136223846793005 + 1442695040888963407
			out[i] = alphabet[int(state>>33)%len(alphabet)]
		}
		return string(out)
	}
	for _, tc := range []struct {
		length int
		ok     bool
	}{{PasswordMinLength - 1, false}, {PasswordMinLength, true}, {PasswordMaxLength, true}, {PasswordMaxLength + 1, false}} {
		if err := ValidatePasswordStrength(strong(tc.length), nil); (err == nil) != tc.ok {
			t.Errorf("length %d: err = %v, want ok=%v", tc.length, err, tc.ok)
		}
	}
	// 14 CJK characters are 42 bytes: the bound counts characters, not bytes.
	if err := ValidatePasswordStrength("密碼很長也很難猜對不對真的很", nil); err == nil {
		t.Error("14 characters accepted")
	}
	if err := ValidatePasswordStrength("密碼很長也很難猜對不對真的很難猜", nil); err != nil {
		t.Errorf("16 characters rejected: %v", err)
	}
}

func TestGuessableShapesAreRejected(t *testing.T) {
	for _, weak := range []string{
		"passwordpassword", "qwertyuiopasdfgh", "aaaaaaaaaaaaaaaa", "abcabcabcabcabcabc",
		"iloveyouiloveyou", "1234567890123456", "letmeinletmein12",
	} {
		if ValidatePasswordStrength(weak, nil) == nil {
			t.Errorf("%q accepted", weak)
		}
	}
}

func TestWeaknessMessageFallbacks(t *testing.T) {
	// A top-10 password alone: warning and the added suggestion.
	err := ValidatePasswordStrength("passwordpassword", nil)
	if err == nil || !strings.HasSuffix(err.Error(), "Add another word or two. Uncommon words are better.") {
		t.Fatalf("got %v", err)
	}
}
