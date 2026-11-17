package views

import (
	"strconv"

	"github.com/henry40408/rdrs/internal/flash"
)

// LoginPage is the data of GET /login.
type LoginPage struct {
	GitVersion string
	// SetupAvailable shows the first-run link; only when no accounts exist.
	SetupAvailable   bool
	FlashMessages    []flash.Message
	LocalAuthEnabled bool
	// CSRFToken is for the no-JS POST /login, from the anonymous session.
	CSRFToken string
	// Error is the sign-in error of the no-JS path.
	Error string
	// Next is the validated page to land on after sign-in.
	Next string
}

// Login renders the sign-in page.
func Login(page LoginPage) string {
	card := el("div", attrs(class("auth-card"), when(page.Next != "", a("data-next", page.Next))),
		flashStatic(page.FlashMessages),
		el("h1", nil, text("rdrs")),
		// Filled server-side after a failed native POST, or by login.js on the fetch path.
		el("div", attrs(a("id", "error"), a("class", "error"), when(page.Error == "", flag("hidden")),
			a("data-testid", "login-error")), text(page.Error)),
		passkeySection(page.LocalAuthEnabled),
		show(page.LocalAuthEnabled, loginForm(page)),
		show(page.SetupAvailable, el("div", attrs(class("link")),
			el("a", attrs(a("href", "/setup")), text("Set up this instance")))))
	return string(document(documentParts{
		title:      "Login - RDRS",
		gitVersion: page.GitVersion,
		head:       moduleScript(page.GitVersion, "login.js"),
		body:       el("div", attrs(class("auth-layout")), card),
	}))
}

func passkeySection(localAuthEnabled bool) node {
	return el("div", attrs(a("id", "passkey-section"), flag("hidden")),
		el("button", attrs(a("type", "button"), a("id", "passkey-login-btn"), a("class", "auth-passkey-btn")),
			text(" Login with Passkey ")),
		show(localAuthEnabled, el("p", attrs(class("muted", "auth-divider")), text("or use password"))))
}

func loginForm(page LoginPage) node {
	input := func(id, kind, autocomplete, testID string) node {
		return void("input", a("type", kind), a("id", id), a("name", id), flag("required"),
			a("autocomplete", autocomplete), a("data-testid", testID))
	}
	// Real action/method so a no-JS submit never puts the password in a GET URL.
	return el("form", attrs(a("id", "login-form"), a("method", "post"), a("action", "/login"),
		a("data-testid", "login-form")),
		csrfField(page.CSRFToken),
		text(" "),
		show(page.Next != "", group(void("input", a("type", "hidden"), a("name", "next"), a("value", page.Next)), text(" "))),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "username")), text("Username")), text(" "),
			input("username", "text", "username", "username-input")),
		el("div", attrs(class("form-group")),
			el("label", attrs(a("for", "password")), text("Password")), text(" "),
			input("password", "password", "current-password", "password-input")),
		el("button", attrs(a("type", "submit"), a("class", "btn-block"), a("data-testid", "login-submit")),
			text("Sign In")))
}

// passwordHint sits under a new-password field; shared by setup and invite.
func passwordHint(minLength int) node {
	return el("p", attrs(class("muted", "form-hint")),
		text("At least "), text(strconv.Itoa(minLength)),
		text(" characters. A phrase of a few unrelated words works well — no mixture of cases or symbols is required."))
}

// newPasswordInput is a password field bounded like the server's validation.
func newPasswordInput(id, name, testID string, minLength, maxLength int) node {
	return void("input", a("type", "password"), a("id", id), a("name", name), flag("required"),
		a("minlength", strconv.Itoa(minLength)), a("maxlength", strconv.Itoa(maxLength)),
		a("autocomplete", "new-password"), a("data-testid", testID))
}

// SetupPage is the data of GET /setup.
type SetupPage struct {
	GitVersion    string
	Error         string
	FlashMessages []flash.Message
	// PasswordMinLength and PasswordMaxLength match the server validation.
	PasswordMinLength int
	PasswordMaxLength int
	// CSRFToken is for the no-JS POST /setup.
	CSRFToken string
}

