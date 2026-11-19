package models

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// Session lifetimes.
const (
	SessionExpiryDays      = 7
	SessionAbsoluteMaxDays = 90
	tokenBytes             = 32

	// RotationGrace is how long a rotated-out token keeps authenticating, so
	// in-flight requests are not signed out; far below the rotation interval, so
	// at most one predecessor is live.
	RotationGrace = 60 * time.Second

	// ReauthWindow is how long after proving its credentials a session may
	// perform a sensitive operation without re-authenticating.
	ReauthWindow = 5 * time.Minute

	touchInterval = time.Minute
)

const (
	sessionTTL = SessionExpiryDays * 24 * time.Hour
	sessionCap = SessionAbsoluteMaxDays * 24 * time.Hour
)

// Session is a row of the session table.
type Session struct {
	ID             int64
	UserID         int64
	SessionToken   string
	OriginalUserID *int64
	CreatedAt      time.Time
	ExpiresAt      time.Time
	UserAgent      string
	IPAddress      string
	LastSeenAt     time.Time
	// PreviousToken is the pre-rotation token, accepted by FindSessionByToken
	// until PreviousTokenExpiresAt.
	PreviousToken          *string
	PreviousTokenExpiresAt *time.Time
	// LastAuthenticatedAt is when credentials were last proved (login or
	// re-auth); nil only for rows that predate the backfill.
	LastAuthenticatedAt *time.Time
}

const sessionColumns = "id, user_id, session_token, original_user_id, created_at, expires_at, " +
	"user_agent, ip_address, last_seen_at, previous_token, previous_token_expires_at, " +
	"last_authenticated_at"

func scanSession(row scanner) (Session, error) {
	var s Session
	err := row.Scan(&s.ID, &s.UserID, &s.SessionToken, &s.OriginalUserID,
		db.TimeDest(&s.CreatedAt), db.TimeDest(&s.ExpiresAt), &s.UserAgent, &s.IPAddress,
		db.TimeDest(&s.LastSeenAt), &s.PreviousToken, db.NullTimeDest(&s.PreviousTokenExpiresAt),
		db.NullTimeDest(&s.LastAuthenticatedAt))
	return s, err
}

// IsMasquerading reports whether an admin is acting as another user.
func (s *Session) IsMasquerading() bool { return s.OriginalUserID != nil }

// IsExpired reports whether the session is past its expiry.
func (s *Session) IsExpired() bool { return time.Now().After(s.ExpiresAt) }

// AuthenticatedRecently reports whether credentials were proved within
// ReauthWindow. A missing timestamp counts as stale, the safe direction.
func (s *Session) AuthenticatedRecently(now time.Time) bool {
	return s.LastAuthenticatedAt != nil && now.Sub(*s.LastAuthenticatedAt) < ReauthWindow
}

// ComputeRefreshedExpiry returns the new expiry when the remaining TTL has
// fallen below half of SessionExpiryDays and the session has not reached its
// absolute cap (CreatedAt + SessionAbsoluteMaxDays); nil means no refresh.
func (s *Session) ComputeRefreshedExpiry(now time.Time) *time.Time {
	absoluteCap := s.CreatedAt.Add(sessionCap)
	if !s.ExpiresAt.Before(absoluteCap) {
		return nil
	}
	if s.ExpiresAt.Sub(now) >= sessionTTL/2 {
		return nil
	}
	refreshed := now.Add(sessionTTL)
	if refreshed.After(absoluteCap) {
		refreshed = absoluteCap
	}
	return &refreshed
}

// GenerateToken returns a fresh random session token. The anonymous-session
// middleware also uses it to mint a cookie without a database row.
func GenerateToken() string {
	buf := make([]byte, tokenBytes)
	if _, err := rand.Read(buf); err != nil {
		panic("models: crypto/rand failed: " + err.Error())
	}
	return base64.RawURLEncoding.EncodeToString(buf)
}

