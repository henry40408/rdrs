package views

import "testing"

const goldenCSRF = "@HEX64@"

func TestAuthPagesMatchTheRustGoldens(t *testing.T) {
	setup := func(message string) SetupPage {
		return SetupPage{GitVersion: goldenVersion, Error: message, PasswordMinLength: 15,
			PasswordMaxLength: 128, CSRFToken: goldenCSRF}
	}
	login := func(page LoginPage) string {
		page.GitVersion, page.CSRFToken, page.LocalAuthEnabled = goldenVersion, goldenCSRF, true
		return Login(page)
	}
	invite := func(message string) string {
		return Invite(InvitePage{GitVersion: goldenVersion, CSRFToken: goldenCSRF, Username: "bob",
			Token: "golden-invite-token", Error: message, PasswordMinLength: 15, PasswordMaxLength: 128})
	}
	cases := map[string]string{
		"login.html":                 login(LoginPage{}),
		"login_next.html":            login(LoginPage{Next: "/entries"}),
		"login_bad_credentials.html": login(LoginPage{Next: "/entries", Error: "Invalid credentials"}),
		"login_setup_available.html": login(LoginPage{SetupAvailable: true}),
		"setup.html":                 Setup(setup("")),
		"setup_mismatch.html":        Setup(setup("Passwords do not match")),
		"setup_bad_token.html": Setup(setup(
			"Invalid setup token. Find it on the server's stderr from startup.")),
		"invite.html":          invite(""),
		"invite_mismatch.html": invite("Passwords do not match."),
		"invite_invalid.html": Invite(InvitePage{GitVersion: goldenVersion, PasswordMinLength: 15,
			PasswordMaxLength: 128}),
	}
	for name, got := range cases {
		if want := readGolden(t, name); got != want {
			t.Errorf("%s differs:\n got: %.400s\nwant: %.400s", name, got, want)
		}
	}
}
