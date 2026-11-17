package server

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/views"
)

// authRoutes are the sign-in, setup, invite and sign-out routes.
func (a *App) authRoutes() []Route {
	return []Route{
		{"POST", "/api/setup", a.setupAPI},
		{"POST", "/api/session", a.loginAPI},
		{"DELETE", "/api/session", a.logoutAPI},
		{"POST", "/api/session/reauth", a.reauthenticate},
		{"GET", "/login", a.loginPage},
		{"POST", "/login", a.loginForm},
		{"POST", "/logout", a.logoutForm},
		{"GET", "/setup", a.setupPage},
		{"POST", "/setup", a.setupForm},
		{"GET", "/invite/{token}", a.invitePage},
		{"POST", "/invite/{token}", a.redeemInvite},
	}
}

// identity is the JSON of a created or signed-in account.
type identity struct {
	ID       int64       `json:"id"`
	Username string      `json:"username"`
	Role     models.Role `json:"role"`
}

func identityOf(user *models.User) identity {
	return identity{ID: user.ID, Username: user.Username, Role: user.Role}
}

const defaultCategoryName = "Uncategorized"

// tooManyAttempts is the sign-in form's wording of a rate limit.
func tooManyAttempts(retryAfterSecs int) string {
	return fmt.Sprintf("Too many attempts. Please try again in %d seconds.", retryAfterSecs)
}

// --- setup ---

type setupInput struct {
	Username   string
	Password   string
	SetupToken string
}

// setupAPI is POST /api/setup: create the instance's first (admin) account.
// It is the only anonymous account-creating endpoint and refuses once any
// account exists; later accounts go through invites.
func (a *App) setupAPI(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Username   *string `json:"username"`
		Password   *string `json:"password"`
		SetupToken string  `json:"setup_token"`
	}
	if !decodeJSON(w, r, &body) {
		return
	}
	if body.Username == nil || body.Password == nil {
		field := "username"
		if body.Username != nil {
			field = "password"
		}
		rejection(w, http.StatusUnprocessableEntity,
			"Failed to deserialize the JSON body into the target type: missing field `"+field+"`")
		return
	}
	input := setupInput{Username: *body.Username, Password: *body.Password, SetupToken: body.SetupToken}
	user, err := a.performSetup(r, input, "POST /api/setup")
	if err != nil {
		writeError(w, err)
		return
	}
	// Flash cookies are signed, so only the server can mint one.
	flash.Set(w, flash.NewSuccess("Account created. Please sign in."))
	writeJSON(w, http.StatusCreated, identityOf(user))
}

// performSetup is shared by the JSON and form endpoints; endpoint only labels
// logs.
func (a *App) performSetup(r *http.Request, input setupInput, endpoint string) (*models.User, error) {
	username, err := models.ValidateUsername(input.Username)
	if err != nil {
		return nil, appError(err)
	}

	// Reserved before any query or hashing, and never released: unlike login
	// there is no "correct credential" outcome to refund.
	ip := a.clientIP(r)
	if decision := a.Limiter.TryAcquire(auth.BucketAccountSetup, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "setup", endpoint, "setup")
		return nil, TooManyRequests(decision.RetryAfterSecs)
	}

	ctx := r.Context()
	// Before the expensive work, so a closed endpoint costs one count.
	count, err := models.CountUsers(ctx, a.DB)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if !a.Config.CanSetup(count) {
		return nil, ErrRegistrationNotAllowed
	}
	if token := a.options.SetupToken; token != "" && !secret.VerifySetupToken(a.Config.Secret, token, input.SetupToken) {
		a.Audit.LoginFailed(utf8.RuneCountInString(input.Username), "bad_setup_token", a.request(r))
		return nil, ErrInvalidSetupToken
	}

	// Behind the limiter: zxcvbn's worst case rivals Argon2.
	if err := auth.ValidatePasswordStrength(input.Password, []string{username}); err != nil {
		return nil, appError(err)
	}
	hash, err := auth.HashPassword(input.Password)
	if err != nil {
		return nil, InternalError(err)
	}
	user, err := a.createFirstAdmin(ctx, username, hash)
	if err != nil {
		return nil, err
	}
	a.Audit.AccountCreated(user.ID, user.ID, utf8.RuneCountInString(user.Username), string(user.Role))
	return user, nil
}

