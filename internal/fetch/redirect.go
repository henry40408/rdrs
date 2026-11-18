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
// Authorization header once any hop differs in scheme, host or port from the
// original request. The comparison is against the first request, not the
// previous hop: Go copies headers from the first request again on every hop,
// so a header dropped once would come back on the next same-origin hop.
func DropAuthorizationOnOriginChange(req *http.Request, via []*http.Request) error {
	if len(via) >= maxRedirects {
		return errors.New("stopped after too many redirects")
	}
	origin := via[0].URL
	hops := append(append([]*http.Request{}, via[1:]...), req)
	for _, hop := range hops {
		if hop.URL.Scheme != origin.Scheme || hop.URL.Host != origin.Host {
			req.Header.Del("Authorization")
			return nil
		}
	}
	return nil
}
