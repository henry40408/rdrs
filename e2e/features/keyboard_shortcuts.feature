@parallel
Feature: Keyboard shortcuts

  Background:
    Given I am signed in
    And I have a feed "Shortcut Feed" with 3 test entries in category "Shortcut Category"

  Scenario: m toggles the active row's read state
    When I open the inbox
    And I press the "j" key
    And I press the "m" key
    Then the entry row for "Test Entry 1" shows as read

  Scenario: f toggles the active row's star state
    When I open the inbox
    And I press the "j" key
    And I press the "f" key
    Then the entry row for "Test Entry 1" shows as starred

  Scenario: A marks every loaded entry as read (on feed page)
    When I open the entries page for feed "Shortcut Feed"
    And I confirm the next dialog
    And I press the "A" key
    Then I see 0 entries in the entry list

  Scenario: A marks every loaded entry as read (on unread page)
    When I open the inbox
    And I confirm the next dialog
    And I press the "A" key
    Then I see 0 entries in the entry list

  Scenario: Enter opens the selected entry into the reading pane
    When I open the inbox
    And I press the "j" key
    And I press the "Enter" key
    Then the reading pane shows the title "Test Entry 1"

  Scenario: o also opens the selected entry
    When I open the inbox
    And I press the "j" key
    And I press the "o" key
    Then the reading pane shows the title "Test Entry 1"

  Scenario: Esc clears the reading pane back to empty
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "Escape" key
    Then the reading pane is empty

  Scenario: Space does not toggle read when the pane is empty
    When I open the inbox
    And I press the "j" key
    And I press the " " key
    Then the entry row for "Test Entry 1" shows as unread

  Scenario: g f jumps to the selected entry's feed page
    When I open the inbox
    And I press the "j" key
    And I press the "g" key
    And I press the "f" key
    Then I am on the entries page for feed "Shortcut Feed"

  Scenario: g c jumps to the selected entry's category page
    When I open the inbox
    And I press the "j" key
    And I press the "g" key
    And I press the "c" key
    Then I am on the entries page for category "Shortcut Category"

  Scenario: g u returns to the unread inbox from a category page
    When I open the entries page for category "Shortcut Category"
    And I press the "g" key
    And I press the "u" key
    Then I am on the unread inbox

  Scenario: g a jumps to All entries from a non-entries page
    When I open the categories page
    And I press the "g" key
    And I press the "a" key
    Then I am on the all entries page

  Scenario: g s jumps to Starred instead of triggering save
    When I open the inbox
    And I press the "g" key
    And I press the "s" key
    Then I am on the starred entries page

  Scenario: 3 jumps to the Read filter on a feed page
    When I open the entries page for feed "Shortcut Feed"
    And I press the "3" key
    Then I am on the Read filter for feed "Shortcut Feed"

  Scenario: v opens the active entry's original URL in a new tab
    When I open the inbox
    And I press the "j" key
    Then pressing the "v" key opens a new tab at "/entry/1"

  Scenario: Pressing g shows the go-to hint until the sequence completes
    When I open the inbox
    And I press the "g" key
    Then the go-to hint is visible
    When I press the "u" key
    Then the go-to hint is gone