// createFirstAdmin runs in one transaction so two concurrent setups cannot
// both pass the empty-instance check; this re-count, not the early one, is the
// authoritative gate.
func (a *App) createFirstAdmin(ctx context.Context, username, passwordHash string) (*models.User, error) {
	tx, err := a.DB.Begin(ctx)
	if err != nil {
		return nil, DatabaseError(err)
	}
	defer func() { _ = tx.Rollback() }()
	if err := tx.LockUserTable(ctx); err != nil {
		return nil, DatabaseError(err)
	}
	count, err := models.CountUsers(ctx, tx)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if !a.Config.CanSetup(count) {
		return nil, ErrRegistrationNotAllowed
	}
	user, err := models.CreateUser(ctx, tx, username, passwordHash, models.RoleAdmin)
	if err != nil {
		return nil, appError(err)
	}
	// Same default as OPML import and the GReader subscription API.
	if _, err := models.CreateCategory(ctx, tx, user.ID, defaultCategoryName); err != nil {
		return nil, appError(err)
	}
	if err := tx.Commit(); err != nil {
		return nil, DatabaseError(err)
	}
	return user, nil
}

func (a *App) logRateLimited(ip, bucket, endpoint, auditBucket string) {
	slog.Warn("credential attempt rate limited", "event", "auth.rate_limited",
		"ip", ip, "bucket", bucket, "endpoint", endpoint)
	a.Audit.LoginRateLimited(endpoint, auditBucket, ip)
}

// setupFormMessage is what the no-JS setup form shows for a failure.
func setupFormMessage(err error) string {
	var appErr *AppError
	if !errors.As(err, &appErr) {
		return "Could not create the account"
	}
	switch {
	case appErr.RetryAfter > 0:
		return tooManyAttempts(appErr.RetryAfter)
	case errors.Is(appErr, ErrUsernameExists):
		return "Username already exists"
	case errors.Is(appErr, ErrInvalidSetupToken):
		return "Invalid setup token. Find it on the server's stderr from startup."
	case appErr.Status == http.StatusBadRequest:
		return appErr.Message
	}
	return "Could not create the account"
}

// setupForm is POST /setup, the no-JS fallback of the first-run form.
func (a *App) setupForm(w http.ResponseWriter, r *http.Request) {
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "username", "password", "confirm-password") {
		return
	}
	renderError := func(message string) {
		writeHTML(w, http.StatusOK, a.setupHTML(r, message, nil))
	}
	if form.Get("password") != form.Get("confirm-password") {
		renderError("Passwords do not match")
		return
	}
	input := setupInput{Username: form.Get("username"), Password: form.Get("password"), SetupToken: form.Get("setup-token")}
	if _, err := a.performSetup(r, input, "POST /setup"); err != nil {
		renderError(setupFormMessage(err))
		return
	}
	flash.Redirect(w, "/login", flash.NewSuccess("Account created. Please sign in."))
}

func (a *App) setupHTML(r *http.Request, message string, banners []flash.Message) string {
	return views.Setup(views.SetupPage{
		GitVersion:        gitVersion(),
		Error:             message,
		FlashMessages:     banners,
		PasswordMinLength: auth.PasswordMinLength,
		PasswordMaxLength: auth.PasswordMaxLength,
		CSRFToken:         a.cookies.CSRFToken(r),
	})
}

// setupPage is GET /setup: first-run only, a redirect to /login once any
// account exists.
func (a *App) setupPage(w http.ResponseWriter, r *http.Request) {
	if !a.setupAvailable(r.Context()) {
		redirect(w, "/login")
		return
	}
	writeHTML(w, http.StatusOK, a.setupHTML(r, "", flash.Take(w, r)))
}

