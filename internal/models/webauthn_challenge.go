package models

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

const challengeTTL = 5 * time.Minute

// ErrChallengeNotFound means no unexpired challenge matched; it was never
// issued, already used, or timed out.
var ErrChallengeNotFound = errors.New("challenge not found or expired")

// ChallengeType says which ceremony a challenge belongs to.
type ChallengeType string

// The stored challenge types.
const (
	ChallengeRegistration   ChallengeType = "registration"
	ChallengeAuthentication ChallengeType = "authentication"
)

// ParseChallengeType returns false for a value that is not a type.
func ParseChallengeType(s string) (ChallengeType, bool) {
	switch t := ChallengeType(s); t {
	case ChallengeRegistration, ChallengeAuthentication:
		return t, true
	}
	return "", false
}

// WebauthnChallenge is a row of the webauthn_challenge table.
type WebauthnChallenge struct {
	ID            int64
	Challenge     []byte
	UserID        *int64
	ChallengeType ChallengeType
	StateData     string
	CreatedAt     time.Time
	ExpiresAt     time.Time
}

const challengeColumns = "id, challenge, user_id, challenge_type, state_data, created_at, expires_at"

func scanChallenge(row scanner) (WebauthnChallenge, error) {
	var c WebauthnChallenge
	var kind string
	err := row.Scan(&c.ID, &c.Challenge, &c.UserID, &kind, &c.StateData,
		db.TimeDest(&c.CreatedAt), db.TimeDest(&c.ExpiresAt))
	// An unparseable type reads as registration, as the Rust build did.
	c.ChallengeType = ChallengeRegistration
	if parsed, ok := ParseChallengeType(kind); ok {
		c.ChallengeType = parsed
	}
	return c, err
}

// NewChallenge holds the fields of a challenge to insert.
type NewChallenge struct {
	Challenge     []byte
	UserID        *int64
	ChallengeType ChallengeType
	StateData     string
}

// CreateChallenge stores a challenge that expires after five minutes.
func CreateChallenge(ctx context.Context, e db.Executor, in NewChallenge) (*WebauthnChallenge, error) {
	created, err := scanChallenge(e.QueryRowContext(ctx,
		`INSERT INTO webauthn_challenge (challenge, user_id, challenge_type, state_data, expires_at)
		 VALUES ($1, $2, $3, $4, $5)
		 RETURNING `+challengeColumns,
		in.Challenge, in.UserID, string(in.ChallengeType), in.StateData, stamp(time.Now().Add(challengeTTL))))
	if err != nil {
		return nil, err
	}
	return &created, nil
}

// TakeChallenge returns the newest unexpired challenge of userID (nil for an
// anonymous ceremony) and type, and deletes it so it is single-use. It fails
// with ErrChallengeNotFound when there is none.
//
// Run it inside a transaction when two requests could race for one challenge.
func TakeChallenge(ctx context.Context, e db.Executor, userID *int64, kind ChallengeType) (*WebauthnChallenge, error) {
	owner, args := "user_id IS NULL", []any{string(kind), stamp(time.Now())}
	if userID != nil {
		owner, args = "user_id = $3", append(args, *userID)
	}
	found, err := scanChallenge(e.QueryRowContext(ctx,
		`SELECT `+challengeColumns+` FROM webauthn_challenge
		 WHERE `+owner+` AND challenge_type = $1 AND expires_at > $2
		 ORDER BY created_at DESC, id DESC LIMIT 1`, args...))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, ErrChallengeNotFound
	}
	if err != nil {
		return nil, err
	}
	if _, err := e.ExecContext(ctx, `DELETE FROM webauthn_challenge WHERE id = $1`, found.ID); err != nil {
		return nil, err
	}
	return &found, nil
}
