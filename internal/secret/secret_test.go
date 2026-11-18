package secret

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

var testSecret = []byte("0123456789abcdef0123456789abcdef")

type vectorFile struct {
	Secret string `json:"secret"`
	Tags   []struct {
		Token         string `json:"token"`
		TagImage      string `json:"tag_image"`
		SessionCookie string `json:"session_cookie"`
		CSRF          string `json:"csrf"`
		AuditID       string `json:"audit_id"`
	} `json:"tags"`
	IDs []struct {
		ID         int64  `json:"id"`
		OfflineID  string `json:"offline_id"`
		PixelSig42 string `json:"pixel_sig_entry_42"`
	} `json:"ids"`
	Sealed struct {
		Plaintext string `json:"plaintext"`
		Stored    string `json:"stored"`
	} `json:"sealed"`
}

func loadVectors(t *testing.T) vectorFile {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join("..", "..", "testdata", "vectors", "secret.json"))
	if err != nil {
		t.Fatal(err)
	}
	var v vectorFile
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	if v.Secret != string(testSecret) {
		t.Fatalf("vector secret %q", v.Secret)
	}
	return v
}

func TestRustVectors(t *testing.T) {
	v := loadVectors(t)
	for _, c := range v.Tags {
		image := Tag(testSecret, DomainImage, []byte(c.Token))
		if got := hexOf(image[:]); got != c.TagImage {
			t.Errorf("image tag %q: %s != %s", c.Token, got, c.TagImage)
		}
		if got := SignSession(testSecret, c.Token); got != c.SessionCookie {
			t.Errorf("session %q: %s != %s", c.Token, got, c.SessionCookie)
		}
		if got := DeriveCSRF(testSecret, c.Token); got != c.CSRF {
			t.Errorf("csrf %q: %s != %s", c.Token, got, c.CSRF)
		}
		if got := AuditID(testSecret, c.Token); got != c.AuditID {
			t.Errorf("audit %q: %s != %s", c.Token, got, c.AuditID)
		}
		if token, ok := VerifySession(testSecret, c.SessionCookie); !ok || token != c.Token {
			t.Errorf("verify session %q: %q %v", c.Token, token, ok)
		}
		if !VerifyCSRF(testSecret, c.Token, c.CSRF) {
			t.Errorf("verify csrf %q", c.Token)
		}
	}
	for _, c := range v.IDs {
		if got := OfflineID(testSecret, c.ID); got != c.OfflineID {
			t.Errorf("offline %d: %s != %s", c.ID, got, c.OfflineID)
		}
		if got := PixelSig(testSecret, c.ID, 42); got != c.PixelSig42 {
			t.Errorf("pixel %d: %s != %s", c.ID, got, c.PixelSig42)
		}
		if !VerifyPixelSig(testSecret, c.ID, 42, c.PixelSig42) {
			t.Errorf("verify pixel %d", c.ID)
		}
	}
}

func TestOpensValueSealedByRust(t *testing.T) {
	v := loadVectors(t)
	got, ok := Open(testSecret, v.Sealed.Stored)
	if !ok || got != v.Sealed.Plaintext {
		t.Fatalf("got %q %v", got, ok)
	}
}

func TestSealRoundTripAndKeyRotation(t *testing.T) {
	stored, err := Seal(testSecret, "tok")
	if err != nil || !IsSealed(stored) || IsSealed("plain") {
		t.Fatalf("%q %v", stored, err)
	}
	if got, ok := Open(testSecret, stored); !ok || got != "tok" {
		t.Fatalf("%q %v", got, ok)
	}
	if _, ok := Open([]byte("another-secret-value"), stored); ok {
		t.Fatal("rotated key must not open")
	}
	if _, ok := Open(testSecret, stored[:len(stored)-4]); ok {
		t.Fatal("truncated value must not open")
	}
	if _, ok := Open(testSecret, "plain"); ok {
		t.Fatal("legacy plaintext is not sealed")
	}
}

// Rust's base64 rejects non-zero trailing bits; Go must not accept a second
// string form of the same ciphertext.
func TestOpenRejectsNonCanonicalBase64(t *testing.T) {
	// "QQ" is canonical for one byte; "QR" differs only in the trailing bits.
	if _, err := urlSafeNoPad.DecodeString("QQ"); err != nil {
		t.Fatalf("canonical form rejected: %v", err)
	}
	if _, err := urlSafeNoPad.DecodeString("QR"); err == nil {
		t.Fatal("non-zero trailing bits accepted")
	}
}

func TestForgedSessionIsRejected(t *testing.T) {
	cookie := SignSession(testSecret, "abc")
	for _, bad := range []string{"abc", "abc.", "abc.zz", cookie + "0", strings.Replace(cookie, "abc", "abd", 1), ""} {
		if _, ok := VerifySession(testSecret, bad); ok {
			t.Errorf("accepted %q", bad)
		}
	}
}

func TestDomainsSeparateTags(t *testing.T) {
	// CSRF derives from the session token; without separation it would equal
	// the cookie signature.
	cookie := SignSession(testSecret, "abc")
	if strings.HasSuffix(cookie, DeriveCSRF(testSecret, "abc")) {
		t.Fatal("csrf equals the session signature")
	}
}

func TestPixelSigRefusesShortPrefixes(t *testing.T) {
	sig := PixelSig(testSecret, 7, 9)
	if !VerifyPixelSig(testSecret, 7, 9, sig) || VerifyPixelSig(testSecret, 7, 10, sig) {
		t.Fatal("exact match only")
	}
	if VerifyPixelSig(testSecret, 7, 9, sig[:8]) || VerifyPixelSig(testSecret, 7, 9, "zz") {
		t.Fatal("short or malformed signature accepted")
	}
}

func TestSetupToken(t *testing.T) {
	token, err := GenerateSetupToken()
	if err != nil || len(token) != 11 || token[5] != '-' {
		t.Fatalf("%q %v", token, err)
	}
	if !VerifySetupToken(testSecret, "ABCDE-12345", " abcde12345 ") {
		t.Fatal("dashes, spaces and case must not matter")
	}
	if VerifySetupToken(testSecret, "ABCDE-12345", "ABCDE-12346") || VerifySetupToken(testSecret, "ABCDE-12345", "ABCDE-123456") {
		t.Fatal("different token accepted")
	}
}

func hexOf(b []byte) string {
	const digits = "0123456789abcdef"
	out := make([]byte, 0, len(b)*2)
	for _, c := range b {
		out = append(out, digits[c>>4], digits[c&15])
	}
	return string(out)
}
