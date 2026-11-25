package imageproxy

import (
	"testing"
)

var testSecret = []byte("0123456789abcdef0123456789abcdef")

var rustProxyVectors = []struct {
	url, sign, proxyRelative, proxyAbsolute, signReferrer, proxyReferrer string
}{
	{
		url:           "https://example.com/a.png",
		sign:          "ctPtoMZh_G0",
		proxyRelative: "/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=ctPtoMZh_G0",
		proxyAbsolute: "https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=ctPtoMZh_G0",
		signReferrer:  "uZi283Prf4E",
		proxyReferrer: "/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=uZi283Prf4E&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8",
	},
	{
		url:           "https://example.com/a.png?x=1&y=2",
		sign:          "yRMxD1FuTLc",
		proxyRelative: "/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZz94PTEmeT0y&s=yRMxD1FuTLc",
		proxyAbsolute: "https://rdrs.example/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZz94PTEmeT0y&s=yRMxD1FuTLc",
		signReferrer:  "mIwC6fLS4oU",
		proxyReferrer: "/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZz94PTEmeT0y&s=mIwC6fLS4oU&r=aHR0cHM6Ly9yZWYuZXhhbXBsZS8",
	},
}

func TestRustVectors(t *testing.T) {
	const referrer = "https://ref.example/"
	for _, r := range rustProxyVectors {
		if got := SignURL(testSecret, r.url); got != r.sign {
			t.Errorf("SignURL(%q) = %q, want %q", r.url, got, r.sign)
		}
		if got := CreateProxyURL(testSecret, r.url, ""); got != r.proxyRelative {
			t.Errorf("relative = %q, want %q", got, r.proxyRelative)
		}
		if got := CreateProxyURL(testSecret, r.url, "https://rdrs.example/"); got != r.proxyAbsolute {
			t.Errorf("absolute = %q, want %q", got, r.proxyAbsolute)
		}
		if got := SignURLWithReferrer(testSecret, r.url, referrer); got != r.signReferrer {
			t.Errorf("SignURLWithReferrer = %q, want %q", got, r.signReferrer)
		}
		if got := CreateProxyURLWithReferrer(testSecret, r.url, referrer, ""); got != r.proxyReferrer {
			t.Errorf("referrer proxy = %q, want %q", got, r.proxyReferrer)
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
