package server

import (
	"errors"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/greader"
	"github.com/henry40408/rdrs/internal/models"
)

// greaderPrefix is where every GReader route is mounted a second time, for
// clients that expect the PHP-style path.
const greaderPrefix = "/api/greader.php"

// googleLoginPrefix starts an Authorization header carrying an api_token.
const googleLoginPrefix = "GoogleLogin auth="

// greaderUser is who a GReader request acts as. Exactly one of session and
// apiToken is set: a cookie is a full web session, an Authorization header is
// a narrower, independently revocable api_token.
type greaderUser struct {
	user     models.User
	session  *models.Session
	apiToken *models.APIToken
}

// viaCookie reports whether the request is a web session, which the CSRF
// guard already covered.
func (u greaderUser) viaCookie() bool { return u.session != nil }

// postTokenSubject is the MAC subject of a POST token. Session and api_token
// values never overlap (the latter carries the rdrs_gr_ prefix).
func (u greaderUser) postTokenSubject() string {
	if u.apiToken != nil {
		return u.apiToken.Token
	}
	return u.session.SessionToken
}

// requireGReader authenticates a GReader request, answering the failure
// itself. It reports false after answering.
func (a *App) requireGReader(w http.ResponseWriter, r *http.Request) (greaderUser, bool) {
	user, err := a.greaderUser(r)
	if err != nil {
		writeError(w, err)
		return greaderUser{}, false
	}
	return user, true
}

// greaderUser tries the Authorization header first. A header that names a
// token is final: it is never retried against the session table, so a web
// session token cannot be smuggled in as an api_token.
func (a *App) greaderUser(r *http.Request) (greaderUser, error) {
	if token := googleLoginToken(r); token != "" {
		return a.apiTokenUser(r, token)
	}
	token := a.cookies.SessionToken(r)
	if token == "" {
		return greaderUser{}, ErrUnauthorized
	}
	return a.sessionUser(r, token)
}

func googleLoginToken(r *http.Request) string {
	header := r.Header.Get("Authorization")
	token, ok := strings.CutPrefix(header, googleLoginPrefix)
	if !ok {
		return ""
	}
	return strings.TrimSpace(token)
}

// sessionUser validates a signed-cookie session. It neither slides nor rotates
// the session: that is the page routes' business.
func (a *App) sessionUser(r *http.Request, token string) (greaderUser, error) {
	ctx := r.Context()
	session, err := models.FindSessionByToken(ctx, a.DB, token)
	if err != nil {
		return greaderUser{}, DatabaseError(err)
	}
	if session == nil {
		return greaderUser{}, ErrUnauthorized
	}
	if session.IsExpired() {
		if err := models.DeleteSession(ctx, a.DB, token); err != nil {
			return greaderUser{}, DatabaseError(err)
		}
		a.Audit.SessionDestroyed(token, session.UserID, "expired")
		return greaderUser{}, ErrUnauthorized
	}
	user, err := a.activeUser(r, session.UserID)
	if err != nil {
		return greaderUser{}, err
	}
	return greaderUser{user: *user, session: session}, nil
}

// apiTokenUser validates a GoogleLogin token against api_token only.
func (a *App) apiTokenUser(r *http.Request, token string) (greaderUser, error) {
	ctx := r.Context()
	apiToken, err := models.FindAPIToken(ctx, a.DB, token)
	if err != nil {
		return greaderUser{}, DatabaseError(err)
	}
	if apiToken == nil {
		return greaderUser{}, ErrUnauthorized
	}
	if apiToken.IsExpired() {
		// Lazy delete, mirroring the session equivalent.
		if err := models.DeleteToken(ctx, a.DB, apiToken.ID, apiToken.UserID); err != nil {
			return greaderUser{}, DatabaseError(err)
		}
		return greaderUser{}, ErrUnauthorized
	}
	user, err := a.activeUser(r, apiToken.UserID)
	if err != nil {
		return greaderUser{}, err
	}
	// Best-effort: must not fail the request being authenticated.
	_, touchErr := models.TouchAndRefreshAPIToken(ctx, a.DB, apiToken)
	logQueryFailed("greader.touch_token", touchErr)
	return greaderUser{user: *user, apiToken: apiToken}, nil
}

