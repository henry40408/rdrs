package e2e

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// The admin and statistics pages.

func registerAdminSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I am signed in as an admin$`, w.signedInAsAdmin)
	sc.Step(`^there is another registered user$`, w.anotherRegisteredUser)
	sc.Step(`^I open the admin page$`, func() error { return w.goto_("/admin") })
	sc.Step(`^I open the statistics page$`, func() error { return w.goto_("/statistics") })
	sc.Step(`^I disable the other user$`, w.disableOtherUser)
	sc.Step(`^I see my username in the users table$`, func() error {
		return w.browser.ExpectText("admin-users-table", w.user.Username)
	})
	sc.Step(`^the other user is shown as disabled in the table$`, w.otherUserDisabled)
	sc.Step(`^the statistics show at least (\d+) feed$`, func(minimum int) error {
		return w.expectAtLeast("stat-site-feeds-total", minimum)
	})
	sc.Step(`^the statistics show at least (\d+) entries$`, func(minimum int) error {
		return w.expectAtLeast("stat-site-entries-total", minimum)
	})
}

// signedInAsAdmin promotes this scenario's account to admin, then signs in.
func (w *world) signedInAsAdmin() error {
	if err := w.api.Register(w.user.Username, w.user.Password); err != nil {
		return err
	}
	userID, err := w.currentUserID()
	if err != nil {
		return err
	}
	if err := w.seed.MakeAdmin(userID); err != nil {
		return err
	}
	return w.logIn()
}

// anotherRegisteredUser creates a second account and remembers its name:
// disabling the bootstrap admin instead would break every later scenario.
func (w *world) anotherRegisteredUser() error {
	other := "other-" + w.user.Username
	if err := w.api.Register(other, harness.Password); err != nil {
		return err
	}
	w.otherUsername = other
	return nil
}

func (w *world) otherUserRow() (*harness.Element, error) {
	if w.otherUsername == "" {
		return nil, fmt.Errorf("no second account: no step registered another user")
	}
	return w.browser.RowWithText(w.otherUsername)
}

func (w *world) disableOtherUser() error {
	row, err := w.otherUserRow()
	if err != nil {
		return err
	}
	button, err := row.WaitFind("[data-testid=admin-disable-btn]")
	if err != nil {
		return err
	}
	// Redirects to the same page, so wait for document replacement, not the URL.
	return w.browser.SubmitElement(button)
}

func (w *world) otherUserDisabled() error {
	row, err := w.otherUserRow()
	if err != nil {
		return err
	}
	badge, err := row.WaitFind("[data-testid=admin-user-disabled]")
	if err != nil {
		return err
	}
	shown, err := badge.Displayed()
	if err != nil {
		return err
	}
	if !shown {
		return fmt.Errorf("`%s` is not marked disabled", w.otherUsername)
	}
	return nil
}

func (w *world) expectAtLeast(id string, minimum int) error {
	text, err := w.browser.TextOf(id)
	if err != nil {
		return err
	}
	count, err := strconv.Atoi(strings.TrimSpace(text))
	if err != nil {
		return fmt.Errorf("`%s` reads %q, which is not a count", id, text)
	}
	if count < minimum {
		return fmt.Errorf("`%s` is %d, expected at least %d", id, count, minimum)
	}
	return nil
}
