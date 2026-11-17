package server

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/models"
)

const verifiedPresence = flagPresent | flagVerified

func (f *signedInFixture) useRelyingParty(t *testing.T) {
	t.Helper()
	relyingParty, err := auth.NewWebAuthn(config.Config{
		WebAuthnRPID: testRPID, WebAuthnRPOrigin: testOrigin, WebAuthnRPName: "rdrs",
	})
	if err != nil {
		t.Fatal(err)
	}
	f.app.webauthn = relyingParty
}

func (f *signedInFixture) postJSON(path string, body any) *httptest.ResponseRecorder {
	encoded, _ := json.Marshal(body)
	request := jsonRequest(http.MethodPost, path, string(encoded))
	request.Header.Set("X-CSRF-Token", f.csrf)
	request.AddCookie(f.session)
	return f.do(request)
}

// anonymousPost is a sign-in request: no cookies at all.
func (f *authFixture) anonymousPost(path string, body any) *httptest.ResponseRecorder {
	encoded, _ := json.Marshal(body)
	return f.do(jsonRequest(http.MethodPost, path, string(encoded)))
}

func creationOptions(t *testing.T, rec *httptest.ResponseRecorder) (challenge string, userHandle []byte, options map[string]any) {
	t.Helper()
	if rec.Code != http.StatusOK {
		t.Fatalf("start = %d %s", rec.Code, rec.Body)
	}
	var body struct {
		Options struct {
			PublicKey map[string]any `json:"publicKey"`
		} `json:"options"`
	}
	if err := json.Unmarshal(rec.Body.Bytes(), &body); err != nil {
		t.Fatal(err)
	}
	options = body.Options.PublicKey
	challenge, _ = options["challenge"].(string)
	if user, ok := options["user"].(map[string]any); ok {
		id, _ := user["id"].(string)
		userHandle = decodeB64(t, id)
	}
	return challenge, userHandle, options
}

func (f *signedInFixture) registerPasskey(t *testing.T, name string) *softPasskey {
	t.Helper()
	passkey := newSoftPasskey(t)
	challenge, handle, _ := creationOptions(t, f.postAPI("/api/passkey/register/start"))
	passkey.userHandle = handle
	rec := f.postJSON("/api/passkey/register/finish",
		map[string]any{"name": name, "credential": passkey.registration(challenge, verifiedPresence)})
	if rec.Code != http.StatusCreated {
		t.Fatalf("finish = %d %s", rec.Code, rec.Body)
	}
	return passkey
}

func (f *signedInFixture) startSignIn(t *testing.T) string {
	t.Helper()
	challenge, _, _ := creationOptions(t, f.anonymousPost("/api/passkey/auth/start", nil))
	return challenge
}

func TestRegisterStartRequiresADiscoverableVerifiedCredential(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	_, _, options := creationOptions(t, f.postAPI("/api/passkey/register/start"))
	selection, _ := options["authenticatorSelection"].(map[string]any)
	if selection["residentKey"] != "required" || selection["requireResidentKey"] != true || selection["userVerification"] != "required" {
		t.Errorf("selection = %v", selection)
	}
	if options["attestation"] != "none" {
		t.Errorf("attestation = %v", options["attestation"])
	}
}

func TestRegisteredPasskeySignsIn(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	passkey := f.registerPasskey(t, "Laptop")

	stored, err := models.ListUserPasskeys(context.Background(), f.database, 1)
	if err != nil || len(stored) != 1 || stored[0].Name != "Laptop" {
		t.Fatalf("stored = %v %v", stored, err)
	}
	if stored[0].Transports == nil || *stored[0].Transports != "internal" {
		t.Errorf("transports = %v", stored[0].Transports)
	}
	if !strings.Contains(string(stored[0].PublicKey), `"EC_EC2"`) || !strings.Contains(string(stored[0].PublicKey), `"ES256"`) {
		t.Errorf("public key is not in the webauthn-rs shape: %s", stored[0].PublicKey)
	}

	challenge := f.startSignIn(t)
	rec := f.anonymousPost("/api/passkey/auth/finish", map[string]any{"credential": passkey.assertion(challenge, verifiedPresence)})
	if rec.Code != http.StatusOK {
		t.Fatalf("sign in = %d %s", rec.Code, rec.Body)
	}
	if body := decodeBody(t, rec); body["username"] != "root" {
		t.Errorf("body = %v", body)
	}
	if cookieNamed(rec, auth.SessionCookieName) == nil || cookieNamed(rec, auth.CSRFCookieName) == nil {
		t.Error("sign-in set no session and CSRF cookies")
	}
	after, _ := models.FindPasskeyByID(context.Background(), f.database, stored[0].ID)
	if after.Counter != 1 || after.LastUsedAt == nil {
		t.Errorf("counter = %d, last used = %v", after.Counter, after.LastUsedAt)
	}
}

