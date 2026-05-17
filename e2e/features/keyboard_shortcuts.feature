@parallel
Feature: Keyboard shortcut overhaul (toggle read, mark all, rebinds)

  Background:
    Given I am signed in
    And I have a feed "Shortcut Feed" with 3 test entries in category "Shortcut Category"

  Scenario: u toggles the active row's read state
    When I open the inbox
    And I press the "j" key
    And I press the "u" key
    Then the entry row for "Test Entry 1" shows as read

  Scenario: r is an alias for u (also toggles read)
    When I open the inbox
    And I press the "j" key
    And I press the "r" key
    Then the entry row for "Test Entry 1" shows as read

  Scenario: Shift+K marks every entry in the current list as read
    When I open the inbox
    And I confirm the next dialog
    And I press the "K" key
    Then I see 0 entries in the entry list
