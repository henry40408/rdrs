package server

import (
	"context"
	"net/http"
	"net/url"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
)

func (f *signedInFixture) refuseWrites(t *testing.T, table string, events ...string) {
	t.Helper()
	for _, event := range events {
		query := "CREATE TRIGGER refuse_" + event + "_" + table + " BEFORE " + event + " ON " + table +
			" BEGIN SELECT RAISE(ABORT, 'refused'); END"
		if _, err := f.database.ExecContext(context.Background(), query); err != nil {
			t.Fatal(err)
		}
	}
}

func TestAdminFormsRejectIncompleteBodies(t *testing.T) {
	f := newSignedIn(t)
	bob := f.makeUser(t, "bob")
	cases := []struct {
		name, path string
		form       url.Values
		want       string
	}{
		{"create without a username", "/admin/users", url.Values{"role": {"user"}}, "missing field `username`"},
		{"create without a role", "/admin/users", url.Values{"username": {"carol"}}, "missing field `role`"},
		{"create with an unknown role", "/admin/users", url.Values{"username": {"carol"}, "role": {"owner"}}, "unknown variant `owner`"},
		{"role without a value", idPath("/admin/users/{id}/role", bob.ID), nil, "missing field `role`"},
		{"role with an unknown value", idPath("/admin/users/{id}/role", bob.ID), url.Values{"role": {"root"}}, "unknown variant `root`"},
		{"status without a flag", idPath("/admin/users/{id}/status", bob.ID), nil, "missing field `disabled`"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			rec := f.postForm(c.path, c.form)
			if rec.Code != http.StatusUnprocessableEntity || !strings.Contains(rec.Body.String(), c.want) {
				t.Errorf("POST %s = %d %s, want 422 mentioning %q", c.path, rec.Code, rec.Body, c.want)
			}
		})
	}
}

func TestAdminFormsReportAccountsThatDoNotExist(t *testing.T) {
	f := newSignedIn(t)
	cases := []struct {
		path string
		form url.Values
		want string
	}{
		{"/admin/users/9999/role", url.Values{"role": {"admin"}}, "Failed to update role."},
		{"/admin/users/9999/status", url.Values{"disabled": {"true"}}, "Failed to update user status."},
		{"/admin/users/9999/invite", nil, "Failed to issue a link."},
		{"/admin/users/9999/masquerade", nil, "Failed to start masquerade."},
		{"/admin/users/9999/invite/revoke", nil, "There is no outstanding link for that account."},
	}
	for _, c := range cases {
		f.expectFlash(t, c.path, c.form, "/admin", c.want)
	}
}

func TestAdminFormsReportRefusedWrites(t *testing.T) {
	cases := []struct {
		name, table, event, path string
		form                     url.Values
		want                     string
	}{
		{"role", "user", "update", "/admin/users/{id}/role", url.Values{"role": {"admin"}}, "Failed to update role."},
		{"disable", "user", "update", "/admin/users/{id}/status", url.Values{"disabled": {"true"}}, "Failed to update user status."},
		{"delete", "user", "delete", "/admin/users/{id}/delete", nil, "Failed to delete user."},
		{"reissue", "user_invite", "insert", "/admin/users/{id}/invite", nil, "Failed to issue a link."},
		{"revoke", "user_invite", "delete", "/admin/users/{id}/invite/revoke", nil, "Failed to revoke the link."},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			f := newSignedIn(t)
			bob := f.makeUser(t, "bob")
			if c.name == "revoke" {
				f.postForm(idPath("/admin/users/{id}/invite", bob.ID), nil)
			}
			f.refuseWrites(t, c.table, c.event)
			f.expectFlash(t, idPath(c.path, bob.ID), c.form, "/admin", c.want)
			if found, _ := models.FindUserByUsername(context.Background(), f.database, "bob"); found == nil && c.name != "delete" {
				t.Error("the account vanished")
			}
		})
	}
}

func TestAdminCreateUserLeavesNoHalfMadeAccountWhenTheInviteFails(t *testing.T) {
	f := newSignedIn(t)
	f.refuseWrites(t, "user_invite", "insert")
	f.expectFlash(t, "/admin/users", url.Values{"username": {"carol"}, "role": {"user"}}, "/admin", "Failed to create the account.")
	if carol, err := models.FindUserByUsername(context.Background(), f.database, "carol"); err != nil || carol != nil {
		t.Errorf("the failed creation kept the username: %+v, %v", carol, err)
	}
}