func TestSignInChallengeIsSingleUse(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	passkey := f.registerPasskey(t, "Laptop")
	challenge := f.startSignIn(t)
	credential := map[string]any{"credential": passkey.assertion(challenge, verifiedPresence)}
	if rec := f.anonymousPost("/api/passkey/auth/finish", credential); rec.Code != http.StatusOK {
		t.Fatalf("first = %d", rec.Code)
	}
	replay := f.anonymousPost("/api/passkey/auth/finish", credential)
	if replay.Code != http.StatusBadRequest || !strings.Contains(replay.Body.String(), "Challenge not found or expired") {
		t.Errorf("replay = %d %s", replay.Code, replay.Body)
	}
}

func TestSignInRefusals(t *testing.T) {
	cases := map[string]struct {
		status int
		mutate func(f *signedInFixture, p *softPasskey, challenge string) map[string]any
	}{
		"unverified user": {http.StatusUnauthorized, func(_ *signedInFixture, p *softPasskey, c string) map[string]any {
			return p.assertion(c, flagPresent)
		}},
		"foreign origin": {http.StatusUnauthorized, func(_ *signedInFixture, p *softPasskey, c string) map[string]any {
			p.counter++
			return p.signedAssertion(c, verifiedPresence, "https://evil.example")
		}},
		"unknown credential": {http.StatusNotFound, func(_ *signedInFixture, p *softPasskey, c string) map[string]any {
			p.credentialID = append([]byte("x"), p.credentialID...)
			return p.assertion(c, verifiedPresence)
		}},
		"wrong challenge": {http.StatusUnauthorized, func(_ *signedInFixture, p *softPasskey, _ string) map[string]any {
			return p.assertion(b64([]byte("not the challenge, 16+ bytes")), verifiedPresence)
		}},
	}
	for name, c := range cases {
		f := newSignedIn(t)
		f.useRelyingParty(t)
		passkey := f.registerPasskey(t, "Laptop")
		challenge := f.startSignIn(t)
		rec := f.anonymousPost("/api/passkey/auth/finish", map[string]any{"credential": c.mutate(f, passkey, challenge)})
		if rec.Code != c.status || cookieNamed(rec, auth.SessionCookieName) != nil {
			t.Errorf("%s = %d %s", name, rec.Code, rec.Body)
		}
	}
}

func TestDisabledUserCannotSignInWithAPasskey(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	passkey := f.registerPasskey(t, "Laptop")
	if _, err := f.database.ExecContext(context.Background(), "UPDATE user SET disabled_at = datetime('now')"); err != nil {
		t.Skipf("cannot disable the fixture user: %v", err)
	}
	challenge := f.startSignIn(t)
	rec := f.anonymousPost("/api/passkey/auth/finish", map[string]any{"credential": passkey.assertion(challenge, verifiedPresence)})
	if rec.Code != http.StatusForbidden {
		t.Errorf("= %d %s", rec.Code, rec.Body)
	}
}

