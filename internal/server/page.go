package server

import (
	"net/http"
	"net/url"
	"strconv"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/flash"
	"github.com/henry40408/rdrs/internal/models"
)

// requirePage authenticates a page request. An anonymous reader is sent to the
// login page with a warning, remembering the page to return to after sign-in.
func (a *App) requirePage(w http.ResponseWriter, r *http.Request) (*auth.Principal, bool) {
	if principal := a.Auth.AuthenticatePage(r); principal != nil {
		return principal, true
	}
	flash.Redirect(w, loginLocation(r), flash.NewWarning("Please log in to continue."))
	return nil, false
}

// loginLocation is /login, with ?next= when the request is a page GET worth
// coming back to.
func loginLocation(r *http.Request) string {
	target := r.URL.RequestURI()
	if r.Method != http.MethodGet || target == "/" {
		return "/login"
	}
	next, ok := SafeReturnTo(target, IsLoginDestination)
	if !ok {
		return "/login"
	}
	return "/login?" + url.Values{"next": {next}}.Encode()
}

// pathID is the {id} path segment. A value that is not an integer is a 400,
// as axum's Path extractor answers.
func pathID(w http.ResponseWriter, r *http.Request) (int64, bool) {
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil {
		rejection(w, http.StatusBadRequest, "Invalid URL: Cannot parse `id` with value `"+r.PathValue("id")+"`: invalid digit found in string")
		return 0, false
	}
	return id, true
}

// chronoUTC renders t as serde does a chrono DateTime<Utc>: "Z" for the zone
// and a fraction of 3, 6 or 9 digits only when the sub-second part is non-zero.
func chronoUTC(t time.Time) string {
	t = t.UTC()
	nanos := t.Nanosecond()
	switch {
	case nanos == 0:
		return t.Format("2006-01-02T15:04:05Z")
	case nanos%1_000_000 == 0:
		return t.Format("2006-01-02T15:04:05.000Z")
	case nanos%1_000 == 0:
		return t.Format("2006-01-02T15:04:05.000000Z")
	}
	return t.Format("2006-01-02T15:04:05.000000000Z")
}

// userView is the JSON of a user; the password hash is never serialised.
type userView struct {
	ID         int64       `json:"id"`
	Username   string      `json:"username"`
	Role       models.Role `json:"role"`
	DisabledAt *string     `json:"disabled_at"`
	CreatedAt  string      `json:"created_at"`
}

func userJSON(user *models.User) userView {
	view := userView{ID: user.ID, Username: user.Username, Role: user.Role, CreatedAt: chronoUTC(user.CreatedAt)}
	if user.DisabledAt != nil {
		disabled := chronoUTC(*user.DisabledAt)
		view.DisabledAt = &disabled
	}
	return view
}
