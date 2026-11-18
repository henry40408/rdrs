package auth

import (
	"fmt"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/zxcvbn"
	"github.com/henry40408/rdrs/internal/zxcvbn/match"
)

func TestHeaderTextAcceptsOnlyVisibleASCIIAndTab(t *testing.T) {
	tests := map[string]string{"a\tb": "a\tb", "a~b": "a~b", "a b": "a b", "a\x1fb": "", "a\x7fb": "", "aé": ""}
	for value, want := range tests {
		r := httptest.NewRequest("GET", "/", nil)
		r.Header.Set("X-Test", value)
		if got := headerText(r, "X-Test"); got != want {
			t.Errorf("headerText(%q) = %q, want %q", value, got, want)
		}
	}
}

func TestWeaknessMessageWithoutFeedbackUsesTheDefaults(t *testing.T) {
	result := zxcvbn.Result{Score: 3, Sequence: []*match.Match{{Pattern: "sequence", Token: "abc"}}}
	want := "That password is too easy to guess. Try a longer phrase of unrelated words"
	if got := weaknessMessage(result); got != want {
		t.Errorf("weaknessMessage = %q", got)
	}
}

func TestParseCostParamsMemoryFloor(t *testing.T) {
	tests := map[string]bool{
		"m=8,t=1,p=1": true, "m=7,t=1,p=1": false, "m=16,t=1,p=2": true, "m=15,t=1,p=2": false,
		"m=64,t=0,p=1": false, "m=64,t=1,p=0": false,
	}
	for field, want := range tests {
		if _, _, _, ok := parseCostParams(field); ok != want {
			t.Errorf("parseCostParams(%q) ok = %v, want %v", field, ok, want)
		}
	}
}

func TestParsePHCAcceptsAFourByteKey(t *testing.T) {
	encode := phcEncoding.EncodeToString
	hash := func(key []byte) string {
		return "$argon2id$v=19$m=8,t=1,p=1$" + encode([]byte("salt")) + "$" + encode(key)
	}
	if _, ok := parsePHC(hash([]byte("1234"))); !ok {
		t.Error("a four byte key was rejected")
	}
	if _, ok := parsePHC(hash([]byte("123"))); ok {
		t.Error("a three byte key was accepted")
	}
}

func TestRSAExponentByteRange(t *testing.T) {
	if _, err := rsaExponentBytes([]int{0, 255}); err != nil {
		t.Errorf("0 and 255 are bytes: %v", err)
	}
	for _, bad := range []int{-1, 256} {
		if _, err := rsaExponentBytes([]int{bad}); err == nil {
			t.Errorf("%d is not a byte", bad)
		}
	}
}

func TestEncodeLegacyOmitsAnEmptyTransportList(t *testing.T) {
	cred, err := DecodeLegacyPasskey([]byte(legacyES256), 0)
	if err != nil {
		t.Fatal(err)
	}
	cred.Transport = nil
	encoded, err := EncodeLegacyPasskey(cred)
	if err != nil || !strings.Contains(string(encoded), `"transports":null`) {
		t.Errorf("%s %v", encoded, err)
	}
}

func TestParseCostParamsMemoryCeiling(t *testing.T) {
	atLimit := fmt.Sprintf("m=%d,t=1,p=1", maxVerifyMemoryKiB)
	overLimit := fmt.Sprintf("m=%d,t=1,p=1", maxVerifyMemoryKiB+1)
	if _, _, _, ok := parseCostParams(atLimit); !ok {
		t.Error("the memory ceiling itself was refused")
	}
	if _, _, _, ok := parseCostParams(overLimit); ok {
		t.Error("memory above the ceiling was accepted")
	}
}

func TestReleaseOnAnEmptySlotDoesNotUnderflow(t *testing.T) {
	limiter := NewRateLimiter(2, 60)
	limiter.Release(BucketLogin, testIP)
	if !limiter.TryAcquire(BucketLogin, testIP).Allowed {
		t.Error("an underflowed counter blocked the first attempt")
	}
}
