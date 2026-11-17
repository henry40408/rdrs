package server

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/views"
)

// Redeeming a one-time invite link is anonymous: the token is the authority.
// Every failure (unknown, expired, spent) must therefore render identically
// and reveal no username, or this becomes an account oracle.

type liveInvite struct {
	invite   *models.UserInvite
	username string
}

// resolveInvite is nil for every failure; callers must not tell them apart in
// what they render.
func (a *App) resolveInvite(ctx context.Context, token string) *liveInvite {
	invite, err := models.FindInviteByToken(ctx, a.DB, a.Config.Secret, token)
	if err != nil || invite == nil || !invite.IsLive(time.Now()) {
		return nil
	}
	account, err := models.FindUserByID(ctx, a.DB, invite.UserID)
	if err != nil || account == nil {
		return nil
	}
	return &liveInvite{invite: invite, username: account.Username}
}

func invitePage(r *http.Request, a *App, page views.InvitePage) string {
	page.GitVersion = gitVersion()
	page.PasswordMinLength = auth.PasswordMinLength
	page.PasswordMaxLength = auth.PasswordMaxLength
	return views.Invite(page)
}

// inviteForm renders the form for a live invite, with an optional error.
func (a *App) inviteForm(r *http.Request, token string, live *liveInvite, message string) string {
	return invitePage(r, a, views.InvitePage{
		CSRFToken: a.cookies.CSRFToken(r), Username: live.username, Token: token, Error: message,
	})
}

// inviteDeadEnd is the one response every failing token gets.
func (a *App) inviteDeadEnd(r *http.Request, message string) string {
	return invitePage(r, a, views.InvitePage{Error: message})
}

// invitePage is GET /invite/{token}: the "choose a password" form, or a dead
// end. The username is shown only after the token is accepted.
func (a *App) invitePage(w http.ResponseWriter, r *http.Request) {
	token := r.PathValue("token")
	live := a.resolveInvite(r.Context(), token)
	if live == nil {
		writeHTML(w, http.StatusOK, a.inviteDeadEnd(r, ""))
		return
	}
	writeHTML(w, http.StatusOK, a.inviteForm(r, token, live, ""))
}

// redeemInvite is POST /invite/{token}: set the password and spend the link.
func (a *App) redeemInvite(w http.ResponseWriter, r *http.Request) {
	form, ok := parseForm(w, r)
	if !ok || !requireFields(w, form, "password", "confirm_password") {
		return
	}
	token := r.PathValue("token")
	ctx := r.Context()
	// Throttled before the lookup: this is anonymous and runs zxcvbn and Argon2.
	ip := a.clientIP(r)
	if decision := a.Limiter.TryAcquire(auth.BucketAccountSetup, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "account_setup", "POST /invite", "invite_redeem")
		writeHTML(w, http.StatusOK, a.inviteDeadEnd(r, tooManyAttempts(decision.RetryAfterSecs)))
		return
	}
	live := a.resolveInvite(ctx, token)
	if live == nil {
		writeHTML(w, http.StatusOK, a.inviteDeadEnd(r, ""))
		return
	}
	// Re-rendered on every failure below, so the retry carries a live token.
	retry := func(message string) {
		writeHTML(w, http.StatusOK, a.inviteForm(r, token, live, message))
	}
	password := form.Get("password")
	if password != form.Get("confirm_password") {
		retry("Passwords do not match.")
		return
	}
	if err := auth.ValidatePasswordStrength(password, []string{live.username}); err != nil {
		var validation *models.ValidationError
		if errors.As(err, &validation) {
			retry(validation.Message)
			return
		}
		retry("Could not set the password.")
		return
	}
	hash, err := auth.HashPassword(password)
	if err != nil {
		retry("Could not set the password.")
		return
	}
	spent, err := models.RedeemInvite(ctx, a.DB, models.RedeemInviteParams{
		InviteID: live.invite.ID, UserID: live.invite.UserID, PasswordHash: hash,
	})
	if err != nil {
		slog.Error("invite redemption failed", "event", "invite.redeem_failed", "error", err)
		retry("Could not set the password.")
		return
	}
	if !spent {
		writeHTML(w, http.StatusOK, a.inviteDeadEnd(r, ""))
		return
	}
	a.auditRedeemed(r, token, live.invite.UserID)
	flash.Redirect(w, "/login", flash.NewSuccess("Password set. You can sign in now."))
}

func (a *App) auditRedeemed(r *http.Request, token string, userID int64) {
	a.Audit.SessionsDestroyedBulk(userID, "invite_redeem", nil)
	a.Audit.APITokensDestroyed(userID, "invite_redeem", nil)
	a.Audit.InviteConsumed(audit.InviteConsumed{Token: token, UserID: userID, Request: a.request(r)})
}
