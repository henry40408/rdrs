package models

import (
	"context"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/henry40408/rdrs/internal/db"
)

// Role is a user's privilege level.
type Role string

// The two roles.
const (
	RoleAdmin Role = "admin"
	RoleUser  Role = "user"
)

// ParseRole maps stored text to a Role; an unknown value is the least
// privileged role rather than an error, so a bad row never locks anyone out.
func ParseRole(text string) Role {
	if text == string(RoleAdmin) {
		return RoleAdmin
	}
	return RoleUser
}

// User is a row of the "user" table.
type User struct {
	ID           int64
	Username     string
	PasswordHash string
	Role         Role
	DisabledAt   *time.Time
	CreatedAt    time.Time
}

// IsDisabled reports whether the account is disabled.
func (u *User) IsDisabled() bool { return u.DisabledAt != nil }

// IsAdmin reports whether the account is an administrator.
func (u *User) IsAdmin() bool { return u.Role == RoleAdmin }

// UsernameMaxChars is the longest accepted username, in characters.
const UsernameMaxChars = 64

const userColumns = "id, username, password_hash, role, disabled_at, created_at"

func scanUser(row scanner) (User, error) {
	var user User
	var role string
	err := row.Scan(&user.ID, &user.Username, &user.PasswordHash, &role,
		db.NullTimeDest(&user.DisabledAt), db.TimeDest(&user.CreatedAt))
	user.Role = ParseRole(role)
	return user, err
}

// ValidateUsername trims, then requires a non-empty name of at most
// UsernameMaxChars characters without control characters. Any other character
// is allowed because SSO identities (emails, DOMAIN\user, spaces, non-ASCII)
// must keep working. It returns the trimmed name to store.
func ValidateUsername(raw string) (string, error) {
	username := strings.TrimSpace(raw)
	if username == "" {
		return "", validationf("Username is required")
	}
	if utf8.RuneCountInString(username) > UsernameMaxChars {
		return "", validationf("Username must be at most %d characters", UsernameMaxChars)
	}
	if strings.IndexFunc(username, unicode.IsControl) >= 0 {
		return "", validationf("Username must not contain control characters")
	}
	return username, nil
}

// CreateUser inserts an account; a taken name is ErrUsernameExists.
func CreateUser(ctx context.Context, e db.Executor, username, passwordHash string, role Role) (*User, error) {
	row := e.QueryRowContext(ctx,
		`INSERT INTO "user" (username, password_hash, role) VALUES ($1, $2, $3) RETURNING `+userColumns,
		username, passwordHash, string(role))
	user, err := scanUser(row)
	if db.IsUniqueViolation(err) {
		return nil, ErrUsernameExists
	}
	if err != nil {
		return nil, err
	}
	return &user, nil
}

// FindUserByUsername looks an account up by its exact stored name.
func FindUserByUsername(ctx context.Context, e db.Executor, username string) (*User, error) {
	return findUser(ctx, e, "username = $1", username)
}

// FindUserByID looks an account up by id.
func FindUserByID(ctx context.Context, e db.Executor, id int64) (*User, error) {
	return findUser(ctx, e, "id = $1", id)
}

func findUser(ctx context.Context, e db.Executor, where string, arg any) (*User, error) {
	row := e.QueryRowContext(ctx, `SELECT `+userColumns+` FROM "user" WHERE `+where, arg)
	user, err := scanUser(row)
	return orNil(&user, err)
}

// FindUserForLogin is the login lookup: the raw value first, so accounts
// stored before validation trimmed names (possibly with surrounding spaces)
// still sign in, then the trimmed value. It also returns the per-account rate
// limit key: the stored name on a hit, else the trimmed input.
func FindUserForLogin(ctx context.Context, e db.Executor, raw string) (rateKey string, user *User, err error) {
	user, err = FindUserByUsername(ctx, e, raw)
	if err != nil {
		return "", nil, err
	}
	if user != nil {
		return user.Username, user, nil
	}
	trimmed := strings.TrimSpace(raw)
	if trimmed == raw {
		return trimmed, nil, nil
	}
	user, err = FindUserByUsername(ctx, e, trimmed)
	return trimmed, user, err
}

