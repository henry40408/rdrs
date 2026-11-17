package server

import (
	"net/http"
	"net/url"
	"testing"
)

func TestMarkReadRefusesBlankSearchAndMissingSnapshot(t *testing.T) {
	f := newSignedIn(t)
	cases := map[string]url.Values{
		"blank q":          {"q": {"  "}, "snapshot": {"2026-01-01T00:00:00Z"}},
		"absent snapshot":  {"q": {"rust"}},
		"garbled snapshot": {"q": {"rust"}, "snapshot": {"nonsense"}},
	}
	for name, fields := range cases {
		rec := f.postForm("/entries/mark-read", fields)
		if rec.Code != http.StatusSeeOther {
			t.Errorf("%s: status %d, want 303", name, rec.Code)
		}
	}
	if rec := f.postForm("/entries/today/mark-read", url.Values{}); rec.Code != http.StatusSeeOther || rec.Header().Get("Location") != "/entries/today" {
		t.Errorf("today without snapshot: %d %q", rec.Code, rec.Header().Get("Location"))
	}
}

func TestEntryAPIRequiresAuthAndOwnership(t *testing.T) {
	f := newSignedIn(t)
	if rec := f.get("/api/entries/999/neighbors"); rec.Code != http.StatusNotFound {
		t.Errorf("unknown entry neighbors: %d, want 404", rec.Code)
	}
	if rec := f.get("/api/entries/999/summary"); rec.Code != http.StatusNotFound {
		t.Errorf("unknown entry summary: %d, want 404", rec.Code)
	}
	if rec := f.get("/api/entries/1/neighbors?unread_only=maybe"); rec.Code != http.StatusBadRequest && rec.Code != http.StatusNotFound {
		t.Errorf("bad flag: %d", rec.Code)
	}
	anonymous := newAuthFixture(t)
	if rec := anonymous.do(jsonRequest("GET", "/api/entries/1/position", "")); rec.Code != http.StatusUnauthorized {
		t.Errorf("anonymous position: %d, want 401", rec.Code)
	}
}
