package server

import (
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"testing"
)

// TestHandlersAnswer500WhenTheirTableIsUnavailable removes one table behind a
// signed-in session, so authentication keeps working while the handlers that
// need the table hit a database error. Each must answer 500, not a partial
// page or a panic.
func TestHandlersAnswer500WhenTheirTableIsUnavailable(t *testing.T) {
	entryRoutes := func(id int64) []string {
		return []string{
			fmt.Sprintf("/api/entries/%d/neighbors", id),
			fmt.Sprintf("/api/entries/%d/position", id),
			fmt.Sprintf("/entries/%d/fragment", id),
			fmt.Sprintf("/entries/%d/summary/fragment", id),
		}
	}
	cases := map[string]func(entryID int64) []string{
		"entry": func(id int64) []string {
			return append(entryRoutes(id), "/api/sidebar/categories/1/feeds", "/reader/api/0/unread-count")
		},
		"feed": func(id int64) []string {
			return append(entryRoutes(id), "/feeds/1/entries", "/feeds/1/edit", "/api/feeds/1/icon",
				"/api/sidebar/categories/1/feeds", "/reader/api/0/subscription/list", "/reader/api/0/unread-count")
		},
		"category": func(id int64) []string {
			return append(entryRoutes(id), "/categories/1/entries", "/feeds/1/entries", "/feeds/1/edit",
				"/reader/api/0/subscription/list", "/reader/api/0/tag/list", "/reader/api/0/unread-count")
		},
		"user_settings": func(id int64) []string {
			return []string{fmt.Sprintf("/entries/%d/fragment", id), "/api/offline/manifest"}
		},
		"entry_summary": func(id int64) []string {
			return []string{fmt.Sprintf("/entries/%d/summary/fragment", id), fmt.Sprintf("/api/entries/%d/summary", id)}
		},
		"passkey": func(int64) []string { return []string{"/api/passkeys"} },
	}
	for table, routes := range cases {
		t.Run(table, func(t *testing.T) {
			app := defaultITestApp(t)
			_, entryID := app.SeedUserWithEntry("alice", fixturePassword)
			app.Login("alice")
			if _, err := app.DB.ExecContext(t.Context(), "ALTER TABLE "+table+" RENAME TO "+table+"_gone"); err != nil {
				t.Fatal(err)
			}
			for _, path := range routes(entryID) {
				if got := app.Get(path).Code; got != http.StatusInternalServerError {
					t.Errorf("GET %s = %d, want 500", path, got)
				}
			}
		})
	}
}

// TestWritesThatTheDatabaseRefusesAreReportedNotSwallowed makes every write to
// one table abort, then submits the form that writes to it. The user must be
// told it failed, not shown a success.
func TestWritesThatTheDatabaseRefusesAreReportedNotSwallowed(t *testing.T) {
	cases := []struct {
		table, path string
		form        url.Values
		want        string
	}{
		{"category", "/categories", url.Values{"name": {"New"}}, "Failed to create category."},
		{"category", "/categories/1/rename", url.Values{"name": {"Renamed"}}, "Failed to rename category."},
		{"category", "/categories/1/delete", nil, "Failed to delete category."},
		{"feed", "/feeds/1/delete", nil, "Failed to delete feed."},
	}
	for _, c := range cases {
		t.Run(c.path, func(t *testing.T) {
			app := defaultITestApp(t)
			app.SeedUserWithEntry("alice", fixturePassword)
			app.Login("alice")
			for _, event := range []string{"INSERT", "UPDATE", "DELETE"} {
				trigger := fmt.Sprintf("CREATE TRIGGER refuse_%s BEFORE %s ON %s BEGIN SELECT RAISE(ABORT, 'refused'); END", strings.ToLower(event), event, c.table)
				if _, err := app.DB.ExecContext(t.Context(), trigger); err != nil {
					t.Fatal(err)
				}
			}
			response := app.PostForm(c.path, c.form)
			if got := flashText(t, response); !strings.Contains(got, `"level":"error"`) || !strings.Contains(got, c.want) {
				t.Errorf("flash = %s, want an error saying %q", got, c.want)
			}
		})
	}
}

// TestEntryActionsAnswer500WhenTheirTableIsUnavailable pins which table each
// reading-pane action needs: a lost table is a server error, not a swap that
// shows the action as done.
func TestEntryActionsAnswer500WhenTheirTableIsUnavailable(t *testing.T) {
	every := []string{"star", "unstar", "read", "unread", "summarize", "summarize/cancel", "fetch-full-content", "save"}
	cases := map[string][]string{
		"entry":         every,
		"feed":          every,
		"entry_summary": {"star", "unstar", "read", "unread", "summarize", "summarize/cancel"},
		"user_settings": {"save"},
	}
	for table, actions := range cases {
		t.Run(table, func(t *testing.T) {
			f := newSignedIn(t)
			id := f.withEntries(t)[0]
			f.mustExec(t, "ALTER TABLE "+table+" RENAME TO "+table+"_gone")
			for _, action := range actions {
				path := "/entries/" + itoa(id) + "/" + action
				if got := f.postForm(path, nil).Code; got != http.StatusInternalServerError {
					t.Errorf("POST %s = %d, want 500", path, got)
				}
			}
		})
	}
}
