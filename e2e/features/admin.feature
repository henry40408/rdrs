@parallel
Feature: Admin and statistics

  Background:
    Given I am signed in as an admin

  Scenario: Admin sees the list of all users
    When I open the admin page
    Then I see my username in the users table

  # No admin "create user" form exists in the product — user creation is via /register only.
  @skip
  Scenario: Admin creates a new user account
    When I open the admin page
    And I create a user with a random username and password "password123"
    Then the new user appears in the users table

  Scenario: Admin disables a user account
    Given there is another registered user
    When I open the admin page
    And I disable the first non-self user
    Then that user is shown as disabled

  Scenario: Statistics page shows feed and entry counts
    Given I have a feed with 3 test entries
    When I open the statistics page
    Then the statistics show at least 1 feed
    And the statistics show at least 3 entries