// Setup renders the first-run page.
func Setup(page SetupPage) string {
	minLen, maxLen := page.PasswordMinLength, page.PasswordMaxLength
	group1 := func(children ...node) node { return el("div", attrs(class("form-group")), children...) }
	form := el("form", attrs(a("id", "setup-form"), a("method", "post"), a("action", "/setup"),
		a("data-testid", "setup-form")),
		csrfField(page.CSRFToken),
		text(" "),
		group1(
			el("label", attrs(a("for", "setup-token")), text("Setup token")), text(" "),
			void("input", a("type", "text"), a("id", "setup-token"), a("name", "setup-token"), flag("required"),
				a("autocomplete", "off"), a("spellcheck", "false"), a("data-testid", "setup-token")),
			el("p", attrs(class("muted", "form-hint")),
				text("Printed on the server's stderr when rdrs started with no accounts."))),
		group1(
			el("label", attrs(a("for", "username")), text("Username")), text(" "),
			void("input", a("type", "text"), a("id", "username"), a("name", "username"), flag("required"),
				a("autocomplete", "username"), a("data-testid", "setup-username"))),
		group1(
			el("label", attrs(a("for", "password")), text("Password")), text(" "),
			newPasswordInput("password", "password", "setup-password", minLen, maxLen),
			passwordHint(minLen)),
		group1(
			el("label", attrs(a("for", "confirm-password")), text("Confirm Password")), text(" "),
			newPasswordInput("confirm-password", "confirm-password", "setup-confirm-password", minLen, maxLen)),
		el("button", attrs(a("type", "submit"), a("class", "btn-block"), a("data-testid", "setup-submit")),
			text("Create Account")))
	card := el("div", attrs(class("auth-card")),
		flashStatic(page.FlashMessages),
		el("h1", nil, text("rdrs")),
		el("p", attrs(class("muted", "auth-lead")),
			text("Create the administrator account for this instance. Everyone else is added from the admin panel afterwards.")),
		// Filled server-side after a failed native POST, or by setup.js on the fetch path.
		el("div", attrs(a("id", "error"), a("class", "error"), when(page.Error == "", flag("hidden")),
			a("data-testid", "setup-error")), text(page.Error)),
		form,
		el("div", attrs(class("link")), el("a", attrs(a("href", "/login")), text("Already have an account? Sign in"))))
	return string(document(documentParts{
		title:      "Set up rdrs",
		gitVersion: page.GitVersion,
		head:       moduleScript(page.GitVersion, "setup.js"),
		body:       el("div", attrs(class("auth-layout")), card),
	}))
}

// InvitePage is the "set your password" page: a form, a dead end or a
// throttled notice. The dead end must not reveal why (unknown, expired or
// used) or whose link it is.
type InvitePage struct {
	GitVersion string
	// CSRFToken is the anonymous-session token; empty when no form renders.
	CSRFToken string
	// Username is empty for the dead end; otherwise the form is for that account.
	Username string
	// Token is echoed into the form action so the POST lands on the same link.
	Token             string
	Error             string
	PasswordMinLength int
	PasswordMaxLength int
}

// Invite renders the invite redemption page.
func Invite(page InvitePage) string {
	var content node
	if page.Username != "" {
		content = inviteForm(page)
	} else {
		content = inviteDeadEnd(page)
	}
	card := el("div", attrs(class("auth-card")),
		el("h1", nil, text("rdrs")),
		content,
		el("div", attrs(class("link")), el("a", attrs(a("href", "/login")), text("Back to sign in"))))
	return string(document(documentParts{
		title:      "Set your password - RDRS",
		gitVersion: page.GitVersion,
		body:       el("div", attrs(class("auth-layout")), card),
	}))
}

func inviteError(message string) node {
	return el("div", attrs(class("error"), a("data-testid", "invite-error")), text(message))
}

func inviteForm(page InvitePage) node {
	minLen, maxLen := page.PasswordMinLength, page.PasswordMaxLength
	return group(
		show(page.Error != "", inviteError(page.Error)),
		el("p", attrs(class("muted", "auth-lead")), text("Choose a password for "),
			el("strong", nil, text(page.Username)), text(".")),
		// Plain form POST; the CSRF token comes from the anonymous session minted on GET.
		el("form", attrs(a("method", "post"), a("action", "/invite/"+page.Token), a("data-testid", "invite-form")),
			csrfField(page.CSRFToken),
			text(" "),
			el("div", attrs(class("form-group")),
				el("label", attrs(a("for", "password")), text("Password")), text(" "),
				newPasswordInput("password", "password", "invite-password", minLen, maxLen),
				passwordHint(minLen)),
			el("div", attrs(class("form-group")),
				el("label", attrs(a("for", "confirm-password")), text("Confirm Password")), text(" "),
				newPasswordInput("confirm-password", "confirm_password", "invite-confirm-password", minLen, maxLen)),
			el("button", attrs(a("type", "submit"), a("class", "btn-block"), a("data-testid", "invite-submit")),
				text("Set Password"))))
}

// inviteDeadEnd is the one page every failing token gets, naming no account or
// reason, so it cannot become an account oracle.
func inviteDeadEnd(page InvitePage) node {
	if page.Error != "" {
		return inviteError(page.Error)
	}
	return group(
		el("div", attrs(class("error"), a("data-testid", "invite-invalid")),
			text("This link is not valid. It may have expired or already been used.")),
		el("p", attrs(class("muted")), text("Ask whoever set up your account for a new one.")))
}
