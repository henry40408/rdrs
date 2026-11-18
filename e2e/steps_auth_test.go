package e2e

import (
	"fmt"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Registration, sign-in, invites and sign-out.

func registerAuthSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^an admin has created an account for me$`, w.adminCreatedAccount)
	sc.Step(`^I am a registered user$`, w.registeredUser)
	sc.Step(`^the instance already has an owner account$`, w.instanceHasOwner)
	sc.Step(`^I am signed in$`, w.signedIn)
	sc.Step(`^I open my one-time link and choose a password$`, w.redeemInvite)
	sc.Step(`^I open my one-time link and mistype the confirmation$`, w.mistypeConfirmation)
	sc.Step(`^I sign in with my credentials$`, w.signIn)
	sc.Step(`^I sign in with the wrong password$`, w.signInWrongPassword)
	sc.Step(`^I log out$`, w.logOut)
	sc.Step(`^I visit the home page$`, func() error { return w.goto_("/") })
	sc.Step(`^I visit "([^"]*)"$`, w.goto_)
	sc.Step(`^I am redirected to the login page with a success message$`, w.redirectedWithSuccess)
	sc.Step(`^I land on the unread inbox$`, w.landOnInbox)
	sc.Step(`^I see a login error$`, func() error { return w.browser.ExpectVisible("login-error") })
	sc.Step(`^I see "([^"]*)" on the invite page$`, func(message string) error {
		return w.browser.ExpectText("invite-error", message)
	})
	sc.Step(`^the sidebar does not offer the app settings link$`, w.noAppSettingsLink)
	sc.Step(`^I am not shown the app settings page$`, w.notShownAppSettings)
	sc.Step(`^I see the logged-out flash message$`, func() error {
		return w.browser.ExpectText("flash-message", "You have been logged out.")
	})
	sc.Step(`^the sidebar's cached data no longer survives in session storage$`, w.sidebarCacheCleared)
	sc.Step(`^I am redirected to "([^"]*)"$`, w.expectPath)
	sc.Step(`^I am redirected to the login page$`, func() error { return w.expectPath("/login") })
}

// adminCreatedAccount stores the invite path for a later step.
func (w *world) adminCreatedAccount() error {
	invite, err := w.api.InviteAccount(w.user.Username)
	w.invitePath = invite
	return err
}

func (w *world) registeredUser() error {
	return w.api.Register(w.user.Username, w.user.Password)
}

// instanceHasOwner registers another account first, since the first user
// becomes admin.
func (w *world) instanceHasOwner() error {
	return w.api.Register("e2e-owner", harness.Password)
}

func (w *world) signedIn() error {
	if err := w.api.Register(w.user.Username, w.user.Password); err != nil {
		return err
	}
	return w.logIn()
}

// logIn signs this scenario's account in through the login form.
func (w *world) logIn() error {
	if err := w.goto_("/login"); err != nil {
		return err
	}
	if err := w.signIn(); err != nil {
		return err
	}
	return w.expectPath("/")
}

func (w *world) redeemInvite() error {
	return w.submitInvite(w.user.Password)
}

// mistypeConfirmation gives a confirmation that clears minlength, so the
// server's mismatch check, not constraint validation, rejects it.
func (w *world) mistypeConfirmation() error {
	return w.submitInvite("badger-kestrel-19-plume")
}

func (w *world) submitInvite(confirmation string) error {
	if w.invitePath == "" {
		return fmt.Errorf("no invite link: no step created an account for this scenario")
	}
	if err := w.goto_(w.invitePath); err != nil {
		return err
	}
	if err := w.browser.Fill("invite-password", w.user.Password); err != nil {
		return err
	}
	if err := w.browser.Fill("invite-confirm-password", confirmation); err != nil {
		return err
	}
	return w.browser.Click("invite-submit")
}

func (w *world) signIn() error {
	return w.fillLoginForm(w.user.Password)
}

func (w *world) signInWrongPassword() error {
	if err := w.goto_("/login"); err != nil {
		return err
	}
	return w.fillLoginForm("wrongpassword")
}

func (w *world) fillLoginForm(password string) error {
	if err := w.browser.Fill("username-input", w.user.Username); err != nil {
		return err
	}
	if err := w.browser.Fill("password-input", password); err != nil {
		return err
	}
	return w.browser.Click("login-submit")
}

func (w *world) logOut() error {
	if err := w.browser.Click("logout-btn"); err != nil {
		return err
	}
	return w.expectPath("/login")
}

func (w *world) redirectedWithSuccess() error {
	if err := w.expectPath("/login"); err != nil {
		return err
	}
	return w.browser.ExpectText("flash-message", "Password set")
}

func (w *world) landOnInbox() error {
	if err := w.expectPath("/"); err != nil {
		return err
	}
	return w.browser.ExpectVisible("main-nav")
}

func (w *world) noAppSettingsLink() error {
	if err := w.browser.ExpectVisible("main-nav"); err != nil {
		return err
	}
	return w.browser.ExpectAbsent("nav-app-settings")
}

// notShownAppSettings: the admin guard redirects to /login, which bounces a
// signed-in session back to the inbox.
func (w *world) notShownAppSettings() error {
	rendered, err := w.browser.HeadingExists("App")
	if err != nil {
		return err
	}
	if rendered {
		return fmt.Errorf("the app settings heading rendered")
	}
	current, err := w.browser.URL()
	if err != nil {
		return err
	}
	if strings.Contains(current.String(), "/settings") {
		return fmt.Errorf("still on the settings page: %s", current)
	}
	return nil
}

// sidebarCacheCleared: logout sends Clear-Site-Data so the sidebar's
// sessionStorage mirror does not leak to the next user.
func (w *world) sidebarCacheCleared() error {
	cached, err := harness.Eval[*string](w.browser, "sessionStorage.getItem('rdrs.sidebar.v1')")
	if err != nil {
		return err
	}
	if cached != nil {
		return fmt.Errorf("the sidebar cache survived logout: %q", *cached)
	}
	return nil
}