// setupAvailable is false on a store error, hiding the first-run path.
func (a *App) setupAvailable(ctx context.Context) bool {
	count, err := models.CountUsers(ctx, a.DB)
	return err == nil && a.Config.CanSetup(count)
}

// --- login ---

type credentials struct {
	Username string
	Password string
}

// loginAPI is POST /api/session.
func (a *App) loginAPI(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Username *string `json:"username"`
		Password *string `json:"password"`
	}
	if !decodeJSON(w, r, &body) {
		return
	}
	if body.Username == nil || body.Password == nil {
		field := "username"
		if body.Username != nil {
			field = "password"
		}
		rejection(w, http.StatusUnprocessableEntity,
			"Failed to deserialize the JSON body into the target type: missing field `"+field+"`")
		return
	}
	user, err := a.performLogin(w, r, credentials{*body.Username, *body.Password}, "POST /api/session")
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, identityOf(user))
}

// performLogin verifies the credentials and, on success, adds the new session
// and CSRF cookies to w. endpoint only labels logs.
func (a *App) performLogin(w http.ResponseWriter, r *http.Request, creds credentials, endpoint string) (*models.User, error) {
	if a.Config.DisableLocalAuth {
		return nil, ErrForbidden
	}
	ctx := r.Context()
	ip := a.clientIP(r)
	request := a.request(r)
	// Reserved before lookup and verification, so guesses cannot force Argon2 work.
	if decision := a.Limiter.TryAcquire(auth.BucketLogin, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "login", endpoint, "login")
		return nil, TooManyRequests(decision.RetryAfterSecs)
	}
	// Resolved first so the budget is keyed by the name that matched, not by
	// whichever spelling the client typed.
	accountKey, user, err := models.FindUserForLogin(ctx, a.DB, creds.Username)
	if err != nil {
		return nil, DatabaseError(err)
	}
	// Per-account too: the per-IP budget is useless against rotating IPs.
	if decision := a.Limiter.TryAcquireAccount(auth.BucketLogin, accountKey); !decision.Allowed {
		a.logRateLimited(ip.String(), "login", endpoint, "login_account")
		return nil, TooManyRequests(decision.RetryAfterSecs)
	}
	if err := a.checkPassword(user, creds, request); err != nil {
		return nil, err
	}
	// Correct password: refund both reservations, before the disabled check so
	// it cannot leak through a rate-limit side channel.
	a.Limiter.Release(auth.BucketLogin, ip)
	a.Limiter.ReleaseAccount(auth.BucketLogin, accountKey)
	if user.IsDisabled() {
		a.Audit.LoginFailed(len(creds.Username), "disabled", request)
		return nil, ErrUserDisabled
	}
	session, err := models.CreateSession(ctx, a.DB, models.NewSession{
		UserID: user.ID, UserAgent: request.UserAgent, IPAddress: request.IP,
	})
	if err != nil {
		return nil, DatabaseError(err)
	}
	a.Audit.SessionCreated(audit.CredentialCreated{
		Token: session.SessionToken, UserID: user.ID, Method: "password", Request: request,
	})
	http.SetCookie(w, a.cookies.Session(session.SessionToken))
	// The pre-login CSRF token no longer verifies against the new session.
	http.SetCookie(w, a.cookies.CSRF(session.SessionToken))
	return user, nil
}

// checkPassword fails with the same error and cost for an unknown user and a
// wrong password, to avoid user enumeration.
func (a *App) checkPassword(user *models.User, creds credentials, request audit.Request) error {
	if user == nil {
		auth.VerifyDummyPassword(creds.Password)
		a.Audit.LoginFailed(len(creds.Username), "unknown_user", request)
		return ErrInvalidCredentials
	}
	if !auth.VerifyPassword(creds.Password, user.PasswordHash) {
		a.Audit.LoginFailed(len(creds.Username), "bad_password", request)
		return ErrInvalidCredentials
	}
	return nil
}