// activeUser loads a credential's owner and refuses a disabled one.
func (a *App) activeUser(r *http.Request, userID int64) (*models.User, error) {
	user, err := models.FindUserByID(r.Context(), a.DB, userID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if user == nil {
		return nil, ErrUnauthorized
	}
	if user.IsDisabled() {
		return nil, ErrUserDisabled
	}
	return user, nil
}

// writeText sends a text/plain body, as axum answers a String.
func writeText(w http.ResponseWriter, body string) {
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	_, _ = io.WriteString(w, body)
}

// affectedHeader carries how many rows a bulk write changed. GReader clients
// parse the bare "OK" body literally, so the count rides in a vendor header
// that rdrs' own script reads.
const affectedHeader = "X-Rdrs-Affected"

func writeOKWithAffected(w http.ResponseWriter, count int64) {
	w.Header().Set(affectedHeader, strconv.FormatInt(count, 10))
	writeText(w, "OK")
}

// greaderClientLogin is POST /accounts/ClientLogin: it mints an api_token
// and returns it as Auth, so a leaked client token is not a session takeover.
func (a *App) greaderClientLogin(w http.ResponseWriter, r *http.Request) {
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "Email", "Passwd") {
		return
	}
	creds := credentials{Username: form.Get("Email"), Password: form.Get("Passwd")}
	user, err := a.verifyClientLogin(r, creds)
	if err != nil {
		writeError(w, err)
		return
	}
	request := a.request(r)
	// The client's User-Agent labels the token on /user-settings.
	token, err := models.CreateAPIToken(r.Context(), a.DB, models.NewAPIToken{
		UserID: user.ID, Kind: "greader", Label: request.UserAgent,
		UserAgent: request.UserAgent, IPAddress: request.IP,
	})
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	a.Audit.APITokenCreated(audit.CredentialCreated{
		Token: token.Token, UserID: user.ID, Method: "client_login", Request: request,
	})
	writeText(w, "SID=unused\nLSID=unused\nAuth="+token.Token)
}

// verifyClientLogin applies the web login's budgets and checks: a per-IP and a
// per-account reservation before any verification, refunded on a correct
// password.
func (a *App) verifyClientLogin(r *http.Request, creds credentials) (*models.User, error) {
	const endpoint = "POST /accounts/ClientLogin"
	ip := a.clientIP(r)
	request := a.request(r)
	if decision := a.Limiter.TryAcquire(auth.BucketLogin, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "login", endpoint, "login")
		return nil, TooManyRequests(decision.RetryAfterSecs)
	}
	accountKey, user, err := models.FindUserForLogin(r.Context(), a.DB, creds.Username)
	if err != nil {
		return nil, DatabaseError(err)
	}
	// So a spray cannot pick the protocol nobody watches.
	if decision := a.Limiter.TryAcquireAccount(auth.BucketLogin, accountKey); !decision.Allowed {
		a.logRateLimited(ip.String(), "login", endpoint, "login_account")
		return nil, TooManyRequests(decision.RetryAfterSecs)
	}
	if err := a.checkPassword(user, creds, request); err != nil {
		return nil, err
	}
	a.Limiter.Release(auth.BucketLogin, ip)
	a.Limiter.ReleaseAccount(auth.BucketLogin, accountKey)
	if user.IsDisabled() {
		a.Audit.LoginFailed(len(creds.Username), "disabled", request)
		return nil, ErrUserDisabled
	}
	return user, nil
}

// greaderPostToken is GET /reader/api/0/token.
func (a *App) greaderPostToken(w http.ResponseWriter, r *http.Request) {
	user, ok := a.requireGReader(w, r)
	if !ok {
		return
	}
	writeText(w, greader.GeneratePostToken(a.Config.Secret, user.postTokenSubject(), time.Now()))
}

// checkPostToken verifies the optional T field. Cookie requests already passed
// the CSRF guard (the web UI sends X-CSRF-Token, not T), and many clients omit
// T, so only a token that is present is checked.
func (a *App) checkPostToken(user greaderUser, form url.Values) error {
	if user.viaCookie() || !form.Has("T") {
		return nil
	}
	err := greader.VerifyPostToken(a.Config.Secret, user.postTokenSubject(), form.Get("T"), time.Now())
	if errors.Is(err, greader.ErrBadPostToken) {
		return ErrUnauthorized
	}
	return err
}
