@parallel
Feature: Reading entries

  Background:
    Given I am signed in
    And I have a feed "Reading Feed" with 5 test entries in category "Reading Category"

  Scenario: Unread inbox lists my unread entries newest first
    When I open the inbox
    Then I see 5 entries in the entry list
    And the first entry is titled "Test Entry 1"

  @skip
  Scenario: Opening an entry swaps the reading pane to show its title and body
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the title "Test Entry 1"
    And the reading pane shows the content "Content for test entry 1"

  @skip
  Scenario: Reading pane shows feed title and published time
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the feed title "Reading Feed"
    And the reading pane shows a published time

  Scenario: Read filter shows only read entries
    Given the entry titled "Test Entry 1" is marked read
    When I open the read entries page
    Then I see 1 entry in the entry list
    And the first entry is titled "Test Entry 1"

  Scenario: Starred filter shows only starred entries
    Given the entry titled "Test Entry 2" is starred
    When I open the starred entries page
    Then I see 1 entry in the entry list
    And the first entry is titled "Test Entry 2"

  Scenario: Summarized filter shows only summarized entries
    Given the entry titled "Test Entry 3" has a summary
    When I open the summarized entries page
    Then I see 1 entry in the entry list
    And the first entry is titled "Test Entry 3"

  Scenario: Single-feed view filters by that feed
    When I open the entries page for feed "Reading Feed"
    Then I see 5 entries in the entry list

  Scenario: Single-category view filters by that category
    When I open the entries page for category "Reading Category"
    Then I see 5 entries in the entry list

  @skip
  Scenario: Load More appends the next page without scroll reset
    Given the feed has 30 entries
    When I open the inbox
    And I click "Load more"
    Then I see more than 20 entries in the entry list

  @skip
  Scenario: Keyboard j and k move selection between entries
    When I open the inbox
    And I press the "j" key
    Then the second entry is selected
    When I press the "k" key
    Then the first entry is selected

  Scenario: The question-mark key shows the keyboard shortcut help overlay
    When I open the inbox
    And I press the "?" key
    Then the keyboard shortcut help overlay is visible

  @skip
  Scenario: Reader can toggle between full content and original feed body
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Fetch Full Content" button
    Then the reading pane shows the original feed body