// loginForm is POST /login, the no-JS variant of loginAPI: a failure
// re-renders the form (200) with the same generic message.
func (a *App) loginForm(w http.ResponseWriter, r *http.Request) {
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "username", "password") {
		return
	}
	next := loginNext(form.Get("next"))
	creds := credentials{Username: form.Get("username"), Password: form.Get("password")}
	if _, err := a.performLogin(w, r, creds, "POST /login"); err != nil {
		writeHTML(w, http.StatusOK, a.loginHTML(r, loginFormMessage(err), next, nil))
		return
	}
	if next == "" {
		next = "/"
	}
	redirect(w, next)
}

// loginFormMessage is deliberately uninformative and matches the JSON
// endpoint's wording.
func loginFormMessage(err error) string {
	var appErr *AppError
	switch {
	case !errors.As(err, &appErr):
		return "Invalid credentials"
	case appErr.RetryAfter > 0:
		return tooManyAttempts(appErr.RetryAfter)
	case errors.Is(appErr, ErrForbidden):
		return "Password sign-in is disabled on this instance."
	case errors.Is(appErr, ErrUserDisabled):
		return "This account is disabled."
	}
	return "Invalid credentials"
}

func (a *App) loginHTML(r *http.Request, message, next string, banners []flash.Message) string {
	return views.Login(views.LoginPage{
		GitVersion:       gitVersion(),
		SetupAvailable:   a.setupAvailable(r.Context()),
		FlashMessages:    banners,
		LocalAuthEnabled: !a.Config.DisableLocalAuth,
		CSRFToken:        a.cookies.CSRFToken(r),
		Error:            message,
		Next:             next,
	})
}

// loginPage is GET /login.
func (a *App) loginPage(w http.ResponseWriter, r *http.Request) {
	// Not "next": a signed-in user sent here was refused a page (an admin one).
	if a.Auth.AuthenticatePage(r) != nil {
		redirect(w, "/")
		return
	}
	next := loginNext(r.URL.Query().Get("next"))
	writeHTML(w, http.StatusOK, a.loginHTML(r, "", next, flash.Take(w, r)))
}

// --- reauth ---