// NewSession is the input to CreateSession.
type NewSession struct {
	UserID    int64
	UserAgent string
	IPAddress string
}

// CreateSession starts a session, already inside the re-auth window because a
// login just succeeded.
func CreateSession(ctx context.Context, e db.Executor, in NewSession) (*Session, error) {
	now := time.Now()
	row := e.QueryRowContext(ctx,
		`INSERT INTO session
		     (user_id, session_token, expires_at, user_agent, ip_address, last_seen_at, last_authenticated_at)
		 VALUES ($1, $2, $3, $4, $5, $6, $7)
		 RETURNING `+sessionColumns,
		in.UserID, GenerateToken(), stamp(now.Add(sessionTTL)), in.UserAgent, in.IPAddress, stamp(now), stamp(now))
	session, err := scanSession(row)
	if err != nil {
		return nil, err
	}
	return &session, nil
}

// MarkAuthenticated restarts the AuthenticatedRecently window.
func MarkAuthenticated(ctx context.Context, e db.Executor, sessionID int64) (time.Time, error) {
	now := time.Now()
	_, err := e.ExecContext(ctx, "UPDATE session SET last_authenticated_at = $1 WHERE id = $2", stamp(now), sessionID)
	return now, err
}

// FindSessionByToken looks a session up by its token, or by a previous token
// still in its grace interval. The grace arm lives here so every caller gets it.
func FindSessionByToken(ctx context.Context, e db.Executor, token string) (*Session, error) {
	row := e.QueryRowContext(ctx,
		`SELECT `+sessionColumns+` FROM session
		 WHERE session_token = $1
		    OR (previous_token = $1 AND previous_token_expires_at > $2)`,
		token, stamp(time.Now()))
	session, err := scanSession(row)
	return orNil(&session, err)
}

// TouchLastSeen bumps last_seen_at, at most once per minute per session.
func TouchLastSeen(ctx context.Context, e db.Executor, session *Session) error {
	now := time.Now()
	if now.Sub(session.LastSeenAt) < touchInterval {
		return nil
	}
	_, err := e.ExecContext(ctx, "UPDATE session SET last_seen_at = $1 WHERE id = $2", stamp(now), session.ID)
	return err
}

// RefreshSessionIfNeeded slides expires_at forward when within the refresh
// window. A non-nil result is also the cue to call RotateToken.
func RefreshSessionIfNeeded(ctx context.Context, e db.Executor, session *Session) (*time.Time, error) {
	refreshed := session.ComputeRefreshedExpiry(time.Now())
	if refreshed == nil {
		return nil, nil
	}
	_, err := e.ExecContext(ctx, "UPDATE session SET expires_at = $1 WHERE id = $2", stamp(*refreshed), session.ID)
	if err != nil {
		return nil, err
	}
	return refreshed, nil
}

// RotateToken swaps the token of the session answering to token (OWASP renewal
// timeout) and returns the new one, or "" if another request rotated it first.
//
// It matches session_token only, never the grace token, so concurrent requests
// cannot chain rotations.
func RotateToken(ctx context.Context, e db.Executor, token string) (string, error) {
	newToken := GenerateToken()
	graceUntil := time.Now().Add(RotationGrace)
	affected, err := rowsAffected(e.ExecContext(ctx,
		`UPDATE session
		 SET session_token = $1, previous_token = session_token, previous_token_expires_at = $2
		 WHERE session_token = $3`,
		newToken, stamp(graceUntil), token))
	if err != nil || affected == 0 {
		return "", err
	}
	return newToken, nil
}

// DeleteSession deletes the session for token. It also matches the previous
// token, since a logout can arrive on the grace token.
func DeleteSession(ctx context.Context, e db.Executor, token string) error {
	_, err := e.ExecContext(ctx, "DELETE FROM session WHERE session_token = $1 OR previous_token = $1", token)
	return err
}

