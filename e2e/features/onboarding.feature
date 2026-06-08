@parallel
Feature: Onboarding from a fresh account to the first read

  A brand-new account should reach "reading feeds" without manual setup. A
  default "Uncategorized" category lets the first feed be added immediately, and
  the empty landing page guides the user toward adding or importing feeds
  instead of claiming they are already "all caught up".

  Scenario: A new account has a default category so the first feed adds immediately
    Given I am signed in
    And I am on the feeds page
    Then the category dropdown offers "Uncategorized"
    When I add a feed from the mock RSS server under "Uncategorized"
    Then I see a success flash "Feed added"
    And the feeds table contains "Test Feed"

  Scenario: The empty landing page guides a new user to add their first feed
    Given I am signed in
    Then the landing page shows the getting-started guide
    And I see an "Add your first feed" call to action
    And I see an "Import OPML" call to action

  Scenario: The landing page reserves "All caught up" for an account that has feeds
    Given I am signed in
    And I have a feed "My First Feed" in category "Uncategorized"
    When I open the landing page
    Then the landing page does not show the getting-started guide
    And I see "All caught up" on the landing page

  Scenario: Settings shows the active WebAuthn relying-party origin
    Given I am signed in
    When I am on the settings page
    Then I see the active WebAuthn RP origin
