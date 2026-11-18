package server

import (
	"net/url"
	"testing"
)

func feedsOnly(path string) bool { return path == "/feeds" }

func TestSafeReturnToKeepsAnAllowedPathAndItsQuery(t *testing.T) {
	for _, raw := range []string{"/feeds?category=3&sort=unread", "/feeds"} {
		if got, ok := SafeReturnTo(raw, feedsOnly); !ok || got != raw {
			t.Errorf("%q = %q, %v", raw, got, ok)
		}
	}
}

func TestSafeReturnToDropsTheFragment(t *testing.T) {
	got, ok := SafeReturnTo("/feeds?filter=errors#row-feed-1", feedsOnly)
	if !ok || got != "/feeds?filter=errors" {
		t.Errorf("got %q, %v", got, ok)
	}
}

func TestSafeReturnToRejectsAnythingLeavingTheOriginOrTheAllowlist(t *testing.T) {
	for _, raw := range []string{
		"", "feeds", "//evil.example/feeds", "/\\evil.example/feeds", "/feeds\\..\\admin",
		"https://evil.example/feeds", "javascript:alert(1)", "/feeds\r\nSet-Cookie: x=1", "/feeds\t",
		"/admin", "/feeds/../admin", "/feeds/1/edit", "/feeds/%2e%2e/admin", "/feeds/%2E./admin",
		"/x/../../admin", "/.//evil.example", "/a/..//evil.example", "/%2e//evil.example",
		"/..//evil.example/x?y",
	} {
		if got, ok := SafeReturnTo(raw, feedsOnly); ok {
			t.Errorf("%q accepted as %q", raw, got)
		}
	}
}

func TestSafeReturnToNormalisesBeforeJudging(t *testing.T) {
	cases := map[string]string{
		"/a/../feeds":          "/feeds",
		"/./feeds":             "/feeds",
		"/x/y/../../feeds":     "/feeds",
		"/feeds/.":             "/feeds/",
		"/feeds?q=a b":         "/feeds?q=a%20b",
		"/feeds?q=\"<>'":       "/feeds?q=%22%3C%3E%27",
		"/feeds?q=é":           "/feeds?q=%C3%A9",
		"/feeds?a=1?b=2":       "/feeds?a=1?b=2",
		"/feeds?":              "/feeds?",
		"/feeds?x=%2e%2e#frag": "/feeds?x=%2e%2e",
	}
	allow := func(path string) bool { return path == "/feeds" || path == "/feeds/" }
	for raw, want := range cases {
		if got, ok := SafeReturnTo(raw, allow); !ok || got != want {
			t.Errorf("%q = %q, %v; want %q", raw, got, ok, want)
		}
	}
}

func TestFeedsListReturnFallsBackToTheBareList(t *testing.T) {
	cases := map[string]string{
		"":                    "/feeds",
		"//evil.example":      "/feeds",
		"/admin":              "/feeds",
		"/feeds?filter=stale": "/feeds?filter=stale",
	}
	for raw, want := range cases {
		if got := FeedsListReturn(raw); got != want {
			t.Errorf("%q = %q, want %q", raw, got, want)
		}
	}
}

func TestLoginDestinationsExcludeSignedOutAndMachineRoutes(t *testing.T) {
	for _, path := range []string{"/", "/feeds", "/entries/starred", "/admin", "/loginfo", "/apis"} {
		if !IsLoginDestination(path) {
			t.Errorf("%s should be allowed", path)
		}
	}
	for _, path := range []string{"/login", "/logout", "/setup", "/invite/abc", "/api/session",
		"/reader/api/0/stream/contents", "/static/js/app.js"} {
		if IsLoginDestination(path) {
			t.Errorf("%s should be refused", path)
		}
	}
}

func TestReturnToQueryRoundTripsThroughSafeReturnTo(t *testing.T) {
	original := "/feeds?category=3&sort=unread&filter=all"
	query := ReturnToQuery(original)
	if query != "return_to=%2Ffeeds%3Fcategory%3D3%26sort%3Dunread%26filter%3Dall" {
		t.Fatalf("query = %s", query)
	}
	values, err := url.ParseQuery(query)
	if err != nil {
		t.Fatal(err)
	}
	if got, ok := SafeReturnTo(values.Get("return_to"), feedsOnly); !ok || got != original {
		t.Errorf("got %q, %v", got, ok)
	}
}
