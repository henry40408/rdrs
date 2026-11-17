package auth

import (
	"context"
	"log/slog"
	"net/http"
	"strings"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
)

// disabledPasswordHash never verifies, so a proxy-provisioned account has no
// local password login.
const disabledPasswordHash = "!"

// defaultCategoryName is the category every new account starts with.
const defaultCategoryName = "Uncategorized"

// ForwardAuthIdentity is the identity a trusted forward-auth proxy asserted,
// or "" when the feature is off, the peer is not a trusted proxy, or the
// header is empty. The auth checks share it so the trust logic lives once.
func ForwardAuthIdentity(cfg config.Config, r *http.Request) string {
	if !cfg.AuthProxyEnabled() || !cfg.IsTrustedPeer(PeerIP(r)) {
		return ""
	}
	return strings.TrimSpace(headerText(r, cfg.AuthProxyHeader))
}

// ParseGroups splits a comma-separated groups header into trimmed, non-empty
// names.
func ParseGroups(raw string) []string {
	var groups []string
	for _, part := range strings.Split(raw, ",") {
		if name := strings.TrimSpace(part); name != "" {
			groups = append(groups, name)
		}
	}
	return groups
}

// RoleFromGroups maps groups to a role: admin if and only if adminGroup is
// among them.
func RoleFromGroups(groups []string, adminGroup string) models.Role {
	for _, group := range groups {
		if group == adminGroup {
			return models.RoleAdmin
		}
	}
	return models.RoleUser
}

// ForwardLogin turns a proxy-asserted identity into a session.
type ForwardLogin struct {
	DB     *db.DB
	Config config.Config
	Audit  *audit.Logger
}

// SignIn resolves or creates the account named by the proxy and starts a
// session for it. A nil session with a nil error means the account is refused.
func (f ForwardLogin) SignIn(r *http.Request, username string) (*models.Session, error) {
	ctx := r.Context()
	user, err := f.resolveUser(ctx, r, username)
	if err != nil || user == nil {
		return nil, err
	}
	userAgent, ip := UserAgent(r), ClientIP(f.Config, r).String()
	session, err := models.CreateSession(ctx, f.DB, models.NewSession{
		UserID: user.ID, UserAgent: userAgent, IPAddress: ip,
	})
	if err != nil {
		return nil, err
	}
	f.Audit.SessionCreated(audit.CredentialCreated{
		Token: session.SessionToken, UserID: user.ID, Method: "forward_auth",
		Request: audit.Request{IP: ip, UserAgent: userAgent},
	})
	return session, nil
}

func (f ForwardLogin) resolveUser(ctx context.Context, r *http.Request, username string) (*models.User, error) {
	desiredRole := f.desiredRole(r)
	user, err := models.FindUserByUsername(ctx, f.DB, username)
	if err != nil {
		return nil, err
	}
	if user != nil {
		return f.syncExisting(ctx, user, desiredRole)
	}
	if !f.Config.AuthProxyUserCreation {
		return nil, nil
	}
	if _, err := models.ValidateUsername(username); err != nil {
		// Length and reason only: the name is untrusted header input.
		slog.Warn("forward-auth username rejected; not creating an account",
			"event", "forward_auth.rejected", "username_chars", utf8.RuneCountInString(username), "reason", err.Error())
		return nil, nil
	}
	created, err := f.createUser(ctx, username, desiredRole)
	if err != nil {
		return nil, err
	}
	// Self-provisioned by the proxy login, hence actor equals subject (as in setup).
	f.Audit.AccountCreated(created.ID, created.ID, utf8.RuneCountInString(created.Username), string(created.Role))
	return created, nil
}

// desiredRole is the role the groups header asks for, recomputed on every
// login when group mapping is on; nil when it is off.
func (f ForwardLogin) desiredRole(r *http.Request) *models.Role {
	if !f.Config.GroupMappingEnabled() {
		return nil
	}
	role := RoleFromGroups(ParseGroups(headerText(r, f.Config.AuthProxyGroupsHeader)), f.Config.AuthProxyAdminGroup)
	return &role
}

func (f ForwardLogin) syncExisting(ctx context.Context, user *models.User, desiredRole *models.Role) (*models.User, error) {
	if user.IsDisabled() {
		return nil, nil
	}
	if desiredRole != nil && user.Role != *desiredRole {
		if err := models.UpdateRole(ctx, f.DB, user.ID, *desiredRole); err != nil {
			return nil, err
		}
	}
	return user, nil
}

// createUser counts, creates and adds the default category in one transaction,
// so two concurrent first logins cannot both see an empty instance and both
// become admin.
func (f ForwardLogin) createUser(ctx context.Context, username string, desiredRole *models.Role) (*models.User, error) {
	tx, err := f.DB.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer func() { _ = tx.Rollback() }()
	if err := tx.LockUserTable(ctx); err != nil {
		return nil, err
	}
	role, err := roleForNewUser(ctx, tx, desiredRole)
	if err != nil {
		return nil, err
	}
	created, err := models.CreateUser(ctx, tx, username, disabledPasswordHash, role)
	if err != nil {
		return nil, err
	}
	if _, err := models.CreateCategory(ctx, tx, created.ID, defaultCategoryName); err != nil {
		return nil, err
	}
	return created, tx.Commit()
}

func roleForNewUser(ctx context.Context, tx *db.Tx, desiredRole *models.Role) (models.Role, error) {
	if desiredRole != nil {
		return *desiredRole, nil
	}
	count, err := models.CountUsers(ctx, tx)
	if err != nil {
		return "", err
	}
	if count == 0 {
		return models.RoleAdmin, nil
	}
	return models.RoleUser, nil
}