// reauthenticate is POST /api/session/reauth: re-prove credentials, restarting
// the recent-authentication window. It shares the PasswordChange budget so a
// hijacked session can neither brute-force the password here nor sidestep
// that limit.
func (a *App) reauthenticate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		// Absent for a forward-auth session, which has no password to give.
		Password string `json:"password"`
	}
	if !decodeJSON(w, r, &body) {
		return
	}
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	if err := a.confirmCredentials(r, principal, body.Password); err != nil {
		writeError(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (a *App) confirmCredentials(r *http.Request, principal *auth.Principal, password string) error {
	ctx := r.Context()
	// The proxy re-asserts forward-auth identity per request; nothing to check.
	if principal.ViaForwardAuth {
		return a.markAuthenticated(ctx, principal, "forward_auth")
	}
	if a.Config.DisableLocalAuth {
		return ErrForbidden
	}
	ip := a.clientIP(r)
	if decision := a.Limiter.TryAcquire(auth.BucketPasswordChange, ip); !decision.Allowed {
		a.Audit.LoginRateLimited("POST /api/session/reauth", "reauth", ip.String())
		return TooManyRequests(decision.RetryAfterSecs)
	}
	if !auth.VerifyPassword(password, principal.User.PasswordHash) {
		return ErrInvalidCredentials
	}
	// Correct password: refund the reservation.
	a.Limiter.Release(auth.BucketPasswordChange, ip)
	return a.markAuthenticated(ctx, principal, "password")
}

func (a *App) markAuthenticated(ctx context.Context, principal *auth.Principal, method string) error {
	if _, err := models.MarkAuthenticated(ctx, a.DB, principal.Session.ID); err != nil {
		return DatabaseError(err)
	}
	a.Audit.SessionReauthenticated(principal.Session.SessionToken, principal.User.ID, method)
	return nil
}

// --- logout ---

// logoutClearSiteData clears the sidebar mirror in sessionStorage. It omits
// "cookies" (would race the flash cookie) and "executionContexts" (would force
// a reload fighting the redirect).
const logoutClearSiteData = `"cache", "storage"`

type logoutResult struct {
	RedirectTo string `json:"redirect_to"`
	// ViaForwardAuth is whether the trusted identity header is on the request.
	ViaForwardAuth bool `json:"via_forward_auth"`
	// LogoutURLConfigured is whether RedirectTo is the proxy's logout URL.
	LogoutURLConfigured bool `json:"logout_url_configured"`
}

// logoutAPI is DELETE /api/session.
func (a *App) logoutAPI(w http.ResponseWriter, r *http.Request) {
	result, err := a.destroySession(w, r)
	if err != nil {
		writeError(w, err)
		return
	}
	// Flash cookies are signed, so only the server can mint one.
	flash.Set(w, loggedOutFlash(result))
	writeJSON(w, http.StatusOK, result)
}

// logoutForm is POST /logout: forms cannot send DELETE.
func (a *App) logoutForm(w http.ResponseWriter, r *http.Request) {
	result, err := a.destroySession(w, r)
	if err != nil {
		writeError(w, err)
		return
	}
	flash.Set(w, loggedOutFlash(result))
	redirect(w, result.RedirectTo)
}

// loggedOutFlash warns instead of claiming success under forward-auth with no
// logout URL, where the proxy signs the reader straight back in.
func loggedOutFlash(result logoutResult) flash.Message {
	if result.ViaForwardAuth && !result.LogoutURLConfigured {
		return flash.NewWarning("You are signed in via your reverse proxy. To end your session, log out at your proxy or SSO provider.")
	}
	return flash.NewInfo("You have been logged out.")
}

// destroySession clears the local session and every cookie it could be
// carried under. An already-expired session is not an error: the cookies and
// storage still need clearing.
func (a *App) destroySession(w http.ResponseWriter, r *http.Request) (logoutResult, error) {
	viaForwardAuth, err := a.deleteSession(r)
	if err != nil {
		return logoutResult{}, err
	}
	w.Header().Set("Clear-Site-Data", logoutClearSiteData)
	a.clearAuthCookies(w, r)

	result := logoutResult{
		RedirectTo:          "/login",
		ViaForwardAuth:      viaForwardAuth,
		LogoutURLConfigured: a.Config.AuthProxyLogoutURL != "",
	}
	if result.LogoutURLConfigured {
		result.RedirectTo = a.Config.AuthProxyLogoutURL
	}
	return result, nil
}

// deleteSession reports whether the trusted proxy header is on the request.
func (a *App) deleteSession(r *http.Request) (bool, error) {
	principal, err := a.Auth.Authenticate(r)
	if errors.Is(err, auth.ErrUnauthorized) {
		// Nothing to destroy; the proxy header still decides the banner.
		return auth.ForwardAuthIdentity(a.Config, r) != "", nil
	}
	if err != nil {
		return false, authError(err)
	}
	token := principal.Session.SessionToken
	if err := models.DeleteSession(r.Context(), a.DB, token); err != nil {
		return false, DatabaseError(err)
	}
	a.Audit.SessionDestroyed(token, principal.User.ID, "logout")
	return principal.ViaForwardAuth, nil
}

// clearAuthCookies removes the session and CSRF cookies under both names. The
// plain removals are sent only when the request carried that name, as the
// __Host- ones always are: the browser ignores them without Secure, and the
// server cannot know whether an earlier configuration wrote them.
func (a *App) clearAuthCookies(w http.ResponseWriter, r *http.Request) {
	for _, removal := range append(a.cookies.RemoveSession(), a.cookies.RemoveCSRF()...) {
		if _, carried := r.Cookie(removal.Name); !removal.Secure && carried != nil {
			continue
		}
		http.SetCookie(w, removal)
	}
}
