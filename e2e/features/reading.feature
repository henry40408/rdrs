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

  Scenario: Feed meta link only covers its text, not the blank row space
    When I open the inbox
    Then the feed link does not span the full meta row

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

  Scenario: Sidebar shows a Summarized count badge
    Given the entry titled "Test Entry 1" has a summary
    When I open the inbox
    Then the sidebar Summarized item shows a count of "1"

  Scenario: Failed summary shows an error with Retry and Clear
    Given the user has Kagi configured
    And the entry titled "Test Entry 3" has a failed summary
    When I open the inbox
    And I click the entry titled "Test Entry 3"
    Then I see the summary error banner
    And I see a "Retry" summary action
    And I see a "Clear" summary action
    When I click the "Clear" summary action
    Then I do not see the summary error banner

  Scenario: Retry regenerates instead of dismissing a failed summary
    Given the user has Kagi configured
    And the entry titled "Test Entry 3" has a failed summary
    When I open the inbox
    And I click the entry titled "Test Entry 3"
    And I click the "Retry" summary action
    Then the reading pane shows a summary

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

  Scenario: Reading past the loaded page pulls the list forward and moves the selection
    Given the feed has 60 entries
    When I open the inbox
    And I click the entry titled "Test Entry 50"
    And I press the "j" key
    Then the reading pane shows the title "Test Entry 51"
    And I see more than 50 entries in the entry list
    And exactly one entry is selected
    And the selected entry is titled "Test Entry 51"

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

  Scenario: Reader can cancel a slow full-content fetch
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And the fetch full content response for the entry titled "Test Entry 1" is delayed
    And I click the "Fetch Full Content" button
    Then I see a "Cancel" fetch full content action
    When I click the "Cancel" button
    Then the delayed fetch full content response has settled
    And the reading pane shows the original feed body
    And I see a "Fetch Full Content" button
    And I see no flash message

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
    And I press the "m" key
    Then I see a success flash "Marked as unread"
    When I click the entry titled "Test Entry 2"
    Then the reading pane shows the title "Test Entry 2"
    And I see no flash message

  Scenario: Acting on the same entry preserves an earlier flash banner
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "m" key
    Then I see a success flash "Marked as unread"
    When I press the "f" key
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

  # Unread navigation uses snapshot semantics: entries read *during* this
  # page view stay reachable (so "Previous" can return to the entry just
  # read), while entries already read when the page loaded are skipped —
  # the same set the list rendered.
  Scenario: Unread navigation returns to the entry just read
    When I open the inbox
    And I click the entry titled "Test Entry 3"
    And I navigate to the "Next" entry in the reading pane
    Then the reading pane shows the title "Test Entry 4"
    When I navigate to the "Previous" entry in the reading pane
    Then the reading pane shows the title "Test Entry 3"

  Scenario: Unread navigation skips entries read before the page loaded
    Given the entry titled "Test Entry 2" was marked read an hour ago
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I navigate to the "Next" entry in the reading pane
    Then the reading pane shows the title "Test Entry 3"

  Scenario: With the pane open in the inbox, k returns to the just-read entry
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "j" key
    Then the reading pane shows the title "Test Entry 2"
    When I press the "k" key
    Then the reading pane shows the title "Test Entry 1"

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
    And the entry titled "Test Entry 3" cannot have its full content fetched
    When I open the all entries page
    And I click the entry titled "Test Entry 3"
    And I click the "Fetch Full Content" button
    And I see a flash message
    Then the reading-pane "Next" button is enabled
    And the reading-pane "Previous" button is enabled
    When I navigate to the "Next" entry in the reading pane
    Then the reading pane shows the title "Test Entry 4"

  # Regression: cancelPaneImages() used to drop `src` on every pane image,
  # including the meta-row favicon, blanking it on the still-visible outgoing
  # pane for the whole fragment fetch — a visible favicon flash on each switch.
  # It must now only cancel the slow .reading-pane-article content images.
  Scenario: Switching entries does not blank the reading-pane favicon mid-load
    Given the "Reading Feed" feed has a favicon
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows an image favicon
    When the fragment response for the entry titled "Test Entry 2" is delayed
    And I click the entry titled "Test Entry 2" without waiting for the pane
    Then the reading pane favicon still has its image
    When the delayed fragment response has settled
    Then the reading pane shows the title "Test Entry 2"

  # Regression: every open sends the row's marker form back, and once the entry
  # is read that fragment is byte-identical to what is already there. Replacing
  # it anyway repaints the whole grid row — favicon included, which WebKit
  # re-rasterizes — so the icons blinked on repeated clicks with nothing about
  # the row actually changing. An identical row fragment is now skipped.
  Scenario: Re-opening an entry that is already read leaves its row untouched
    Given the "Reading Feed" feed has a favicon
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the title "Test Entry 1"
    When I click the entry titled "Test Entry 2"
    Then the reading pane shows the title "Test Entry 2"
    When I tag the entry rows
    And I click the entry titled "Test Entry 1"
    Then the reading pane shows the title "Test Entry 1"
    And the entry rows are still the ones I tagged

  Scenario: A stale slow fragment response never overwrites a newer click
    When I open the inbox
    And the fragment response for the entry titled "Test Entry 1" is delayed
    And I click the entry titled "Test Entry 1" without waiting for the pane
    And I click the entry titled "Test Entry 2"
    And the delayed fragment response has settled
    Then the reading pane shows the title "Test Entry 2"
    And the URL has the ?entry= parameter for "Test Entry 2"

  Scenario: Esc in the shortcut help closes only the help
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "?" key
    And I press the "Escape" key without refocusing
    Then the keyboard shortcut help overlay is hidden
    And the reading pane shows the title "Test Entry 1"

  Scenario: Help overlay descriptions align across rows
    When I open the inbox
    And I press the "?" key
    Then the help overlay descriptions are aligned

  # The overlay's Shadow DOM stylesheet references design tokens with no
  # fallback values, so a token renamed in app.css would silently render it
  # unstyled — every other help assertion here would still pass.
  Scenario: Help overlay picks up the document's design tokens
    When I open the inbox
    And I press the "?" key
    Then the help overlay resolves its design tokens

  Scenario: All Entries stays highlighted across the /entries pages
    When I open the read entries page
    Then the sidebar highlights All Entries
    When I open the summarized entries page
    Then the sidebar highlights Summarized
    When I open the all entries page
    Then the sidebar highlights All Entries
    When I open the starred entries page
    Then the sidebar highlights Starred

  @mobile
  Scenario: Reading-pane actions sit in a touch-sized bottom bar on mobile
    Given I am viewing on a mobile screen
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane is visible on mobile
    And the ".rp-action" control is at least 44px tall
    And the ".reading-pane-actions" control is at least 360px wide

  Scenario: A broken content image shows the dashed-box fallback
    Given the entry titled "Test Entry 3" has content with a broken image
    When I open the inbox
    And I click the entry titled "Test Entry 3"
    Then the reading pane shows a broken-image fallback

  Scenario: Line-numbered code blocks do not stack nested pre padding
    Given the entry titled "Test Entry 1" contains a line-numbered code block
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the nested code-block pre has no padding while the outer pre does
