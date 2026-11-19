package server

import (
	"crypto/rand"
	"encoding/json"
	"errors"
	"net/http"
	"strings"

	"github.com/go-webauthn/webauthn/protocol"
	"github.com/go-webauthn/webauthn/webauthn"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/models"
)

func (a *App) passkeyRoutes() []Route {
	return []Route{
		{"POST", "/api/passkey/register/start", a.passkeyRegisterStart},
		{"POST", "/api/passkey/register/finish", a.passkeyRegisterFinish},
		{"POST", "/api/passkey/auth/start", a.passkeyAuthStart},
		{"POST", "/api/passkey/auth/finish", a.passkeyAuthFinish},
	}
}

var errChallengeNotFound = newError(http.StatusBadRequest, "Challenge not found or expired")

// relyingParty is nil when the RP settings were invalid; Rust refused to start
// in that case, here the ceremonies answer 500.
func (a *App) relyingParty() (*webauthn.WebAuthn, error) {
	if a.webauthn == nil {
		return nil, InternalError(a.webauthnErr)
	}
	return a.webauthn, nil
}

// passkeyRegisterStart is POST /api/passkey/register/start. Re-authentication
// is checked here, not at finish: the challenge is single-use, so a 403 at
// finish would consume it and strand the retry.
func (a *App) passkeyRegisterStart(w http.ResponseWriter, r *http.Request) {
	principal, err := a.Auth.RecentlyAuthenticated(r)
	if err != nil {
		writeError(w, authError(err))
		return
	}
	if principal.Session.IsMasquerading() {
		writeError(w, ErrMasqueradeBlocksPasskeys)
		return
	}
	relyingParty, err := a.relyingParty()
	if err != nil {
		writeError(w, err)
		return
	}
	existing, err := models.ListUserPasskeys(r.Context(), a.DB, principal.User.ID)
	if err != nil {
		writeError(w, DatabaseError(err))
		return
	}
	exclude := make([]protocol.CredentialDescriptor, 0, len(existing))
	for _, p := range existing {
		exclude = append(exclude, protocol.CredentialDescriptor{Type: protocol.PublicKeyCredentialType, CredentialID: p.CredentialID})
	}
	handle := rand.Text()
	creation, session, err := relyingParty.BeginRegistration(
		auth.PasskeyUser{Handle: []byte(handle), Name: principal.User.Username},
		auth.RegistrationOptions(exclude)...)
	if err != nil {
		writeError(w, ValidationError("Passkey registration failed: "+err.Error()))
		return
	}
	if err := a.storeChallenge(r, &principal.User.ID, models.ChallengeRegistration, creation.Response.Challenge, session); err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"options": creation})
}

func (a *App) storeChallenge(r *http.Request, userID *int64, kind models.ChallengeType, challenge []byte, session *webauthn.SessionData) error {
	state, err := json.Marshal(session)
	if err != nil {
		return InternalError(err)
	}
	_, err = models.CreateChallenge(r.Context(), a.DB, models.NewChallenge{
		Challenge: challenge, UserID: userID, ChallengeType: kind, StateData: string(state),
	})
	if err != nil {
		return DatabaseError(err)
	}
	return nil
}

// takeChallenge consumes the pending ceremony of this owner and type.
func (a *App) takeChallenge(r *http.Request, userID *int64, kind models.ChallengeType) (*webauthn.SessionData, error) {
	challenge, err := models.TakeChallenge(r.Context(), a.DB, userID, kind)
	if errors.Is(err, models.ErrChallengeNotFound) {
		return nil, errChallengeNotFound
	}
	if err != nil {
		return nil, DatabaseError(err)
	}
	var session webauthn.SessionData
	if err := json.Unmarshal([]byte(challenge.StateData), &session); err != nil {
		return nil, InternalError(err)
	}
	return &session, nil
}

// passkeyBody is the finish request: the credential stays raw for the
// WebAuthn parser, which owns its shape.
type passkeyBody struct {
	Name       *string         `json:"name"`
	Credential json.RawMessage `json:"credential"`
}

