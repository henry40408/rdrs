@parallel
Feature: Preferences

  Background:
    Given I am signed in
    And I am on the user settings page

  Scenario: Switching to dark theme sets data-theme to dark
    When I switch the theme to "dark"
    Then the html element has data-theme "dark"
    And the body uses "antialiased" font smoothing

  Scenario: Switching to light theme sets data-theme to light
    When I switch the theme to "light"
    Then the html element has data-theme "light"
    And the body uses "auto" font smoothing

  Scenario: Switching to system theme removes the data-theme attribute
    When I switch the theme to "dark"
    And the html element has data-theme "dark"
    And I switch the theme to "system"
    Then the html element has no data-theme attribute

  Scenario: Changing my password lets me sign in with the new password
    When I change my password to "heron-lantern-53-drift"
    Then I can sign in with "heron-lantern-53-drift"

  # The number used to decide nothing: every list paginated by a hardcoded 50
  # and this field was read only to fill itself in.
  Scenario: Entries per page decides how long a list is
    Given I have a feed "Paging Feed" with 12 test entries in category "Paging Category"
    And I am on the user settings page
    When I set entries per page to "10"
    And I open the inbox
    Then I see 10 entries in the entry list
    When I click "Load more"
    Then I see 12 entries in the entry list

  Scenario: Setting a read-article retention period persists
    When I set the retention period to "30" days
    Then the retention period field shows "30"
