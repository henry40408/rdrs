package models

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
)

func TestValidateUsername(t *testing.T) {
	accepted := []string{"alice", `DOMAIN\user`, "a@example.com", "with space", "王小明", strings.Repeat("a", 64), strings.Repeat("王", 64)}
	for _, name := range accepted {
		if got, err := ValidateUsername(name); err != nil || got != name {
			t.Errorf("ValidateUsername(%q) = %q, %v", name, got, err)
		}
	}
	if got, err := ValidateUsername("  bob \t"); err != nil || got != "bob" {
		t.Errorf("trim: got %q, %v", got, err)
	}
	rejected := map[string]string{
		"":                      "Username is required",
		"   ":                   "Username is required",
		strings.Repeat("a", 65): "Username must be at most 64 characters",
		"bad\x00name":           "Username must not contain control characters",
		"tab\tinside":           "Username must not contain control characters",
	}
	for name, message := range rejected {
		_, err := ValidateUsername(name)
		var validation *ValidationError
		if !errors.As(err, &validation) || validation.Message != message {
			t.Errorf("ValidateUsername(%q) err = %v, want %q", name, err, message)
		}
	}
}

func TestCreateAndFindUser(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		created, err := CreateUser(ctx, database, "alice", "hash", RoleAdmin)
		if err != nil {
			t.Fatal(err)
		}
		if created.Username != "alice" || !created.IsAdmin() || created.IsDisabled() {
			t.Errorf("unexpected user %+v", created)
		}
		approxNow(t, created.CreatedAt, 5e9)

		byName, err := FindUserByUsername(ctx, database, "alice")
		if err != nil || byName == nil || byName.ID != created.ID {
			t.Fatalf("by name: %+v %v", byName, err)
		}
		byID, err := FindUserByID(ctx, database, created.ID)
		if err != nil || byID == nil || byID.Username != "alice" {
			t.Fatalf("by id: %+v %v", byID, err)
		}
		missing, err := FindUserByID(ctx, database, 999)
		if err != nil || missing != nil {
			t.Errorf("missing = %+v, %v", missing, err)
		}
	})
}

func TestDuplicateUsername(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		mustUser(t, database, "alice")
		_, err := CreateUser(context.Background(), database, "alice", "other", RoleUser)
		if !errors.Is(err, ErrUsernameExists) {
			t.Errorf("err = %v, want ErrUsernameExists", err)
		}
	})
}

func TestFindUserForLogin(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		mustUser(t, database, "alice")
		mustUser(t, database, " padded ")

		key, user, err := FindUserForLogin(ctx, database, " alice ")
		if err != nil || user == nil || key != "alice" {
			t.Errorf("trimmed fallback: %q %+v %v", key, user, err)
		}
		key, user, err = FindUserForLogin(ctx, database, " padded ")
		if err != nil || user == nil || key != " padded " {
			t.Errorf("raw match first: %q %+v %v", key, user, err)
		}
		key, user, err = FindUserForLogin(ctx, database, "nobody ")
		if err != nil || user != nil || key != "nobody" {
			t.Errorf("miss: %q %+v %v", key, user, err)
		}
	})
}

func TestUnknownRoleDecodesAsUser(t *testing.T) {
	// A CHECK constraint keeps such a row out of the database, so test the decoder.
	for _, text := range []string{"wizard", "", "ADMIN"} {
		if got := ParseRole(text); got != RoleUser {
			t.Errorf("ParseRole(%q) = %q, want user", text, got)
		}
	}
	if ParseRole("admin") != RoleAdmin {
		t.Error("admin must parse as admin")
	}
}

func seedCredentials(t *testing.T, database *db.DB, userID int64) {
	t.Helper()
	ctx := context.Background()
	if _, err := CreateSession(ctx, database, NewSession{UserID: userID, UserAgent: "ua", IPAddress: "1.2.3.4"}); err != nil {
		t.Fatal(err)
	}
	if _, err := CreateAPIToken(ctx, database, NewAPIToken{UserID: userID, Kind: "greader", Label: "x"}); err != nil {
		t.Fatal(err)
	}
}

