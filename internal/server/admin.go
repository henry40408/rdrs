package server

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/views"
)

const adminLocation = "/admin"

func (a *App) adminRoutes() []Route {
	return []Route{
		{"GET", "/admin", a.adminPage},
		{"POST", "/admin/reauth", a.adminReauth},
		{"POST", "/admin/users", a.adminCreateUser},
		{"POST", "/admin/users/{id}/invite", a.adminReissueInvite},
		{"POST", "/admin/users/{id}/invite/revoke", a.adminRevokeInvite},
		{"POST", "/admin/users/{id}/role", a.adminUpdateRole},
		{"POST", "/admin/users/{id}/status", a.adminUpdateStatus},
		{"POST", "/admin/users/{id}/masquerade", a.adminStartMasquerade},
		{"POST", "/admin/users/{id}/delete", a.adminDeleteUser},
		{"POST", "/api/admin/unmasquerade", a.adminStopMasquerade},
	}
}

func adminFlash(w http.ResponseWriter, message flash.Message) {
	flash.Redirect(w, adminLocation, message)
}

// requireAdmin authenticates an admin form or API call, answering the
// failure itself. An admin who is masquerading is still the admin.
func (a *App) requireAdmin(w http.ResponseWriter, r *http.Request) (*auth.Principal, bool) {
	principal, err := a.Auth.Admin(r)
	if err != nil {
		writeError(w, authError(err))
		return nil, false
	}
	return principal, true
}

// realAdminID is the signed-in human: the original admin while masquerading.
func realAdminID(principal *auth.Principal) int64 {
	if principal.Session.OriginalUserID != nil {
		return *principal.Session.OriginalUserID
	}
	return principal.User.ID
}

// needsRecentAuthentication mirrors the page's reauth banner: a redirect, not
// an API error, so the forms work without script. Forward-auth sessions are
// exempt, and so is stopping a masquerade (it would ask for the impersonated
// user's password).
func needsRecentAuthentication(principal *auth.Principal) bool {
	return !principal.ViaForwardAuth && !principal.Session.AuthenticatedRecently(time.Now())
}

// adminMutation authenticates an admin POST and demands a recent password
// confirmation, answering the refusal itself.
func (a *App) adminMutation(w http.ResponseWriter, r *http.Request) (*auth.Principal, bool) {
	principal, ok := a.requireAdmin(w, r)
	if !ok {
		return nil, false
	}
	if needsRecentAuthentication(principal) {
		adminFlash(w, flash.NewError("Confirm your password before changing accounts, then try again."))
		return nil, false
	}
	return principal, true
}

// rotatedCookies reissues the session and CSRF cookies after a token
// rotation; the CSRF token derives from the session token.
func (a *App) rotatedCookies(w http.ResponseWriter, newToken string) {
	http.SetCookie(w, a.cookies.Session(newToken))
	http.SetCookie(w, a.cookies.CSRF(newToken))
}

// adminStopMasquerade is POST /api/admin/unmasquerade.
func (a *App) adminStopMasquerade(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireAdmin(w, r)
	if !ok {
		return
	}
	if !principal.Session.IsMasquerading() {
		writeError(w, ErrNotMasquerading)
		return
	}
	token := principal.Session.SessionToken
	adminID := realAdminID(principal)
	newToken, err := models.StopMasquerade(r.Context(), a.DB, token)
	if err != nil {
		writeError(w, appError(err))
		return
	}
	a.Audit.MasqueradeStopped(audit.MasqueradeStopped{
		Token: token, NewToken: newToken, ActorUserID: adminID, RestoredUserID: adminID,
	})
	a.rotatedCookies(w, newToken)
	w.WriteHeader(http.StatusOK)
}

// adminReauth is POST /admin/reauth, the no-JS twin of /api/session/reauth.
// It shares the PasswordChange budget so it cannot brute-force the password.
func (a *App) adminReauth(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireAdmin(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "password") {
		return
	}
	adminFlash(w, a.confirmAdminPassword(r, principal, form.Get("password")))
}

