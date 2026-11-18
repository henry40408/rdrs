package server

import (
	"net/http/httptest"
	"testing"
)

func TestHasJSONSuffix(t *testing.T) {
	t.Parallel()
	cases := map[string]bool{
		"application/vnd.api+json":               true,
		"application/problem+json; charset=utf8": true,
		"application/json":                       false,
		"application/xml":                        false,
		"text/vnd.api+json":                      false,
		"application/+json":                      true,
		"application/x+jsonx":                    false,
		"":                                       false,
		"application/vnd.api+json; bad":          false,
	}
	for contentType, want := range cases {
		request := httptest.NewRequest("POST", "/", nil)
		if contentType != "" {
			request.Header.Set("Content-Type", contentType)
		}
		if got := hasJSONSuffix(request); got != want {
			t.Errorf("%q = %v, want %v", contentType, got, want)
		}
	}
}
