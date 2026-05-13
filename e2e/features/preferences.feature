@parallel
Feature: Preferences

  Background:
    Given I am signed in
    And I am on the user settings page

  Scenario: Switching to dark theme sets data-theme to dark
    When I switch the theme to "dark"
    Then the html element has data-theme "dark"

  Scenario: Switching to light theme sets data-theme to light
    When I switch the theme to "light"
    Then the html element has data-theme "light"

  Scenario: Switching to system theme removes the data-theme attribute
    When I switch the theme to "dark"
    And the html element has data-theme "dark"
    And I switch the theme to "system"
    Then the html element has no data-theme attribute

  Scenario: Changing my password lets me sign in with the new password
    When I change my password to "newpassword123"
    Then I can sign in with "newpassword123"
