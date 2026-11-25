package secret

import (
	"strings"
	"testing"
)

var testSecret = []byte("0123456789abcdef0123456789abcdef")

var rustTagVectors = []struct {
	token, tagImage, sessionCookie, csrf, auditID string
}{
	{
		token:         "",
		tagImage:      "eb5967eea1df83b35ffb9ce266e4eccf2d719a5661e8d23d103cad62fd4bf894",
		sessionCookie: ".f046cb8fe8729af25a24f06c769ee53ff1955351a2c2102baeb4e5a3e53e18ec",
		csrf:          "c2285c94893729ea817c6bd4c102d7f3a6d5e59d17689a70c871593ccc79dbf1",
		auditID:       "94b2e55462bb314f",
	},
	{
		token:         "abc",
		tagImage:      "c49442e7033fcf70b75586e5e3e0e5ef48943770f07cf29c3242ed21b91a7176",
		sessionCookie: "abc.2706446e0f6f5147a65dbd4b08b33dd1a5762b35a5d8f76e6491f2cc6313da10",
		csrf:          "22bb4fbe5ffd961db1d28e15ef31793de1b51d98992666cbe87b1e7a622ddd3a",
		auditID:       "3a8135cd403f9d82",
	},
	{
		token:         "token-with.dot",
		tagImage:      "a75e283c0a065b53c7cd02c8ed1fb0b118ff3250e8ca17928a133d1320dc714d",
		sessionCookie: "token-with.dot.adddfb50f989e83517db2c132ff7fa4a684e9b8d150aba12028384f56aa41591",
		csrf:          "6bd04451d865a2dbddd57d5abde4f4a514454e800a6307d0e8e5d82cef909303",
		auditID:       "9e8885229e4acb65",
	},
	{
		token:         "日本語",
		tagImage:      "7fc2e344b6a12e19307232dfd958fd4d3b665ba6fd97ecbcdebd1a581c726192",
		sessionCookie: "日本語.9dc3f74b0d166b81a8c9793bd538dc5bca9b38c43a7700875e6a6b4bdf8c2883",
		csrf:          "9f0fe2866378396fd2c4a24a8cfb280c86b4eb6e298dbd244f38298ee6fff4ea",
		auditID:       "e9c0c119562f7ced",
	},
}

var rustIDVectors = []struct {
	id                    int64
	offlineID, pixelSig42 string
}{
	{id: 0, offlineID: "9f7a76084eb09ce4", pixelSig42: "6b14b421b04cbc2b1fff7b52be064e4b"},
	{id: 1, offlineID: "c237ad6d902e9857", pixelSig42: "084d782f30556d55aba824eef13dabf5"},
	{id: 7, offlineID: "43c41414e1a44052", pixelSig42: "768ecc13f8e773d6549b150cc17f0851"},
	{id: -1, offlineID: "ed59d53fad39200c", pixelSig42: "7de0c7726b9390040dbe866e2f3c96bf"},
	{id: 9223372036854775807, offlineID: "9a599686a0f00d27", pixelSig42: "eb06c9a5fd8032119c466910704d402b"},
}

const rustSealedPlaintext = "hunter2-token"
const rustSealedStored = "rdrs.v1.0GncUVDLmcZrWwW_JktMBVEj4cnhe7F44GietTqluT65yNM-xsk_oOwzOaI9pnyZbP66Ako"

func TestRustVectors(t *testing.T) {
	for _, c := range rustTagVectors {
		image := Tag(testSecret, DomainImage, []byte(c.token))
		if got := hexOf(image[:]); got != c.tagImage {
			t.Errorf("image tag %q: %s != %s", c.token, got, c.tagImage)
		}
		if got := SignSession(testSecret, c.token); got != c.sessionCookie {
			t.Errorf("session %q: %s != %s", c.token, got, c.sessionCookie)
		}
		if got := DeriveCSRF(testSecret, c.token); got != c.csrf {
			t.Errorf("csrf %q: %s != %s", c.token, got, c.csrf)
		}
		if got := AuditID(testSecret, c.token); got != c.auditID {
			t.Errorf("audit %q: %s != %s", c.token, got, c.auditID)
		}
		if token, ok := VerifySession(testSecret, c.sessionCookie); !ok || token != c.token {
			t.Errorf("verify session %q: %q %v", c.token, token, ok)
		}
		if !VerifyCSRF(testSecret, c.token, c.csrf) {
			t.Errorf("verify csrf %q", c.token)
		}
	}
	for _, c := range rustIDVectors {
		if got := OfflineID(testSecret, c.id); got != c.offlineID {
			t.Errorf("offline %d: %s != %s", c.id, got, c.offlineID)
		}
		if got := PixelSig(testSecret, c.id, 42); got != c.pixelSig42 {
			t.Errorf("pixel %d: %s != %s", c.id, got, c.pixelSig42)
		}
		if !VerifyPixelSig(testSecret, c.id, 42, c.pixelSig42) {
			t.Errorf("verify pixel %d", c.id)
		}
	}
}

func TestOpensValueSealedByRust(t *testing.T) {
	got, ok := Open(testSecret, rustSealedStored)
	if !ok || got != rustSealedPlaintext {
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
