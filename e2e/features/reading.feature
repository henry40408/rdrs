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

  # Regression: opening an entry used to REPLACE the row's DOM node
  # (insertBefore the new node, removeChild the old). If that replacement
  # landed between a user's mousedown and mouseup on the same row — a slow
  # fragment fetch resolving mid-click — the browser fired no `click` and the
  # open was silently dropped ("click does nothing; hover shows; click again
  # works"). The swap now morphs single-element targets in place, preserving
  # the row's node identity. A JS property set on the node survives the open
  # only when that identity is kept.
  Scenario: Opening an entry preserves the row's DOM node (no dropped clicks)
    When I open the inbox
    And I mark the entry row for "Test Entry 1" with an identity probe
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the title "Test Entry 1"
    And the entry row for "Test Entry 1" kept its identity probe

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

  # Uses the All view so reading an entry doesn't drop it from the list —
  # neighbour membership stays stable while we step through it.
  Scenario: Reading pane Next and Previous open adjacent entries
    When I open the all entries page
    And I click the entry titled "Test Entry 3"
    And I navigate to the "Next" entry in the reading pane
    Then the reading pane shows the title "Test Entry 4"
    When I navigate to the "Previous" entry in the reading pane
    Then the reading pane shows the title "Test Entry 3"
    When I navigate to the "Previous" entry in the reading pane
    Then the reading pane shows the title "Test Entry 2"

  Scenario: With the reading pane open, j and k open adjacent entries
    When I open the all entries page
    And I click the entry titled "Test Entry 3"
    And I press the "j" key
    Then the reading pane shows the title "Test Entry 4"
    When I press the "k" key
    Then the reading pane shows the title "Test Entry 3"
    When I press the "k" key
    Then the reading pane shows the title "Test Entry 2"

  # Neighbours honour the active filter: in the Unread inbox, opening an
  # entry marks it read and so drops it from the unread set. "Previous"
  # therefore skips the entry just read and lands on the next still-unread
  # one — the same set the list filters to.
  Scenario: Reading-pane navigation honours the unread filter
    When I open the inbox
    And I click the entry titled "Test Entry 3"
    And I navigate to the "Next" entry in the reading pane
    Then the reading pane shows the title "Test Entry 4"
    When I navigate to the "Previous" entry in the reading pane
    Then the reading pane shows the title "Test Entry 2"

  Scenario: Previous is disabled on the newest entry
    When I open the all entries page
    And I click the entry titled "Test Entry 1"
    Then the reading-pane "Next" button is enabled
    And the reading-pane "Previous" button is disabled

  Scenario: Next is disabled on the oldest entry
    When I open the all entries page
    And I click the entry titled "Test Entry 5"
    Then the reading-pane "Previous" button is enabled
    And the reading-pane "Next" button is disabled

  # Regression: Fetch Full Content re-renders the pane for the *same* entry,
  # resetting prev/next to their default disabled state. The neighbour
  # re-resolve skipped re-enabling them because the entry id was unchanged,
  # so the freshly-rendered buttons stayed disabled forever. Disabled buttons
  # swallow taps, killing mobile navigation permanently (desktop j/k bypasses
  # the buttons, which is why the breakage was mobile-only).
  @mobile
  Scenario: Reading-pane navigation survives Fetch Full Content on mobile
    Given I am viewing on a mobile screen
    When I open the all entries page
    And I click the entry titled "Test Entry 3"
    And I click the "Fetch Full Content" button
    And I see a flash message
    Then the reading-pane "Next" button is enabled
    And the reading-pane "Previous" button is enabled
    When I navigate to the "Next" entry in the reading pane
    Then the reading pane shows the title "Test Entry 4"
