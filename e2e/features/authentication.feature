@parallel
Feature: Authentication

  Scenario: New user can register, sign in, and reach the unread inbox
    When I register with matching passwords
    Then I am redirected to the login page with a success message
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

  Scenario: Authenticated user visiting /login is redirected to the inbox
    Given I am signed in
    When I visit "/login"
    Then I land on the unread inbox
