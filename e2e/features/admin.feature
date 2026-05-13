@parallel
Feature: Admin and statistics

  Background:
    Given I am signed in as an admin

  Scenario: Admin sees the list of all users
    When I open the admin page
    Then I see my username in the users table

  Scenario: Admin disables a user account
    Given there is another registered user
    When I open the admin page
    And I disable the first non-self user
    Then a user is shown as disabled in the table

  Scenario: Statistics page shows feed and entry counts
    Given I have a feed with 3 test entries
    When I open the statistics page
    Then the statistics show at least 1 feed
    And the statistics show at least 3 entries