// ListUsers returns every account ordered by id.
func ListUsers(ctx context.Context, e db.Executor) ([]User, error) {
	rows, err := e.QueryContext(ctx, `SELECT `+userColumns+` FROM "user" ORDER BY id`)
	return collect(rows, err, scanUser)
}

// ChangePassword sets a new password and revokes every credential issued under
// the old one in one transaction: a failed revocation must not leave the new
// password live beside old sessions and API tokens.
func ChangePassword(ctx context.Context, database *db.DB, userID int64, newPasswordHash string) error {
	tx, err := database.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	if err := ChangePasswordTx(ctx, tx, userID, newPasswordHash); err != nil {
		return err
	}
	return tx.Commit()
}

// ChangePasswordTx is ChangePassword inside a caller-owned transaction.
func ChangePasswordTx(ctx context.Context, e db.Executor, userID int64, newPasswordHash string) error {
	if err := updateUser(ctx, e, `UPDATE "user" SET password_hash = $1 WHERE id = $2`, newPasswordHash, userID); err != nil {
		return err
	}
	return revokeCredentials(ctx, e, userID)
}

// revokeCredentials deletes sessions and API tokens; tokens bypass the session
// table, so both must go.
func revokeCredentials(ctx context.Context, e db.Executor, userID int64) error {
	if err := DeleteUserSessions(ctx, e, userID); err != nil {
		return err
	}
	_, err := DeleteUserTokens(ctx, e, userID)
	return err
}

// updateUser runs an UPDATE that must match exactly the user in its last arg.
func updateUser(ctx context.Context, e db.Executor, query string, args ...any) error {
	affected, err := rowsAffected(e.ExecContext(ctx, query, args...))
	if err != nil {
		return err
	}
	if affected == 0 {
		return ErrUserNotFound
	}
	return nil
}

// UpdateRole changes an account's role.
func UpdateRole(ctx context.Context, e db.Executor, userID int64, role Role) error {
	return updateUser(ctx, e, `UPDATE "user" SET role = $1 WHERE id = $2`, string(role), userID)
}

// DisableUser disables the account and revokes its sessions and API tokens in
// one transaction, so a disabled user is never left holding a live credential.
func DisableUser(ctx context.Context, database *db.DB, userID int64) error {
	tx, err := database.Begin(ctx)
	if err != nil {
		return err
	}
	defer func() { _ = tx.Rollback() }()
	err = updateUser(ctx, tx, `UPDATE "user" SET disabled_at = $1 WHERE id = $2`, stamp(time.Now()), userID)
	if err != nil {
		return err
	}
	if err := revokeCredentials(ctx, tx, userID); err != nil {
		return err
	}
	return tx.Commit()
}

// EnableUser clears the disabled flag.
func EnableUser(ctx context.Context, e db.Executor, userID int64) error {
	return updateUser(ctx, e, `UPDATE "user" SET disabled_at = NULL WHERE id = $1`, userID)
}

// DeleteUser removes the account; foreign keys cascade to its data. The table
// name is quoted because "user" is reserved in PostgreSQL. Sessions the user
// holds while masquerading go first: original_user_id is ON DELETE SET NULL,
// which would turn them into ordinary, unguarded sessions of the target.
func DeleteUser(ctx context.Context, e db.Executor, userID int64) error {
	if _, err := e.ExecContext(ctx, "DELETE FROM session WHERE original_user_id = $1", userID); err != nil {
		return err
	}
	return updateUser(ctx, e, `DELETE FROM "user" WHERE id = $1`, userID)
}

// CountUsers returns the number of accounts.
func CountUsers(ctx context.Context, e db.Executor) (int64, error) {
	return countScalar(ctx, e, `SELECT COUNT(*) FROM "user"`)
}

// CountActiveAdmins returns the number of enabled administrators.
func CountActiveAdmins(ctx context.Context, e db.Executor) (int64, error) {
	return countScalar(ctx, e, `SELECT COUNT(*) FROM "user" WHERE role = 'admin' AND disabled_at IS NULL`)
}

func countScalar(ctx context.Context, e db.Executor, query string, args ...any) (int64, error) {
	var n int64
	err := e.QueryRowContext(ctx, query, args...).Scan(&n)
	return n, err
}