func credentialCounts(t *testing.T, database *db.DB, userID int64) (sessions, tokens int64) {
	t.Helper()
	ctx := context.Background()
	sessions, err := countScalar(ctx, database, "SELECT COUNT(*) FROM session WHERE user_id = $1", userID)
	if err != nil {
		t.Fatal(err)
	}
	tokens, err = countScalar(ctx, database, "SELECT COUNT(*) FROM api_token WHERE user_id = $1", userID)
	if err != nil {
		t.Fatal(err)
	}
	return sessions, tokens
}

func TestChangePasswordRevokesCredentials(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		seedCredentials(t, database, user.ID)

		if err := ChangePassword(ctx, database, user.ID, "new-hash"); err != nil {
			t.Fatal(err)
		}
		found, _ := FindUserByID(ctx, database, user.ID)
		if found.PasswordHash != "new-hash" {
			t.Errorf("hash = %q", found.PasswordHash)
		}
		if s, k := credentialCounts(t, database, user.ID); s != 0 || k != 0 {
			t.Errorf("credentials left: sessions=%d tokens=%d", s, k)
		}
		if err := ChangePassword(ctx, database, 999, "x"); !errors.Is(err, ErrUserNotFound) {
			t.Errorf("unknown user err = %v", err)
		}
	})
}

func TestChangePasswordRollsBackWhenRevocationFails(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		mustExec(t, database, "DROP TABLE api_token")

		if err := ChangePassword(ctx, database, user.ID, "new-hash"); err == nil {
			t.Fatal("expected an error")
		}
		found, _ := FindUserByID(ctx, database, user.ID)
		if found.PasswordHash != "hash" {
			t.Errorf("password changed despite failure: %q", found.PasswordHash)
		}
	})
}

func TestDisableUserRevokesCredentials(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		seedCredentials(t, database, user.ID)

		if err := DisableUser(ctx, database, user.ID); err != nil {
			t.Fatal(err)
		}
		found, _ := FindUserByID(ctx, database, user.ID)
		if !found.IsDisabled() {
			t.Error("user not disabled")
		}
		if s, k := credentialCounts(t, database, user.ID); s != 0 || k != 0 {
			t.Errorf("credentials left: sessions=%d tokens=%d", s, k)
		}
		if err := EnableUser(ctx, database, user.ID); err != nil {
			t.Fatal(err)
		}
		found, _ = FindUserByID(ctx, database, user.ID)
		if found.IsDisabled() {
			t.Error("user still disabled")
		}
		if err := DisableUser(ctx, database, 999); !errors.Is(err, ErrUserNotFound) {
			t.Errorf("unknown user err = %v", err)
		}
	})
}

func TestDisableUserRollsBackWhenRevocationFails(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		user := mustUser(t, database, "alice")
		mustExec(t, database, "DROP TABLE api_token")
		if err := DisableUser(ctx, database, user.ID); err == nil {
			t.Fatal("expected an error")
		}
		found, _ := FindUserByID(ctx, database, user.ID)
		if found.IsDisabled() {
			t.Error("user disabled despite failure")
		}
	})
}

func TestUpdateRoleDeleteAndCounts(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		admin := mustUser(t, database, "admin")
		other := mustUser(t, database, "other")
		if err := UpdateRole(ctx, database, admin.ID, RoleAdmin); err != nil {
			t.Fatal(err)
		}
		if err := UpdateRole(ctx, database, 999, RoleAdmin); !errors.Is(err, ErrUserNotFound) {
			t.Errorf("err = %v", err)
		}
		if n, _ := CountActiveAdmins(ctx, database); n != 1 {
			t.Errorf("active admins = %d, want 1", n)
		}
		if err := DisableUser(ctx, database, admin.ID); err != nil {
			t.Fatal(err)
		}
		if n, _ := CountActiveAdmins(ctx, database); n != 0 {
			t.Errorf("active admins after disable = %d, want 0", n)
		}
		if n, _ := CountUsers(ctx, database); n != 2 {
			t.Errorf("users = %d, want 2", n)
		}
		users, err := ListUsers(ctx, database)
		if err != nil || len(users) != 2 || users[0].ID != admin.ID {
			t.Errorf("list = %+v, %v", users, err)
		}
		if err := DeleteUser(ctx, database, other.ID); err != nil {
			t.Fatal(err)
		}
		if err := DeleteUser(ctx, database, other.ID); !errors.Is(err, ErrUserNotFound) {
			t.Errorf("second delete err = %v", err)
		}
	})
}
