@parallel
Feature: Authentication

  Scenario: New user can register, sign in, and reach the unread inbox
    When I register with matching passwords
    Then I am redirected to the login page with a success message
    And the flash banner shows a timestamp
    When I sign in with my credentials
    Then I land on the unread inbox

  Scenario: Sign-in with the wrong password shows an error
    Given I am a registered user
    When I sign in with the wrong password
    Then I see a login error

  Scenario: Mismatched passwords on registration show a client-side error
    When I register with mismatched passwords
    Then I see "Passwords do not match" on the register page
    And I am still on the register page


  Scenario: A non-admin account is not offered the app settings page
    Given the instance already has an owner account
    And I am signed in
    Then the sidebar does not offer the app settings link
    When I am on the settings page
    Then I am not shown the app settings page
