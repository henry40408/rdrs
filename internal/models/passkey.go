package models

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// ErrPasskeyNotFound is returned when a passkey does not exist or belongs to
// another user; the two are indistinguishable on purpose.
var ErrPasskeyNotFound = errors.New("passkey not found")

// Passkey is a row of the passkey table. CredentialID and PublicKey must never
// be serialised to a client.
type Passkey struct {
	ID           int64
	UserID       int64
	CredentialID []byte
	PublicKey    []byte
	Counter      int64
	Name         string
	Transports   *string
	CreatedAt    time.Time
	LastUsedAt   *time.Time
}

const passkeyColumns = "id, user_id, credential_id, public_key, counter, name, transports, created_at, last_used_at"

func scanPasskey(row scanner) (Passkey, error) {
	var p Passkey
	err := row.Scan(&p.ID, &p.UserID, &p.CredentialID, &p.PublicKey, &p.Counter, &p.Name, &p.Transports,
		db.TimeDest(&p.CreatedAt), db.NullTimeDest(&p.LastUsedAt))
	return p, err
}

// NewPasskey holds the fields of a passkey to insert.
type NewPasskey struct {
	UserID       int64
	CredentialID []byte
	PublicKey    []byte
	Counter      int64
	Name         string
	Transports   *string
}

// CreatePasskey stores a passkey.
func CreatePasskey(ctx context.Context, e db.Executor, in NewPasskey) (*Passkey, error) {
	created, err := scanPasskey(e.QueryRowContext(ctx,
		`INSERT INTO passkey (user_id, credential_id, public_key, counter, name, transports)
		 VALUES ($1, $2, $3, $4, $5, $6)
		 RETURNING `+passkeyColumns,
		in.UserID, in.CredentialID, in.PublicKey, in.Counter, in.Name, in.Transports))
	if err != nil {
		return nil, err
	}
	return &created, nil
}

// FindPasskeyByID looks a passkey up by id.
func FindPasskeyByID(ctx context.Context, e db.Executor, id int64) (*Passkey, error) {
	found, err := scanPasskey(e.QueryRowContext(ctx, `SELECT `+passkeyColumns+` FROM passkey WHERE id = $1`, id))
	return orNil(&found, err)
}

// FindPasskeyByCredentialID looks a passkey up by its credential id.
func FindPasskeyByCredentialID(ctx context.Context, e db.Executor, credentialID []byte) (*Passkey, error) {
	found, err := scanPasskey(e.QueryRowContext(ctx,
		`SELECT `+passkeyColumns+` FROM passkey WHERE credential_id = $1`, credentialID))
	return orNil(&found, err)
}

// ListUserPasskeys returns a user's passkeys, newest first.
//
// There is deliberately no "every passkey on the instance" query: exposing it
// through allowCredentials leaked every credential ID to unauthenticated
// callers.
func ListUserPasskeys(ctx context.Context, e db.Executor, userID int64) ([]Passkey, error) {
	rows, err := e.QueryContext(ctx,
		`SELECT `+passkeyColumns+` FROM passkey WHERE user_id = $1 ORDER BY created_at DESC`, userID)
	return collect(rows, err, scanPasskey)
}

// UpdatePasskeyCounter records a use of the passkey.
func UpdatePasskeyCounter(ctx context.Context, e db.Executor, id, counter int64) error {
	_, err := e.ExecContext(ctx,
		`UPDATE passkey SET counter = $1, last_used_at = $2 WHERE id = $3`, counter, stamp(time.Now()), id)
	return err
}

// RenamePasskey renames a passkey the user owns.
func RenamePasskey(ctx context.Context, e db.Executor, id, userID int64, name string) error {
	return requirePasskeyRow(e.ExecContext(ctx,
		`UPDATE passkey SET name = $1 WHERE id = $2 AND user_id = $3`, name, id, userID))
}

// DeletePasskey removes a passkey the user owns.
func DeletePasskey(ctx context.Context, e db.Executor, id, userID int64) error {
	return requirePasskeyRow(e.ExecContext(ctx,
		`DELETE FROM passkey WHERE id = $1 AND user_id = $2`, id, userID))
}

func requirePasskeyRow(result sql.Result, err error) error {
	affected, err := rowsAffected(result, err)
	if err != nil {
		return err
	}
	if affected == 0 {
		return ErrPasskeyNotFound
	}
	return nil
}
