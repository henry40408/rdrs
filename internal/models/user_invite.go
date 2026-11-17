package models

import (
	"context"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/secret"
)

// InviteTTL is how long an unused link stays usable: it survives a weekend,
// but a link leaked into a log stops being a credential soon.
const InviteTTL = 7 * 24 * time.Hour

// UserInvite is a one-time link that lets an admin-created account set its own
// password. It also backs admin password resets, where the current password
// must keep working until the link is redeemed.
type UserInvite struct {
	ID     int64
	UserID int64
	// TokenHash is the HMAC of the token, never the token itself.
	TokenHash string
	CreatedAt time.Time
	ExpiresAt time.Time
	// ConsumedAt is when the link was spent; nil while it is still live.
	ConsumedAt *time.Time
}

const inviteColumns = "id, user_id, token_hash, created_at, expires_at, consumed_at"

func scanInvite(row scanner) (UserInvite, error) {
	var i UserInvite
	err := row.Scan(&i.ID, &i.UserID, &i.TokenHash,
		db.TimeDest(&i.CreatedAt), db.TimeDest(&i.ExpiresAt), db.NullTimeDest(&i.ConsumedAt))
	return i, err
}

// IsLive reports whether the invite can be redeemed now. Redemption re-checks
// it because a link can expire between form render and submit.
func (i *UserInvite) IsLive(now time.Time) bool {
	return i.ConsumedAt == nil && now.Before(i.ExpiresAt)
}

// IssueInvite issues a link for userID, revoking any outstanding one so only
// one link is ever live. The caller holds the only copy of the raw token.
func IssueInvite(ctx context.Context, database *db.DB, secretKey []byte, userID int64, token string) (*UserInvite, error) {
	tx, err := database.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer func() { _ = tx.Rollback() }()
	invite, err := IssueInviteTx(ctx, tx, secretKey, userID, token)
	if err != nil {
		return nil, err
	}
	return invite, tx.Commit()
}

// IssueInviteTx is IssueInvite inside a caller-owned transaction. The revoke
// and the insert stand or fall together: a failed insert must not cost the
// account its old link.
func IssueInviteTx(ctx context.Context, e db.Executor, secretKey []byte, userID int64, token string) (*UserInvite, error) {
	if _, err := e.ExecContext(ctx, "DELETE FROM user_invite WHERE user_id = $1", userID); err != nil {
		return nil, err
	}
	now := time.Now()
	row := e.QueryRowContext(ctx,
		`INSERT INTO user_invite (user_id, token_hash, created_at, expires_at)
		 VALUES ($1, $2, $3, $4) RETURNING `+inviteColumns,
		userID, secret.InviteHash(secretKey, token), stamp(now), stamp(now.Add(InviteTTL)))
	invite, err := scanInvite(row)
	if err != nil {
		return nil, err
	}
	return &invite, nil
}

// FindInviteByToken looks a token up by its hashed form, live or not. The
// caller must answer identically for unknown, expired and spent links.
func FindInviteByToken(ctx context.Context, e db.Executor, secretKey []byte, token string) (*UserInvite, error) {
	row := e.QueryRowContext(ctx,
		`SELECT `+inviteColumns+` FROM user_invite WHERE token_hash = $1`,
		secret.InviteHash(secretKey, token))
	invite, err := scanInvite(row)
	return orNil(&invite, err)
}

// consumeInvite spends the invite and reports whether this caller spent it.
// "consumed_at IS NULL" makes racing submissions single-winner.
func consumeInvite(ctx context.Context, e db.Executor, inviteID int64) (bool, error) {
	updated, err := rowsAffected(e.ExecContext(ctx,
		"UPDATE user_invite SET consumed_at = $1 WHERE id = $2 AND consumed_at IS NULL",
		stamp(time.Now()), inviteID))
	return updated == 1, err
}

// RedeemInvite spends the invite and sets the invitee's password in one
// transaction, reporting whether this caller spent it. Spending first keeps a
// racing double-submit single-winner; on false nothing changed. Prior sessions
// and API tokens are revoked with the password, as admin resets land here too.
func RedeemInvite(ctx context.Context, database *db.DB, inviteID, userID int64, passwordHash string) (bool, error) {
	tx, err := database.Begin(ctx)
	if err != nil {
		return false, err
	}
	defer func() { _ = tx.Rollback() }()
	spent, err := consumeInvite(ctx, tx, inviteID)
	if err != nil || !spent {
		return false, err
	}
	if err := ChangePasswordTx(ctx, tx, userID, passwordHash); err != nil {
		return false, err
	}
	return true, tx.Commit()
}

// RevokeInvitesForUser deletes (not consumes) any outstanding link for userID;
// the audit log records it.
func RevokeInvitesForUser(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	return rowsAffected(e.ExecContext(ctx, "DELETE FROM user_invite WHERE user_id = $1", userID))
}

// FindLiveInviteForUser is the live invite for userID, if any: what /admin
// shows as "pending".
func FindLiveInviteForUser(ctx context.Context, e db.Executor, userID int64) (*UserInvite, error) {
	row := e.QueryRowContext(ctx, `SELECT `+inviteColumns+` FROM user_invite WHERE user_id = $1`, userID)
	invite, err := scanInvite(row)
	found, err := orNil(&invite, err)
	if err != nil || found == nil || !found.IsLive(time.Now()) {
		return nil, err
	}
	return found, nil
}

// DeleteStaleInvites deletes spent or expired invites. It is housekeeping only;
// IsLive enforces expiry.
func DeleteStaleInvites(ctx context.Context, e db.Executor) (int64, error) {
	return rowsAffected(e.ExecContext(ctx,
		"DELETE FROM user_invite WHERE consumed_at IS NOT NULL OR expires_at <= $1", stamp(time.Now())))
}