func TestSignInStartNamesNoCredentialsAndIsRateLimited(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	passkey := f.registerPasskey(t, "Laptop")
	f.app.Limiter = auth.NewRateLimiter(5, 60)
	for range 5 {
		rec := f.anonymousPost("/api/passkey/auth/start", nil)
		if rec.Code != http.StatusOK || strings.Contains(rec.Body.String(), b64(passkey.credentialID)) {
			t.Fatalf("start = %d %s", rec.Code, rec.Body)
		}
		var body struct {
			Options struct {
				PublicKey map[string]any `json:"publicKey"`
			} `json:"options"`
		}
		_ = json.Unmarshal(rec.Body.Bytes(), &body)
		if allow, present := body.Options.PublicKey["allowCredentials"]; present && len(allow.([]any)) != 0 {
			t.Fatalf("allowCredentials = %v", allow)
		}
	}
	if rec := f.anonymousPost("/api/passkey/auth/start", nil); rec.Code != http.StatusTooManyRequests {
		t.Errorf("sixth = %d", rec.Code)
	}
}

func TestRegisterStartNeedsRecentAuthentication(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	if rec := f.postAPI("/api/passkey/register/start"); rec.Code != http.StatusOK {
		t.Fatalf("fresh = %d", rec.Code)
	}
	if _, err := f.database.ExecContext(context.Background(), "UPDATE session SET last_authenticated_at = NULL"); err != nil {
		t.Fatal(err)
	}
	rec := f.postAPI("/api/passkey/register/start")
	if rec.Code != http.StatusForbidden || !strings.Contains(rec.Body.String(), "Reauthentication required") {
		t.Errorf("stale = %d %s", rec.Code, rec.Body)
	}
}

func TestRegisterFinishRefusals(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	passkey := newSoftPasskey(t)
	if rec := f.postJSON("/api/passkey/register/finish", map[string]any{"name": "", "credential": map[string]any{}}); rec.Code != http.StatusBadRequest {
		t.Errorf("empty name = %d %s", rec.Code, rec.Body)
	}
	noChallenge := f.postJSON("/api/passkey/register/finish", map[string]any{"name": "x", "credential": passkey.registration("AAAA", verifiedPresence)})
	if noChallenge.Code != http.StatusBadRequest || !strings.Contains(noChallenge.Body.String(), "Challenge not found or expired") {
		t.Errorf("no challenge = %d %s", noChallenge.Code, noChallenge.Body)
	}
	challenge, _, _ := creationOptions(t, f.postAPI("/api/passkey/register/start"))
	unverified := f.postJSON("/api/passkey/register/finish", map[string]any{"name": "x", "credential": passkey.registration(challenge, flagPresent)})
	if unverified.Code != http.StatusBadRequest {
		t.Errorf("unverified = %d %s", unverified.Code, unverified.Body)
	}
	if left, _ := models.ListUserPasskeys(context.Background(), f.database, 1); len(left) != 0 {
		t.Errorf("a refused registration stored %d passkeys", len(left))
	}
}

func TestRegisterStartExcludesExistingPasskeys(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	passkey := f.registerPasskey(t, "Laptop")
	_, _, options := creationOptions(t, f.postAPI("/api/passkey/register/start"))
	excluded, _ := options["excludeCredentials"].([]any)
	if len(excluded) != 1 || excluded[0].(map[string]any)["id"] != b64(passkey.credentialID) {
		t.Errorf("excludeCredentials = %v", excluded)
	}
}

func TestSignInRefusesACounterThatWentBack(t *testing.T) {
	f := newSignedIn(t)
	f.useRelyingParty(t)
	passkey := f.registerPasskey(t, "Laptop")
	passkey.counter = 4
	first := f.anonymousPost("/api/passkey/auth/finish",
		map[string]any{"credential": passkey.signedAssertion(f.startSignIn(t), verifiedPresence, testOrigin)})
	if first.Code != http.StatusOK {
		t.Fatalf("first = %d %s", first.Code, first.Body)
	}
	passkey.counter = 2
	cloned := f.anonymousPost("/api/passkey/auth/finish",
		map[string]any{"credential": passkey.signedAssertion(f.startSignIn(t), verifiedPresence, testOrigin)})
	if cloned.Code != http.StatusUnauthorized || cookieNamed(cloned, auth.SessionCookieName) != nil {
		t.Errorf("rollback = %d %s", cloned.Code, cloned.Body)
	}
	stored, _ := models.ListUserPasskeys(context.Background(), f.database, 1)
	if stored[0].Counter != 4 {
		t.Errorf("counter = %d, want 4", stored[0].Counter)
	}
}
