@parallel
Feature: Triage entries (star, mark-read, summarize)

  Background:
    Given I am signed in
    And I have a feed "Triage Feed" with 3 test entries in category "Triage Category"

  @skip
  Scenario: Starring an entry updates the row and the sidebar starred count
    When I open the inbox
    And I star the entry titled "Test Entry 1"
    Then the entry titled "Test Entry 1" is marked starred
    And the sidebar starred count is at least 1

  @skip
  Scenario: Marking an entry read updates the row and the sidebar unread count
    When I open the inbox
    And I mark the entry titled "Test Entry 1" read
    Then the entry row for "Test Entry 1" shows as read
    And the sidebar unread count decreases by 1

  @skip
  Scenario: Marking all entries read empties the unread list
    When I open the inbox
    And I click "Mark all read"
    Then I see 0 entries in the entry list

  @skip
  Scenario: Summarizing an entry shows the summary in the reading pane
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Summarize" button
    Then the reading pane shows a summary

  Scenario: Dismissing a summary clears the summary from the reading pane
    Given the entry titled "Test Entry 1" has a summary
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Dismiss" button
    Then the reading pane summary is dismissed