func readPasskeyBody(w http.ResponseWriter, r *http.Request) (passkeyBody, bool) {
	var body passkeyBody
	if !decodeJSON(w, r, &body) {
		return body, false
	}
	if len(body.Credential) == 0 {
		rejection(w, http.StatusUnprocessableEntity,
			"Failed to deserialize the JSON body into the target type: missing field `credential`")
		return body, false
	}
	return body, true
}

// passkeyRegisterFinish is POST /api/passkey/register/finish. It takes a plain
// session: freshness was checked at start, and re-checking here could fail
// after the challenge is spent.
func (a *App) passkeyRegisterFinish(w http.ResponseWriter, r *http.Request) {
	principal, ok := a.requireUser(w, r)
	if !ok {
		return
	}
	if principal.Session.IsMasquerading() {
		writeError(w, ErrMasqueradeBlocksPasskeys)
		return
	}
	body, ok := readPasskeyBody(w, r)
	if !ok {
		return
	}
	if body.Name == nil {
		rejection(w, http.StatusUnprocessableEntity,
			"Failed to deserialize the JSON body into the target type: missing field `name`")
		return
	}
	if *body.Name == "" {
		writeError(w, ValidationError("Passkey name is required"))
		return
	}
	created, err := a.finishRegistration(r, principal, *body.Name, body.Credential)
	if err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusCreated, map[string]any{"id": created.ID, "name": created.Name})
}

func (a *App) finishRegistration(r *http.Request, principal *auth.Principal, name string, raw []byte) (*models.Passkey, error) {
	relyingParty, err := a.relyingParty()
	if err != nil {
		return nil, err
	}
	userID := principal.User.ID
	session, err := a.takeChallenge(r, &userID, models.ChallengeRegistration)
	if err != nil {
		return nil, err
	}
	parsed, err := protocol.ParseCredentialCreationResponseBytes(raw)
	if err != nil {
		return nil, ValidationError("Passkey registration failed: " + err.Error())
	}
	user := auth.PasskeyUser{Handle: session.UserID, Name: principal.User.Username}
	credential, err := relyingParty.CreateCredential(user, *session, parsed)
	if err != nil {
		return nil, ValidationError("Passkey registration failed: " + err.Error())
	}
	return a.savePasskey(r, principal, name, credential)
}

func (a *App) savePasskey(r *http.Request, principal *auth.Principal, name string, credential *webauthn.Credential) (*models.Passkey, error) {
	stored, err := auth.EncodeLegacyPasskey(*credential)
	if err != nil {
		return nil, InternalError(err)
	}
	created, err := models.CreatePasskey(r.Context(), a.DB, models.NewPasskey{
		UserID: principal.User.ID, CredentialID: credential.ID, PublicKey: stored, Name: name,
		Transports: joinTransports(credential.Transport),
	})
	if err != nil {
		return nil, DatabaseError(err)
	}
	a.Audit.PasskeyRegistered(audit.PasskeyRegistered{
		Token: principal.Session.SessionToken, UserID: principal.User.ID, PasskeyID: created.ID,
		Name: created.Name, Request: a.request(r),
	})
	return created, nil
}

func joinTransports(transports []protocol.AuthenticatorTransport) *string {
	if len(transports) == 0 {
		return nil
	}
	names := make([]string, len(transports))
	for i, transport := range transports {
		names[i] = strings.ToLower(string(transport))
	}
	joined := strings.Join(names, ",")
	return &joined
}

// passkeyAuthStart is POST /api/passkey/auth/start. It names no credentials,
// since listing them would leak every passkey id to unauthenticated callers,
// and it is rate limited because each call writes a challenge row.
func (a *App) passkeyAuthStart(w http.ResponseWriter, r *http.Request) {
	const endpoint = "POST /api/passkey/auth/start"
	ip := a.clientIP(r)
	if decision := a.Limiter.TryAcquire(auth.BucketPasskeyProbe, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "passkey_probe", endpoint, "passkey_probe")
		writeError(w, TooManyRequests(decision.RetryAfterSecs))
		return
	}
	relyingParty, err := a.relyingParty()
	if err != nil {
		writeError(w, err)
		return
	}
	assertion, session, err := relyingParty.BeginDiscoverableLogin(auth.LoginOptions()...)
	if err != nil {
		writeError(w, PasskeyAuthenticationFailed(err.Error()))
		return
	}
	if err := a.storeChallenge(r, nil, models.ChallengeAuthentication, assertion.Response.Challenge, session); err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"options": assertion})
}

