@parallel
Feature: Reading entries

  Background:
    Given I am signed in
    And I have a feed "Reading Feed" with 5 test entries in category "Reading Category"

  Scenario: Unread inbox lists my unread entries newest first
    When I open the inbox
    Then I see 5 entries in the entry list
    And the first entry is titled "Test Entry 1"

  Scenario: Opening an entry swaps the reading pane to show its title and body
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the title "Test Entry 1"
    And the reading pane shows the content "Content for test entry 1"

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

  Scenario: Load More appends the next page without scroll reset
    Given the feed has 60 entries
    When I open the inbox
    And I click "Load more"
    Then I see more than 50 entries in the entry list

  Scenario: Keyboard j and k move selection between entries
    When I open the inbox
    And I press the "j" key
    And I press the "j" key
    Then the second entry is selected
    When I press the "k" key
    Then the first entry is selected

  Scenario: The question-mark key shows the keyboard shortcut help overlay
    When I open the inbox
    And I press the "?" key
    Then the keyboard shortcut help overlay is visible

  Scenario: Reader can toggle between full content and original feed body
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Fetch Full Content" button
    Then the reading pane shows the original feed body

  Scenario: Clicking an entry syncs ?entry= into the URL and survives a reload
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the URL has the ?entry= parameter for "Test Entry 1"
    When I reload the page
    Then the reading pane shows the title "Test Entry 1"

  Scenario: Visiting /?entry={id} directly opens that entry's reading pane
    When I open the inbox deep-linked to entry titled "Test Entry 2"
    Then the reading pane shows the title "Test Entry 2"
    And the reading pane shows the content "Content for test entry 2"

  Scenario: Pressing Esc clears the reading pane and drops ?entry= from the URL
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "Escape" key
    Then the reading pane is empty
    And the URL has no ?entry= parameter

  Scenario: Opening a different entry clears flash banners from prior actions
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "u" key
    Then I see a success flash "Marked as unread"
    When I click the entry titled "Test Entry 2"
    Then the reading pane shows the title "Test Entry 2"
    And I see no flash message

  Scenario: Acting on the same entry preserves an earlier flash banner
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "u" key
    Then I see a success flash "Marked as unread"
    When I press the "s" key
    Then I see a success flash "Marked as unread"