// confirmAdminPassword answers the flash of a password confirmation.
func (a *App) confirmAdminPassword(r *http.Request, principal *auth.Principal, password string) flash.Message {
	if principal.ViaForwardAuth {
		return flash.NewSuccess("Confirmed.")
	}
	if a.Config.DisableLocalAuth {
		return flash.NewError("Password confirmation is disabled.")
	}
	ip := a.clientIP(r)
	if decision := a.Limiter.TryAcquire(auth.BucketPasswordChange, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "password_change", "POST /admin/reauth", "reauth")
		return flash.NewError(tooManyAttempts(decision.RetryAfterSecs))
	}
	// The real admin's password, not the impersonated user's.
	actorID := realAdminID(principal)
	actor, err := models.FindUserByID(r.Context(), a.DB, actorID)
	if err != nil || actor == nil {
		return flash.NewError("Password confirmation failed.")
	}
	if !auth.VerifyPassword(password, actor.PasswordHash) {
		return flash.NewError("Incorrect password.")
	}
	// Success must not consume the rate-limit budget.
	a.Limiter.Release(auth.BucketPasswordChange, ip)
	if _, err := models.MarkAuthenticated(r.Context(), a.DB, principal.Session.ID); err != nil {
		return flash.NewError("Password confirmation failed.")
	}
	a.Audit.SessionReauthenticated(principal.Session.SessionToken, actorID, "password")
	return flash.NewSuccess("Confirmed. You can now change accounts.")
}

// noPasswordHash marks an account that never set a password; it never parses
// as a PHC string, so no password verifies.
const noPasswordHash = "!"

// inviteURL is absolute when RDRS_PUBLIC_BASE_URL is set, else the bare path.
func (a *App) inviteURL(path string) string {
	if a.Config.PublicBaseURL == "" {
		return path
	}
	return strings.TrimRight(a.Config.PublicBaseURL, "/") + path
}

// announceInvite audits a committed invite and returns its path.
func (a *App) announceInvite(token string, invite *models.UserInvite, actorID int64, reason string) string {
	a.Audit.InviteIssued(audit.InviteIssued{
		Token: token, UserID: invite.UserID, ActorUserID: actorID, Reason: reason, ExpiresAt: invite.ExpiresAt,
	})
	return "/invite/" + token
}

// issueInvite mints a link for userID. Only an HMAC is stored, so the raw
// token is unrecoverable once this returns.
func (a *App) issueInvite(ctx context.Context, userID, actorID int64, reason string) (string, error) {
	token := models.GenerateToken()
	invite, err := models.IssueInvite(ctx, a.DB, models.IssueInviteParams{SecretKey: a.Config.Secret, UserID: userID, Token: token})
	if err != nil {
		return "", err
	}
	return a.announceInvite(token, invite, actorID, reason), nil
}

// createAccountWithInvite makes the account, its default category and its
// first invite in one transaction, so a failure never leaves a half-made
// account holding the username.
func (a *App) createAccountWithInvite(ctx context.Context, username string, role models.Role, actorID int64) (string, error) {
	token := models.GenerateToken()
	tx, err := a.DB.Begin(ctx)
	if err != nil {
		return "", err
	}
	defer func() { _ = tx.Rollback() }()
	created, err := models.CreateUser(ctx, tx, username, noPasswordHash, role)
	if err != nil {
		return "", err
	}
	if _, err := models.CreateCategory(ctx, tx, created.ID, defaultCategoryName); err != nil {
		return "", err
	}
	invite, err := models.IssueInviteTx(ctx, tx, models.IssueInviteParams{SecretKey: a.Config.Secret, UserID: created.ID, Token: token})
	if err != nil {
		return "", err
	}
	if err := tx.Commit(); err != nil {
		return "", err
	}
	a.Audit.AccountCreated(created.ID, actorID, len([]rune(username)), string(role))
	return a.announceInvite(token, invite, actorID, "account_created"), nil
}

// formReader reads typed form fields the way serde does; fail answers 422
// with the first unusable value.
type formReader struct {
	queryReader
}

func newFormReader(form url.Values) *formReader {
	return &formReader{queryReader{values: form}}
}

