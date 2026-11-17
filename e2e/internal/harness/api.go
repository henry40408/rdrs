package harness

import (
	"bytes"
	"crypto/rand"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/cookiejar"
	"net/url"
	"regexp"
	"strings"
)

// Password is the password of every account this suite creates.
const Password = "vulture-mango-77-quilt"

var invitePathPattern = regexp.MustCompile(`/invite/[A-Za-z0-9_-]+`)

// Credentials is a username and password pair.
type Credentials struct {
	Username string
	Password string
}

// Session is a Cookie header value plus the CSRF token to echo on mutations.
type Session struct {
	Cookie string
	CSRF   string
}

// RandomSlug is eight lowercase alphanumerics; accounts are never cleaned up
// within a server, so each scenario needs a unique name.
func RandomSlug() string {
	const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789"
	raw := make([]byte, 8)
	if _, err := rand.Read(raw); err != nil {
		panic(fmt.Sprintf("reading random bytes: %v", err))
	}
	for i, value := range raw {
		raw[i] = alphabet[int(value)%len(alphabet)]
	}
	return string(raw)
}

// API talks to one server's account endpoints. rdrs has no public sign-up, so
// the first account claims the one-time /api/setup and later ones go through
// the real admin and invite flow.
type API struct {
	server *Server
	client *http.Client
	admin  *Credentials
}

// NewAPI returns the account API of a server. It does not follow redirects:
// the invite link lives only in the flash of a 303 a following client would
// consume.
func NewAPI(server *Server) *API {
	return &API{
		server: server,
		client: &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		}},
	}
}

// Register creates an account with a password, whatever it takes.
func (a *API) Register(username, password string) error {
	admin, err := a.ensureAdmin()
	if err != nil {
		return err
	}
	if admin.Username == username {
		return nil
	}
	invite, err := a.InviteAccount(username)
	if err != nil {
		return err
	}
	return a.redeemInvite(invite, password)
}

// SetupFirstAccount claims the one-time setup endpoint, making username the
// admin.
func (a *API) SetupFirstAccount(username, password string) error {
	return a.claimSetup(username, password)
}

// InviteAccount creates an account and returns its unredeemed one-time
// invite path.
func (a *API) InviteAccount(username string) (string, error) {
	admin, err := a.ensureAdmin()
	if err != nil {
		return "", err
	}
	session, err := a.Login(admin.Username, admin.Password)
	if err != nil {
		return "", err
	}
	return a.createAccount(session, username)
}

// Login signs in and returns the session cookie and the CSRF token.
func (a *API) Login(username, password string) (Session, error) {
	body, _ := json.Marshal(map[string]string{"username": username, "password": password})
	response, err := a.client.Post(a.server.BaseURL+"/api/session", "application/json", bytes.NewReader(body))
	if err != nil {
		return Session{}, fmt.Errorf("posting /api/session: %w", err)
	}
	defer response.Body.Close()
	if response.StatusCode >= 300 {
		return Session{}, statusError("login failed", response)
	}
	return sessionFrom(response), nil
}

// sessionFrom folds a response's cookies into a Session.
func sessionFrom(response *http.Response) Session {
	cookies := response.Cookies()
	pairs := make([]string, 0, len(cookies))
	csrf := ""
	for _, cookie := range cookies {
		pairs = append(pairs, cookie.Name+"="+cookie.Value)
		if cookie.Name == "csrf_token" {
			csrf = cookie.Value
		}
	}
	return Session{Cookie: strings.Join(pairs, "; "), CSRF: csrf}
}

func statusError(what string, response *http.Response) error {
	body, _ := io.ReadAll(response.Body)
	return fmt.Errorf("%s (%d): %s", what, response.StatusCode, body)
}

// ensureAdmin is this server's bootstrap admin, claiming /api/setup the first
// time.
func (a *API) ensureAdmin() (Credentials, error) {
	if a.admin != nil {
		return *a.admin, nil
	}
	admin := Credentials{Username: "e2e-bootstrap-" + RandomSlug(), Password: Password}
	if err := a.claimSetup(admin.Username, admin.Password); err != nil {
		return Credentials{}, err
	}
	a.admin = &admin
	return admin, nil
}

func (a *API) claimSetup(username, password string) error {
	body, _ := json.Marshal(map[string]string{
		"username":    username,
		"password":    password,
		"setup_token": a.server.SetupToken,
	})
	response, err := a.client.Post(a.server.BaseURL+"/api/setup", "application/json", bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("posting /api/setup: %w", err)
	}
	defer response.Body.Close()
	if response.StatusCode >= 300 {
		return statusError("setup failed", response)
	}
	return nil
}

// createAccount posts the new account and reads the one-time link off the
// page the 303 leads to: the link is shown once, in a flash, and the
// database holds only an HMAC.
func (a *API) createAccount(session Session, username string) (string, error) {
	form := url.Values{"username": {username}, "role": {"user"}}
	response, err := a.send(session, http.MethodPost, "/admin/users", form)
	if err != nil {
		return "", err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusSeeOther {
		return "", statusError("creating the account failed", response)
	}
	// The flash cookie rides on the 303; the next request carries it back.
	flash := sessionFrom(response)
	session.Cookie += "; " + flash.Cookie
	next, err := a.send(session, http.MethodGet, response.Header.Get("Location"), nil)
	if err != nil {
		return "", err
	}
	defer next.Body.Close()
	page, _ := io.ReadAll(next.Body)
	link := invitePathPattern.Find(page)
	if link == nil {
		return "", fmt.Errorf("no invite link on the page after creating the account")
	}
	return string(link), nil
}

func (a *API) send(session Session, method, path string, form url.Values) (*http.Response, error) {
	var body io.Reader
	if form != nil {
		body = strings.NewReader(form.Encode())
	}
	request, err := http.NewRequest(method, a.server.BaseURL+path, body)
	if err != nil {
		return nil, err
	}
	request.Header.Set("Cookie", session.Cookie)
	request.Header.Set("X-CSRF-Token", session.CSRF)
	if form != nil {
		request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	}
	response, err := a.client.Do(request)
	if err != nil {
		return nil, fmt.Errorf("%s %s: %w", method, path, err)
	}
	return response, nil
}

func (a *API) redeemInvite(invitePath, password string) error {
	// GET first: it mints the session and CSRF cookie the POST must echo.
	jar, _ := cookiejar.New(nil)
	client := &http.Client{Jar: jar, CheckRedirect: a.client.CheckRedirect}
	page, err := client.Get(a.server.BaseURL + invitePath)
	if err != nil {
		return fmt.Errorf("loading %s: %w", invitePath, err)
	}
	_ = page.Body.Close()
	csrf := ""
	base, _ := url.Parse(a.server.BaseURL)
	for _, cookie := range jar.Cookies(base) {
		if cookie.Name == "csrf_token" {
			csrf = cookie.Value
		}
	}
	form := url.Values{"password": {password}, "confirm_password": {password}}
	request, _ := http.NewRequest(http.MethodPost, a.server.BaseURL+invitePath, strings.NewReader(form.Encode()))
	request.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	request.Header.Set("X-CSRF-Token", csrf)
	response, err := client.Do(request)
	if err != nil {
		return fmt.Errorf("posting %s: %w", invitePath, err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusSeeOther {
		return statusError("invite redemption failed", response)
	}
	return nil
}
