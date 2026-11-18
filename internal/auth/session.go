package auth

import (
	"context"
	"errors"
	"net/http"
	"sync/atomic"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
)

// Why a request is not authenticated. Handlers map them to their status.
var (
	ErrUnauthorized   = errors.New("unauthorized")
	ErrUserDisabled   = errors.New("user is disabled")
	ErrForbidden      = errors.New("forbidden")
	ErrReauthRequired = errors.New("reauthentication required")
)

// Principal is who a request acts as.
type Principal struct {
	User    models.User
	Session models.Session
	// ViaForwardAuth is true when the trusted proxy asserted this identity on
	// this very request; such sessions are exempt from password reconfirmation.
	ViaForwardAuth bool
	// CSRFToken is for no-JS forms. It derives from the cookie token, not
	// Session.SessionToken, which differs during a rotation's grace interval.
	CSRFToken string
}

// Authenticator resolves the session cookie of a request into a Principal.
type Authenticator struct {
	DB     db.Executor
	Config config.Config
	Audit  *audit.Logger
}

// Cookies are the deployment's session and CSRF cookie builders.
func (a *Authenticator) Cookies() Cookies {
	return Cookies{Secret: a.Config.Secret, Secure: a.Config.CookieSecure}
}

// RotationSlot lets Authenticate ask the slide-session layer to rotate the
// session token. Rotation is deferred to that layer because only it knows the
// response can carry the new cookie; an undelivered rotation would sign the
// client out.
type RotationSlot struct{ requested atomic.Bool }

type rotationKey struct{}

// WithRotationSlot returns a context carrying a fresh slot.
func WithRotationSlot(ctx context.Context) (context.Context, *RotationSlot) {
	slot := &RotationSlot{}
	return context.WithValue(ctx, rotationKey{}, slot), slot
}

// RequestRotation asks for a rotation on the way out. It does nothing on
// routes outside the slide-session layer (such as /events).
func RequestRotation(ctx context.Context) {
	if slot, ok := ctx.Value(rotationKey{}).(*RotationSlot); ok {
		slot.requested.Store(true)
	}
}

// Requested reports whether a rotation was asked for.
func (s *RotationSlot) Requested() bool { return s.requested.Load() }

// Authenticate is the strict form for API routes: a store failure is an
// error, not an anonymous request.
func (a *Authenticator) Authenticate(r *http.Request) (*Principal, error) {
	return a.authenticate(r, true)
}

// AuthenticatePage is the lenient form for page routes, which redirect to
// login: any failure means nil.
func (a *Authenticator) AuthenticatePage(r *http.Request) *Principal {
	principal, err := a.authenticate(r, false)
	if err != nil {
		return nil
	}
	return principal
}

func (a *Authenticator) authenticate(r *http.Request, strict bool) (*Principal, error) {
	ctx := r.Context()
	token := a.Cookies().SessionToken(r)
	if token == "" {
		return nil, ErrUnauthorized
	}
	session, err := a.liveSession(r, token, strict)
	if err != nil {
		return nil, err
	}
	// A failed touch is not worth failing the request.
	_ = models.TouchLastSeen(ctx, a.DB, session)

	user, err := models.FindUserByID(ctx, a.DB, session.UserID)
	if err != nil {
		return nil, passOn(strict, err)
	}
	if user == nil {
		return nil, ErrUnauthorized
	}
	if user.IsDisabled() {
		return nil, ErrUserDisabled
	}
	return &Principal{
		User:           *user,
		Session:        *session,
		ViaForwardAuth: ForwardAuthIdentity(a.Config, r) != "",
		CSRFToken:      a.Cookies().CSRFToken(r),
	}, nil
}

// passOn keeps a store error for strict callers; lenient ones see "no session".
func passOn(strict bool, err error) error {
	if strict {
		return err
	}
	return ErrUnauthorized
}

// liveSession loads the session, deletes it when expired and slides its
// expiry, asking for a token rotation when it did.
func (a *Authenticator) liveSession(r *http.Request, token string, strict bool) (*models.Session, error) {
	ctx := r.Context()
	session, err := models.FindSessionByToken(ctx, a.DB, token)
	if err != nil {
		return nil, passOn(strict, err)
	}
	if session == nil {
		return nil, ErrUnauthorized
	}
	if session.IsExpired() {
		if err := models.DeleteSession(ctx, a.DB, token); err != nil && strict {
			return nil, err
		}
		a.Audit.SessionDestroyed(token, session.UserID, "expired")
		return nil, ErrUnauthorized
	}
	renewed, err := models.RefreshSessionIfNeeded(ctx, a.DB, session)
	if err != nil && strict {
		return nil, err
	}
	if err == nil && renewed != nil {
		a.Audit.SessionRenewed(token, session.UserID, *renewed)
		session.ExpiresAt = *renewed
		// The token is due for rotation too; the slide-session layer does it.
		RequestRotation(ctx)
	}
	return session, nil
}

// Admin authenticates an administrator. While masquerading, the admin is the
// original user, not the impersonated one.
func (a *Authenticator) Admin(r *http.Request) (*Principal, error) {
	principal, err := a.Authenticate(r)
	if err != nil {
		return nil, err
	}
	ok, err := a.isAdminPrincipal(r.Context(), principal)
	if err != nil {
		return nil, err
	}
	if !ok {
		return nil, ErrForbidden
	}
	return principal, nil
}

// AdminPage is Admin for page routes: anything short of an admin is nil.
func (a *Authenticator) AdminPage(r *http.Request) *Principal {
	principal := a.AuthenticatePage(r)
	if principal == nil {
		return nil
	}
	ok, err := a.isAdminPrincipal(r.Context(), principal)
	if err != nil || !ok {
		return nil
	}
	return principal
}

func (a *Authenticator) isAdminPrincipal(ctx context.Context, principal *Principal) (bool, error) {
	if !principal.Session.IsMasquerading() {
		return principal.User.IsAdmin(), nil
	}
	original, err := models.FindUserByID(ctx, a.DB, *principal.Session.OriginalUserID)
	if err != nil {
		return false, err
	}
	if original == nil {
		return false, ErrUnauthorized
	}
	return original.IsAdmin(), nil
}

// RecentlyAuthenticated authenticates a request whose credentials were proved
// within models.ReauthWindow (OWASP reauthentication), guarding passkey
// add and remove so a hijacked session cannot mint a credential that survives
// a password change. Forward-auth sessions are exempt: the proxy asserts the
// identity per request, and the account may have no password.
func (a *Authenticator) RecentlyAuthenticated(r *http.Request) (*Principal, error) {
	principal, err := a.Authenticate(r)
	if err != nil {
		return nil, err
	}
	if !principal.ViaForwardAuth && !principal.Session.AuthenticatedRecently(time.Now()) {
		return nil, ErrReauthRequired
	}
	return principal, nil
}
