package auth

import (
	"context"
	"errors"
	"net/http"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
)

func (f *fixture) exec(query string) {
	f.t.Helper()
	if _, err := f.database.ExecContext(context.Background(), query); err != nil {
		f.t.Fatal(err)
	}
}

func (f *fixture) refuseWrites(table string, events ...string) {
	f.t.Helper()
	for _, event := range events {
		f.exec("CREATE TRIGGER refuse_" + event + "_" + table + " BEFORE " + event + " ON " + table +
			` BEGIN SELECT RAISE(ABORT, 'refused'); END`)
	}
}

func TestAuthenticateSurfacesStoreFailuresOnlyToStrictCallers(t *testing.T) {
	for _, table := range []string{"session", "user"} {
		t.Run(table, func(t *testing.T) {
			f := newFixture(t)
			session := f.session(f.user("alice", models.RoleUser).ID)
			f.exec("ALTER TABLE " + table + " RENAME TO " + table + "_gone")
			_, err := f.authenticator.Authenticate(f.request(session.SessionToken))
			if err == nil || errors.Is(err, ErrUnauthorized) {
				t.Errorf("strict err = %v, want the store error", err)
			}
			if f.authenticator.AuthenticatePage(f.request(session.SessionToken)) != nil {
				t.Error("page variant must treat a store failure as anonymous")
			}
		})
	}
}

func TestAuthenticateReportsAnExpiredSessionThatCannotBeDeleted(t *testing.T) {
	f := newFixture(t)
	session := f.session(f.user("alice", models.RoleUser).ID)
	f.setSessionTimes(session.ID, time.Now().Add(-48*time.Hour), time.Now().Add(-time.Hour))
	f.refuseWrites("session", "delete")
	_, err := f.authenticator.Authenticate(f.request(session.SessionToken))
	if err == nil || errors.Is(err, ErrUnauthorized) {
		t.Errorf("strict err = %v, want the delete error", err)
	}
	if f.authenticator.AuthenticatePage(f.request(session.SessionToken)) != nil {
		t.Error("page variant must treat it as anonymous")
	}
}

func TestAuthenticateReportsASessionThatCannotBeRenewed(t *testing.T) {
	f := newFixture(t)
	session := f.session(f.user("alice", models.RoleUser).ID)
	f.setSessionTimes(session.ID, time.Now().Add(-4*24*time.Hour), time.Now().Add(2*24*time.Hour))
	f.refuseWrites("session", "update")
	if _, err := f.authenticator.Authenticate(f.request(session.SessionToken)); err == nil {
		t.Error("strict call must report the failed renewal")
	}
	if f.authenticator.AuthenticatePage(f.request(session.SessionToken)) == nil {
		t.Error("page variant keeps the session despite a failed renewal")
	}
}

func TestMasqueradingAdminCheckFollowsTheOriginalUser(t *testing.T) {
	f := newFixture(t)
	admin := f.user("root", models.RoleAdmin)
	target := f.user("bob", models.RoleUser)
	masquerading := func(original int64) *Principal {
		p := &Principal{User: *target}
		p.Session.OriginalUserID = &original
		return p
	}
	ctx := context.Background()
	if ok, err := f.authenticator.isAdminPrincipal(ctx, masquerading(admin.ID)); err != nil || !ok {
		t.Errorf("original admin: %v, %v", ok, err)
	}
	if ok, err := f.authenticator.isAdminPrincipal(ctx, masquerading(target.ID)); err != nil || ok {
		t.Errorf("original non-admin: %v, %v", ok, err)
	}
	if _, err := f.authenticator.isAdminPrincipal(ctx, masquerading(9999)); !errors.Is(err, ErrUnauthorized) {
		t.Errorf("vanished original: %v", err)
	}
	f.exec("ALTER TABLE user RENAME TO user_gone")
	if _, err := f.authenticator.isAdminPrincipal(ctx, masquerading(admin.ID)); err == nil || errors.Is(err, ErrUnauthorized) {
		t.Errorf("store failure: %v", err)
	}
}

func (f *fixture) forwardLogin(creation bool) ForwardLogin {
	cfg := f.authenticator.Config
	cfg.AuthProxyHeader = "X-Remote-User"
	cfg.AuthProxyUserCreation = creation
	return ForwardLogin{DB: f.database, Config: cfg, Audit: f.authenticator.Audit}
}

func TestForwardLoginReportsStoreFailures(t *testing.T) {
	signIn := func(login ForwardLogin, name string) (*models.Session, error) {
		r, _ := http.NewRequest(http.MethodGet, "/", nil)
		return login.SignIn(r, name)
	}
	t.Run("user lookup", func(t *testing.T) {
		f := newFixture(t)
		f.exec("ALTER TABLE user RENAME TO user_gone")
		if _, err := signIn(f.forwardLogin(true), "alice"); err == nil {
			t.Error("want the lookup error")
		}
	})
	t.Run("session creation", func(t *testing.T) {
		f := newFixture(t)
		f.user("alice", models.RoleUser)
		f.refuseWrites("session", "insert")
		session, err := signIn(f.forwardLogin(false), "alice")
		if err == nil || session != nil {
			t.Errorf("session = %+v, err = %v", session, err)
		}
	})
	t.Run("account creation rolls back", func(t *testing.T) {
		f := newFixture(t)
		f.refuseWrites("category", "insert")
		if _, err := signIn(f.forwardLogin(true), "alice"); err == nil {
			t.Fatal("want the category error")
		}
		if found, _ := models.FindUserByUsername(context.Background(), f.database, "alice"); found != nil {
			t.Errorf("half-created account survived: %+v", found)
		}
	})
	t.Run("role sync", func(t *testing.T) {
		f := newFixture(t)
		f.user("alice", models.RoleUser)
		f.refuseWrites("user", "update")
		login := f.forwardLogin(false)
		login.Config.AuthProxyGroupsHeader = "X-Groups"
		login.Config.AuthProxyAdminGroup = "admins"
		r, _ := http.NewRequest(http.MethodGet, "/", nil)
		r.Header.Set("X-Groups", "admins")
		if _, err := login.SignIn(r, "alice"); err == nil {
			t.Error("want the role update error")
		}
	})
}
