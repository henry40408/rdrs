package models

import (
	"context"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// GReader ClientLogin credentials, deliberately independent of sessions (own
// expiry and revocation).
const (
	// APITokenIdleDays is a sliding idle TTL with deliberately no absolute cap,
	// unlike web sessions: GReader clients cannot re-authenticate interactively.
	APITokenIdleDays = 90
	// APITokenPrefix is recognisable to secret scanners and distinct from
	// session tokens.
	APITokenPrefix = "rdrs_gr_"
	// MaxTokensPerUser bounds a client that never caches Auth=: a successful
	// ClientLogin is effectively unthrottled and mints a row each call.
	MaxTokensPerUser = 20
)

const apiTokenTTL = APITokenIdleDays * 24 * time.Hour

// APIToken is a row of the api_token table.
type APIToken struct {
	ID         int64
	UserID     int64
	Token      string
	Kind       string
	Label      string
	CreatedAt  time.Time
	ExpiresAt  time.Time
	LastSeenAt time.Time
	UserAgent  string
	IPAddress  string
}

const apiTokenColumns = "id, user_id, token, kind, label, created_at, expires_at, last_seen_at, user_agent, ip_address"

func scanAPIToken(row scanner) (APIToken, error) {
	var t APIToken
	err := row.Scan(&t.ID, &t.UserID, &t.Token, &t.Kind, &t.Label,
		db.TimeDest(&t.CreatedAt), db.TimeDest(&t.ExpiresAt), db.TimeDest(&t.LastSeenAt),
		&t.UserAgent, &t.IPAddress)
	return t, err
}

// IsExpired reports whether the token is past its expiry.
func (t *APIToken) IsExpired() bool { return time.Now().After(t.ExpiresAt) }

// ComputeRefreshedExpiry returns the new expiry when less than half of
// APITokenIdleDays remains (there is no absolute cap); nil means no refresh.
func (t *APIToken) ComputeRefreshedExpiry(now time.Time) *time.Time {
	if t.ExpiresAt.Sub(now) >= apiTokenTTL/2 {
		return nil
	}
	refreshed := now.Add(apiTokenTTL)
	return &refreshed
}

// GenerateAPIToken is APITokenPrefix plus a session-style token. It must never
// contain "/": the GReader post-token MAC input relies on that to be unambiguous.
func GenerateAPIToken() string {
	return APITokenPrefix + GenerateToken()
}

// NewAPIToken is the input to CreateAPIToken.
type NewAPIToken struct {
	UserID    int64
	Kind      string
	Label     string
	UserAgent string
	IPAddress string
}

// CreateAPIToken mints a token and prunes the user's oldest beyond the cap.
func CreateAPIToken(ctx context.Context, e db.Executor, in NewAPIToken) (*APIToken, error) {
	now := time.Now()
	row := e.QueryRowContext(ctx,
		`INSERT INTO api_token (user_id, token, kind, label, expires_at, last_seen_at, user_agent, ip_address)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
		 RETURNING `+apiTokenColumns,
		in.UserID, GenerateAPIToken(), in.Kind, in.Label, stamp(now.Add(apiTokenTTL)), stamp(now),
		in.UserAgent, in.IPAddress)
	created, err := scanAPIToken(row)
	if err != nil {
		return nil, err
	}
	// After the insert, so the token just minted always survives.
	if err := pruneUserTokens(ctx, e, in.UserID); err != nil {
		return nil, err
	}
	return &created, nil
}

// pruneUserTokens deletes everything past MaxTokensPerUser, oldest first. id
// breaks ties because SQLite's created_at has second resolution.
func pruneUserTokens(ctx context.Context, e db.Executor, userID int64) error {
	_, err := e.ExecContext(ctx,
		`DELETE FROM api_token WHERE user_id = $1 AND id NOT IN (
		   SELECT id FROM api_token WHERE user_id = $2
		   ORDER BY created_at DESC, id DESC LIMIT $3
		 )`,
		userID, userID, MaxTokensPerUser)
	return err
}

// FindAPIToken looks a token up by its value.
func FindAPIToken(ctx context.Context, e db.Executor, token string) (*APIToken, error) {
	row := e.QueryRowContext(ctx, `SELECT `+apiTokenColumns+` FROM api_token WHERE token = $1`, token)
	found, err := scanAPIToken(row)
	return orNil(&found, err)
}

// TouchAndRefreshAPIToken writes last_seen_at at most once a minute and slides
// expires_at when due. Best-effort: a failure must not block the request.
func TouchAndRefreshAPIToken(ctx context.Context, e db.Executor, t *APIToken) (*time.Time, error) {
	now := time.Now()
	if now.Sub(t.LastSeenAt) >= touchInterval {
		if _, err := e.ExecContext(ctx, "UPDATE api_token SET last_seen_at = $1 WHERE id = $2", stamp(now), t.ID); err != nil {
			return nil, err
		}
	}
	refreshed := t.ComputeRefreshedExpiry(now)
	if refreshed == nil {
		return nil, nil
	}
	if _, err := e.ExecContext(ctx, "UPDATE api_token SET expires_at = $1 WHERE id = $2", stamp(*refreshed), t.ID); err != nil {
		return nil, err
	}
	return refreshed, nil
}

// ListUserTokens returns a user's tokens, newest first.
func ListUserTokens(ctx context.Context, e db.Executor, userID int64) ([]APIToken, error) {
	rows, err := e.QueryContext(ctx,
		`SELECT `+apiTokenColumns+` FROM api_token WHERE user_id = $1 ORDER BY created_at DESC`, userID)
	return collect(rows, err, scanAPIToken)
}

// DeleteToken revokes one token, scoped to userID so one user can never revoke
// another's.
func DeleteToken(ctx context.Context, e db.Executor, id, userID int64) error {
	_, err := e.ExecContext(ctx, "DELETE FROM api_token WHERE id = $1 AND user_id = $2", id, userID)
	return err
}

// DeleteUserTokens revokes every token of a user and returns how many.
func DeleteUserTokens(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	return rowsAffected(e.ExecContext(ctx, "DELETE FROM api_token WHERE user_id = $1", userID))
}

// DeleteExpiredTokens is the periodic backstop for the lazy delete at
// validation time. Binding now keeps it dialect-neutral.
func DeleteExpiredTokens(ctx context.Context, e db.Executor) (int64, error) {
	return rowsAffected(e.ExecContext(ctx, "DELETE FROM api_token WHERE expires_at <= $1", stamp(time.Now())))
}
