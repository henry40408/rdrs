package server

import "testing"

// Port of tests/compression_test.rs.

func TestLoginPageCompressedWithTheAcceptedEncoding(t *testing.T) {
	app := defaultITestApp(t)
	for _, encoding := range []string{"gzip", "br"} {
		response := app.Do(itestRequest{Path: "/login", Headers: map[string]string{"Accept-Encoding": encoding}})
		if response.Code != 200 {
			t.Fatalf("%s: status %d", encoding, response.Code)
		}
		if applied := response.Header().Get("Content-Encoding"); applied != encoding {
			t.Errorf("Accept-Encoding %s: Content-Encoding = %q", encoding, applied)
		}
	}
}

func TestLoginPageNotCompressedWithoutAcceptEncoding(t *testing.T) {
	response := defaultITestApp(t).Get("/login")
	if response.Code != 200 {
		t.Fatalf("status %d", response.Code)
	}
	if encoding := response.Header().Get("Content-Encoding"); encoding != "" {
		t.Errorf("Content-Encoding = %q without client support", encoding)
	}
}
