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

  Scenario: o marks every loaded entry above as read (on feed page)
    When I open the entries page for feed "Shortcut Feed"
    And I confirm the next dialog
    And I press the "o" key
    Then I see 0 entries in the entry list

  Scenario: Enter opens the selected entry into the reading pane
    When I open the inbox
    And I press the "j" key
    And I press the "Enter" key
    Then the reading pane shows the title "Test Entry 1"

  Scenario: Esc clears the reading pane back to empty
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "Escape" key
    Then the reading pane is empty

  Scenario: f jumps to the selected entry's feed page
    When I open the inbox
    And I press the "j" key
    And I press the "f" key
    Then I am on the entries page for feed "Shortcut Feed"

  Scenario: c jumps to the selected entry's category page
    When I open the inbox
    And I press the "j" key
    And I press the "c" key
    Then I am on the entries page for category "Shortcut Category"

  Scenario: x returns to the unread inbox from a category page
    When I open the entries page for category "Shortcut Category"
    And I press the "x" key
    Then I am on the unread inbox

  Scenario: 3 jumps to the Read filter on a feed page
    When I open the entries page for feed "Shortcut Feed"
    And I press the "3" key
    Then I am on the Read filter for feed "Shortcut Feed"

  Scenario: b opens the active entry's original URL in a new tab
    When I open the inbox
    And I press the "j" key
    Then pressing the "b" key opens a new tab at "/entry/1"