// failed answers 422 and reports true when a field was unusable.
func (f *formReader) failed(w http.ResponseWriter) bool {
	if f.err == nil {
		return false
	}
	rejection(w, http.StatusUnprocessableEntity, "Failed to deserialize form body: "+f.err.Error())
	return true
}

// role reads a required role field: "admin" or "user".
func (f *formReader) role(name string) models.Role {
	value, ok := f.lookup(name)
	switch {
	case !ok:
		f.err = errors.New("missing field `" + name + "`")
	case value == string(models.RoleAdmin) || value == string(models.RoleUser):
		return models.Role(value)
	default:
		f.err = errors.New(name + ": unknown variant `" + value + "`, expected `admin` or `user`")
	}
	return ""
}

// required is flag for a field that must be present.
func (f *formReader) required(name string) bool {
	if _, ok := f.lookup(name); !ok && f.err == nil {
		f.err = errors.New("missing field `" + name + "`")
		return false
	}
	return f.flag(name)
}

// adminCreateUser is POST /admin/users: create an account and issue its first
// invite. The account is unusable until the invite is redeemed.
func (a *App) adminCreateUser(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.adminMutation(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	reader := newFormReader(form)
	rawName, role := reader.text("username"), reader.role("role")
	if rawName == nil && reader.err == nil {
		reader.err = errors.New("missing field `username`")
	}
	if reader.failed(w) {
		return
	}
	username, err := models.ValidateUsername(*rawName)
	if err != nil {
		var validation *models.ValidationError
		if errors.As(err, &validation) {
			adminFlash(w, flash.NewError(validation.Message+"."))
			return
		}
		adminFlash(w, flash.NewError("Invalid username."))
		return
	}
	path, err := a.createInvitedAccount(r.Context(), username, role, realAdminID(principal))
	switch {
	case errors.Is(err, models.ErrUsernameExists):
		adminFlash(w, flash.NewError("That username is already taken."))
	case errors.Is(err, ErrRegistrationNotAllowed):
		adminFlash(w, flash.NewError("This instance is single-user. Set RDRS_MULTI_USER_ENABLED=true to add accounts."))
	case err != nil:
		slog.Error("account creation failed", "error", err)
		adminFlash(w, flash.NewError("Failed to create the account."))
	default:
		adminFlash(w, flash.NewInviteLink(a.inviteURL(path)))
	}
}

func (a *App) createInvitedAccount(ctx context.Context, username string, role models.Role, actorID int64) (string, error) {
	count, err := models.CountUsers(ctx, a.DB)
	if err != nil {
		return "", err
	}
	if !a.Config.CanCreateAccount(count) {
		return "", ErrRegistrationNotAllowed
	}
	return a.createAccountWithInvite(ctx, username, role, actorID)
}

// adminReissueInvite is POST /admin/users/{id}/invite: a fresh link that
// revokes any current one. It is also the password-reset path; the old
// password works until the link is redeemed.
func (a *App) adminReissueInvite(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.adminMutation(w, r)
	if !ok {
		return
	}
	userID, ok := pathID(w, r)
	if !ok {
		return
	}
	path, err := a.reissueInvite(r.Context(), userID, realAdminID(principal))
	if err != nil {
		adminFlash(w, flash.NewError("Failed to issue a link."))
		return
	}
	adminFlash(w, flash.NewInviteLink(a.inviteURL(path)))
}

func (a *App) reissueInvite(ctx context.Context, userID, actorID int64) (string, error) {
	target, err := models.FindUserByID(ctx, a.DB, userID)
	if err != nil {
		return "", err
	}
	if target == nil {
		return "", ErrUserNotFound
	}
	reason := "password_reset"
	if target.PasswordHash == noPasswordHash {
		reason = "account_created"
	}
	return a.issueInvite(ctx, userID, actorID, reason)
}

// adminRevokeInvite is POST /admin/users/{id}/invite/revoke.
func (a *App) adminRevokeInvite(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.adminMutation(w, r)
	if !ok {
		return
	}
	userID, ok := pathID(w, r)
	if !ok {
		return
	}
	revoked, err := models.RevokeInvitesForUser(r.Context(), a.DB, userID)
	switch {
	case err != nil:
		adminFlash(w, flash.NewError("Failed to revoke the link."))
	case revoked == 0:
		adminFlash(w, flash.NewError("There is no outstanding link for that account."))
	default:
		a.Audit.InviteRevoked(userID, realAdminID(principal))
		adminFlash(w, flash.NewSuccess("Link revoked."))
	}
}

// adminTarget authenticates an admin POST aimed at another account, reading
// the {id}. It refuses the admin's own account with selfMessage.
func (a *App) adminTarget(w http.ResponseWriter, r *http.Request, selfMessage string) (targetID int64, ok bool) {
	principal, ok := a.adminMutation(w, r)
	if !ok {
		return 0, false
	}
	targetID, ok = pathID(w, r)
	if !ok {
		return 0, false
	}
	if targetID == realAdminID(principal) {
		adminFlash(w, flash.NewError(selfMessage))
		return 0, false
	}
	return targetID, true
}

// adminUpdateRole is POST /admin/users/{id}/role.
func (a *App) adminUpdateRole(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.adminMutation(w, r)
	if !ok {
		return
	}
	userID, ok := pathID(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	reader := newFormReader(form)
	role := reader.role("role")
	if reader.failed(w) {
		return
	}
	if userID == realAdminID(principal) {
		adminFlash(w, flash.NewError("You cannot modify your own role."))
		return
	}
	if err := a.setRole(r.Context(), userID, role); err != nil {
		adminFlash(w, flash.NewError("Failed to update role."))
		return
	}
	adminFlash(w, flash.NewSuccess("Role updated to "+string(role)+"."))
}

func (a *App) setRole(ctx context.Context, userID int64, role models.Role) error {
	target, err := models.FindUserByID(ctx, a.DB, userID)
	if err != nil {
		return err
	}
	if target == nil {
		return ErrUserNotFound
	}
	if target.Role == role {
		return nil
	}
	return models.UpdateRole(ctx, a.DB, userID, role)
}

// adminUpdateStatus is POST /admin/users/{id}/status.
func (a *App) adminUpdateStatus(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.adminMutation(w, r)
	if !ok {
		return
	}
	userID, ok := pathID(w, r)
	if !ok {
		return
	}
	form, ok := parseForm(w, r)
	if !ok {
		return
	}
	reader := newFormReader(form)
	disabled := reader.required("disabled")
	if reader.failed(w) {
		return
	}
	if userID == realAdminID(principal) {
		adminFlash(w, flash.NewError("You cannot modify your own status."))
		return
	}
	if err := a.setDisabled(r.Context(), userID, disabled); err != nil {
		adminFlash(w, flash.NewError("Failed to update user status."))
		return
	}
	if disabled {
		adminFlash(w, flash.NewSuccess("User disabled."))
		return
	}
	adminFlash(w, flash.NewSuccess("User enabled."))
}

func (a *App) setDisabled(ctx context.Context, userID int64, disabled bool) error {
	target, err := models.FindUserByID(ctx, a.DB, userID)
	if err != nil {
		return err
	}
	if target == nil {
		return ErrUserNotFound
	}
	switch {
	case disabled && !target.IsDisabled():
		if err := models.DisableUser(ctx, a.DB, userID); err != nil {
			return err
		}
		a.Audit.SessionsDestroyedBulk(userID, "admin_disable", nil)
		a.Audit.APITokensDestroyed(userID, "admin_disable", nil)
	case !disabled && target.IsDisabled():
		return models.EnableUser(ctx, a.DB, userID)
	}
	return nil
}

// adminStartMasquerade is POST /admin/users/{id}/masquerade.
func (a *App) adminStartMasquerade(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.adminMutation(w, r)
	if !ok {
		return
	}
	targetID, ok := pathID(w, r)
	if !ok {
		return
	}
	if principal.Session.IsMasquerading() {
		adminFlash(w, flash.NewError("You are already masquerading as another user."))
		return
	}
	token := principal.Session.SessionToken
	newToken, err := a.startMasquerade(r.Context(), token, targetID)
	if err != nil {
		adminFlash(w, flash.NewError("Failed to start masquerade."))
		return
	}
	request := a.request(r)
	a.Audit.MasqueradeStarted(audit.MasqueradeStarted{
		Token: token, NewToken: newToken, ActorUserID: principal.User.ID, TargetUserID: targetID, Request: request,
	})
	a.rotatedCookies(w, newToken)
	flash.Redirect(w, "/", flash.NewInfo("You are now masquerading as another user."))
}

func (a *App) startMasquerade(ctx context.Context, token string, targetID int64) (string, error) {
	target, err := models.FindUserByID(ctx, a.DB, targetID)
	if err != nil {
		return "", err
	}
	if target == nil {
		return "", ErrUserNotFound
	}
	if target.IsDisabled() {
		return "", ErrUserDisabled
	}
	return models.StartMasquerade(ctx, a.DB, token, targetID)
}

// adminDeleteUser is POST /admin/users/{id}/delete.
func (a *App) adminDeleteUser(w http.ResponseWriter, r *http.Request) {
	userID, ok := a.adminTarget(w, r, "You cannot delete your own account.")
	if !ok {
		return
	}
	if err := models.DeleteUser(r.Context(), a.DB, userID); err != nil {
		adminFlash(w, flash.NewError("Failed to delete user."))
		return
	}
	a.Sidebar.Bust(userID)
	adminFlash(w, flash.NewSuccess("User deleted."))
}

// --- page ---

// adminPage is GET /admin. Anyone but an admin is sent to the login page.
func (a *App) adminPage(w http.ResponseWriter, r *http.Request) {
	principal := a.Auth.AdminPage(r)
	if principal == nil {
		flash.Redirect(w, "/login", flash.NewWarning("Please log in to continue."))
		return
	}
	banners := flash.Take(w, r)
	inviteLink := takeInviteLink(&banners)
	layout := a.buildLayout(r, principal, banners)
	ctx := r.Context()
	count, err := models.CountUsers(ctx, a.DB)
	canCreate := err == nil && a.Config.CanCreateAccount(count)
	page := views.AdminPage{
		Layout:           &layout,
		CSRFToken:        principal.CSRFToken,
		Users:            a.adminUserRows(ctx, principal),
		InviteLink:       inviteLink,
		CanCreateAccount: canCreate,
		NeedsReauth:      needsRecentAuthentication(principal) && !a.Config.DisableLocalAuth,
	}
	writeHTML(w, http.StatusOK, views.Admin(page))
}

// takeInviteLink pulls the one-time link out of the banners. The banner is
// replaced, so the page still confirms the action without echoing the link
// through the flash area.
func takeInviteLink(banners *[]flash.Message) string {
	for _, banner := range *banners {
		if banner.Kind == flash.InviteLinkKind {
			*banners = []flash.Message{flash.NewSuccess("Account link ready.")}
			return banner.Text
		}
	}
	return ""
}

func (a *App) adminUserRows(ctx context.Context, principal *auth.Principal) []views.AdminUserRow {
	users, err := models.ListUsers(ctx, a.DB)
	if err != nil {
		slog.Error("admin user list failed", "error", err)
		return nil
	}
	rows := make([]views.AdminUserRow, 0, len(users))
	for _, user := range users {
		rows = append(rows, views.AdminUserRow{
			ID: user.ID, Username: user.Username, Role: string(user.Role), Disabled: user.IsDisabled(),
			CreatedAt: user.CreatedAt.UTC().Format("2006-01-02"), CreatedAtISO: chronoRFC3339(user.CreatedAt),
			IsSelf:           user.ID == principal.User.ID || user.ID == realAdminID(principal),
			AwaitingPassword: user.PasswordHash == noPasswordHash,
			InviteExpiresAt:  a.inviteExpiry(ctx, user.ID),
		})
	}
	return rows
}

// inviteExpiry is when the outstanding link of userID expires, or "" for none.
// A lookup per row is fine: self-hosted instances have few accounts.
func (a *App) inviteExpiry(ctx context.Context, userID int64) string {
	invite, err := models.FindLiveInviteForUser(ctx, a.DB, userID)
	if err != nil || invite == nil {
		return ""
	}
	return invite.ExpiresAt.UTC().Format("2006-01-02 15:04")
}
