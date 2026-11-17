package fetch

import (
	"errors"
	"net/http"
)

// NewExternalAPIClient is the client for Linkding and Kagi calls.
func NewExternalAPIClient() *http.Client {
	return &http.Client{Timeout: ExternalAPITimeout, CheckRedirect: DropAuthorizationOnOriginChange}
}

// DropAuthorizationOnOriginChange is a CheckRedirect that removes the
// Authorization header when a redirect changes scheme, host or port. Go keeps
// it while only the hostname matches, which would send a token to another
// service on the same host or over cleartext http; reqwest drops it.
func DropAuthorizationOnOriginChange(req *http.Request, via []*http.Request) error {
	if len(via) >= maxRedirects {
		return errors.New("stopped after too many redirects")
	}
	previous := via[len(via)-1]
	if req.URL.Scheme != previous.URL.Scheme || req.URL.Host != previous.URL.Host {
		req.Header.Del("Authorization")
	}
	return nil
}
