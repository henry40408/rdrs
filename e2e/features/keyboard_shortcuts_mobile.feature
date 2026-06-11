@parallel
Feature: Keyboard shortcuts on the mobile layout

  Background:
    Given I am signed in
    And I have a feed "Mobile KB Feed" with 3 test entries in category "Mobile KB Category"
    And I am viewing on a mobile screen

  @mobile
  Scenario: Enter opens the selected entry as a full-screen overlay
    When I open the inbox
    And I press the "j" key without refocusing
    And I press the "Enter" key without refocusing
    Then the reading pane is visible on mobile
    And the reading pane shows the title "Test Entry 1"

  @mobile
  Scenario: Esc dismisses the reading-pane overlay back to the list
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "Escape" key without refocusing
    Then the reading pane overlay is dismissed
    And the URL has no ?entry= parameter

  @mobile
  Scenario: j and k switch entries while the overlay is open
    When I open the all entries page
    And I click the entry titled "Test Entry 2"
    And I press the "j" key without refocusing
    Then the reading pane shows the title "Test Entry 3"
    And the reading pane is visible on mobile
    When I press the "k" key without refocusing
    Then the reading pane shows the title "Test Entry 2"

  @mobile
  Scenario: m and f act on the selected row on mobile
    When I open the inbox
    And I press the "j" key without refocusing
    And I press the "m" key without refocusing
    Then the entry row for "Test Entry 1" shows as read
    When I press the "f" key without refocusing
    Then the entry row for "Test Entry 1" shows as starred

  @mobile
  Scenario: g u jumps to the inbox from a category page on mobile
    When I open the entries page for category "Mobile KB Category"
    And I press the "g" key without refocusing
    And I press the "u" key without refocusing
    Then I am on the unread inbox

  @mobile
  Scenario: A marks loaded entries as read on mobile
    When I open the inbox
    And I confirm the next dialog
    And I press the "A" key without refocusing
    Then I see 0 entries in the entry list

  @mobile
  Scenario: The help overlay opens on the mobile viewport
    When I open the inbox
    And I press the "?" key without refocusing
    Then the keyboard shortcut help overlay is visible
