@parallel
Feature: Authentication

  Scenario: An invited user sets their own password and signs in
    Given an admin has created an account for me
    When I open my one-time link and choose a password
    Then I am redirected to the login page with a success message
    And the flash banner shows a timestamp
    When I sign in with my credentials
    Then I land on the unread inbox

  Scenario: Sign-in with the wrong password shows an error
    Given I am a registered user
    When I sign in with the wrong password
    Then I see a login error

  Scenario: Mismatched passwords on the invite form are refused
    Given an admin has created an account for me
    When I open my one-time link and mistype the confirmation
    Then I see "Passwords do not match" on the invite page


  Scenario: A non-admin account is not offered the app settings page
    Given the instance already has an owner account
    And I am signed in
    Then the sidebar does not offer the app settings link
    When I am on the settings page
    Then I am not shown the app settings page

  Scenario: Logging out clears the session, the sidebar cache, and shows a flash
    Given I am signed in
    When I log out
    Then I see the logged-out flash message
    And the sidebar's cached data no longer survives in session storage
    When I visit the home page
    Then I am redirected to the login page
