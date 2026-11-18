package server

import (
	"net/http"
	"strconv"
	"strings"
	"testing"
)

func TestSniffImageTypeNeedsTheWholeWebPHeader(t *testing.T) {
	t.Parallel()
	cases := map[string]string{
		"RIFFxxxxWEBP":  "image/webp", // exactly 12 bytes
		"RIFFxxxxWEB":   "",
		"RIFFxxxxWEBPx": "image/webp",
	}
	for body, want := range cases {
		if got := sniffImageType([]byte(body)); got != want {
			t.Errorf("sniffImageType(%q) = %q, want %q", body, got, want)
		}
	}
}

func TestProxyAcceptsEveryUpstream2xxAndRefusesTheNext(t *testing.T) {
	cases := map[int]int{
		http.StatusOK:                   http.StatusOK,
		299:                             http.StatusOK,
		300:                             http.StatusBadGateway,
		http.StatusNonAuthoritativeInfo: http.StatusOK,
	}
	for upstreamStatus, want := range cases {
		t.Run(strconv.Itoa(upstreamStatus), func(t *testing.T) {
			origin := upstream(t, func(w http.ResponseWriter, _ *http.Request) {
				w.Header().Set("Content-Type", "image/png")
				w.WriteHeader(upstreamStatus)
				_, _ = w.Write([]byte(pngMagic + "rest"))
			})
			response := defaultITestApp(t).Get(signedProxyPath(origin.URL+"/a", ""))
			if response.Code != want {
				t.Errorf("upstream %d: got %d %s, want %d", upstreamStatus, response.Code, response.Text(), want)
			}
		})
	}
}

func TestProxyTreatsAMissingContentTypeAsUnlabelled(t *testing.T) {
	origin := upstream(t, func(w http.ResponseWriter, _ *http.Request) {
		// A nil entry stops net/http from sniffing a type of its own.
		w.Header()["Content-Type"] = nil
		_, _ = w.Write([]byte(pngMagic + "rest"))
	})
	response := defaultITestApp(t).Get(signedProxyPath(origin.URL+"/a", ""))
	if response.Code != http.StatusOK || response.Header().Get("Content-Type") != "image/png" {
		t.Errorf("got %d %q: %s", response.Code, response.Header().Get("Content-Type"), response.Text())
	}
}

func TestProxyImageSizeLimitIsInclusive(t *testing.T) {
	cases := []struct {
		name     string
		size     int
		declared bool
		want     int
	}{
		{"declared exactly the limit", maxProxiedImageBytes, true, http.StatusOK},
		{"streamed exactly the limit", maxProxiedImageBytes, false, http.StatusOK},
		{"declared one over", maxProxiedImageBytes + 1, true, http.StatusBadRequest},
		{"streamed one over", maxProxiedImageBytes + 1, false, http.StatusBadRequest},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			body := []byte(pngMagic + strings.Repeat("x", tc.size-len(pngMagic)))
			origin := upstream(t, func(w http.ResponseWriter, _ *http.Request) {
				w.Header().Set("Content-Type", "image/png")
				if tc.declared {
					w.Header().Set("Content-Length", strconv.Itoa(len(body)))
				}
				_, _ = w.Write(body)
			})
			response := defaultITestApp(t).Get(signedProxyPath(origin.URL+"/a", ""))
			if response.Code != tc.want {
				t.Errorf("got %d, want %d", response.Code, tc.want)
			}
			if tc.want == http.StatusOK && response.Body.Len() != tc.size {
				t.Errorf("relayed %d bytes, want %d", response.Body.Len(), tc.size)
			}
		})
	}
}
