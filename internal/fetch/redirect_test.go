package fetch

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestAuthorizationSurvivesOnlySameOriginRedirects(t *testing.T) {
	var got string
	other := httptest.NewServer(http.HandlerFunc(func(_ http.ResponseWriter, r *http.Request) {
		got = r.Header.Get("Authorization")
	}))
	defer other.Close()
	same := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/hop" {
			got = r.Header.Get("Authorization")
			return
		}
		target := other.URL
		if r.URL.Path == "/stay" {
			target = "/hop"
		}
		http.Redirect(w, r, target, http.StatusTemporaryRedirect)
	}))
	defer same.Close()

	for path, want := range map[string]string{"/stay": "Token secret", "/leave": ""} {
		got = "unset"
		request, _ := http.NewRequest(http.MethodPost, same.URL+path, nil)
		request.Header.Set("Authorization", "Token secret")
		response, err := NewExternalAPIClient().Do(request)
		if err != nil {
			t.Fatal(err)
		}
		_ = response.Body.Close()
		if got != want {
			t.Errorf("%s: forwarded %q, want %q", path, got, want)
		}
	}
}

func TestRedirectChainIsBounded(t *testing.T) {
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, server.URL, http.StatusTemporaryRedirect)
	}))
	defer server.Close()
	if _, err := NewExternalAPIClient().Get(server.URL); err == nil {
		t.Fatal("a redirect loop must fail")
	}
}

func TestAuthorizationStaysDroppedOnLaterSameOriginHops(t *testing.T) {
	var got string
	other := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/second" {
			got = r.Header.Get("Authorization")
			return
		}
		http.Redirect(w, r, "/second", http.StatusTemporaryRedirect)
	}))
	defer other.Close()
	first := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Redirect(w, r, other.URL+"/first", http.StatusTemporaryRedirect)
	}))
	defer first.Close()

	got = "unset"
	request, _ := http.NewRequest(http.MethodPost, first.URL, nil)
	request.Header.Set("Authorization", "Token secret")
	response, err := NewExternalAPIClient().Do(request)
	if err != nil {
		t.Fatal(err)
	}
	_ = response.Body.Close()
	if got != "" {
		t.Errorf("the second hop got %q", got)
	}
}

func TestAuthorizationIsDroppedOnAnHTTPSDowngrade(t *testing.T) {
	origin, _ := http.NewRequest(http.MethodGet, "https://example.test/", nil)
	hop, _ := http.NewRequest(http.MethodGet, "http://example.test/x", nil)
	hop.Header.Set("Authorization", "Token secret")
	if err := DropAuthorizationOnOriginChange(hop, []*http.Request{origin}); err != nil {
		t.Fatal(err)
	}
	if hop.Header.Get("Authorization") != "" {
		t.Error("a scheme change must drop the token")
	}
	later, _ := http.NewRequest(http.MethodGet, "http://example.test/y", nil)
	later.Header.Set("Authorization", "Token secret")
	if err := DropAuthorizationOnOriginChange(later, []*http.Request{origin, hop}); err != nil {
		t.Fatal(err)
	}
	if later.Header.Get("Authorization") != "" {
		t.Error("the token must not return on a hop after the downgrade")
	}
}