// DeleteUserSessions deletes every session of a user, including the ones the
// user holds while masquerading: those carry the target's user_id, so matching
// user_id alone would leave a disabled admin's session alive.
func DeleteUserSessions(ctx context.Context, e db.Executor, userID int64) error {
	_, err := e.ExecContext(ctx, "DELETE FROM session WHERE user_id = $1 OR original_user_id = $1", userID)
	return err
}

// ListUserSessions returns a user's sessions, newest first, including expired rows.
func ListUserSessions(ctx context.Context, e db.Executor, userID int64) ([]Session, error) {
	rows, err := e.QueryContext(ctx,
		`SELECT `+sessionColumns+` FROM session WHERE user_id = $1 ORDER BY created_at DESC`, userID)
	return collect(rows, err, scanSession)
}

// DeleteUserSessionsExcept deletes every session of userID except keepToken's
// and returns the count. keepToken may be a grace token, so a matching previous
// token is exempted too.
func DeleteUserSessionsExcept(ctx context.Context, e db.Executor, userID int64, keepToken string) (int64, error) {
	return rowsAffected(e.ExecContext(ctx,
		`DELETE FROM session
		 WHERE user_id = $1
		   AND session_token <> $2
		   AND (previous_token IS NULL OR previous_token <> $2)`,
		userID, keepToken))
}

// DeleteUserSessionByID deletes one session, scoped to userID so a guessed id
// cannot revoke another user's session. It returns the rows deleted.
func DeleteUserSessionByID(ctx context.Context, e db.Executor, id, userID int64) (int64, error) {
	return rowsAffected(e.ExecContext(ctx, "DELETE FROM session WHERE id = $1 AND user_id = $2", id, userID))
}

// DeleteExpiredSessions is the cleanup worker's backstop for abandoned rows the
// lazy deletes never touch. Binding now keeps it dialect-neutral.
func DeleteExpiredSessions(ctx context.Context, e db.Executor) (int64, error) {
	return rowsAffected(e.ExecContext(ctx, "DELETE FROM session WHERE expires_at <= $1", stamp(time.Now())))
}

// StartMasquerade makes the session answering to token act as targetUserID and
// returns its new token. The token rotates in the same UPDATE as user_id (a
// privilege change, done atomically); callers must reissue the session and
// CSRF cookies from it.
func StartMasquerade(ctx context.Context, e db.Executor, token string, targetUserID int64) (string, error) {
	session, err := requireSession(ctx, e, token)
	if err != nil {
		return "", err
	}
	if session.IsMasquerading() {
		return "", ErrAlreadyMasquerading
	}
	newToken := GenerateToken()
	_, err = e.ExecContext(ctx,
		`UPDATE session
		 SET original_user_id = user_id, user_id = $1, session_token = $2,
		     previous_token = session_token, previous_token_expires_at = $3
		 WHERE id = $4`,
		targetUserID, newToken, stamp(time.Now().Add(RotationGrace)), session.ID)
	return newToken, err
}

// StopMasquerade returns the session to its admin and gives it a new token; the
// token used while impersonating must not survive as the admin's.
func StopMasquerade(ctx context.Context, e db.Executor, token string) (string, error) {
	session, err := requireSession(ctx, e, token)
	if err != nil {
		return "", err
	}
	if !session.IsMasquerading() {
		return "", ErrNotMasquerading
	}
	newToken := GenerateToken()
	_, err = e.ExecContext(ctx,
		`UPDATE session
		 SET user_id = original_user_id, original_user_id = NULL, session_token = $1,
		     previous_token = session_token, previous_token_expires_at = $2
		 WHERE id = $3`,
		newToken, stamp(time.Now().Add(RotationGrace)), session.ID)
	return newToken, err
}

func requireSession(ctx context.Context, e db.Executor, token string) (*Session, error) {
	session, err := FindSessionByToken(ctx, e, token)
	if err != nil {
		return nil, err
	}
	if session == nil {
		return nil, ErrUnauthorized
	}
	return session, nil
}
