package imageproxy

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

var testSecret = []byte("0123456789abcdef0123456789abcdef")

func TestRustVectors(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "testdata", "vectors", "image_proxy.json"))
	if err != nil {
		t.Fatal(err)
	}
	var rows []struct {
		URL           string `json:"url"`
		Sign          string `json:"sign"`
		ProxyRelative string `json:"proxy_relative"`
		ProxyAbsolute string `json:"proxy_absolute"`
		SignReferrer  string `json:"sign_referrer"`
		ProxyReferrer string `json:"proxy_referrer"`
	}
	if err := json.Unmarshal(raw, &rows); err != nil {
		t.Fatal(err)
	}
	if len(rows) == 0 {
		t.Fatal("no vectors")
	}
	const referrer = "https://ref.example/"
	for _, r := range rows {
		if got := SignURL(testSecret, r.URL); got != r.Sign {
			t.Errorf("SignURL(%q) = %q, want %q", r.URL, got, r.Sign)
		}
		if got := CreateProxyURL(testSecret, r.URL, ""); got != r.ProxyRelative {
			t.Errorf("relative = %q, want %q", got, r.ProxyRelative)
		}
		if got := CreateProxyURL(testSecret, r.URL, "https://rdrs.example/"); got != r.ProxyAbsolute {
			t.Errorf("absolute = %q, want %q", got, r.ProxyAbsolute)
		}
		if got := SignURLWithReferrer(testSecret, r.URL, referrer); got != r.SignReferrer {
			t.Errorf("SignURLWithReferrer = %q, want %q", got, r.SignReferrer)
		}
		if got := CreateProxyURLWithReferrer(testSecret, r.URL, referrer, ""); got != r.ProxyReferrer {
			t.Errorf("referrer proxy = %q, want %q", got, r.ProxyReferrer)
		}
	}
}

func TestSignatureLengthIsElevenCharacters(t *testing.T) {
	if got := SignURL(testSecret, "https://example.com/image.jpg"); len(got) != 11 {
		t.Fatalf("len = %d", len(got))
	}
}

func TestVerifySignature(t *testing.T) {
	const url = "https://example.com/image.jpg"
	sig := SignURL(testSecret, url)
	other := []byte("other_secret_key_32_bytes_long!!")
	tests := []struct {
		name string
		ok   bool
	}{
		{"valid", VerifySignature(testSecret, url, sig)},
		{"garbage", VerifySignature(testSecret, url, "invalid_sig")},
		{"empty", VerifySignature(testSecret, url, "")},
		{"other url", VerifySignature(testSecret, url+"x", sig)},
		{"other secret", VerifySignature(other, url, sig)},
	}
	for i, tc := range tests {
		if want := i == 0; tc.ok != want {
			t.Errorf("%s: got %v, want %v", tc.name, tc.ok, want)
		}
	}
}

func TestVerifySignatureWithReferrerBindsReferrer(t *testing.T) {
	const url, ref = "https://example.com/a.png", "https://ref.example/"
	sig := SignURLWithReferrer(testSecret, url, ref)
	if !VerifySignatureWithReferrer(testSecret, url, ref, sig) {
		t.Fatal("valid signature rejected")
	}
	if VerifySignatureWithReferrer(testSecret, url, "https://evil.example/", sig) {
		t.Fatal("signature replayed with another referrer")
	}
	if VerifySignature(testSecret, url, sig) {
		t.Fatal("referrer signature accepted as a plain one")
	}
}

func TestBaseURLTrailingSlashesAreTrimmed(t *testing.T) {
	got := CreateProxyURL(testSecret, "https://example.com/a.png", "https://rdrs.example//")
	if want := "https://rdrs.example/api/proxy/image?url="; got[:len(want)] != want {
		t.Fatalf("got %q", got)
	}
}