// passkeyAuthFinish is POST /api/passkey/auth/finish.
func (a *App) passkeyAuthFinish(w http.ResponseWriter, r *http.Request) {
	const endpoint = "POST /api/passkey/auth/finish"
	body, ok := readPasskeyBody(w, r)
	if !ok {
		return
	}
	ip := a.clientIP(r)
	// Reserved before any work an attacker's guess could spend.
	if decision := a.Limiter.TryAcquire(auth.BucketLogin, ip); !decision.Allowed {
		a.logRateLimited(ip.String(), "login", endpoint, "login")
		writeError(w, TooManyRequests(decision.RetryAfterSecs))
		return
	}
	user, err := a.finishAuthentication(r, body.Credential)
	if err != nil {
		writeError(w, err)
		return
	}
	// Released so successful sign-ins never lock a user out.
	a.Limiter.Release(auth.BucketLogin, ip)
	if err := a.startPasskeySession(w, r, user); err != nil {
		writeError(w, err)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"id": user.ID, "username": user.Username})
}

func (a *App) finishAuthentication(r *http.Request, raw []byte) (*models.User, error) {
	relyingParty, err := a.relyingParty()
	if err != nil {
		return nil, err
	}
	session, err := a.takeChallenge(r, nil, models.ChallengeAuthentication)
	if err != nil {
		return nil, err
	}
	parsed, err := protocol.ParseCredentialRequestResponseBytes(raw)
	if err != nil {
		return nil, PasskeyAuthenticationFailed(err.Error())
	}
	// The client-supplied id only selects which public key to verify against.
	stored, err := models.FindPasskeyByCredentialID(r.Context(), a.DB, parsed.RawID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if stored == nil {
		return nil, ErrPasskeyNotFound
	}
	user, err := models.FindUserByID(r.Context(), a.DB, stored.UserID)
	if err != nil {
		return nil, DatabaseError(err)
	}
	if user == nil {
		return nil, ErrUserNotFound
	}
	if user.IsDisabled() {
		return nil, ErrUserDisabled
	}
	credential, err := auth.DecodeLegacyPasskey(stored.PublicKey, uint32(stored.Counter))
	if err != nil {
		return nil, InternalError(err)
	}
	verified, err := verifyAssertion(relyingParty, *session, parsed, credential)
	if err != nil {
		return nil, err
	}
	if err := models.UpdatePasskeyCounter(r.Context(), a.DB, stored.ID, int64(verified.Authenticator.SignCount)); err != nil {
		return nil, DatabaseError(err)
	}
	return user, nil
}

// verifyAssertion checks the assertion against the single stored credential,
// which stands in for the allow-list, and refuses a counter that went back.
func verifyAssertion(relyingParty *webauthn.WebAuthn, session webauthn.SessionData,
	parsed *protocol.ParsedCredentialAssertionData, credential webauthn.Credential) (*webauthn.Credential, error) {
	lookup := func(_, userHandle []byte) (webauthn.User, error) {
		return auth.PasskeyUser{Handle: userHandle, Credentials: []webauthn.Credential{credential}}, nil
	}
	_, verified, err := relyingParty.ValidatePasskeyLogin(lookup, session, parsed)
	if err != nil {
		return nil, PasskeyAuthenticationFailed(err.Error())
	}
	if verified.Authenticator.CloneWarning {
		return nil, PasskeyAuthenticationFailed("Credential possible compromise: the signature counter went back")
	}
	return verified, nil
}

func (a *App) startPasskeySession(w http.ResponseWriter, r *http.Request, user *models.User) error {
	request := a.request(r)
	session, err := models.CreateSession(r.Context(), a.DB, models.NewSession{
		UserID: user.ID, UserAgent: request.UserAgent, IPAddress: request.IP,
	})
	if err != nil {
		return DatabaseError(err)
	}
	a.Audit.SessionCreated(audit.CredentialCreated{
		Token: session.SessionToken, UserID: user.ID, Method: "passkey", Request: request,
	})
	http.SetCookie(w, a.cookies.Session(session.SessionToken))
	http.SetCookie(w, a.cookies.CSRF(session.SessionToken))
	return nil
}
