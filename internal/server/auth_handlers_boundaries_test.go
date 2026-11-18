package server

import (
	"net/http"
	"strings"
	"testing"
)

func TestLoginAPINamesTheFirstMissingField(t *testing.T) {
	f := newAuthFixture(t)
	for body, field := range map[string]string{
		`{"password":"b"}`:  "username",
		`{}`:                "username",
		`{"username":"a"}`:  "password",
		`{"username":null}`: "username",
	} {
		rec := f.do(jsonRequest("POST", "/api/session", body))
		if rec.Code != http.StatusUnprocessableEntity || !strings.Contains(rec.Body.String(), "missing field `"+field+"`") {
			t.Errorf("%s: %d %s, want missing %s", body, rec.Code, rec.Body, field)
		}
	}
}
